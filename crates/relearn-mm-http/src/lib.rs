//! Relearn Multimodal HTTP API (master-only).
//!
//! ```text
//! GET  /health
//! GET  /v1/status
//! POST /v1/submissions          miner submit (digest + encoder manifest)
//! GET  /v1/submissions
//! GET  /v1/submissions/{id}
//! POST /v1/admin/promote        operator-audited champion flip
//! ```
//!
//! `/v1/status` publishes the champion LM weights hash: an encoder-only miner
//! needs it to know which language model to attach to, and publishing it costs
//! nothing because it is a hash of already-public champion weights.

#![forbid(unsafe_code)]
#![allow(
    clippy::doc_markdown,
    clippy::module_name_repetitions,
    clippy::must_use_candidate,
    clippy::too_many_lines
)]

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use relearn_mm_eval::{eval_after_freeze, EvalBackend};
use relearn_mm_score::{judge_challenger, LM_EPSILON, MIN_SHUFFLE_DROP};
use relearn_mm_store::{
    freeze_submission_digest, EncoderManifest, MemoryStore, Submission, SubmissionState,
};
use relearn_mm_task::{
    license_is_permissive, RelearnMmPin, VisionTask, CHALLENGE_ID, PERMISSIVE_LICENSES, SCORE_MAX,
    SCORING_VERSION,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Shared HTTP state.
#[derive(Clone)]
pub struct AppState {
    /// Submission store.
    pub store: MemoryStore,
    /// Eval / model pins.
    pub pin: RelearnMmPin,
    /// Eval backend resolved once at boot.
    pub backend: EvalBackend,
    /// Operator bearer hashes (sha256 hex). Empty → admin 503.
    pub admin_hashes: Arc<Vec<String>>,
}

/// Build the router.
pub fn relearn_mm_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/v1/status", get(status))
        .route("/v1/submissions", post(submit).get(list_subs))
        .route("/v1/submissions/{id}", get(get_sub))
        .route("/v1/admin/promote", post(promote))
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
    Json(serde_json::json!({
        "challenge_id": CHALLENGE_ID,
        "scoring_version": SCORING_VERSION,
        "score_max": SCORE_MAX,
        "lm_base_model": st.pin.lm_base_model,
        "encoder_model": st.pin.encoder_model,
        "encoder_license": st.pin.encoder_license,
        "encoder_revision": st.pin.encoder_revision,
        "permissive_licenses": PERMISSIVE_LICENSES,
        "eval_image": st.pin.eval_image,
        "eval_image_digest": st.pin.eval_image_digest,
        "eval_backend": st.backend,
        "vision_tasks": VisionTask::ALL.map(VisionTask::as_str),
        "vision_items_per_task": st.pin.vision_items_per_task,
        "agentic_traces": st.pin.agentic_traces,
        "text_holdout_items": st.pin.text_holdout_items,
        "lm_epsilon": LM_EPSILON,
        "min_shuffle_drop": MIN_SHUFFLE_DROP,
        "champion_lm_weights_hash": st.store.champion_lm_hash().unwrap_or_default(),
        "champion_id": st.store.champion_id().ok().flatten(),
    }))
}

#[derive(Debug, Deserialize)]
struct SubmitBody {
    miner_hotkey: String,
    artifact_digest: String,
    artifact_uri: Option<String>,
    #[serde(default)]
    manifest: EncoderManifest,
}

#[derive(Debug, Serialize)]
struct SubmitResp {
    id: String,
    submission_digest: String,
    state: SubmissionState,
    eval_backend: EvalBackend,
    text_items: usize,
    vision_items: usize,
    lm_intact: bool,
    eligible: bool,
}

fn parse_hex64(s: &str, field: &str) -> Result<String, (StatusCode, Json<serde_json::Value>)> {
    let t = s.trim().trim_start_matches("0x");
    if t.len() != 64 || !t.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(err(StatusCode::BAD_REQUEST, &format!("invalid {field}")));
    }
    Ok(t.to_ascii_lowercase())
}

fn nonce_from(hotkey: &str, digest: &str) -> String {
    let mut h = Sha256::new();
    h.update(b"relearn-mm-nonce-v1");
    h.update(hotkey.as_bytes());
    h.update(digest.as_bytes());
    hex::encode(h.finalize())
}

async fn submit(
    State(st): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<SubmitBody>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let hotkey = parse_hex64(&body.miner_hotkey, "miner_hotkey")?;
    let artifact = parse_hex64(&body.artifact_digest, "artifact_digest")?;
    // Miner BYOK: accepted and never logged.
    let _lium_present = headers
        .get("x-lium-api-key")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|s| !s.is_empty());

    st.pin
        .attest_encoder(&body.manifest.encoder_model, &body.manifest.encoder_license)
        .map_err(|e| err(StatusCode::BAD_REQUEST, &e.to_string()))?;

    let nonce = nonce_from(&hotkey, &artifact);
    let submission_digest = freeze_submission_digest(&hotkey, &artifact, &nonce);

    let row = st
        .store
        .insert(Submission {
            id: String::new(),
            miner_hotkey: hotkey,
            artifact_digest: artifact.clone(),
            artifact_uri: body.artifact_uri,
            manifest: body.manifest.clone(),
            nonce,
            submission_digest: submission_digest.clone(),
            state: SubmissionState::Evaluating,
            receipt_json: None,
            verdict: None,
            detail: None,
        })
        .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "store"))?;

    let eval = eval_after_freeze(
        &st.pin,
        &submission_digest,
        &artifact,
        &body.manifest,
        st.backend,
    )
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let champ = st.store.champion_scores().ok().flatten().ok_or_else(|| {
        err(
            StatusCode::SERVICE_UNAVAILABLE,
            "no champion baseline recorded",
        )
    })?;
    let champion_lm_hash = st.store.champion_lm_hash().unwrap_or_default();
    let verdict = judge_challenger(
        &champ,
        &eval.scores,
        &champion_lm_hash,
        license_is_permissive(&body.manifest.encoder_license),
    );
    st.store
        .record_scores(&row.id, eval.scores.clone())
        .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "store"))?;

    let eligible = verdict.eligible;
    let lm_intact = verdict.lm_intact.is_some_and(|g| g.passes);
    let state = if eligible {
        SubmissionState::AwaitingAdmin
    } else {
        SubmissionState::Rejected
    };
    let detail = if eligible {
        None
    } else {
        Some(format!("gates={:?}", verdict.failed))
    };
    let receipt = serde_json::to_string(&eval.receipt).unwrap_or_default();
    let row = st
        .store
        .patch(&row.id, Some(state), Some(receipt), Some(verdict), detail)
        .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "store"))?;

    Ok((
        StatusCode::CREATED,
        Json(SubmitResp {
            id: row.id,
            submission_digest: row.submission_digest,
            state: row.state,
            eval_backend: eval.backend,
            text_items: eval.text_items,
            vision_items: eval.vision_items,
            lm_intact,
            eligible,
        }),
    ))
}

async fn list_subs(State(st): State<AppState>) -> impl IntoResponse {
    let rows = st.store.list().unwrap_or_default();
    Json(serde_json::json!({ "items": rows }))
}

async fn get_sub(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let row = st
        .store
        .get(&id)
        .map_err(|_| err(StatusCode::NOT_FOUND, "not_found"))?;
    Ok(Json(row))
}

#[derive(Debug, Deserialize)]
struct PromoteBody {
    submission_id: String,
}

async fn promote(
    State(st): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<PromoteBody>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    if st.admin_hashes.is_empty() {
        return Err(err(StatusCode::SERVICE_UNAVAILABLE, "auth_unconfigured"));
    }
    if !admin_ok(&headers, &st.admin_hashes) {
        return Err(err(StatusCode::UNAUTHORIZED, "unauthorized"));
    }
    let row = st.store.promote(&body.submission_id).map_err(|e| {
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

/// Hash an admin token the same way the server does.
#[must_use]
pub fn hash_admin_token(token: &str) -> String {
    let mut h = Sha256::new();
    h.update(token.as_bytes());
    hex::encode(h.finalize())
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use relearn_mm_eval::base_champion_scores;
    use tower::ServiceExt;

    use super::*;

    const CHAMP_HASH: &str = "aaaa1111";

    fn app(token: &str) -> Router {
        let pin = RelearnMmPin::default();
        let store = MemoryStore::new();
        store.set_champion_lm_hash(CHAMP_HASH).expect("hash");
        store
            .set_base_champion(base_champion_scores(&pin, CHAMP_HASH))
            .expect("base");
        relearn_mm_router(AppState {
            store,
            pin,
            backend: EvalBackend::Sim,
            admin_hashes: Arc::new(vec![hash_admin_token(token)]),
        })
    }

    fn digest(label: &str) -> String {
        let mut h = Sha256::new();
        h.update(label.as_bytes());
        hex::encode(h.finalize())
    }

    fn manifest(license: &str, kind: &str, lm_hash: &str) -> serde_json::Value {
        serde_json::json!({
            "encoder_model": "google/siglip2-so400m-patch14-384",
            "encoder_license": license,
            "projector": "2-layer MLP",
            "kind": kind,
            "lm_weights_hash": lm_hash,
        })
    }

    fn submit_body(label: &str, manifest: &serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "miner_hotkey": digest("miner-hotkey"),
            "artifact_digest": digest(label),
            "manifest": manifest,
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

    #[tokio::test]
    async fn status_publishes_the_pins_and_the_champion_lm_hash() {
        let (st, body) =
            json_req(app("op"), "GET", "/v1/status", serde_json::json!({}), None).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(body["lm_base_model"], "Qwen/Qwen3.8-27B");
        assert_eq!(body["encoder_model"], "google/siglip2-so400m-patch14-384");
        assert_eq!(body["encoder_license"], "apache-2.0");
        assert_eq!(body["champion_lm_weights_hash"], CHAMP_HASH);
        assert_eq!(body["eval_backend"], "sim");
        let tasks = body["vision_tasks"].as_array().expect("tasks");
        assert_eq!(tasks.len(), 4);
        assert!(tasks.iter().any(|t| t == "ocr"));
    }

    #[tokio::test]
    async fn encoder_only_submission_with_the_champion_lm_can_promote() {
        let token = "op-test-token";
        let app = app(token);
        let (st, created) = json_req(
            app.clone(),
            "POST",
            "/v1/submissions",
            submit_body(
                "trained-encoder",
                &manifest("apache-2.0", "encoder_only", CHAMP_HASH),
            ),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{created}");
        assert_eq!(created["eval_backend"], "sim");
        assert_eq!(created["lm_intact"], true);
        assert_eq!(created["text_items"], 120);
        assert_eq!(created["vision_items"], 160);
        assert_eq!(created["eligible"], true, "{created}");

        let id = created["id"].as_str().expect("id");
        let (st, promoted) = json_req(
            app,
            "POST",
            "/v1/admin/promote",
            serde_json::json!({ "submission_id": id }),
            Some(token),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{promoted}");
        assert_eq!(promoted["state"], "champion");
    }

    #[tokio::test]
    async fn encoder_only_submission_with_a_different_lm_is_rejected() {
        let (st, created) = json_req(
            app("op"),
            "POST",
            "/v1/submissions",
            submit_body(
                "swapped-lm",
                &manifest("apache-2.0", "encoder_only", "dddd4444"),
            ),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{created}");
        assert_eq!(created["eligible"], false);
        assert_eq!(created["state"], "rejected");
    }

    #[tokio::test]
    async fn non_permissive_encoder_is_rejected_at_the_door() {
        let (st, body) = json_req(
            app("op"),
            "POST",
            "/v1/submissions",
            submit_body(
                "openrail-encoder",
                &manifest("creativeml-openrail-m", "encoder_only", CHAMP_HASH),
            ),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
        assert!(
            body["error"]
                .as_str()
                .unwrap_or("")
                .contains("OSI-permissive"),
            "{body}"
        );
    }

    #[tokio::test]
    async fn promote_requires_bearer() {
        let (st, _) = json_req(
            app("op"),
            "POST",
            "/v1/admin/promote",
            serde_json::json!({ "submission_id": "mm_0" }),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);
    }
}
