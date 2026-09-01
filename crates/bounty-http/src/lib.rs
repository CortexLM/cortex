//! Bounty HTTP API (master-only, **internal ingest**).
//!
//! Cortex does **not** serve a public leaderboard/report API. Public consumers
//! hit CortexLM/backend (`GET /v1/bounty/public/leaderboard|reports`). This
//! service only reads that feed for scoring (see `bounty-challenge::backend`).
//!
//! Gateway proxies `/challenge/bounty/*` onto this service:
//!
//! ```text
//! GET  /health
//! GET  /v1/status
//! POST /v1/pair                 verify hotkey sig, bind account, session claim
//! POST /v1/reports              bug report + session (optional X-Lium-Api-Key)
//! GET  /v1/reports              internal ingest list (not a public board)
//! GET  /v1/reports/{id}
//! POST /v1/admin/adjudicate     valid | already_fixed_not_prod | invalid_malicious | duplicate
//! ```
//!
//! There is no `/v1/public/*` route.

#![forbid(unsafe_code)]
#![allow(
    clippy::missing_errors_doc,
    clippy::doc_markdown,
    clippy::must_use_candidate,
    clippy::items_after_statements,
    clippy::too_many_lines
)]

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use bounty_challenge_task::{
    backend_public_url, force_sim, hotkey_hex, parse_hotkey, parse_signature,
    verify_pair_signature, PairChallenge, ScoringBackend, CHALLENGE_ID,
    MAX_PENDING_REPORTS_PER_HOTKEY, MIN_REPORT_BODY_CHARS, MIN_REPORT_INTERVAL_SECS,
    MIN_REPRO_CHARS, SCORE_MAX, SCORING_VERSION, TERMS_TEXT,
};
use bounty_score::{Adjudication, Severity, MAX_TRIAGE_NOISE_BPS, MIN_PRECISION_BPS};
use bounty_store::{report_fingerprint, MemoryStore, Report, ReportState, StoreError};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Shared HTTP state.
#[derive(Clone)]
pub struct AppState {
    /// Pairings + reports.
    pub store: MemoryStore,
    /// HMAC secret for session claims.
    pub session_secret: Arc<Vec<u8>>,
    /// Where this host's scores come from, resolved once at boot.
    pub scoring: ScoringBackend,
    /// Operator bearer hashes (sha256 hex). Empty → admin 503.
    pub admin_hashes: Arc<Vec<String>>,
}

impl AppState {
    /// Whether an accepted report can ever become weight on this host.
    fn can_score(&self) -> bool {
        self.scoring != ScoringBackend::Unconfigured
    }
}

/// Build the router.
pub fn bounty_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/v1/status", get(status))
        .route("/v1/pair", post(pair))
        .route("/v1/reports", post(submit_report).get(list_reports))
        .route("/v1/reports/{id}", get(get_report))
        .route("/v1/admin/adjudicate", post(adjudicate))
        .with_state(state)
}

async fn health() -> impl IntoResponse {
    Json(serde_json::json!({
        "ok": true,
        "challenge_id": CHALLENGE_ID,
        "scoring_version": SCORING_VERSION,
    }))
}

async fn status(State(st): State<AppState>) -> impl IntoResponse {
    let champ = st.store.champion_hotkey().ok().flatten();
    Json(serde_json::json!({
        "challenge_id": CHALLENGE_ID,
        "scoring_version": SCORING_VERSION,
        "score_max": SCORE_MAX,
        "champion_hotkey": champ,
        // Where scores come from, and whether the offline scorer was opted
        // into. A miner can see that this host is not producing weight.
        "scoring_backend": st.scoring,
        "force_sim": force_sim(),
        "can_score": st.can_score(),
        "backend_public_configured": backend_public_url().is_some(),
        // Published so miners can see what is being measured — and what is
        // not: `triage_noise` never enters the score they are paid on.
        "scoring": {
            "paid_on": ["precision", "severity_impact"],
            "off_score_gates": ["triage_noise"],
            "min_precision_bps": MIN_PRECISION_BPS,
            "max_triage_noise_bps": MAX_TRIAGE_NOISE_BPS,
            "severities": Severity::ALL.map(Severity::as_str),
        },
        "quotas": {
            "max_pending_reports_per_hotkey": MAX_PENDING_REPORTS_PER_HOTKEY,
            "min_report_interval_secs": MIN_REPORT_INTERVAL_SECS,
            "min_report_body_chars": MIN_REPORT_BODY_CHARS,
            "min_repro_chars": MIN_REPRO_CHARS,
        },
        "terms": TERMS_TEXT,
    }))
}

#[derive(Debug, Deserialize)]
struct PairBody {
    account_id: String,
    hotkey: String,
    nonce: String,
    exp: u64,
    signature: String,
    terms_accepted: bool,
}

#[derive(Debug, Serialize)]
struct PairResp {
    session: String,
    account_id: String,
    miner_hotkey: String,
    session_id: String,
}

async fn pair(
    State(st): State<AppState>,
    Json(body): Json<PairBody>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    if !body.terms_accepted {
        return Err(err(StatusCode::FORBIDDEN, "terms_required"));
    }
    let challenge = PairChallenge {
        account_id: body.account_id.clone(),
        nonce: body.nonce.clone(),
        exp: body.exp,
    };
    let encoded = challenge
        .encode()
        .map_err(|e| err(StatusCode::BAD_REQUEST, &e.to_string()))?;
    let now = unix_now();
    challenge
        .ensure_fresh(now)
        .map_err(|e| err(StatusCode::BAD_REQUEST, &e.to_string()))?;
    let hotkey =
        parse_hotkey(&body.hotkey).map_err(|e| err(StatusCode::BAD_REQUEST, &e.to_string()))?;
    let sig = parse_signature(&body.signature)
        .map_err(|e| err(StatusCode::BAD_REQUEST, &e.to_string()))?;
    verify_pair_signature(&hotkey, &encoded, &sig)
        .map_err(|e| err(StatusCode::UNAUTHORIZED, &e.to_string()))?;
    let hk = hotkey_hex(&hotkey);
    let claim = st
        .store
        .bind_pair(&body.account_id, &hk, &body.nonce, now, &st.session_secret)
        .map_err(|e| err(StatusCode::CONFLICT, &e.to_string()))?;
    Ok((
        StatusCode::CREATED,
        Json(PairResp {
            session: claim.token,
            account_id: claim.account_id,
            miner_hotkey: claim.miner_hotkey,
            session_id: claim.session_id,
        }),
    ))
}

#[derive(Debug, Deserialize)]
struct ReportBody {
    session: String,
    hotkey: Option<String>,
    title: String,
    body: String,
    repro_steps: Option<String>,
}

#[derive(Debug, Serialize)]
struct ReportResp {
    id: String,
    miner_hotkey: String,
    state: ReportState,
    fingerprint: String,
}

async fn submit_report(
    State(st): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ReportBody>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    // Miner BYOK: accepted and never logged. Absence is OK (no live Lium).
    let _lium_present = headers
        .get("x-lium-api-key")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|s| !s.is_empty());

    // A host with no scoring backend cannot turn this report into weight.
    // Accepting it anyway would take real work — finding a real bug — and pay
    // nothing for it, so refuse before anything is stored.
    if !st.can_score() {
        return Err(err(
            StatusCode::SERVICE_UNAVAILABLE,
            "scoring unconfigured: set BOUNTY_BACKEND_PUBLIC_URL, \
             or BOUNTY_FORCE_SIM=1 for CI",
        ));
    }

    let pairing = st
        .store
        .lookup_session(&body.session, &st.session_secret)
        .map_err(|_| err(StatusCode::UNAUTHORIZED, "invalid_session"))?;
    if let Some(ref hk) = body.hotkey {
        let parsed = parse_hotkey(hk).map_err(|e| err(StatusCode::BAD_REQUEST, &e.to_string()))?;
        if hotkey_hex(&parsed) != pairing.miner_hotkey {
            return Err(err(StatusCode::FORBIDDEN, "hotkey_mismatch"));
        }
    }
    let repro = body.repro_steps.unwrap_or_default();
    validate_substance(&body.title, &body.body, &repro)?;

    let fingerprint = report_fingerprint(&body.title, &body.body);
    let row = Report {
        id: String::new(),
        miner_hotkey: pairing.miner_hotkey,
        account_id: pairing.account_id,
        title: body.title,
        body: body.body,
        repro_steps: repro,
        fingerprint,
        state: ReportState::Pending,
        adjudication: None,
        severity: None,
        duplicate_of: None,
        champion_verdict: None,
        created_at: 0,
    };
    let row = st.store.insert_report(row, unix_now()).map_err(|e| match e {
        StoreError::Quota(m) => err(StatusCode::TOO_MANY_REQUESTS, &m),
        _ => err(StatusCode::INTERNAL_SERVER_ERROR, "store"),
    })?;
    Ok((
        StatusCode::CREATED,
        Json(ReportResp {
            id: row.id,
            miner_hotkey: row.miner_hotkey,
            state: row.state,
            fingerprint: row.fingerprint,
        }),
    ))
}

async fn list_reports(State(st): State<AppState>) -> impl IntoResponse {
    let rows = st.store.list_reports().unwrap_or_default();
    Json(serde_json::json!({ "items": rows }))
}

async fn get_report(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let row = st
        .store
        .get_report(&id)
        .map_err(|_| err(StatusCode::NOT_FOUND, "not_found"))?;
    Ok(Json(row))
}

/// Reject reports too thin to be worth a triage pass.
///
/// This is not a quality bar — an operator still decides whether the bug is
/// real. It exists so a one-word submission cannot occupy a queue slot.
fn validate_substance(
    title: &str,
    body: &str,
    repro: &str,
) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    if title.trim().is_empty() || body.trim().is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "title_and_body_required"));
    }
    if body.trim().chars().count() < MIN_REPORT_BODY_CHARS {
        return Err(err(
            StatusCode::BAD_REQUEST,
            &format!("body must be at least {MIN_REPORT_BODY_CHARS} characters"),
        ));
    }
    if repro.trim().chars().count() < MIN_REPRO_CHARS {
        return Err(err(
            StatusCode::BAD_REQUEST,
            &format!("repro_steps must be at least {MIN_REPRO_CHARS} characters"),
        ));
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct AdjudicateBody {
    report_id: String,
    verdict: Adjudication,
    /// Operator severity. Required to credit a `valid` verdict.
    #[serde(default)]
    severity: Option<Severity>,
    duplicate_of: Option<String>,
}

async fn adjudicate(
    State(st): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<AdjudicateBody>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    if st.admin_hashes.is_empty() {
        return Err(err(StatusCode::SERVICE_UNAVAILABLE, "auth_unconfigured"));
    }
    if !admin_ok(&headers, &st.admin_hashes) {
        return Err(err(StatusCode::UNAUTHORIZED, "unauthorized"));
    }
    let row = st
        .store
        .adjudicate(
            &body.report_id,
            body.verdict,
            body.severity,
            body.duplicate_of,
        )
        .map_err(|e| {
            let code = if e.to_string().contains("unknown") {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::CONFLICT
            };
            err(code, &e.to_string())
        })?;
    Ok(Json(row))
}

fn admin_ok(headers: &HeaderMap, hashes: &[String]) -> bool {
    let Some(raw) = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    else {
        return false;
    };
    let token = raw.strip_prefix("Bearer ").unwrap_or(raw).trim();
    if token.is_empty() {
        return false;
    }
    let mut h = Sha256::new();
    h.update(token.as_bytes());
    let got = hex::encode(h.finalize());
    hashes.iter().any(|x| x == &got)
}

fn err(code: StatusCode, msg: &str) -> (StatusCode, Json<serde_json::Value>) {
    (code, Json(serde_json::json!({ "error": msg })))
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Hash an admin token the same way the server does.
#[must_use]
pub fn hash_admin_token(token: &str) -> String {
    let mut h = Sha256::new();
    h.update(token.as_bytes());
    hex::encode(h.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use bounty_challenge_task::{
        hotkey_ss58, pairing_code, public_from_mini_secret, sign_pair_challenge, PairChallenge,
    };
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn dummy_secret() -> [u8; 32] {
        let mut s = [0x11u8; 32];
        s[0] = 0x42;
        s
    }

    fn dummy_public() -> [u8; 32] {
        public_from_mini_secret(&dummy_secret()).expect("pk")
    }

    fn app() -> (Router, String) {
        app_scoring(ScoringBackend::Sim)
    }

    fn app_scoring(scoring: ScoringBackend) -> (Router, String) {
        let token = "op-test-token";
        let router = bounty_router(AppState {
            store: MemoryStore::new(),
            session_secret: Arc::new(b"test-session-secret".to_vec()),
            scoring,
            admin_hashes: Arc::new(vec![hash_admin_token(token)]),
        });
        (router, token.to_owned())
    }

    /// A report body that clears the substance floor.
    fn report_body(session: &str, title: &str) -> serde_json::Value {
        serde_json::json!({
            "session": session,
            "title": title,
            "body": format!(
                "{title}: POST /v1/admin/seal returns 500 when the bundle is empty, \
                 instead of the documented 400. Observed on master at commit tip."
            ),
            "repro_steps": "curl the seal route with no leaves and watch it 500",
        })
    }

    async fn json_req(
        app: Router,
        method: &str,
        uri: &str,
        body: serde_json::Value,
        auth: Option<&str>,
    ) -> (StatusCode, serde_json::Value) {
        let mut b = Request::builder().method(method).uri(uri);
        if let Some(a) = auth {
            b = b.header(axum::http::header::AUTHORIZATION, format!("Bearer {a}"));
        }
        let req = b
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .expect("req");
        let resp = app.oneshot(req).await.expect("resp");
        let status = resp.status();
        let bytes = resp.into_body().collect().await.expect("body").to_bytes();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or(serde_json::json!({}));
        (status, v)
    }

    fn pair_payload(exp: u64) -> serde_json::Value {
        let pk = dummy_public();
        let c = PairChallenge {
            account_id: "acct-http".into(),
            nonce: "0123456789abcdef".into(),
            exp,
        };
        let challenge = c.encode().expect("enc");
        let sig = sign_pair_challenge(&dummy_secret(), &challenge).expect("sign");
        let _code = pairing_code(&challenge, &hex::encode(sig), &hotkey_ss58(&pk));
        serde_json::json!({
            "account_id": c.account_id,
            "hotkey": hotkey_ss58(&pk),
            "nonce": c.nonce,
            "exp": c.exp,
            "signature": hex::encode(sig),
            "terms_accepted": true,
        })
    }

    #[tokio::test]
    async fn pair_report_adjudicate_happy_path() {
        let (app, token) = app();
        let (st, health) =
            json_req(app.clone(), "GET", "/health", serde_json::json!({}), None).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(health["challenge_id"], CHALLENGE_ID);

        let exp = unix_now().saturating_add(600);
        let (st, paired) = json_req(app.clone(), "POST", "/v1/pair", pair_payload(exp), None).await;
        assert_eq!(st, StatusCode::CREATED, "{paired}");
        let session = paired["session"].as_str().expect("session");

        let (st, created) = json_req(
            app.clone(),
            "POST",
            "/v1/reports",
            report_body(session, "gateway 500 on seal"),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{created}");
        assert_eq!(created["state"], "pending");
        let id = created["id"].as_str().expect("id");

        let (st, adj) = json_req(
            app,
            "POST",
            "/v1/admin/adjudicate",
            serde_json::json!({
                "report_id": id,
                "verdict": "valid",
                "severity": "major",
            }),
            Some(&token),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{adj}");
        assert_eq!(adj["state"], "valid");
        assert_eq!(adj["adjudication"], "valid");
        assert_eq!(adj["severity"], "major");
    }

    #[tokio::test]
    async fn pair_requires_terms() {
        let (app, _) = app();
        let mut body = pair_payload(unix_now().saturating_add(600));
        body["terms_accepted"] = serde_json::json!(false);
        let (st, v) = json_req(app, "POST", "/v1/pair", body, None).await;
        assert_eq!(st, StatusCode::FORBIDDEN);
        assert_eq!(v["error"], "terms_required");
    }

    #[tokio::test]
    async fn pair_rejects_bad_signature() {
        let (app, _) = app();
        let mut body = pair_payload(unix_now().saturating_add(600));
        body["signature"] = serde_json::json!("00".repeat(64));
        let (st, _) = json_req(app, "POST", "/v1/pair", body, None).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn adjudicate_requires_bearer() {
        let (app, _) = app();
        let (st, _) = json_req(
            app,
            "POST",
            "/v1/admin/adjudicate",
            serde_json::json!({ "report_id": "by_0", "verdict": "valid" }),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);
    }

    /// Each verdict gets its own fresh host: the per-hotkey rate window is a
    /// real gate, and back-to-back submits from one miner are supposed to be
    /// refused (`one_hotkey_cannot_file_back_to_back`).
    async fn adjudicated_once(title: &str, verdict: &str) -> serde_json::Value {
        let (app, token) = app();
        let exp = unix_now().saturating_add(600);
        let (st, paired) = json_req(app.clone(), "POST", "/v1/pair", pair_payload(exp), None).await;
        assert_eq!(st, StatusCode::CREATED, "{paired}");
        let session = paired["session"].as_str().expect("session");

        let (st, created) = json_req(
            app.clone(),
            "POST",
            "/v1/reports",
            report_body(session, title),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{created}");
        let (st, adj) = json_req(
            app,
            "POST",
            "/v1/admin/adjudicate",
            serde_json::json!({ "report_id": created["id"], "verdict": verdict }),
            Some(&token),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{adj}");
        adj
    }

    #[tokio::test]
    async fn already_fixed_is_an_ack_and_malicious_is_a_penalty() {
        let ack = adjudicated_once("fixed already", "already_fixed_not_prod").await;
        assert_eq!(ack["state"], "already_fixed_not_prod");
        assert_eq!(ack["severity"], serde_json::Value::Null);

        let bad = adjudicated_once("invented crash", "invalid_malicious").await;
        assert_eq!(bad["state"], "invalid_malicious");
    }

    /// A host with no backend feed and no sim opt-in cannot turn a report into
    /// weight, so it must refuse the work instead of banking it unpaid.
    #[tokio::test]
    async fn an_unconfigured_host_refuses_reports_instead_of_collecting_them() {
        let (app, _) = app_scoring(ScoringBackend::Unconfigured);
        let exp = unix_now().saturating_add(600);
        let (st, paired) = json_req(app.clone(), "POST", "/v1/pair", pair_payload(exp), None).await;
        assert_eq!(st, StatusCode::CREATED, "{paired}");
        let session = paired["session"].as_str().expect("session");

        let (st, body) = json_req(
            app.clone(),
            "POST",
            "/v1/reports",
            report_body(session, "a real bug"),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE, "{body}");
        assert!(body["error"]
            .as_str()
            .unwrap_or_default()
            .contains("scoring unconfigured"));

        let (st, list) = json_req(app, "GET", "/v1/reports", serde_json::json!({}), None).await;
        assert_eq!(st, StatusCode::OK);
        assert!(
            list["items"].as_array().is_some_and(Vec::is_empty),
            "a refused report must not be banked: {list}"
        );
    }

    #[tokio::test]
    async fn status_publishes_the_scorer_and_the_off_score_gate() {
        let (app, _) = app_scoring(ScoringBackend::Unconfigured);
        let (st, body) = json_req(app, "GET", "/v1/status", serde_json::json!({}), None).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(body["scoring_backend"], "unconfigured");
        assert_eq!(body["can_score"], false);
        assert_eq!(body["force_sim"], false);
        let paid = body["scoring"]["paid_on"].to_string();
        assert!(paid.contains("precision") && paid.contains("severity_impact"), "{paid}");
        assert!(
            body["scoring"]["off_score_gates"]
                .to_string()
                .contains("triage_noise"),
            "the canary must be named and must not be in paid_on"
        );
        assert!(!paid.contains("triage_noise"), "{paid}");
        assert_eq!(
            body["quotas"]["max_pending_reports_per_hotkey"],
            MAX_PENDING_REPORTS_PER_HOTKEY
        );
    }

    /// Thin reports occupy a triage slot without carrying enough to triage.
    #[tokio::test]
    async fn reports_too_thin_to_triage_are_refused() {
        let (app, _) = app();
        let exp = unix_now().saturating_add(600);
        let (_st, paired) =
            json_req(app.clone(), "POST", "/v1/pair", pair_payload(exp), None).await;
        let session = paired["session"].as_str().expect("session");

        for body in [
            serde_json::json!({ "session": session, "title": "x", "body": "broken" }),
            serde_json::json!({
                "session": session,
                "title": "no repro",
                "body": "a".repeat(MIN_REPORT_BODY_CHARS),
            }),
        ] {
            let (st, v) = json_req(app.clone(), "POST", "/v1/reports", body, None).await;
            assert_eq!(st, StatusCode::BAD_REQUEST, "{v}");
        }
    }

    #[tokio::test]
    async fn one_hotkey_cannot_file_back_to_back() {
        let (app, _) = app();
        let exp = unix_now().saturating_add(600);
        let (_st, paired) =
            json_req(app.clone(), "POST", "/v1/pair", pair_payload(exp), None).await;
        let session = paired["session"].as_str().expect("session");

        let (st, _) = json_req(
            app.clone(),
            "POST",
            "/v1/reports",
            report_body(session, "first finding"),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::CREATED);

        let (st, v) = json_req(
            app,
            "POST",
            "/v1/reports",
            report_body(session, "second finding"),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::TOO_MANY_REQUESTS, "{v}");
    }

    #[tokio::test]
    async fn does_not_serve_public_leaderboard() {
        let (app, _) = app();
        let (st, _) = json_req(
            app,
            "GET",
            "/v1/public/leaderboard",
            serde_json::json!({}),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::NOT_FOUND);
    }
}
