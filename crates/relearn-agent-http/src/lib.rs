//! Relearn Agent HTTP API (master-only).
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
    clippy::too_many_lines
)]

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};

use relearn_agent_eval::{
    contamination_evidence, eval_after_freeze, force_sim, scoring_readiness, EvalBackend,
    EvalError, LiveScorer,
};
use relearn_agent_score::{
    judge_challenger, pre_eval_contamination_verdict, AgentSliceScores, ContaminationEvidence,
    PromoteVerdict,
};
use relearn_agent_store::{
    freeze_submission_digest, ArtifactManifest, MemoryStore, Submission, SubmissionState,
};
use relearn_agent_task::{RelearnAgentPin, CHALLENGE_ID, SCORE_MAX, SCORING_VERSION};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Shared HTTP state.
#[derive(Clone)]
pub struct AppState {
    /// Submission store.
    pub store: MemoryStore,
    /// Eval / model pins.
    pub pin: RelearnAgentPin,
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

    /// Verified holdout episodes are loaded (commitment matched at boot).
    fn holdout_loaded(&self) -> bool {
        self.store.holdout_seal().ok().is_some_and(|s| s.loaded)
    }

    /// Whether a champion baseline is in the store.
    fn champion_recorded(&self) -> bool {
        self.store.champion_scores().ok().flatten().is_some()
    }

    /// Whether this host can produce a verdict at all.
    ///
    /// False until the holdout is verified loaded **and** a champion baseline
    /// is recorded. Submit already 503s in both cases; status must not
    /// contradict that.
    fn can_score(&self) -> bool {
        scoring_readiness(&self.pin, self.backend, self.live(), self.holdout_loaded()).is_ok()
            && self.champion_recorded()
    }
}

/// Build the router.
pub fn relearn_agent_router(state: AppState) -> Router {
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
        // The arms that make this an agent challenge rather than a prompt set.
        "gates": ["trace_replay", "tool_ablation", "observation_shuffle"],
        "relearn_git": st.pin.relearn_git,
        "relearn_git_sha": st.pin.relearn_git_sha,
        // Commitment + size + loaded. Never ids, goals, or observation hashes.
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
    episodes_scored: usize,
    eval_backend: EvalBackend,
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
    h.update(b"relearn-agent-nonce-v1");
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
    // scoring never started, so it must not leave a spammable row behind that
    // carries no scores and shows up on no operator surface.
    let episodes = st
        .store
        .unseal_episodes(&submission_digest)
        .map_err(|e| err(StatusCode::SERVICE_UNAVAILABLE, &e.to_string()))?;

    // Root cause first: an unpinned digest is why there is no harvest and no
    // baseline either, so report that rather than a downstream symptom.
    scoring_readiness(&st.pin, st.backend, st.live(), st.holdout_loaded())
        .map_err(|e| eval_err(&e))?;

    // Before the eval, not after: on a live host the eval spends the miner's
    // Lium budget, and there is no verdict to be had without a baseline.
    let champ = st.store.champion_scores().ok().flatten().ok_or_else(|| {
        err(
            StatusCode::SERVICE_UNAVAILABLE,
            "no champion baseline recorded",
        )
    })?;

    let contamination = contamination_evidence(&body.manifest, &episodes);
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
            episodes.len(),
        );
    }

    let eval = eval_after_freeze(
        &st.pin,
        &submission_digest,
        &artifact,
        &episodes,
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
    // final state.
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
            episodes_scored: eval.episodes,
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
    episodes: usize,
) -> Result<(StatusCode, Json<SubmitResp>), (StatusCode, Json<serde_json::Value>)> {
    let scores = AgentSliceScores {
        contamination,
        ..AgentSliceScores::default()
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
            episodes_scored: episodes,
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
    hashes
        .iter()
        .any(|x| x == &hex::encode(h.clone().finalize()))
}

fn err(code: StatusCode, msg: &str) -> (StatusCode, Json<serde_json::Value>) {
    (code, Json(serde_json::json!({ "error": msg })))
}

/// A host that cannot score is unavailable, not broken. `503` keeps miners
/// retrying instead of reading a sim number as a verdict.
fn eval_err(e: &EvalError) -> (StatusCode, Json<serde_json::Value>) {
    let code = match e {
        EvalError::EpisodesSealed
        | EvalError::EvalImageUnpinned
        | EvalError::LiveHarvestUnavailable
        | EvalError::Backend(_)
        | EvalError::Baseline(_) => StatusCode::SERVICE_UNAVAILABLE,
        EvalError::Integrity(_) => StatusCode::INTERNAL_SERVER_ERROR,
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
    use async_trait::async_trait;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use relearn_agent_eval::{
        boot_base_champion, sim_slice_scores_at_skill, BaselineMeasurement, BASE_CHAMPION_ARTIFACT,
        BASE_CHAMPION_SKILL,
    };
    use relearn_agent_score::AgentSliceScores;
    use relearn_agent_task::{episode_commitment, AgentEpisode};
    use tower::ServiceExt;

    use super::*;

    fn digest(label: &str) -> String {
        let mut h = Sha256::new();
        h.update(label.as_bytes());
        hex::encode(h.finalize())
    }

    fn episodes() -> Vec<AgentEpisode> {
        (1..=120)
            .map(|i| {
                AgentEpisode::synthetic(
                    800 + i,
                    format!("episode {i} asks for a figure buried in the ledger"),
                )
            })
            .collect()
    }

    /// Training metadata a real miner declares. An empty manifest is a
    /// separate case (`empty_manifest_cannot_dodge_the_contamination_gate`).
    fn declared_manifest() -> serde_json::Value {
        serde_json::json!({ "train_environment_ids": ["cortex-public-envs-v0"] })
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
            _pin: &RelearnAgentPin,
            _frozen: &str,
            artifact: &str,
            eps: &[AgentEpisode],
        ) -> Result<AgentSliceScores, EvalError> {
            Ok(sim_slice_scores_at_skill(artifact, eps, self.skill))
        }
    }

    struct CountingScorer {
        hits: std::sync::atomic::AtomicUsize,
        skill: f64,
    }

    #[async_trait]
    impl LiveScorer for CountingScorer {
        async fn score(
            &self,
            _pin: &RelearnAgentPin,
            _frozen: &str,
            artifact: &str,
            eps: &[AgentEpisode],
        ) -> Result<AgentSliceScores, EvalError> {
            self.hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(sim_slice_scores_at_skill(artifact, eps, self.skill))
        }
    }

    fn pin(digest: &str) -> RelearnAgentPin {
        let eps = episodes();
        RelearnAgentPin {
            holdout_commitment: episode_commitment(&eps),
            holdout_size: eps.len(),
            public_ids: (1..=40).collect(),
            eval_image_digest: digest.to_owned(),
            ..RelearnAgentPin::default()
        }
    }

    fn recorded(p: &RelearnAgentPin) -> Option<BaselineMeasurement> {
        if !p.can_rent() {
            return None;
        }
        let s = sim_slice_scores_at_skill(BASE_CHAMPION_ARTIFACT, &episodes(), BASE_CHAMPION_SKILL);
        Some(BaselineMeasurement {
            eval_image_digest: p.eval_image_digest.clone(),
            holdout_commitment: p.holdout_commitment.clone(),
            holdout: s.holdout.by_cluster,
            public: s.public.by_cluster,
            trace_valid: s.trace_valid.by_cluster,
            capability_canary: s.capability_canary.by_cluster,
            tool_ablation: s.tool_ablation,
            observation_shuffle: s.observation_shuffle,
        })
    }

    async fn app(token: &str) -> Router {
        app_full(token, EvalBackend::Sim, "", None, true, true).await
    }

    async fn app_full(
        token: &str,
        backend: EvalBackend,
        eval_digest: &str,
        live: Option<Arc<dyn LiveScorer>>,
        load: bool,
        baseline: bool,
    ) -> Router {
        let p = pin(eval_digest);
        let store = MemoryStore::new();
        store
            .set_holdout_commitment(&p.holdout_commitment, p.holdout_size)
            .expect("commit");
        if load {
            store
                .load_episodes(episodes(), &[], &p.public_ids)
                .expect("load");
            if baseline {
                if let Ok(scores) =
                    boot_base_champion(&p, &episodes(), backend, recorded(&p), live.as_deref())
                        .await
                {
                    store.set_base_champion(scores).expect("base");
                }
            }
        }
        relearn_agent_router(AppState {
            store,
            pin: p,
            backend,
            live_scorer: live,
            admin_hashes: Arc::new(vec![hash_admin_token(token)]),
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

    fn submit_body(label: &str, manifest: &serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "miner_hotkey": digest("miner-hotkey"),
            "artifact_digest": digest(label),
            "manifest": manifest,
        })
    }

    #[tokio::test]
    async fn submit_eval_promote_happy_path() {
        let token = "op-test-token";
        let app = app(token).await;

        let (st, health) =
            json_req(app.clone(), "GET", "/health", serde_json::json!({}), None).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(health["challenge_id"], "relearn-agent");

        let (st, created) = json_req(
            app.clone(),
            "POST",
            "/v1/submissions",
            submit_body("miner-strong-agent", &declared_manifest()),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{created}");
        assert_eq!(created["eval_backend"], "sim");
        assert_eq!(created["episodes_scored"], 120);

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
    async fn status_publishes_the_seal_and_the_agent_gates_not_the_episodes() {
        let (st, body) = json_req(
            app("op").await,
            "GET",
            "/v1/status",
            serde_json::json!({}),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(body["holdout"]["loaded"], true);
        assert_eq!(body["holdout"]["size"], 120);
        assert_eq!(body["base_model"], "Qwen/Qwen3.8-27B");
        let gates = body["gates"].to_string();
        for arm in ["trace_replay", "tool_ablation", "observation_shuffle"] {
            assert!(gates.contains(arm), "{gates}");
        }
        let dump = body.to_string();
        assert!(!dump.contains("buried in the ledger"), "{dump}");
        assert!(!dump.contains("\"id\":801"));
    }

    #[tokio::test]
    async fn promote_requires_bearer() {
        let (st, _) = json_req(
            app("x").await,
            "POST",
            "/v1/admin/promote",
            serde_json::json!({ "submission_id": "ag_0" }),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);
    }

    /// Status must not advertise `can_score` when submit would 503. A sim
    /// host with no verified episodes is the live-replay case: holdout file
    /// missing, `eval_backend: sim`, submit 503, status used to lie.
    #[tokio::test]
    async fn status_cannot_score_until_episodes_are_loaded() {
        let (st, body) = json_req(
            app_full("op", EvalBackend::Sim, "", None, false, false).await,
            "GET",
            "/v1/status",
            serde_json::json!({}),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(body["eval_backend"], "sim");
        assert_eq!(body["holdout"]["loaded"], false);
        assert_eq!(body["can_score"], false, "{body}");

        let (st, live) = json_req(
            app_full(
                "op",
                EvalBackend::Lium,
                &format!("sha256:{}", "ab".repeat(32)),
                Some(Arc::new(StubScorer { skill: 0.4 })),
                false,
                false,
            )
            .await,
            "GET",
            "/v1/status",
            serde_json::json!({}),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(live["live_harvest_wired"], true);
        assert_eq!(live["holdout"]["loaded"], false);
        assert_eq!(live["can_score"], false, "{live}");
    }

    #[tokio::test]
    async fn submit_without_loaded_episodes_is_unavailable_not_scored() {
        let (st, body) = json_req(
            app_full("op", EvalBackend::Sim, "", None, false, false).await,
            "POST",
            "/v1/submissions",
            submit_body("x", &declared_manifest()),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    }

    #[tokio::test]
    async fn live_submit_refuses_without_a_pinned_eval_image() {
        let (st, body) = json_req(
            app_full("op", EvalBackend::Lium, "", None, true, false).await,
            "POST",
            "/v1/submissions",
            submit_body("x", &declared_manifest()),
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
        let (st, body) = json_req(
            app_full(
                "op",
                EvalBackend::Lium,
                &format!("sha256:{}", "ab".repeat(32)),
                None,
                true,
                false,
            )
            .await,
            "POST",
            "/v1/submissions",
            submit_body("x", &declared_manifest()),
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
    #[tokio::test]
    async fn refused_submissions_leave_no_rows() {
        let unpinned = app_full("op", EvalBackend::Lium, "", None, true, false).await;
        let no_baseline = app_full(
            "op",
            EvalBackend::Lium,
            &format!("sha256:{}", "cd".repeat(32)),
            Some(Arc::new(StubScorer { skill: 0.4 })),
            true,
            false,
        )
        .await;
        let sealed = app_full("op", EvalBackend::Sim, "", None, false, false).await;

        for (label, app) in [
            ("unpinned digest", unpinned),
            ("no baseline", no_baseline),
            ("sealed episodes", sealed),
        ] {
            for _ in 0..3 {
                let (st, body) = json_req(
                    app.clone(),
                    "POST",
                    "/v1/submissions",
                    submit_body("spam", &declared_manifest()),
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
    /// agent gates rather than stopping at a missing baseline.
    #[tokio::test]
    async fn live_submit_with_a_pinned_digest_reaches_the_gates() {
        let token = "op-test-token";
        let app = app_full(
            token,
            EvalBackend::Lium,
            &format!("sha256:{}", "ab".repeat(32)),
            Some(Arc::new(StubScorer {
                skill: BASE_CHAMPION_SKILL + 0.35,
            })),
            true,
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
        assert_eq!(status["eval_backend"], "lium");
        assert_eq!(status["force_sim"], false);
        assert_eq!(status["live_harvest_wired"], true);
        assert_eq!(status["champion_baseline_recorded"], true, "{status}");
        assert_eq!(status["can_score"], true);

        // Contamination is a real gate on this path, not a skipped one.
        let hold_id = episodes()[0].id;
        let (st, dirty) = json_req(
            app.clone(),
            "POST",
            "/v1/submissions",
            submit_body(
                "contaminated",
                &serde_json::json!({ "train_episode_ids": [hold_id] }),
            ),
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
            submit_body("clean-live-agent", &declared_manifest()),
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

    #[tokio::test]
    async fn empty_manifest_cannot_dodge_the_contamination_gate() {
        let app = app("op").await;
        for manifest in [
            serde_json::json!({}),
            serde_json::json!({
                "train_episode_ids": [],
                "train_observation_hashes": [],
                "train_environment_ids": [],
            }),
        ] {
            let (st, created) = json_req(
                app.clone(),
                "POST",
                "/v1/submissions",
                submit_body("miner-strong-agent", &manifest),
                None,
            )
            .await;
            assert_eq!(st, StatusCode::CREATED, "{created}");
            assert_eq!(created["eligible"], false, "manifest={manifest}");

            let id = created["id"].as_str().expect("id");
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
                    .contains("contamination_evidence_missing"),
                "manifest={manifest} row={row}"
            );
        }
    }

    #[tokio::test]
    async fn contamination_is_rejected_without_renting() {
        let scorer = Arc::new(CountingScorer {
            hits: std::sync::atomic::AtomicUsize::new(0),
            skill: BASE_CHAMPION_SKILL + 0.35,
        });
        let app = app_full(
            "op",
            EvalBackend::Lium,
            &format!("sha256:{}", "ab".repeat(32)),
            Some(scorer.clone()),
            true,
            true,
        )
        .await;
        let hold_id = episodes()[0].id;
        for manifest in [
            serde_json::json!({}),
            serde_json::json!({ "train_episode_ids": [hold_id] }),
        ] {
            let (st, created) = json_req(
                app.clone(),
                "POST",
                "/v1/submissions",
                submit_body("junk-agent", &manifest),
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
    async fn can_score_is_false_until_the_champion_baseline_is_recorded() {
        let app = app_full(
            "op",
            EvalBackend::Lium,
            &format!("sha256:{}", "ab".repeat(32)),
            Some(Arc::new(StubScorer {
                skill: BASE_CHAMPION_SKILL + 0.35,
            })),
            true,
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
        assert_eq!(body["live_harvest_wired"], true);
        assert_eq!(body["holdout"]["loaded"], true);
        assert_eq!(body["champion_baseline_recorded"], false, "{body}");
        assert_eq!(body["can_score"], false, "{body}");

        let (st, created) = json_req(
            app,
            "POST",
            "/v1/submissions",
            submit_body("clean-live-agent", &declared_manifest()),
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
            app("op").await,
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
            app_full("op", EvalBackend::Lium, "", None, true, false).await,
            "GET",
            "/v1/status",
            serde_json::json!({}),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(live["eval_backend"], "lium");
        assert_eq!(live["can_score"], false);
        assert_eq!(live["live_harvest_wired"], false);
    }
}
