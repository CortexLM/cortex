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
    eval_after_freeze, force_sim, resolve_teacher_backend, scoring_readiness, EvalBackend,
    EvalError, LiveScorer, RelearnPin,
};
use relearn_score::{
    contamination_evidence, judge_challenger, pre_eval_contamination_verdict,
    ContaminationEvidence, PromoteVerdict, SliceScores,
};
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
    /// Harvest handle for the digest-pinned eval image. `None` on a live host
    /// means nothing can score, so submissions refuse.
    pub live_scorer: Option<Arc<dyn LiveScorer>>,
    /// Operator bearer hashes (sha256 hex). Empty → admin 503.
    pub admin_hashes: Arc<Vec<String>>,
}

impl AppState {
    /// Borrow the live harvest handle, if the operator wired one.
    fn live(&self) -> Option<&dyn LiveScorer> {
        self.live_scorer.as_deref()
    }

    /// Whether a champion baseline is in the store.
    fn champion_recorded(&self) -> bool {
        self.store.champion_scores().ok().flatten().is_some()
    }

    /// Whether the operator holdout is verified loaded on this host.
    fn holdout_loaded(&self) -> bool {
        self.store.holdout_seal().ok().is_some_and(|s| s.loaded)
    }

    /// Whether this host can produce a verdict at all.
    ///
    /// False until the holdout is loaded **and** a champion baseline is
    /// recorded. Submit already 503s in both cases; status must not
    /// contradict that.
    fn can_score(&self) -> bool {
        self.holdout_loaded()
            && scoring_readiness(&self.pin, self.backend, self.live()).is_ok()
            && self.champion_recorded()
    }
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
        "can_score": st.can_score(),
        // Both are prerequisites for a live verdict, so name them separately:
        // "can_score: false" without them is the usual operator confusion.
        "live_harvest_wired": st.live_scorer.is_some(),
        "champion_baseline_recorded": st.champion_recorded(),
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

    // Everything that can refuse runs before the row exists. A 503 means
    // scoring never started, so it must not leave a spammable `evaluating`
    // row behind that carries no scores and shows up on no operator surface.
    let holdout = st
        .store
        .unseal_holdout(&submission_digest)
        .map_err(|e| err(StatusCode::SERVICE_UNAVAILABLE, &e.to_string()))?;

    // Root cause first: an unpinned digest is why there is no harvest and no
    // baseline either, so report that rather than a downstream symptom.
    scoring_readiness(&st.pin, st.backend, st.live()).map_err(|e| eval_err(&e))?;

    // Before the eval, not after: on a live host the eval spends the miner's
    // Lium budget, and there is no verdict to be had without a baseline.
    let champ = st.store.champion_scores().ok().flatten().ok_or_else(|| {
        err(
            StatusCode::SERVICE_UNAVAILABLE,
            "no champion baseline recorded",
        )
    })?;

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
    let contamination =
        contamination_evidence(&train_ids, &train_images, &train_datasets, &holdout);

    // Before the rent: a contaminated or silent manifest cannot produce a
    // lattice score, so it must not spend a Lium pod (or the teacher key).
    if let Some(verdict) = pre_eval_contamination_verdict(&contamination) {
        return persist_pre_eval_reject(
            &st,
            body,
            hotkey,
            artifact,
            nonce,
            submission_digest,
            contamination,
            verdict,
            holdout.len(),
        );
    }

    let eval = eval_after_freeze(
        &st.pin,
        &submission_digest,
        &artifact,
        &holdout,
        st.backend,
        st.live(),
    )
    .await
    .map_err(|e| eval_err(&e))?;

    let mut scores = eval.scores;
    scores.contamination = contamination;

    let verdict = judge_challenger(&champ, &scores);
    let eligible = verdict.eligible;
    let detail = if eligible {
        None
    } else {
        Some(format!("gates={:?}", verdict.failed))
    };
    // Scoring finished, so the attempt is now worth persisting — once, in its
    // final state, rather than inserted as `evaluating` and patched.
    let row = st
        .store
        .insert(Submission {
            id: String::new(),
            miner_hotkey: hotkey,
            artifact_digest: artifact,
            artifact_uri: body.artifact_uri,
            manifest: body.manifest,
            nonce,
            submission_digest,
            state: if eligible {
                SubmissionState::AwaitingAdmin
            } else {
                SubmissionState::Rejected
            },
            receipt_json: Some(serde_json::to_string(&eval.receipt).unwrap_or_default()),
            verdict: Some(verdict),
            detail,
        })
        .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "store"))?;
    st.store
        .record_scores(&row.id, scores)
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

#[allow(clippy::too_many_arguments)]
fn persist_pre_eval_reject(
    st: &AppState,
    body: SubmitBody,
    hotkey: String,
    artifact: String,
    nonce: String,
    submission_digest: String,
    contamination: ContaminationEvidence,
    verdict: PromoteVerdict,
    holdout_items: usize,
) -> Result<(StatusCode, Json<SubmitResp>), (StatusCode, Json<serde_json::Value>)> {
    let scores = SliceScores {
        contamination,
        ..SliceScores::default()
    };
    let row = st
        .store
        .insert(Submission {
            id: String::new(),
            miner_hotkey: hotkey,
            artifact_digest: artifact,
            artifact_uri: body.artifact_uri,
            manifest: body.manifest,
            nonce,
            submission_digest,
            state: SubmissionState::Rejected,
            receipt_json: None,
            detail: Some(format!("gates={:?}", verdict.failed)),
            verdict: Some(verdict),
        })
        .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "store"))?;
    st.store
        .record_scores(&row.id, scores)
        .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "store"))?;
    Ok((
        StatusCode::CREATED,
        Json(SubmitResp {
            id: row.id,
            submission_digest: row.submission_digest,
            state: row.state,
            holdout_unsealed: holdout_items > 0,
            eval_backend: st.backend,
            eligible: false,
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
        // A provider or eval-image failure is the host being unable to score
        // right now, not the miner's mistake: retrying is the correct response.
        EvalError::HoldoutSealed
        | EvalError::EvalImageUnpinned
        | EvalError::LiveHarvestUnavailable
        | EvalError::Backend(_)
        | EvalError::Baseline(_) => StatusCode::SERVICE_UNAVAILABLE,
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
    use async_trait::async_trait;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use relearn_challenge_task::{holdout_commitment, HoldoutItem, HoldoutTask};
    use relearn_eval::{
        boot_base_champion, sim_slice_scores_at_skill, BaselineMeasurement, BASE_CHAMPION_ARTIFACT,
        BASE_CHAMPION_SKILL,
    };
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

    /// Stand-in for the eval image's harvest. Real live scores come from
    /// `CortexLM/relearn`; this exists so the gate path can be exercised.
    struct StubScorer {
        skill: f64,
    }

    #[async_trait]
    impl LiveScorer for StubScorer {
        async fn score(
            &self,
            _pin: &RelearnPin,
            _frozen: &str,
            artifact: &str,
            holdout: &[HoldoutItem],
        ) -> Result<relearn_score::SliceScores, EvalError> {
            Ok(sim_slice_scores_at_skill(artifact, holdout, self.skill))
        }
    }

    struct CountingScorer {
        hits: std::sync::atomic::AtomicUsize,
    }

    #[async_trait]
    impl LiveScorer for CountingScorer {
        async fn score(
            &self,
            _pin: &RelearnPin,
            _frozen: &str,
            artifact: &str,
            holdout: &[HoldoutItem],
        ) -> Result<relearn_score::SliceScores, EvalError> {
            self.hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(sim_slice_scores_at_skill(artifact, holdout, 0.8))
        }
    }

    async fn app_backend(
        token: &str,
        load: bool,
        backend: EvalBackend,
        eval_digest: &str,
    ) -> Router {
        app_full(token, load, backend, eval_digest, None, true).await
    }

    async fn app_full(
        token: &str,
        load: bool,
        backend: EvalBackend,
        eval_digest: &str,
        live: Option<Arc<dyn LiveScorer>>,
        baseline: bool,
    ) -> Router {
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
            // Same as boot: a host that cannot measure a baseline does not get
            // one, and its submissions refuse.
            if baseline {
                if let Ok(scores) = boot_base_champion(
                    &pin,
                    &recs,
                    backend,
                    recorded_baseline(&pin, &recs),
                    live.as_deref(),
                )
                .await
                {
                    store.set_base_champion(scores).expect("base");
                }
            }
        }
        relearn_router(AppState {
            store,
            pin,
            backend,
            live_scorer: live,
            admin_hashes: Arc::new(vec![hash_admin_token(token)]),
        })
    }

    /// What an operator installs via `RELEARN_BASE_CHAMPION_FILE`: the base
    /// model measured by the pinned eval image.
    fn recorded_baseline(pin: &RelearnPin, recs: &[HoldoutItem]) -> Option<BaselineMeasurement> {
        if !pin.can_rent() {
            return None;
        }
        let s = sim_slice_scores_at_skill(BASE_CHAMPION_ARTIFACT, recs, BASE_CHAMPION_SKILL);
        Some(BaselineMeasurement {
            eval_image_digest: pin.eval_image_digest.clone(),
            holdout_commitment: pin.holdout_commitment.clone(),
            holdout: s.holdout.by_cluster,
            public: s.public.by_cluster,
            perturbed: s.perturbed.by_cluster,
            canaries: s.canaries.by_cluster,
            general_canary: s.general_canary.by_cluster,
            agent_trace: s.agent_trace,
            vision_shuffle: s.vision_shuffle,
        })
    }

    async fn app_with(token: &str, load: bool) -> Router {
        app_backend(token, load, EvalBackend::Sim, "").await
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
        let app = app_with(token, true).await;

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
        let app = app_with("x", true).await;
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
        assert_eq!(relearn_challenge_task::TEACHER_MODEL_ID, "glm-5.3");
        assert_eq!(
            relearn_challenge_task::TEACHER_NVFP4_ID,
            "incoai/GLM-5.3-NVFP4"
        );
    }

    #[tokio::test]
    async fn status_publishes_the_seal_not_holdout_items() {
        let (st, body) = json_req(
            app_with("op", true).await,
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
    async fn status_cannot_score_until_the_holdout_is_loaded() {
        let (st, body) = json_req(
            app_with("op", false).await,
            "GET",
            "/v1/status",
            serde_json::json!({}),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(body["holdout"]["loaded"], false, "{body}");
        assert_eq!(body["can_score"], false, "{body}");
        assert_eq!(body["force_sim"], false, "{body}");
    }

    #[tokio::test]
    async fn mismatched_holdout_cannot_load_so_submit_is_unavailable() {
        let recs = holdout();
        let pin = RelearnPin {
            holdout_commitment: holdout_commitment(&recs),
            holdout_size: recs.len(),
            public_ids: (1..=40).collect(),
            ..RelearnPin::default()
        };
        let store = MemoryStore::new();
        store
            .set_holdout_commitment(&pin.holdout_commitment, pin.holdout_size)
            .expect("commit");
        let mut leaked = recs;
        leaked[0].prompt = "leaked".into();
        assert!(
            store.load_holdout(leaked, &[], &pin.public_ids).is_err(),
            "a commitment mismatch must not load"
        );
        let app = relearn_router(AppState {
            store,
            pin,
            backend: EvalBackend::Sim,
            live_scorer: None,
            admin_hashes: Arc::new(vec![hash_admin_token("op")]),
        });
        let (st, status) = json_req(
            app.clone(),
            "GET",
            "/v1/status",
            serde_json::json!({}),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(status["holdout"]["loaded"], false, "{status}");
        assert_eq!(status["can_score"], false, "{status}");
        let (st, body) = json_req(
            app,
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
    async fn submit_without_a_loaded_holdout_is_unavailable_not_scored() {
        let (st, body) = json_req(
            app_with("op", false).await,
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
        let app = app_with("op", true).await;
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
        let app = app_with("op-test-token", true).await;
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
    async fn contamination_is_rejected_without_renting() {
        let scorer = Arc::new(CountingScorer {
            hits: std::sync::atomic::AtomicUsize::new(0),
        });
        let app = app_full(
            "op",
            true,
            EvalBackend::Lium,
            &format!("sha256:{}", "ab".repeat(32)),
            Some(scorer.clone()),
            true,
        )
        .await;
        let hold_id = holdout()[0].id;
        for manifest in [
            serde_json::json!({}),
            serde_json::json!({ "train_item_ids": [hold_id] }),
        ] {
            let (st, created) = json_req(
                app.clone(),
                "POST",
                "/v1/submissions",
                serde_json::json!({
                    "miner_hotkey": digest("miner-hotkey"),
                    "artifact_digest": digest("junk-adapter"),
                    "manifest": manifest,
                }),
                None,
            )
            .await;
            assert_eq!(st, StatusCode::CREATED, "{created}");
            assert_eq!(created["eligible"], false, "{created}");
            assert_eq!(created["state"], "rejected", "{created}");
        }
        assert_eq!(
            scorer.hits.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "contaminated / empty-evidence must not rent a pod"
        );
    }

    #[tokio::test]
    async fn live_submit_refuses_without_a_pinned_eval_image() {
        let (st, body) = json_req(
            app_backend("op", true, EvalBackend::Lium, "").await,
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
            )
            .await,
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

    /// The bug: baseline seeding used to be sim-only, so the moment a `sha256:`
    /// eval image was pinned, every submit answered `no champion baseline
    /// recorded` before contamination / public-holdout / shuffle could run.
    #[tokio::test]
    async fn live_boot_records_a_champion_baseline_without_force_sim() {
        let recs = holdout();
        let pin = RelearnPin {
            holdout_commitment: holdout_commitment(&recs),
            holdout_size: recs.len(),
            eval_image_digest: format!("sha256:{}", "ab".repeat(32)),
            ..RelearnPin::default()
        };
        assert!(!force_sim(), "this test is the live path");

        // Operator-recorded measurement, verified against the pin.
        let recorded = recorded_baseline(&pin, &recs).expect("recorded");
        let live = boot_base_champion(&pin, &recs, EvalBackend::Lium, Some(recorded), None)
            .await
            .expect("live baseline");
        assert_eq!(live.holdout.len(), recs.len());
        assert!(!live.general_canary.is_empty(), "gates need the canary");
        assert!(!live.public.is_empty(), "gates need the public split");

        // Wired harvest is the other live source.
        let stub = StubScorer { skill: 0.4 };
        let harvested = boot_base_champion(&pin, &recs, EvalBackend::Lium, None, Some(&stub))
            .await
            .expect("harvested baseline");
        assert_eq!(harvested.holdout.len(), recs.len());

        // Neither source: refuse, never quietly fall back to sim numbers.
        let err = boot_base_champion(&pin, &recs, EvalBackend::Lium, None, None)
            .await
            .expect_err("no live source");
        assert!(matches!(err, EvalError::LiveHarvestUnavailable), "{err}");

        // And the served host reports it.
        let (st, body) = json_req(
            app_full(
                "op",
                true,
                EvalBackend::Lium,
                &pin.eval_image_digest,
                Some(Arc::new(StubScorer { skill: 0.4 })),
                true,
            )
            .await,
            "GET",
            "/v1/status",
            serde_json::json!({}),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(body["eval_backend"], "lium");
        assert_eq!(body["force_sim"], false);
        assert_eq!(body["champion_baseline_recorded"], true, "{body}");
        assert_eq!(body["can_score"], true);
    }

    /// With a pinned digest and a wired harvest, a submission must actually
    /// reach the gates instead of stopping at a missing baseline.
    #[tokio::test]
    async fn live_submit_with_a_pinned_digest_reaches_the_gates() {
        let token = "op-test-token";
        let app = app_full(
            token,
            true,
            EvalBackend::Lium,
            &format!("sha256:{}", "ab".repeat(32)),
            // Above BASE_CHAMPION_SKILL, so this challenger displaces.
            Some(Arc::new(StubScorer {
                skill: BASE_CHAMPION_SKILL + 0.35,
            })),
            true,
        )
        .await;

        // Contamination is a real gate on this path, not a skipped one.
        let hold_id = holdout()[0].id;
        let (st, dirty) = json_req(
            app.clone(),
            "POST",
            "/v1/submissions",
            serde_json::json!({
                "miner_hotkey": digest("miner-hotkey"),
                "artifact_digest": digest("contaminated"),
                "manifest": { "train_item_ids": [hold_id] },
            }),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{dirty}");
        assert_eq!(dirty["eval_backend"], "lium");
        assert_eq!(dirty["eligible"], false);
        let id = dirty["id"].as_str().expect("id");
        let (_st, row) = json_req(
            app.clone(),
            "GET",
            &format!("/v1/submissions/{id}"),
            serde_json::json!({}),
            None,
        )
        .await;
        let failed = row["verdict"]["failed"].to_string();
        assert!(failed.contains("\"contamination\""), "{failed}");

        // And a clean one clears every gate and can promote.
        let (st, clean) = json_req(
            app.clone(),
            "POST",
            "/v1/submissions",
            serde_json::json!({
                "miner_hotkey": digest("miner-hotkey"),
                "artifact_digest": digest("clean-live-adapter"),
                "manifest": declared_manifest(),
            }),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{clean}");
        assert_eq!(clean["state"], "awaiting_admin", "{clean}");
        assert_eq!(clean["eligible"], true, "{clean}");

        let id = clean["id"].as_str().expect("id");
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

    /// A refusal means scoring never started, so it must not bank a row.
    /// Otherwise anyone can fill the store with `evaluating` rows that carry no
    /// scores and appear on no operator surface.
    #[tokio::test]
    async fn refused_submissions_leave_no_evaluating_rows() {
        let unpinned = app_backend("op", true, EvalBackend::Lium, "").await;
        let no_baseline = app_full(
            "op",
            true,
            EvalBackend::Lium,
            &format!("sha256:{}", "cd".repeat(32)),
            Some(Arc::new(StubScorer { skill: 0.4 })),
            // Digest pinned and harvest wired, but the operator never recorded
            // the baseline: refuse, and bank nothing.
            false,
        )
        .await;
        // Third case: sim host that never loaded a holdout.
        let sealed = app_with("op", false).await;

        for (label, app) in [
            ("unpinned digest", unpinned),
            ("no baseline", no_baseline),
            ("sealed holdout", sealed),
        ] {
            for _ in 0..3 {
                let (st, body) = json_req(
                    app.clone(),
                    "POST",
                    "/v1/submissions",
                    serde_json::json!({
                        "miner_hotkey": digest("miner-hotkey"),
                        "artifact_digest": digest("spam"),
                        "manifest": declared_manifest(),
                    }),
                    None,
                )
                .await;
                assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE, "{label}: {body}");
            }
            let (st, list) =
                json_req(app, "GET", "/v1/submissions", serde_json::json!({}), None).await;
            assert_eq!(st, StatusCode::OK);
            let items = list["items"].as_array().expect("items");
            assert!(items.is_empty(), "{label} banked rows: {list}");
        }
    }

    /// `no baseline` must not be the message when the digest pin is the cause.
    #[tokio::test]
    async fn unpinned_digest_reports_the_pin_not_the_baseline() {
        let (st, body) = json_req(
            app_backend("op", true, EvalBackend::Lium, "").await,
            "POST",
            "/v1/submissions",
            serde_json::json!({
                "miner_hotkey": digest("miner-hotkey"),
                "artifact_digest": digest("x"),
                "manifest": declared_manifest(),
            }),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE);
        let msg = body["error"].as_str().unwrap_or_default();
        assert!(msg.contains("eval image digest not pinned"), "{body}");
        assert!(!msg.contains("baseline"), "{body}");
    }

    #[tokio::test]
    async fn can_score_is_false_until_the_champion_baseline_is_recorded() {
        let app = app_full(
            "op",
            true,
            EvalBackend::Lium,
            &format!("sha256:{}", "ab".repeat(32)),
            Some(Arc::new(StubScorer { skill: 0.8 })),
            false,
        )
        .await;
        let (st, body) = json_req(
            app.clone(),
            "GET",
            "/v1/status",
            serde_json::json!({}),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(body["eval_backend"], "lium");
        assert_eq!(body["live_harvest_wired"], true);
        assert_eq!(body["champion_baseline_recorded"], false, "{body}");
        assert_eq!(body["can_score"], false, "{body}");

        let (st, created) = json_req(
            app,
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
        assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE, "{created}");
        assert!(
            created["error"]
                .as_str()
                .unwrap_or_default()
                .contains("no champion baseline recorded"),
            "{created}"
        );
    }

    #[tokio::test]
    async fn status_reports_the_scorer_this_host_will_use() {
        let (st, sim) = json_req(
            app_with("op", true).await,
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
            app_backend("op", true, EvalBackend::Lium, "").await,
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
