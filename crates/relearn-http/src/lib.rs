//! Relearn HTTP API (master-only).
//!
//! ```text
//! GET  /health
//! GET  /v1/status
//! POST /v1/submissions          miner submit (digest + optional X-Lium-Api-Key)
//! GET  /v1/submissions
//! GET  /v1/submissions/{id}
//! POST /v1/admin/promote        operator-audited champion flip
//! ```

#![forbid(unsafe_code)]
#![allow(
    clippy::missing_errors_doc,
    clippy::doc_markdown,
    clippy::must_use_candidate,
    clippy::items_after_statements,
    clippy::too_many_lines
)]

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use std::collections::BTreeSet;

use relearn_challenge_task::{CHALLENGE_ID, SCORE_MAX, SCORING_VERSION};
use relearn_eval::{
    eval_after_freeze, force_sim, resolve_teacher_backend, EvalBackend, EvalError, RelearnPin,
};
use relearn_score::{contamination_evidence, judge_challenger};
use relearn_store::{
    freeze_submission_digest, ArtifactManifest, MemoryStore, Submission, SubmissionState,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Shared HTTP state.
#[derive(Clone)]
pub struct AppState {
    /// Submission store.
    pub store: MemoryStore,
    /// Eval / model pins.
    pub pin: RelearnPin,
    /// Backend that is allowed to produce scores on this host.
    pub backend: EvalBackend,
    /// Operator bearer hashes (sha256 hex). Empty → admin 503.
    pub admin_hashes: Arc<Vec<String>>,
}

/// Build the router.
pub fn relearn_router(state: AppState) -> Router {
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
    let champ = st.store.champion_id().ok().flatten();
    let seal = st.store.holdout_seal().ok();
    Json(serde_json::json!({
        "challenge_id": CHALLENGE_ID,
        "scoring_version": SCORING_VERSION,
        "score_max": SCORE_MAX,
        "base_model": st.pin.base_model,
        "teacher_model": st.pin.teacher_model,
        "teacher_backend": resolve_teacher_backend(),
        "eval_image": st.pin.eval_image,
        "eval_image_digest": st.pin.eval_image_digest,
        // Which scorer this host will actually use, and whether sim was opted
        // into. Miners can see that a run was not a real eval.
        "eval_backend": st.backend,
        "force_sim": force_sim(),
        "can_score": st.backend == EvalBackend::Sim || st.pin.can_rent(),
        "relearn_git": st.pin.relearn_git,
        "relearn_git_sha": st.pin.relearn_git_sha,
        // Commitment + size + loaded. Never ids, prompts, or image hashes.
        "holdout": seal,
        "public_ids": st.pin.public_ids,
        "champion_id": champ,
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
    holdout_unsealed: bool,
    eval_backend: EvalBackend,
    eligible: bool,
}

fn parse_hex64(s: &str, field: &str) -> Result<String, (StatusCode, Json<serde_json::Value>)> {
    let t = s.trim().trim_start_matches("0x");
    if t.len() != 64 || !t.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": format!("invalid {field}")})),
        ));
    }
    Ok(t.to_ascii_lowercase())
}

fn nonce_from(hotkey: &str, digest: &str) -> String {
    let mut h = Sha256::new();
    h.update(b"relearn-nonce-v1");
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

    let nonce = nonce_from(&hotkey, &artifact);
    let submission_digest = freeze_submission_digest(&hotkey, &artifact, &nonce);

    let row = Submission {
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
    };
    let row = st
        .store
        .insert(row)
        .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "store"))?;

    let holdout = st
        .store
        .unseal_holdout(&submission_digest)
        .map_err(|e| err(StatusCode::SERVICE_UNAVAILABLE, &e.to_string()))?;

    let eval = eval_after_freeze(&st.pin, &submission_digest, &artifact, &holdout, st.backend)
        .map_err(|e| eval_err(&e))?;

    let train_ids: BTreeSet<u32> = body.manifest.train_item_ids.iter().copied().collect();
    let train_images: BTreeSet<String> = body
        .manifest
        .train_image_hashes
        .iter()
        .map(|s| s.trim().to_ascii_lowercase())
        .collect();
    let train_datasets: BTreeSet<String> = body
        .manifest
        .train_dataset_ids
        .iter()
        .map(|s| s.trim().to_owned())
        .collect();
    let mut scores = eval.scores;
    // An empty manifest leaves the evidence undeclared, which the judge treats
    // as a failed gate rather than a clean run.
    scores.contamination =
        contamination_evidence(&train_ids, &train_images, &train_datasets, &holdout);

    let champ = st.store.champion_scores().ok().flatten().ok_or_else(|| {
        err(
            StatusCode::SERVICE_UNAVAILABLE,
            "no champion baseline recorded",
        )
    })?;
    let verdict = judge_challenger(&champ, &scores);
    st.store
        .record_scores(&row.id, scores)
        .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "store"))?;
    let eligible = verdict.eligible;
    let state = if eligible {
        SubmissionState::AwaitingAdmin
    } else {
        SubmissionState::Rejected
    };
    let receipt = serde_json::to_string(&eval.receipt).unwrap_or_default();
    let detail = if eligible {
        None
    } else {
        Some(format!("gates={:?}", verdict.failed))
    };
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
            holdout_unsealed: eval.holdout_items > 0,
            eval_backend: eval.backend,
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

/// A host that cannot score is unavailable, not broken. `503` keeps miners
/// retrying instead of reading a sim number as a verdict.
fn eval_err(e: &EvalError) -> (StatusCode, Json<serde_json::Value>) {
    let code = match e {
        EvalError::HoldoutSealed
        | EvalError::EvalImageUnpinned
        | EvalError::LiveHarvestUnavailable => StatusCode::SERVICE_UNAVAILABLE,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    err(code, &e.to_string())
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
    use http_body_util::BodyExt;
    use relearn_challenge_task::{holdout_commitment, HoldoutItem, HoldoutTask};
    use relearn_eval::base_champion_scores;
    use tower::ServiceExt;

    fn digest(label: &str) -> String {
        let mut h = Sha256::new();
        h.update(label.as_bytes());
        hex::encode(h.finalize())
    }

    fn holdout() -> Vec<HoldoutItem> {
        (1..=120)
            .map(|id| {
                let (task, image_hash) = match id % 5 {
                    1 => (HoldoutTask::Captioning, format!("{id:064x}")),
                    2 => (HoldoutTask::Vqa, format!("{:064x}", id + 200)),
                    3 => (HoldoutTask::Ocr, format!("{:064x}", id + 400)),
                    4 => (HoldoutTask::Spatial, format!("{:064x}", id + 600)),
                    _ => (HoldoutTask::Text, String::new()),
                };
                HoldoutItem {
                    id: 800 + id,
                    prompt: format!("holdout item {id} with enough words for a trigram"),
                    dataset_id: "dev".into(),
                    task,
                    image_hash,
                }
            })
            .collect()
    }

    /// Training metadata a real miner declares. An empty manifest is a
    /// separate case (`empty_manifest_cannot_dodge_the_contamination_gate`).
    fn declared_manifest() -> serde_json::Value {
        serde_json::json!({
            "train_item_ids": (1..=40).collect::<Vec<u32>>(),
            "train_image_hashes": [],
            "train_dataset_ids": ["cortex-public-v0"],
        })
    }

    fn app_backend(token: &str, load: bool, backend: EvalBackend, eval_digest: &str) -> Router {
        let recs = holdout();
        let pin = RelearnPin {
            holdout_commitment: holdout_commitment(&recs),
            holdout_size: recs.len(),
            public_ids: (1..=40).collect(),
            eval_image_digest: eval_digest.to_owned(),
            ..RelearnPin::default()
        };
        let store = MemoryStore::new();
        store
            .set_holdout_commitment(&pin.holdout_commitment, pin.holdout_size)
            .expect("commit");
        if load {
            store
                .load_holdout(recs.clone(), &[], &pin.public_ids)
                .expect("load");
            store
                .set_base_champion(base_champion_scores(&recs))
                .expect("base");
        }
        relearn_router(AppState {
            store,
            pin,
            backend,
            admin_hashes: Arc::new(vec![hash_admin_token(token)]),
        })
    }

    fn app_with(token: &str, load: bool) -> Router {
        app_backend(token, load, EvalBackend::Sim, "")
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
    async fn submit_eval_promote_happy_path() {
        let token = "op-test-token";
        let app = app_with(token, true);

        let (st, health) =
            json_req(app.clone(), "GET", "/health", serde_json::json!({}), None).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(health["challenge_id"], CHALLENGE_ID);

        // High-byte digest tends to beat the base champion in sim.
        let artifact = digest("miner-strong-adapter");
        let (st, created) = json_req(
            app.clone(),
            "POST",
            "/v1/submissions",
            serde_json::json!({
                "miner_hotkey": digest("miner-hotkey"),
                "artifact_digest": artifact,
                "manifest": declared_manifest(),
            }),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{created}");
        assert_eq!(
            created["submission_digest"].as_str().unwrap_or("").len(),
            64
        );
        assert!(created["holdout_unsealed"].as_bool().unwrap_or(false));
        assert_eq!(created["eval_backend"], "sim");
        assert_eq!(created["eligible"], true, "{created}");

        {
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
    async fn promote_requires_bearer() {
        let app = app_with("x", true);
        let (st, _) = json_req(
            app,
            "POST",
            "/v1/admin/promote",
            serde_json::json!({ "submission_id": "rl_0" }),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn model_pins_are_verified_ids() {
        assert_eq!(relearn_challenge_task::BASE_MODEL_ID, "Qwen/Qwen3.8-27B");
        assert_eq!(relearn_challenge_task::TEACHER_MODEL_ID, "glm-5.3-flash");
        assert_eq!(
            relearn_challenge_task::TEACHER_NVFP4_ID,
            "LibertAIDAI/GLM-5.3-Flash-NVFP4"
        );
    }

    #[tokio::test]
    async fn status_publishes_the_seal_not_holdout_items() {
        let (st, body) = json_req(
            app_with("op", true),
            "GET",
            "/v1/status",
            serde_json::json!({}),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(body["holdout"]["loaded"], true);
        assert_eq!(body["holdout"]["size"], 120);
        let dump = body.to_string();
        assert!(!dump.contains("holdout item"), "{dump}");
        assert!(!dump.contains("\"id\":801"));
    }

    #[tokio::test]
    async fn submit_without_a_loaded_holdout_is_unavailable_not_scored() {
        let (st, body) = json_req(
            app_with("op", false),
            "POST",
            "/v1/submissions",
            serde_json::json!({
                "miner_hotkey": digest("miner-hotkey"),
                "artifact_digest": digest("x"),
            }),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    }

    #[tokio::test]
    async fn contaminated_training_metadata_cannot_promote() {
        let app = app_with("op", true);
        let hold_id = holdout()[0].id;
        let (st, created) = json_req(
            app.clone(),
            "POST",
            "/v1/submissions",
            serde_json::json!({
                "miner_hotkey": digest("miner-hotkey"),
                "artifact_digest": digest("miner-strong-adapter"),
                "manifest": { "train_item_ids": [hold_id] },
            }),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{created}");
        assert_eq!(created["eligible"], false);
        assert_eq!(created["state"], "rejected");

        let id = created["id"].as_str().expect("id");
        let (_st, row) = json_req(
            app,
            "GET",
            &format!("/v1/submissions/{id}"),
            serde_json::json!({}),
            None,
        )
        .await;
        let failed = row["verdict"]["failed"].to_string();
        assert!(failed.contains("\"contamination\""), "{failed}");
    }

    /// Same artifact digest that `submit_eval_promote_happy_path` promotes, so
    /// the only difference here is the missing training metadata.
    #[tokio::test]
    async fn empty_manifest_cannot_dodge_the_contamination_gate() {
        let app = app_with("op-test-token", true);
        for manifest in [
            serde_json::Value::Null,
            serde_json::json!({}),
            serde_json::json!({
                "train_item_ids": [],
                "train_image_hashes": [],
                "train_dataset_ids": [],
            }),
        ] {
            let mut body = serde_json::json!({
                "miner_hotkey": digest("miner-hotkey"),
                "artifact_digest": digest("miner-strong-adapter"),
            });
            if !manifest.is_null() {
                body["manifest"] = manifest.clone();
            }
            let (st, created) = json_req(app.clone(), "POST", "/v1/submissions", body, None).await;
            assert_eq!(st, StatusCode::CREATED, "{created}");
            assert_eq!(created["eligible"], false, "manifest={manifest}");
            assert_eq!(created["state"], "rejected", "manifest={manifest}");

            let id = created["id"].as_str().expect("id");
            let (st, row) = json_req(
                app.clone(),
                "GET",
                &format!("/v1/submissions/{id}"),
                serde_json::json!({}),
                None,
            )
            .await;
            assert_eq!(st, StatusCode::OK);
            let failed = row["verdict"]["failed"].to_string();
            assert!(
                failed.contains("contamination_evidence_missing"),
                "manifest={manifest} failed={failed}"
            );
        }
    }

    #[tokio::test]
    async fn live_submit_refuses_without_a_pinned_eval_image() {
        let (st, body) = json_req(
            app_backend("op", true, EvalBackend::Lium, ""),
            "POST",
            "/v1/submissions",
            serde_json::json!({
                "miner_hotkey": digest("miner-hotkey"),
                "artifact_digest": digest("miner-strong-adapter"),
                "manifest": declared_manifest(),
            }),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE, "{body}");
        let msg = body["error"].as_str().unwrap_or_default();
        assert!(msg.contains("eval image digest not pinned"), "{body}");
    }

    #[tokio::test]
    async fn live_submit_never_scores_with_the_sim_harness() {
        let (st, body) = json_req(
            app_backend(
                "op",
                true,
                EvalBackend::Lium,
                &format!("sha256:{}", "ab".repeat(32)),
            ),
            "POST",
            "/v1/submissions",
            serde_json::json!({
                "miner_hotkey": digest("miner-hotkey"),
                "artifact_digest": digest("miner-strong-adapter"),
                "manifest": declared_manifest(),
            }),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE, "{body}");
        assert!(body["error"]
            .as_str()
            .unwrap_or_default()
            .contains("no in-process sim"));
    }

    #[tokio::test]
    async fn status_reports_the_scorer_this_host_will_use() {
        let (st, sim) = json_req(
            app_with("op", true),
            "GET",
            "/v1/status",
            serde_json::json!({}),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(sim["eval_backend"], "sim");
        assert_eq!(sim["can_score"], true);

        let (st, live) = json_req(
            app_backend("op", true, EvalBackend::Lium, ""),
            "GET",
            "/v1/status",
            serde_json::json!({}),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(live["eval_backend"], "lium");
        assert_eq!(live["eval_image_digest"], "");
        assert_eq!(live["can_score"], false);
    }
}
