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
use relearn_t2i_eval::{
    contamination_evidence, eval_after_freeze, force_sim, scoring_readiness, EvalError,
    JudgeBackend, JudgeConfig, LiveJudge,
};
use relearn_t2i_judge::JudgeInference;
use relearn_t2i_score::{
    judge_challenger, pre_eval_contamination_verdict, ContaminationEvidence, PromoteVerdict,
    T2iSliceScores,
};
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
    /// Harvest handle for the digest-pinned eval image. `None` on a live host
    /// means nothing can score, so submissions refuse.
    pub live_judge: Option<Arc<dyn LiveJudge>>,
    /// Operator bearer hashes (sha256 hex). Empty → admin 503.
    pub admin_hashes: Arc<Vec<String>>,
}

impl AppState {
    /// Borrow the live harvest handle, if the operator wired one.
    fn live(&self) -> Option<&dyn LiveJudge> {
        self.live_judge.as_deref()
    }

    /// Whether this host can produce a verdict at all.
    fn can_score(&self) -> bool {
        scoring_readiness(&self.pin, &self.judge, self.live()).is_ok()
    }
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
        // Which scorer this host will actually use, and whether sim was opted
        // into. Miners can see that a run was not a real eval.
        "judge_backend": st.judge.backend,
        "force_sim": force_sim(),
        "can_score": st.can_score(),
        // Both are prerequisites for a live verdict, so name them separately:
        // "can_score: false" without them is the usual operator confusion.
        "live_harvest_wired": st.live_judge.is_some(),
        "champion_baseline_recorded": st.store.champion_scores().ok().flatten().is_some(),
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

    // Everything that can refuse runs before the row exists. A 503 means
    // scoring never started, so it must not leave a spammable `evaluating`
    // row behind that carries no scores and shows up on no operator surface.
    let holdout = st
        .store
        .unseal_holdout(&submission_digest)
        .map_err(|e| err(StatusCode::SERVICE_UNAVAILABLE, &e.to_string()))?;

    // Root cause first: an unpinned digest is why there is no harvest and no
    // baseline either, so report that rather than a downstream symptom.
    scoring_readiness(&st.pin, &st.judge, st.live()).map_err(|e| eval_err(&e))?;

    // Before the eval, not after: on a live host the eval spends the miner's
    // Lium budget, and there is no verdict to be had without a baseline.
    let champ = st.store.champion_scores().ok().flatten().ok_or_else(|| {
        err(
            StatusCode::SERVICE_UNAVAILABLE,
            "no champion baseline recorded",
        )
    })?;

    let holdout_ids: Vec<u32> = holdout.iter().map(|p| p.id).collect();
    let contamination = contamination_evidence(&body.manifest, &holdout_ids);
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
            holdout_ids.len(),
        );
    }

    let eval = eval_after_freeze(
        &st.pin,
        &holdout,
        &submission_digest,
        &artifact,
        &body.manifest,
        &st.judge,
        st.live(),
    )
    .await
    .map_err(|e| eval_err(&e))?;

    let verdict = judge_challenger(&champ, &eval.scores);
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
    // Scoring finished, so the attempt is now worth persisting — once, in its
    // final state, rather than inserted as `evaluating` and patched.
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
            state,
            receipt_json: Some(serde_json::to_string(&eval.receipt).unwrap_or_default()),
            verdict: Some(verdict),
            detail,
        })
        .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "store"))?;
    st.store
        .record_scores(&row.id, eval.scores)
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

fn persist_pre_eval_reject(
    st: &AppState,
    body: SubmitBody,
    hotkey: String,
    artifact: String,
    nonce: String,
    submission_digest: String,
    contamination: ContaminationEvidence,
    verdict: PromoteVerdict,
    holdout_cells: usize,
) -> Result<(StatusCode, Json<SubmitResp>), (StatusCode, Json<serde_json::Value>)> {
    let mut scores = T2iSliceScores::default();
    scores.contamination = contamination;
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
            judge_backend: st.judge.backend,
            holdout_cells,
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
        EvalError::Holdout(_)
        | EvalError::EvalImageUnpinned
        | EvalError::LiveHarvestUnavailable
        | EvalError::JudgeUnconfigured
        | EvalError::Backend(_)
        | EvalError::Baseline(_) => StatusCode::SERVICE_UNAVAILABLE,
        // A rejected base or license is the submission's problem.
        EvalError::Attestation(_) => StatusCode::BAD_REQUEST,
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
    use relearn_t2i_eval::{base_champion_scores, boot_base_champion};
    use relearn_t2i_task::{frozen_prompt_commitment, FrozenPrompt, PromptPin, RelearnT2iPin};
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

    async fn app(token: &str) -> Router {
        app_full(token, test_pin(), JudgeConfig::sim(), None, true).await
    }

    async fn app_full(
        token: &str,
        pin: RelearnT2iPin,
        judge: JudgeConfig,
        live: Option<Arc<dyn LiveJudge>>,
        baseline: bool,
    ) -> Router {
        let store = MemoryStore::new();
        store
            .set_holdout_commitment(&pin.prompts.holdout_commitment, pin.prompts.holdout_size)
            .expect("commit");
        store
            .load_holdout(holdout(), &pin.prompts.public_ids)
            .expect("load holdout");
        if baseline {
            // Same as boot: a host that cannot measure a baseline does not get
            // one, and its submissions refuse.
            if let Ok(scores) = boot_base_champion(
                &pin,
                &holdout(),
                &judge,
                recorded_baseline(&pin, &judge),
                live.as_deref(),
            )
            .await
            {
                store.set_base_champion(scores).expect("seed base");
            }
        }
        relearn_t2i_router(AppState {
            store,
            pin,
            judge,
            live_judge: live,
            admin_hashes: Arc::new(vec![hash_admin_token(token)]),
        })
    }

    /// What an operator installs via `RELEARN_T2I_BASE_CHAMPION_FILE`: the base
    /// checkpoint measured by the pinned eval image.
    fn recorded_baseline(
        pin: &RelearnT2iPin,
        judge: &JudgeConfig,
    ) -> Option<relearn_t2i_eval::T2iBaselineMeasurement> {
        if judge.backend == JudgeBackend::Sim || !pin.can_rent() {
            return None;
        }
        let ids: Vec<u32> = holdout().iter().map(|p| p.id).collect();
        let s = base_champion_scores(pin, &ids).ok()?;
        Some(relearn_t2i_eval::T2iBaselineMeasurement {
            eval_image_digest: pin.eval_image_digest.clone(),
            holdout_commitment: pin.prompts.holdout_commitment.clone(),
            holdout: s.holdout.by_cluster,
            public: s.public.by_cluster,
            holdout_by_pillar: s
                .holdout_by_pillar
                .into_iter()
                .map(|(d, x)| (d, x.by_cluster))
                .collect(),
            capability_canary: s.capability_canary.by_cluster,
            na_rate: s.na_rate,
            replay: s.replay,
            faithfulness: s.faithfulness,
        })
    }

    fn live_pin() -> RelearnT2iPin {
        RelearnT2iPin {
            eval_image_digest: format!("sha256:{}", "ab".repeat(32)),
            ..test_pin()
        }
    }

    fn live_judge_config() -> JudgeConfig {
        JudgeConfig::http_api("http://judge.invalid/v1")
    }

    /// Stand-in for the eval image's harvest. Real live scores come from
    /// `CortexLM/relearn`.
    struct StubJudge;

    #[async_trait::async_trait]
    impl LiveJudge for StubJudge {
        async fn score(
            &self,
            pin: &RelearnT2iPin,
            _frozen: &str,
            artifact: &str,
            holdout: &[FrozenPrompt],
            _manifest: &relearn_t2i_store::ArtifactManifest,
        ) -> Result<relearn_t2i_score::T2iSliceScores, EvalError> {
            let ids: Vec<u32> = holdout.iter().map(|p| p.id).collect();
            let mut scores = relearn_t2i_eval::sim_slice_scores(pin, &ids, artifact)?;
            // Above the base champion, so a clean challenger can displace.
            scores.holdout = bump(&scores.holdout, 0.15);
            scores.public = bump(&scores.public, 0.15);
            scores.holdout_by_pillar = scores
                .holdout_by_pillar
                .iter()
                .map(|(d, s)| (*d, bump(s, 0.15)))
                .collect();
            Ok(scores)
        }
    }

    struct CountingJudge {
        hits: std::sync::atomic::AtomicUsize,
    }

    #[async_trait::async_trait]
    impl LiveJudge for CountingJudge {
        async fn score(
            &self,
            pin: &RelearnT2iPin,
            frozen: &str,
            artifact: &str,
            holdout: &[FrozenPrompt],
            manifest: &relearn_t2i_store::ArtifactManifest,
        ) -> Result<relearn_t2i_score::T2iSliceScores, EvalError> {
            self.hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            StubJudge
                .score(pin, frozen, artifact, holdout, manifest)
                .await
        }
    }

    fn bump(
        series: &prism_competition::ExampleSeries,
        by: f64,
    ) -> prism_competition::ExampleSeries {
        prism_competition::ExampleSeries::from_pairs(
            series
                .by_cluster
                .iter()
                .map(|(k, v)| (k.clone(), (v + by).min(1.0))),
        )
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

    /// Training metadata a real miner declares. An empty manifest is a
    /// separate case (`empty_manifest_cannot_dodge_the_contamination_gate`).
    fn declared_manifest_json() -> serde_json::Value {
        serde_json::json!({
            "base": BASE_MODEL_ID,
            "base_license": BASE_MODEL_LICENSE,
            "train_dataset_ids": ["cortex-public-v0"],
        })
    }

    #[tokio::test]
    async fn health_and_status_report_the_pins() {
        let app = app("op").await;
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
        let (st, body) = json_req(
            app("op").await,
            "GET",
            "/v1/prompts",
            serde_json::json!({}),
            None,
        )
        .await;
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
        let app = app(token).await;
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
            app("op").await,
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
            app("op").await,
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
            live_judge: None,
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
            app("op").await,
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
        assert_eq!(CHALLENGE_ID, "relearn-image");
    }

    #[tokio::test]
    async fn live_submit_refuses_without_a_pinned_eval_image() {
        let app = app_full("op", test_pin(), live_judge_config(), None, false).await;
        let (st, body) = json_req(
            app,
            "POST",
            "/v1/submissions",
            submit_body("x", &pinned_manifest_json()),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE, "{body}");
        let msg = body["error"].as_str().unwrap_or_default();
        assert!(msg.contains("eval image digest not pinned"), "{body}");
        assert!(!msg.contains("baseline"), "{body}");
    }

    #[tokio::test]
    async fn live_submit_never_scores_with_the_sim_harness() {
        let app = app_full("op", live_pin(), live_judge_config(), None, false).await;
        let (st, body) = json_req(
            app,
            "POST",
            "/v1/submissions",
            submit_body("x", &pinned_manifest_json()),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE, "{body}");
        assert!(body["error"]
            .as_str()
            .unwrap_or_default()
            .contains("no in-process sim"));
    }

    /// A refusal means scoring never started, so it must not bank a row.
    /// Otherwise anyone can fill the store with `evaluating` rows that carry
    /// no scores and appear on no operator surface.
    #[tokio::test]
    async fn refused_submissions_leave_no_evaluating_rows() {
        let unpinned = app_full("op", test_pin(), live_judge_config(), None, false).await;
        // Digest pinned and harvest wired, but no baseline was ever recorded.
        let no_baseline = app_full(
            "op",
            live_pin(),
            live_judge_config(),
            Some(Arc::new(StubJudge)),
            false,
        )
        .await;
        let sealed = {
            let pin = test_pin();
            let store = MemoryStore::new();
            store
                .set_holdout_commitment(&pin.prompts.holdout_commitment, pin.prompts.holdout_size)
                .expect("commit");
            relearn_t2i_router(AppState {
                store,
                pin,
                judge: JudgeConfig::sim(),
                live_judge: None,
                admin_hashes: Arc::new(vec![hash_admin_token("op")]),
            })
        };

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
                    submit_body("spam", &pinned_manifest_json()),
                    None,
                )
                .await;
                assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE, "{label}: {body}");
            }
            let (st, list) =
                json_req(app, "GET", "/v1/submissions", serde_json::json!({}), None).await;
            assert_eq!(st, StatusCode::OK);
            assert!(
                list["items"].as_array().is_some_and(Vec::is_empty),
                "{label} banked rows: {list}"
            );
        }
    }

    /// With a pinned digest and a wired harvest, a submission must reach the
    /// gates rather than stopping at a missing baseline.
    #[tokio::test]
    async fn live_submit_with_a_pinned_digest_reaches_the_gates() {
        let token = "op-test-token";
        let app = app_full(
            token,
            live_pin(),
            live_judge_config(),
            Some(Arc::new(StubJudge)),
            true,
        )
        .await;

        let (st, status) = json_req(
            app.clone(),
            "GET",
            "/v1/status",
            serde_json::json!({}),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(status["judge_backend"], "http_api");
        assert_eq!(status["force_sim"], false);
        assert_eq!(status["live_harvest_wired"], true);
        assert_eq!(status["champion_baseline_recorded"], true, "{status}");
        assert_eq!(status["can_score"], true);

        // Contamination is a real gate on this path, not a skipped one.
        let mut dirty_manifest = pinned_manifest_json();
        dirty_manifest["train_prompt_ids"] = serde_json::json!([holdout()[0].id]);
        let (st, dirty) = json_req(
            app.clone(),
            "POST",
            "/v1/submissions",
            submit_body("contaminated", &dirty_manifest),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{dirty}");
        assert_eq!(dirty["judge_backend"], "http_api");
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
        assert!(
            row["verdict"]["failed"]
                .to_string()
                .contains("contamination"),
            "{row}"
        );

        // And a clean one clears every gate and can promote.
        let (st, clean) = json_req(
            app.clone(),
            "POST",
            "/v1/submissions",
            submit_body("clean-live-finetune", &declared_manifest_json()),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{clean}");
        assert_eq!(clean["state"], "awaiting_admin", "{clean}");
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

    /// The same artifact that promotes above, minus the training declaration.
    #[tokio::test]
    async fn empty_manifest_cannot_dodge_the_contamination_gate() {
        let app = app_full(
            "op",
            live_pin(),
            live_judge_config(),
            Some(Arc::new(StubJudge)),
            true,
        )
        .await;
        let (st, created) = json_req(
            app.clone(),
            "POST",
            "/v1/submissions",
            submit_body("clean-live-finetune", &pinned_manifest_json()),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{created}");
        assert_eq!(created["eligible"], false, "{created}");
        let id = created["id"].as_str().expect("id");
        let (_st, row) = json_req(
            app,
            "GET",
            &format!("/v1/submissions/{id}"),
            serde_json::json!({}),
            None,
        )
        .await;
        assert!(
            row["verdict"]["failed"]
                .to_string()
                .contains("contamination_evidence_missing"),
            "{row}"
        );
    }

    #[tokio::test]
    async fn contamination_is_rejected_without_renting() {
        let judge = Arc::new(CountingJudge {
            hits: std::sync::atomic::AtomicUsize::new(0),
        });
        let app = app_full(
            "op",
            live_pin(),
            live_judge_config(),
            Some(judge.clone()),
            true,
        )
        .await;
        let mut dirty = pinned_manifest_json();
        dirty["train_prompt_ids"] = serde_json::json!([holdout()[0].id]);
        for manifest in [pinned_manifest_json(), dirty] {
            let (st, created) = json_req(
                app.clone(),
                "POST",
                "/v1/submissions",
                submit_body("junk-image", &manifest),
                None,
            )
            .await;
            assert_eq!(st, StatusCode::CREATED, "{created}");
            assert_eq!(created["eligible"], false, "{created}");
        }
        assert_eq!(
            judge.hits.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "contaminated / empty-evidence must not rent a pod"
        );
    }

    #[tokio::test]
    async fn status_reports_the_scorer_this_host_will_use() {
        let (st, sim) = json_req(
            app("op").await,
            "GET",
            "/v1/status",
            serde_json::json!({}),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(sim["judge_backend"], "sim");
        assert_eq!(sim["can_score"], true);
        assert_eq!(sim["challenge_id"], "relearn-image");

        let (st, live) = json_req(
            app_full("op", test_pin(), live_judge_config(), None, false).await,
            "GET",
            "/v1/status",
            serde_json::json!({}),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(live["judge_backend"], "http_api");
        assert_eq!(live["can_score"], false);
        assert_eq!(live["live_harvest_wired"], false);
    }
}
