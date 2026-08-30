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
    backend_public_url, hotkey_hex, parse_hotkey, parse_signature, verify_pair_signature,
    PairChallenge, CHALLENGE_ID, SCORE_MAX, SCORING_VERSION, TERMS_TEXT,
};
use bounty_score::Adjudication;
use bounty_store::{report_fingerprint, MemoryStore, Report, ReportState};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Shared HTTP state.
#[derive(Clone)]
pub struct AppState {
    /// Pairings + reports.
    pub store: MemoryStore,
    /// HMAC secret for session claims.
    pub session_secret: Arc<Vec<u8>>,
    /// Operator bearer hashes (sha256 hex). Empty → admin 503.
    pub admin_hashes: Arc<Vec<String>>,
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
        "backend_public_configured": backend_public_url().is_some(),
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
    if body.title.trim().is_empty() || body.body.trim().is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "title_and_body_required"));
    }
    let fingerprint = report_fingerprint(&body.title, &body.body);
    let row = Report {
        id: String::new(),
        miner_hotkey: pairing.miner_hotkey,
        account_id: pairing.account_id,
        title: body.title,
        body: body.body,
        repro_steps: body.repro_steps.unwrap_or_default(),
        fingerprint,
        state: ReportState::Pending,
        adjudication: None,
        duplicate_of: None,
        champion_verdict: None,
    };
    let row = st
        .store
        .insert_report(row)
        .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "store"))?;
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

#[derive(Debug, Deserialize)]
struct AdjudicateBody {
    report_id: String,
    verdict: Adjudication,
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
        .adjudicate(&body.report_id, body.verdict, body.duplicate_of)
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
        let token = "op-test-token";
        let router = bounty_router(AppState {
            store: MemoryStore::new(),
            session_secret: Arc::new(b"test-session-secret".to_vec()),
            admin_hashes: Arc::new(vec![hash_admin_token(token)]),
        });
        (router, token.to_owned())
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
            serde_json::json!({
                "session": session,
                "title": "gateway 500 on seal",
                "body": "POST /v1/admin/seal returns 500 when bundle is empty",
                "repro_steps": "curl the seal route with no leaves",
            }),
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
            }),
            Some(&token),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{adj}");
        assert_eq!(adj["state"], "valid");
        assert_eq!(adj["adjudication"], "valid");
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

    #[tokio::test]
    async fn already_fixed_ack_and_malicious_penalty() {
        let (app, token) = app();
        let exp = unix_now().saturating_add(600);
        let (st, paired) = json_req(app.clone(), "POST", "/v1/pair", pair_payload(exp), None).await;
        assert_eq!(st, StatusCode::CREATED, "{paired}");
        let session = paired["session"].as_str().expect("session");

        let (st, a) = json_req(
            app.clone(),
            "POST",
            "/v1/reports",
            serde_json::json!({
                "session": session,
                "title": "fixed already",
                "body": "this was patched last week",
            }),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{a}");
        let (st, adj) = json_req(
            app.clone(),
            "POST",
            "/v1/admin/adjudicate",
            serde_json::json!({
                "report_id": a["id"],
                "verdict": "already_fixed_not_prod",
            }),
            Some(&token),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{adj}");
        assert_eq!(adj["state"], "already_fixed_not_prod");

        let (st, b) = json_req(
            app.clone(),
            "POST",
            "/v1/reports",
            serde_json::json!({
                "session": session,
                "title": "invented crash",
                "body": "does not exist",
            }),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{b}");
        let (st, adj) = json_req(
            app,
            "POST",
            "/v1/admin/adjudicate",
            serde_json::json!({
                "report_id": b["id"],
                "verdict": "invalid_malicious",
            }),
            Some(&token),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{adj}");
        assert_eq!(adj["state"], "invalid_malicious");
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
