//! Relearn T2I HTTP API (master-only).
//!
//! ```text
//! GET  /health
//! GET  /v1/status
//! GET  /v1/prompts            frozen public split + seeds (holdout stays sealed)
//! POST /v1/submissions        miner submit (digest + manifest + X-Lium-Api-Key)
//! GET  /v1/submissions
//! GET  /v1/submissions/{id}
//! POST /v1/admin/promote      operator-audited champion flip
//! ```
//!
//! `/v1/prompts` publishes the public split verbatim, including the derived
//! seeds, so every miner can reproduce the exact scored cells. It never
//! publishes the holdout: that response carries the commitment and the size
//! only.

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
use relearn_t2i_eval::{eval_after_freeze, JudgeBackend, JudgeConfig};
use relearn_t2i_judge::JudgeInference;
use relearn_t2i_score::judge_challenger;
use relearn_t2i_store::{
    freeze_submission_digest, ArtifactManifest, MemoryStore, Submission, SubmissionState,
};
use relearn_t2i_task::{
    cell_key, RelearnT2iPin, BASE_MODEL_ID, BASE_MODEL_LICENSE, BASE_MODEL_LICENSE_URL,
    CHALLENGE_ID, JUDGE_DATASET_ID, JUDGE_MODEL_ID, REJECTED_BASE_SUBSTRINGS, SCORE_MAX,
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
    pub pin: RelearnT2iPin,
    /// Judge wiring resolved once at boot (never read from env per request).
    pub judge: JudgeConfig,
    /// Operator bearer hashes (sha256 hex). Empty → admin 503.
    pub admin_hashes: Arc<Vec<String>>,
}

/// Build the router.
pub fn relearn_t2i_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/v1/status", get(status))
        .route("/v1/prompts", get(prompts))
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
    let champ = st.store.champion_id().ok().flatten();
    let seal = st.store.holdout_seal().ok();
    let judge = JudgeInference::default();
    Json(serde_json::json!({
        "challenge_id": CHALLENGE_ID,
        "scoring_version": SCORING_VERSION,
        "score_max": SCORE_MAX,
        "base_model": st.pin.base,
        "base_license": st.pin.base_license,
        "base_license_url": st.pin.base_license_url,
        "base_revision": st.pin.base_revision,
        "rejected_base_families": REJECTED_BASE_SUBSTRINGS,
        "judge_model": st.pin.judge_model,
        "judge_dataset": st.pin.judge_dataset,
        "judge_inference": judge,
        "judge_backend": st.judge.backend,
        // Whether an endpoint is set, never the endpoint itself.
        "judge_endpoint_configured": st.judge.endpoint_configured(),
        "sampler": st.pin.sampler,
        "eval_image": st.pin.eval_image,
        "eval_image_digest": st.pin.eval_image_digest,
        "holdout": seal,
        "champion_id": champ,
    }))
}

#[derive(Debug, Serialize)]
struct PromptCell {
    prompt_id: u32,
    variation_index: u32,
    seed: u64,
    cell_key: String,
    prompt: String,
}

async fn prompts(State(st): State<AppState>) -> impl IntoResponse {
    let cells: Vec<PromptCell> = st
        .pin
        .seed_cells(&st.pin.prompts.public_ids)
        .into_iter()
        .filter_map(|c| {
            let record = st.pin.frozen_prompts.iter().find(|p| p.id == c.prompt_id)?;
            Some(PromptCell {
                prompt_id: c.prompt_id,
                variation_index: c.variation_index,
                seed: c.seed,
                cell_key: cell_key(c.prompt_id, c.variation_index),
                prompt: record.generator_input().to_owned(),
            })
        })
        .collect();
    Json(serde_json::json!({
        "dataset": JUDGE_DATASET_ID,
        "pin_salt": st.pin.prompts.pin_salt,
        "variations_per_prompt": st.pin.prompts.variations_per_prompt,
        "sampler": st.pin.sampler,
        "public": cells,
        // Holdout stays sealed: commitment and size only, never ids or text.
        "holdout": st.store.holdout_seal().ok(),
    }))
}

#[derive(Debug, Deserialize)]
struct SubmitBody {
    miner_hotkey: String,
    artifact_digest: String,
    artifact_uri: Option<String>,
    #[serde(default)]
    manifest: ArtifactManifest,
}

#[derive(Debug, Serialize)]
struct SubmitResp {
    id: String,
    submission_digest: String,
    state: SubmissionState,
    judge_backend: JudgeBackend,
    holdout_cells: usize,
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
    h.update(b"relearn-t2i-nonce-v1");
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
    // Miner BYOK: accepted and never logged. Absence is OK for sim.
    let _lium_present = headers
        .get("x-lium-api-key")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|s| !s.is_empty());

    // License attestation is checked before anything is stored, so a Flux
    // fine-tune is a 400 with a reason rather than a row that quietly scores 0.
    st.pin
        .attest_artifact_base(&body.manifest.base, &body.manifest.base_license)
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

    let holdout = st
        .store
        .unseal_holdout(&submission_digest)
        .map_err(|e| err(StatusCode::SERVICE_UNAVAILABLE, &e.to_string()))?;

    let eval = eval_after_freeze(
        &st.pin,
        &holdout,
        &submission_digest,
        &artifact,
        &body.manifest,
        &st.judge,
    )
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let champ = st.store.champion_scores().ok().flatten().ok_or_else(|| {
        err(
            StatusCode::SERVICE_UNAVAILABLE,
            "no champion baseline recorded",
        )
    })?;
    let verdict = judge_challenger(&champ, &eval.scores);
    st.store
        .record_scores(&row.id, eval.scores.clone())
        .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "store"))?;

    let eligible = verdict.eligible;
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
            judge_backend: eval.backend,
            holdout_cells: eval.holdout_cells,
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

/// Manifest a well-formed submission declares (pinned base + license).
#[must_use]
pub fn pinned_manifest() -> ArtifactManifest {
    ArtifactManifest {
        base: BASE_MODEL_ID.into(),
        base_license: BASE_MODEL_LICENSE.into(),
        ..ArtifactManifest::default()
    }
}

/// Documented license URL for the pinned base.
#[must_use]
pub const fn base_license_url() -> &'static str {
    BASE_MODEL_LICENSE_URL
}

/// Judge model id served on `/v1/status`.
#[must_use]
pub const fn judge_model() -> &'static str {
    JUDGE_MODEL_ID
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use relearn_t2i_eval::base_champion_scores;
    use relearn_t2i_task::{frozen_prompt_commitment, FrozenPrompt, PromptPin};
    use tower::ServiceExt;

    use super::*;

    fn prompt(id: u32) -> FrozenPrompt {
        FrozenPrompt {
            id,
            text: format!("prompt {id}"),
            upsampled_json: None,
        }
    }

    fn holdout() -> Vec<FrozenPrompt> {
        (900..=924).map(prompt).collect()
    }

    fn test_pin() -> RelearnT2iPin {
        let public: Vec<FrozenPrompt> = (1..=25).map(prompt).collect();
        RelearnT2iPin {
            prompts: PromptPin {
                pin_salt: "cortex-t2i-test".into(),
                variations_per_prompt: 4,
                public_ids: public.iter().map(|p| p.id).collect(),
                holdout_commitment: frozen_prompt_commitment(&holdout()),
                holdout_size: 25,
            },
            frozen_prompts: public,
            ..RelearnT2iPin::default()
        }
    }

    fn app(token: &str) -> Router {
        let pin = test_pin();
        let store = MemoryStore::new();
        store
            .set_holdout_commitment(&pin.prompts.holdout_commitment, pin.prompts.holdout_size)
            .expect("commit");
        store
            .load_holdout(holdout(), &pin.prompts.public_ids)
            .expect("load holdout");
        let ids: Vec<u32> = holdout().iter().map(|p| p.id).collect();
        store
            .set_base_champion(base_champion_scores(&pin, &ids).expect("base"))
            .expect("seed base");
        relearn_t2i_router(AppState {
            store,
            pin,
            judge: JudgeConfig::sim(),
            admin_hashes: Arc::new(vec![hash_admin_token(token)]),
        })
    }

    fn digest(label: &str) -> String {
        let mut h = Sha256::new();
        h.update(label.as_bytes());
        hex::encode(h.finalize())
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

    fn submit_body(label: &str, manifest: &serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "miner_hotkey": digest("miner-hotkey"),
            "artifact_digest": digest(label),
            "manifest": manifest,
        })
    }

    fn pinned_manifest_json() -> serde_json::Value {
        serde_json::json!({
            "base": BASE_MODEL_ID,
            "base_license": BASE_MODEL_LICENSE,
        })
    }

    #[tokio::test]
    async fn health_and_status_report_the_pins() {
        let app = app("op");
        let (st, health) =
            json_req(app.clone(), "GET", "/health", serde_json::json!({}), None).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(health["challenge_id"], CHALLENGE_ID);

        let (st, status) = json_req(app, "GET", "/v1/status", serde_json::json!({}), None).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(status["base_model"], BASE_MODEL_ID);
        assert_eq!(status["base_license"], "OpenMDW-1.1");
        assert_eq!(status["judge_model"], JUDGE_MODEL_ID);
        assert_eq!(status["judge_inference"]["seed"], 42);
        assert_eq!(status["judge_inference"]["top_k"], 1);
        assert_eq!(status["judge_inference"]["enable_thinking"], true);
        assert_eq!(status["sampler"]["num_inference_steps"], 50);
        assert_eq!(status["holdout"]["loaded"], true);
        assert_eq!(status["holdout"]["size"], 25);
    }

    #[tokio::test]
    async fn public_prompts_are_published_with_seeds_and_the_holdout_is_not() {
        let (st, body) =
            json_req(app("op"), "GET", "/v1/prompts", serde_json::json!({}), None).await;
        assert_eq!(st, StatusCode::OK);
        let cells = body["public"].as_array().expect("cells");
        assert_eq!(cells.len(), 100);
        assert!(cells[0]["seed"].as_u64().unwrap_or(0) > 0);
        assert_eq!(cells[0]["cell_key"], "p1#v0");
        assert_eq!(body["dataset"], JUDGE_DATASET_ID);

        // Sealed holdout: commitment and size, nothing that identifies a prompt.
        let holdout_json = body["holdout"].to_string();
        assert!(holdout_json.contains("commitment"));
        for id in 900..=924 {
            assert!(
                !body["public"]
                    .to_string()
                    .contains(&format!("\"prompt_id\":{id}")),
                "holdout id {id} leaked into the public split"
            );
        }
        assert!(!holdout_json.contains("prompt 900"));
    }

    #[tokio::test]
    async fn submit_eval_promote_happy_path() {
        let token = "op-test-token";
        let app = app(token);
        let (st, created) = json_req(
            app.clone(),
            "POST",
            "/v1/submissions",
            submit_body("miner-strong-finetune", &pinned_manifest_json()),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{created}");
        assert_eq!(created["judge_backend"], "sim");
        assert_eq!(created["holdout_cells"], 100);

        if created["eligible"] == true {
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
    }

    #[tokio::test]
    async fn flux_submission_is_rejected_at_the_door() {
        let (st, body) = json_req(
            app("op"),
            "POST",
            "/v1/submissions",
            submit_body(
                "flux-finetune",
                &serde_json::json!({
                    "base": "black-forest-labs/FLUX.1-dev",
                    "base_license": "OpenMDW-1.1",
                }),
            ),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
        assert!(
            body["error"].as_str().unwrap_or("").contains("refused"),
            "{body}"
        );
    }

    #[tokio::test]
    async fn missing_license_attestation_is_rejected() {
        let (st, _) = json_req(
            app("op"),
            "POST",
            "/v1/submissions",
            submit_body("no-manifest", &serde_json::json!({})),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn submit_without_a_loaded_holdout_is_unavailable_not_scored() {
        let pin = test_pin();
        let store = MemoryStore::new();
        store
            .set_holdout_commitment(&pin.prompts.holdout_commitment, pin.prompts.holdout_size)
            .expect("commit");
        let app = relearn_t2i_router(AppState {
            store,
            pin,
            judge: JudgeConfig::sim(),
            admin_hashes: Arc::new(vec![hash_admin_token("op")]),
        });
        let (st, body) = json_req(
            app,
            "POST",
            "/v1/submissions",
            submit_body("x", &pinned_manifest_json()),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    }

    #[tokio::test]
    async fn promote_requires_bearer() {
        let (st, _) = json_req(
            app("op"),
            "POST",
            "/v1/admin/promote",
            serde_json::json!({ "submission_id": "t2i_0" }),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn helper_pins_match_the_task_crate() {
        assert_eq!(pinned_manifest().base, BASE_MODEL_ID);
        assert_eq!(base_license_url(), "https://openmdw.ai/license/1-1/");
        assert_eq!(judge_model(), "Qwen/Qwen-Image-Bench");
    }
}
