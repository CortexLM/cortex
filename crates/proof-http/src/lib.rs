//! Proof HTTP API (master-only).
//!
//! ```text
//! GET  /health
//! GET  /v1/status
//! GET  /v1/proof/topics
//! GET  /v1/proof/topics/{id}
//! GET  /v1/proof/executor         public EvalExecutorOffer + pin ceilings
//! POST /v1/submissions            miner submit (topic_id required)
//! GET  /v1/submissions
//! GET  /v1/submissions/{id}
//! POST /v1/admin/proof/topics     operator publish (signed document)
//! POST /v1/admin/proof/executor   operator rotate the live executor offer
//! GET  /v1/admin/proof/vm-orchestrator   operator probe: topic-VM orchestrator readiness + agent health
//! ```

#![forbid(unsafe_code)]
#![allow(
    clippy::missing_errors_doc,
    clippy::doc_markdown,
    clippy::must_use_candidate,
    clippy::too_many_lines,
    clippy::too_many_arguments
)]

use std::sync::{Arc, PoisonError, RwLock};

use async_trait::async_trait;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};

use proof_eval::{
    contamination_evidence, custom_ids_ref, eval_after_freeze, force_sim, registered_custom,
    scoring_readiness, secret_backed_base_url, EvalBackend, EvalError, LiveScorer,
};
use proof_executor::{require_open_executor, EvalExecutorOffer, ExecutorPlan};
use proof_score::{
    judge_topic, novelty_bar, primary_from_harness, AgentVerdict, GateFail, HarnessMetrics,
    MinerTopicRun, ProofKind, ProofVerdict,
};
use proof_store::{
    freeze_submission_digest, ArtifactManifest, MemoryStore, Submission, SubmissionState,
};
use proof_task::{
    resolve_inference, InferenceOffer, MetricFamily, OfferError, ProofPin, TopicDocument,
    TopicError, TopicStatus, CHALLENGE_ID, SCORE_MAX, SCORING_VERSION,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Live executor offer slot: rotated at runtime by the admin route, read on
/// every status / submit. In-memory like the rest of the submission state;
/// the boot value comes from `PROOF_EVAL_EXECUTOR_OFFER_FILE`.
pub type ExecutorSlot = Arc<RwLock<Option<EvalExecutorOffer>>>;

/// Build an [`ExecutorSlot`] holding `offer`.
pub fn executor_slot(offer: Option<EvalExecutorOffer>) -> ExecutorSlot {
    Arc::new(RwLock::new(offer))
}

/// The KVM-host agent's health as the control plane saw it on one
/// `GET /v1/health` (mirrors `proof_vm_proto::AgentHealth` field for field).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VmAgentHealth {
    /// Wire version the agent speaks.
    pub api_version: u32,
    /// Whether the hypervisor could boot a VM right now.
    pub ready: bool,
    /// Why not (empty when ready). Never a secret.
    pub reason: String,
    /// Backend name (`firecracker`; `fake` only in tests).
    pub hypervisor: String,
    /// VMs currently bound on the host.
    pub vms: usize,
}

/// `GET /v1/admin/proof/vm-orchestrator` body: what this host resolved for
/// the topic-VM orchestrator and whether its agent answers. Operator data
/// behind the admin bearer — it may name env vars and container paths, never
/// the bearer, a key, or an origin the RLM could reach.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VmOrchestratorReport {
    /// `firecracker` (live client resolved at boot) or `unwired`.
    pub orchestrator: String,
    /// The client's own `ready()`: bearer file present and non-empty, RLM
    /// image digest pinned. Checked per request, so a fix needs no restart.
    pub ready: bool,
    /// Why not ready (empty when ready). Names the env var to fix.
    pub reason: String,
    /// `sha256:` pin of the RLM VM image the client asks the agent to boot
    /// (empty = unpinned = nothing ever boots).
    pub image_digest: String,
    /// RLM VM vCPUs (locked default 4).
    pub vcpus: u32,
    /// RLM VM memory in MiB (locked default 8192).
    pub mem_mib: u32,
    /// The agent's answer to one health call, when it answered.
    pub agent: Option<VmAgentHealth>,
    /// Why the agent did not answer: unreachable, bearer refused, not wired.
    pub agent_error: Option<String>,
    /// Filled by the host: the digest-pinned Lium harvest (`nll` /
    /// `throughput`) is wired. Lium only — informational for the custom
    /// family, which is wired from the topic-VM env on its own.
    #[serde(default)]
    pub live_harvest_wired: bool,
    /// Filled by the host: at least one custom id has a registered runner
    /// (the custom family is routed, harvest or not).
    #[serde(default)]
    pub custom_family_wired: bool,
    /// Filled by the host: custom ids with a registered runner.
    #[serde(default)]
    pub registered_custom: Vec<String>,
}

impl VmOrchestratorReport {
    /// Report for a host that keeps `UnwiredVmOrchestrator`; `reason` names
    /// the env vars a live one reads. `image_digest` is whatever pin the env
    /// carries so "pinned but URL unset" is visible.
    pub fn unwired(reason: &str, image_digest: &str) -> Self {
        Self {
            orchestrator: "unwired".into(),
            ready: false,
            reason: reason.trim().to_owned(),
            image_digest: image_digest.trim().to_owned(),
            vcpus: 0,
            mem_mib: 0,
            agent: None,
            agent_error: None,
            live_harvest_wired: false,
            custom_family_wired: false,
            registered_custom: Vec::new(),
        }
    }
}

/// Operator diagnostic over the topic-VM orchestrator this host resolved at
/// boot. The binary implements it over the live `FirecrackerOrchestrator`
/// (its `ready()` plus one agent health call) or the unwired stand-in; the
/// route only adds what the host knows (harvest wired, registered ids). It
/// changes nothing and spends nothing.
#[async_trait]
pub trait VmOrchestratorProbe: Send + Sync {
    /// Snapshot as of now (bearer file and pin re-read; one agent round trip).
    async fn probe(&self) -> VmOrchestratorReport;
}

/// Shared HTTP state.
#[derive(Clone)]
pub struct AppState {
    /// Submission + topic store.
    pub store: MemoryStore,
    /// Global pin (floors + image + topic key).
    pub pin: ProofPin,
    /// Backend that is allowed to produce scores on this host.
    pub backend: EvalBackend,
    /// Live scorer by metric family: the digest-pinned Lium harvest for
    /// `nll` / `throughput` and/or the custom-family RLM scorer (each wired
    /// on its own; a family whose route is missing refuses per topic). `None`
    /// on a live host means nothing can score, so submissions refuse.
    pub live_scorer: Option<Arc<dyn LiveScorer>>,
    /// Live RLM judge backend (operator state). Missing/closed → can_score false.
    pub offer: Option<InferenceOffer>,
    /// Live `1x` eval executor (operator state). On the Lium path
    /// missing/closed/shape ≠ pin → can_score false.
    pub executor: ExecutorSlot,
    /// Judge API key from `PROOF_INFERENCE_API_KEY_FILE`. Never on `/v1/status`.
    pub judge_api_key: Option<String>,
    /// Operator bearer hashes (sha256 hex). Empty → admin 503.
    pub admin_hashes: Arc<Vec<String>>,
    /// Topic-VM orchestrator diagnostic for `GET /v1/admin/proof/vm-orchestrator`.
    /// `None` = the host resolved none (the route then reports `none`).
    pub vm_probe: Option<Arc<dyn VmOrchestratorProbe>>,
    /// Chain epoch used for topic windows. v0 hosts pass 0.
    pub epoch: u64,
}

impl AppState {
    fn live(&self) -> Option<&dyn LiveScorer> {
        self.live_scorer.as_deref()
    }

    /// Snapshot of the live executor offer (a poisoned lock still reads).
    pub fn executor_offer(&self) -> Option<EvalExecutorOffer> {
        self.executor
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn set_executor_offer(&self, offer: EvalExecutorOffer) {
        *self
            .executor
            .write()
            .unwrap_or_else(PoisonError::into_inner) = Some(offer);
    }

    fn executor_pin_view(&self) -> serde_json::Value {
        serde_json::json!({
            "schema_version": self.pin.eval_executor_schema_version,
            "gpu_class": self.pin.gpu_class,
            "max_proof_deadline_s_ceiling": self.pin.max_proof_deadline_s_ceiling,
            "allowed_lium_template_prefixes": self.pin.allowed_lium_template_prefixes,
            "commitment_alg": self.pin.eval_executor_commitment_alg,
        })
    }

    /// Custom metric ids with a registered runner on this host. There is no
    /// compiled-in list: a topic mints its id, a runner registered under it
    /// makes the topic scorable.
    fn registered_custom(&self) -> Vec<String> {
        registered_custom(self.live())
    }

    /// Whether the digest-pinned Lium harvest — the `nll` / `throughput`
    /// scorer — is wired. Lium only: a custom-only host answers `false`.
    fn live_harvest_wired(&self) -> bool {
        self.live().is_some_and(LiveScorer::harvest_wired)
    }

    /// Registered custom ids whose runner could run right now (topic-VM
    /// orchestrator wired, image pinned). Independent of the harvest and of
    /// which topics are open.
    fn custom_ready(&self) -> Vec<String> {
        self.live()
            .map_or_else(Vec::new, LiveScorer::ready_custom_ids)
    }

    /// Whether the host-wide gates (digest, harvest, judge offer, executor,
    /// key, any open sealed topic) pass.
    fn host_ready(&self) -> bool {
        let open = self.store.any_open_scorable(self.epoch).unwrap_or(false);
        scoring_readiness(
            &self.pin,
            self.backend,
            self.live(),
            open,
            self.offer.as_ref(),
            self.executor_offer().as_ref(),
            self.judge_api_key.as_deref(),
        )
        .is_ok()
    }

    /// Open topics this host can score right now: judge config resolves and,
    /// on a live host, the family's scorer is wired (a custom topic whose
    /// runner is not registered is open but not scorable).
    fn scorable_topics(&self) -> Vec<String> {
        if !self.host_ready() {
            return Vec::new();
        }
        let secret = secret_backed_base_url();
        self.store
            .topics()
            .unwrap_or_default()
            .iter()
            .filter(|t| {
                t.is_open_at(self.epoch)
                    && resolve_inference(
                        &self.pin,
                        Some(&t.inference),
                        secret.as_deref(),
                        self.offer.as_ref(),
                    )
                    .ready_to_score()
                    && self.live().is_none_or(|s| s.ready_for_topic(t).is_ok())
            })
            .map(|t| t.id.clone())
            .collect()
    }

    fn can_score(&self) -> bool {
        !self.scorable_topics().is_empty()
    }
}

/// Build the router.
pub fn proof_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/v1/status", get(status))
        .route("/v1/proof/topics", get(list_topics))
        .route("/v1/proof/topics/{id}", get(get_topic))
        .route("/v1/proof/executor", get(get_executor))
        .route("/v1/submissions", post(submit).get(list_subs))
        .route("/v1/submissions/{id}", get(get_sub))
        .route("/v1/admin/proof/topics", post(publish_topic))
        .route("/v1/admin/proof/executor", post(rotate_executor))
        .route(
            "/v1/admin/proof/vm-orchestrator",
            get(vm_orchestrator_probe),
        )
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
    let open = st.store.open_ids(st.epoch).unwrap_or_default();
    let baseline_sealed = st.store.any_open_scorable(st.epoch).unwrap_or(false);
    // Family wiring is reported per family, never conflated: the harvest
    // flag is Lium-only, the custom family has its own fields.
    let registered_custom = st.registered_custom();
    Json(serde_json::json!({
        "challenge_id": CHALLENGE_ID,
        "scoring_version": SCORING_VERSION,
        "score_max": SCORE_MAX,
        "eval_image": st.pin.eval_image,
        "eval_image_digest": st.pin.eval_image_digest,
        "inference_offer": st.offer.as_ref().map(InferenceOffer::public_view),
        "inference": {
            "provider": st.pin.inference.provider.as_str(),
            "model": st.pin.inference.model,
            "mode": st.pin.inference.mode.as_str(),
            "max_input_tokens": st.pin.inference.max_input_tokens,
            "max_output_tokens": st.pin.inference.max_output_tokens,
        },
        "eval_executor": st.executor_offer().as_ref().map(EvalExecutorOffer::public_view),
        "executor": st.executor_pin_view(),
        "eval_backend": st.backend,
        "force_sim": force_sim(),
        "sim_stub_win": st.backend == EvalBackend::Sim,
        "can_score": st.can_score(),
        "live_harvest_wired": st.live_harvest_wired(),
        "custom_family_wired": !registered_custom.is_empty(),
        "baseline_sealed": baseline_sealed,
        "open_topics": open,
        "scorable_topics": st.scorable_topics(),
        "registered_custom": registered_custom,
        "custom_ready": st.custom_ready(),
        "epoch": st.epoch,
    }))
}

async fn list_topics(State(st): State<AppState>) -> impl IntoResponse {
    let items = st.store.topics().unwrap_or_default();
    Json(serde_json::json!({ "items": items }))
}

async fn get_topic(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let doc = st
        .store
        .topic(&id)
        .map_err(|_| err(StatusCode::NOT_FOUND, "unknown topic"))?;
    Ok(Json(doc))
}

/// Public executor contract: the live offer (every field is public), whether
/// it can rent right now, and the pin ceilings it is bound by. Always 200 —
/// a missing offer is `eval_executor: null` with the reason, never a 404.
async fn get_executor(State(st): State<AppState>) -> impl IntoResponse {
    let offer = st.executor_offer();
    let readiness = require_open_executor(offer.as_ref(), &st.pin).map(|_| ());
    Json(serde_json::json!({
        "eval_executor": offer.as_ref().map(EvalExecutorOffer::public_view),
        "ready": readiness.is_ok(),
        "reason": readiness.err().map(|e| e.to_string()),
        "pin": st.executor_pin_view(),
    }))
}

#[derive(Debug, Deserialize)]
struct SubmitBody {
    miner_hotkey: String,
    artifact_digest: String,
    artifact_uri: Option<String>,
    #[serde(default)]
    claim: String,
    #[serde(default)]
    declared_flops: u64,
    #[serde(default)]
    topic_id: String,
    /// Optional miner label. Not compared to an HF id (that check is retired).
    #[serde(default)]
    architecture: String,
    #[serde(default)]
    manifest: ArtifactManifest,
}

#[derive(Debug, Serialize)]
struct SubmitResp {
    id: String,
    submission_digest: String,
    topic_id: String,
    state: SubmissionState,
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

fn nonce_from(hotkey: &str, topic_id: &str, digest: &str) -> String {
    let mut h = Sha256::new();
    h.update(b"proof-nonce-v1");
    h.update(hotkey.as_bytes());
    h.update(topic_id.as_bytes());
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
    let _lium_present = headers
        .get("x-lium-api-key")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|s| !s.is_empty());

    let topic_id = body.topic_id.trim().to_owned();
    if topic_id.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "topic_id is required"));
    }
    let Ok(topic) = st.store.topic(&topic_id) else {
        return Err(err(StatusCode::BAD_REQUEST, "unknown topic"));
    };
    if !topic.is_open_at(st.epoch) {
        return Err(err(StatusCode::BAD_REQUEST, "topic is not open"));
    }
    if body.declared_flops > topic.flops_budget {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "declared_flops exceeds the topic budget",
        ));
    }
    // A custom-family runner retrieves the artefact from the miner's locator
    // inside the topic VM; with none there is nothing to inspect, so the
    // submission is refused here, before any row or rent.
    let artifact_uri = body
        .artifact_uri
        .as_deref()
        .map(str::trim)
        .filter(|u| !u.is_empty());
    if topic.metric.family == MetricFamily::Custom && artifact_uri.is_none() {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "artifact_uri is required for custom topics",
        ));
    }

    let nonce = nonce_from(&hotkey, &topic_id, &artifact);
    let submission_digest = freeze_submission_digest(&hotkey, &topic_id, &artifact, &nonce);

    // One snapshot of the executor for this request: rotation mid-submit
    // must not score under one offer and stamp another.
    let executor = st.executor_offer();
    scoring_readiness(
        &st.pin,
        st.backend,
        st.live(),
        st.store.any_open_scorable(st.epoch).unwrap_or(false),
        st.offer.as_ref(),
        executor.as_ref(),
        st.judge_api_key.as_deref(),
    )
    .map_err(|e| eval_err(&e))?;
    // A custom topic whose runner is not registered on this host is a 503
    // here, before any row or rent, not a rejected row downstream.
    if let Some(live) = st.live() {
        live.ready_for_topic(&topic).map_err(|e| eval_err(&e))?;
    }
    let Some(offer) = st.offer.as_ref() else {
        return Err(eval_err(&EvalError::InferenceOfferMissing));
    };
    offer
        .serves_topic(&st.pin, &topic)
        .map_err(|e| offer_err(&e))?;
    if st.backend == EvalBackend::Lium {
        executor
            .as_ref()
            .ok_or(EvalError::ExecutorOfferMissing)
            .and_then(|x| x.serves_topic(&topic).map_err(proof_eval::map_executor_err))
            .map_err(|e| eval_err(&e))?;
    }
    let resolved = resolve_inference(
        &st.pin,
        Some(&topic.inference),
        secret_backed_base_url().as_deref(),
        Some(offer),
    );
    if !resolved.ready_to_score() {
        return Err(offer_err(&OfferError::Incomplete));
    }

    let sealed = st
        .store
        .baseline(&topic_id)
        .map_err(|e| store_err(&e))?
        .ok_or_else(|| {
            err(
                StatusCode::SERVICE_UNAVAILABLE,
                "no sealed baseline recorded for this topic",
            )
        })?;

    let holdout = st
        .store
        .unseal_holdout(&topic_id, &submission_digest)
        .map_err(|e| err(StatusCode::SERVICE_UNAVAILABLE, &e.to_string()))?;

    let (declared, hits) = contamination_evidence(&body.manifest, &holdout);
    if !declared || !hits.is_empty() {
        let failed = if declared {
            vec![GateFail::Contamination]
        } else {
            vec![GateFail::EvidenceMissing {
                field: "contamination_evidence".into(),
            }]
        };
        return persist_pre_eval_reject(
            &st,
            executor.as_ref(),
            body,
            &topic,
            hotkey,
            artifact,
            nonce,
            submission_digest,
            &failed,
        );
    }

    let eval = eval_after_freeze(
        &st.pin,
        &topic,
        offer,
        executor.as_ref(),
        &submission_digest,
        &artifact,
        artifact_uri,
        body.declared_flops,
        &holdout,
        &body.claim,
        st.backend,
        st.live(),
        st.judge_api_key.as_deref(),
        Some(&sealed),
    )
    .await
    .map_err(|e| eval_err(&e))?;

    let registered = st.registered_custom();
    let verdict = judge_topic(
        &topic,
        &eval.agent,
        &eval.harness,
        &sealed,
        &hits,
        &custom_ids_ref(&registered),
    );
    let receipt_json = serde_json::to_string(&eval.receipt).unwrap_or_default();
    persist_scored(
        &st,
        executor.as_ref(),
        eval.executor.as_ref(),
        body,
        &topic,
        hotkey,
        artifact,
        nonce,
        submission_digest,
        verdict,
        receipt_json,
        eval.backend,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
fn persist_pre_eval_reject(
    st: &AppState,
    executor: Option<&EvalExecutorOffer>,
    body: SubmitBody,
    topic: &TopicDocument,
    hotkey: String,
    artifact: String,
    nonce: String,
    submission_digest: String,
    failed: &[GateFail],
) -> Result<(StatusCode, Json<SubmitResp>), (StatusCode, Json<serde_json::Value>)> {
    let verdict = ProofVerdict {
        pass: false,
        agent: AgentVerdict {
            verdict: ProofKind::Reject,
            reproduced: false,
            claim_holds_public: false,
            contamination: failed.iter().any(|f| matches!(f, GateFail::Contamination)),
            canary_hit: false,
            flops_used: 0,
            flops_budget: topic.flops_budget,
            cheat_codes: Vec::new(),
            rationale: "pre-eval reject".into(),
            topic_id: topic.id.clone(),
            family: topic.metric.family,
        },
        harness: HarnessMetrics::default(),
        failed: failed.to_vec(),
        lattice: 0,
    };
    let row = st
        .store
        .insert(Submission {
            id: String::new(),
            topic_id: topic.id.clone(),
            miner_hotkey: hotkey,
            artifact_digest: artifact,
            artifact_uri: body.artifact_uri,
            claim: body.claim,
            declared_flops: body.declared_flops,
            architecture: body.architecture,
            inference_offer_id: st
                .offer
                .as_ref()
                .map(|o| o.offer_id.clone())
                .unwrap_or_default(),
            config_commitment: st
                .offer
                .as_ref()
                .map(|o| o.config_commitment.clone())
                .unwrap_or_default(),
            executor_offer_id: executor.map(|x| x.offer_id.clone()).unwrap_or_default(),
            executor_commitment: executor
                .map(|x| x.config_commitment.clone())
                .unwrap_or_default(),
            manifest: body.manifest,
            nonce,
            submission_digest,
            state: SubmissionState::Rejected,
            receipt_json: None,
            verdict: Some(verdict),
            detail: Some(format!("gates={failed:?}")),
        })
        .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "store"))?;
    let _ = st.store.record_topic_run(
        &row.miner_hotkey,
        &topic.id,
        MinerTopicRun {
            pass: false,
            primary: None,
            artifact_digest: row.artifact_digest.clone(),
            near_duplicate: false,
        },
    );
    Ok((
        StatusCode::CREATED,
        Json(SubmitResp {
            id: row.id,
            submission_digest: row.submission_digest,
            topic_id: topic.id.clone(),
            state: row.state,
            eval_backend: st.backend,
            eligible: false,
        }),
    ))
}

#[allow(clippy::too_many_arguments)]
async fn persist_scored(
    st: &AppState,
    executor: Option<&EvalExecutorOffer>,
    plan: Option<&ExecutorPlan>,
    body: SubmitBody,
    topic: &TopicDocument,
    hotkey: String,
    artifact: String,
    nonce: String,
    submission_digest: String,
    verdict: ProofVerdict,
    receipt_json: String,
    backend: EvalBackend,
) -> Result<(StatusCode, Json<SubmitResp>), (StatusCode, Json<serde_json::Value>)> {
    let topic_id = topic.id.clone();
    let pass = verdict.pass;
    let primary = primary_from_harness(topic, &verdict.harness);
    // Automatic promotion is a family decision (custom: pass + green checklist
    // + relative win over the bar). Default scorers never crown.
    let mut promoted = false;
    if pass {
        if let Some(live) = st.live() {
            let bar = novelty_bar(
                topic,
                st.store.baseline(&topic_id).ok().flatten().as_ref(),
                st.store.champion_primary(topic).ok().flatten(),
            );
            promoted = live
                .auto_promote(topic, &submission_digest, pass, primary, bar)
                .await;
        }
    }
    let artifact_digest = artifact.clone();
    let detail = if pass {
        None
    } else {
        Some(format!("gates={:?}", verdict.failed))
    };
    let row = st
        .store
        .insert(Submission {
            id: String::new(),
            topic_id: topic_id.clone(),
            miner_hotkey: hotkey,
            artifact_digest: artifact,
            artifact_uri: body.artifact_uri,
            claim: body.claim,
            declared_flops: body.declared_flops,
            architecture: body.architecture,
            inference_offer_id: st
                .offer
                .as_ref()
                .map(|o| o.offer_id.clone())
                .unwrap_or_default(),
            config_commitment: st
                .offer
                .as_ref()
                .map(|o| o.config_commitment.clone())
                .unwrap_or_default(),
            executor_offer_id: executor.map(|x| x.offer_id.clone()).unwrap_or_default(),
            // A live run stamps the configuration it was actually held to
            // (template, 1x, effective deadline); sim has no plan and no rent.
            executor_commitment: plan
                .map(|p| p.config_commitment.clone())
                .or_else(|| executor.map(|x| x.config_commitment.clone()))
                .unwrap_or_default(),
            manifest: body.manifest,
            nonce,
            submission_digest,
            state: if promoted {
                SubmissionState::Champion
            } else if pass {
                SubmissionState::AwaitingAdmin
            } else {
                SubmissionState::Rejected
            },
            receipt_json: Some(receipt_json),
            verdict: Some(verdict),
            detail,
        })
        .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "store"))?;
    let _ = st.store.record_topic_run(
        &row.miner_hotkey,
        &topic_id,
        MinerTopicRun {
            pass,
            primary,
            artifact_digest,
            near_duplicate: false,
        },
    );
    if let Some(live) = st.live() {
        live.on_persisted(&topic_id, &row.submission_digest, &row.id, promoted)
            .await;
    }
    Ok((
        StatusCode::CREATED,
        Json(SubmitResp {
            id: row.id,
            submission_digest: row.submission_digest,
            topic_id,
            state: row.state,
            eval_backend: backend,
            eligible: pass,
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

async fn publish_topic(
    State(st): State<AppState>,
    headers: HeaderMap,
    Json(doc): Json<TopicDocument>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    if st.admin_hashes.is_empty() {
        return Err(err(StatusCode::SERVICE_UNAVAILABLE, "auth_unconfigured"));
    }
    if !admin_ok(&headers, &st.admin_hashes) {
        return Err(err(StatusCode::UNAUTHORIZED, "unauthorized"));
    }
    let registered = st.registered_custom();
    doc.validate(&st.pin, &custom_ids_ref(&registered))
        .map_err(|e| topic_err(&e))?;
    doc.verify_signature(&st.pin).map_err(|e| topic_err(&e))?;
    if doc.status == TopicStatus::Open && !doc.baseline.is_sealed() {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "an open topic must carry a sealed baseline",
        ));
    }
    st.store.put_topic(doc.clone()).map_err(|e| store_err(&e))?;
    Ok((StatusCode::CREATED, Json(doc)))
}

/// Rotate the live executor offer. The body is the same document as
/// `PROOF_EVAL_EXECUTOR_OFFER_FILE`; it must validate against the pin
/// (shape, deadline ceiling, template allowlist, digest, commitment) or it is
/// a 400 and the previous offer stays. Posting `status: closed` is how an
/// operator takes the executor down without a restart.
async fn rotate_executor(
    State(st): State<AppState>,
    headers: HeaderMap,
    Json(offer): Json<EvalExecutorOffer>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    if st.admin_hashes.is_empty() {
        return Err(err(StatusCode::SERVICE_UNAVAILABLE, "auth_unconfigured"));
    }
    if !admin_ok(&headers, &st.admin_hashes) {
        return Err(err(StatusCode::UNAUTHORIZED, "unauthorized"));
    }
    offer
        .validate(&st.pin)
        .map_err(|e| err(StatusCode::BAD_REQUEST, &e.to_string()))?;
    st.set_executor_offer(offer.clone());
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "eval_executor": offer.public_view(),
            "can_score": st.can_score(),
        })),
    ))
}

/// Operator probe: is the topic-VM orchestrator wired, is its bearer file
/// and RLM image pin in place, and does the KVM-host agent answer? Same
/// bearer gate as the other admin routes; always 200 once authorised (a
/// broken wire is data, not an error). Read-only, no VM, no spend.
async fn vm_orchestrator_probe(
    State(st): State<AppState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    if st.admin_hashes.is_empty() {
        return Err(err(StatusCode::SERVICE_UNAVAILABLE, "auth_unconfigured"));
    }
    if !admin_ok(&headers, &st.admin_hashes) {
        return Err(err(StatusCode::UNAUTHORIZED, "unauthorized"));
    }
    let mut report = match &st.vm_probe {
        Some(probe) => probe.probe().await,
        None => VmOrchestratorReport::unwired("no topic-vm orchestrator resolved on this host", ""),
    };
    report.live_harvest_wired = st.live_harvest_wired();
    report.registered_custom = st.registered_custom();
    report.custom_family_wired = !report.registered_custom.is_empty();
    Ok(Json(report))
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

fn store_err(e: &proof_store::StoreError) -> (StatusCode, Json<serde_json::Value>) {
    err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string())
}

fn topic_err(e: &TopicError) -> (StatusCode, Json<serde_json::Value>) {
    let code = match e {
        TopicError::NoTopicKey => StatusCode::SERVICE_UNAVAILABLE,
        _ => StatusCode::BAD_REQUEST,
    };
    err(code, &e.to_string())
}

fn offer_err(e: &OfferError) -> (StatusCode, Json<serde_json::Value>) {
    err(StatusCode::SERVICE_UNAVAILABLE, &e.to_string())
}

fn eval_err(e: &EvalError) -> (StatusCode, Json<serde_json::Value>) {
    let code = match e {
        EvalError::Integrity(_) => StatusCode::INTERNAL_SERVER_ERROR,
        _ => StatusCode::SERVICE_UNAVAILABLE,
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
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use proof_eval::{sim_document, BaselineMeasurement, FamilyMux, BASELINE_SKILL};
    use proof_task::{
        default_adamw, holdout_commitment, inference_config_commitment, synthetic_holdout,
        Constraints, HoldoutSplit, InferenceConfig, InferenceMode, InferenceOffer,
        InferenceProvider, InferenceProviderKind, MetricDirection, MetricFamily, MetricSpec,
        OfferStatus, TopicDocument, TopicStatus, FLOPS_BUDGET_MAX, HOLDOUT_SIZE,
        METRIC_TOKENS_PER_SEC, STRATUM_SIZE,
    };
    use tower::ServiceExt;

    use super::*;

    fn digest(label: &str) -> String {
        let mut h = Sha256::new();
        h.update(label.as_bytes());
        hex::encode(h.finalize())
    }

    fn sk() -> [u8; 32] {
        let mut s = [3u8; 32];
        s[0] = 17;
        s
    }

    fn pk_hex() -> String {
        hex::encode(crypto::public_key_from_mini_secret(&sk()).expect("pk"))
    }

    fn pin(digest: &str) -> ProofPin {
        let mut p = ProofPin {
            eval_image_digest: digest.to_owned(),
            topic_pubkey: pk_hex(),
            ..ProofPin::default()
        };
        p.inference.model = "master-proxy-v0".into();
        p
    }

    fn offer() -> InferenceOffer {
        named_offer("master-v0")
    }

    /// Staging sim offer id (operator-published; miners do not bind it).
    fn staging_offer() -> InferenceOffer {
        named_offer("openrouter-glm53flash-v0")
    }

    fn named_offer(offer_id: &str) -> InferenceOffer {
        let config = InferenceConfig {
            mode: InferenceMode::Chat,
            model_ref: "master-proxy-v0".into(),
            max_input_tokens: 32_768,
            max_output_tokens: 8_192,
            temperature: Some(0.0),
            top_p: None,
            timeout_ms: None,
        };
        InferenceOffer {
            offer_id: offer_id.into(),
            provider: InferenceProvider {
                kind: InferenceProviderKind::OpenaiCompatible,
                base_url: "http://127.0.0.1:8000/v1".into(),
            },
            config_commitment: inference_config_commitment(&config, "http://127.0.0.1:8000/v1"),
            config,
            status: OfferStatus::Open,
        }
    }

    fn unsigned_topic(recs: &[proof_task::HoldoutRecord]) -> TopicDocument {
        let mut baseline = default_adamw(FLOPS_BUDGET_MAX);
        baseline.optimizer = "nccl-ib-reference".into();
        baseline.wall_budget_s = 14_400;
        baseline.script_sha256 = "11".repeat(32);
        TopicDocument {
            id: "dt-no-ib-v0".into(),
            statement: "No IB/NVLink; 12.5 Gbit/s cap; beat sealed comms baseline.".into(),
            payout_mode: proof_task::PayoutMode::Wta,
            constraints: Constraints {
                no_infiniband: true,
                no_nvlink: true,
                no_nccl_fast_fabric: true,
                max_inter_node_gbps: Some(12.5),
                ..Constraints::default()
            },
            metric: MetricSpec {
                family: MetricFamily::Throughput,
                primary: METRIC_TOKENS_PER_SEC.into(),
                direction: MetricDirection::Max,
                unit: "tokens_per_second".into(),
                epsilon_rel: 0.05,
                quality_floor_nll: 0.02,
                wall_budget_s: 14_400,
                custom_id: String::new(),
            },
            baseline,
            holdout_commitment: holdout_commitment(recs),
            holdout_size: HOLDOUT_SIZE,
            status: TopicStatus::Open,
            ..TopicDocument::default()
        }
    }

    fn unsigned_muon_topic(recs: &[proof_task::HoldoutRecord]) -> TopicDocument {
        let mut baseline = default_adamw(FLOPS_BUDGET_MAX);
        baseline.script_sha256 = "11".repeat(32);
        TopicDocument {
            id: "muon-vs-adamw-10m-v0".into(),
            statement:
                "Beat sealed AdamW holdout NLL with Muon at ~10M params under the same FLOP budget."
                    .into(),
            payout_mode: proof_task::PayoutMode::Wta,
            metric: MetricSpec {
                family: MetricFamily::Nll,
                primary: proof_task::PRIMARY_HOLDOUT_NLL.into(),
                direction: MetricDirection::Min,
                unit: "nll".into(),
                epsilon_rel: 0.0,
                quality_floor_nll: 0.0,
                wall_budget_s: 0,
                custom_id: String::new(),
            },
            baseline,
            holdout_commitment: holdout_commitment(recs),
            holdout_size: HOLDOUT_SIZE,
            status: TopicStatus::Open,
            ..TopicDocument::default()
        }
    }

    fn seal_topic(pin: &ProofPin, topic: TopicDocument) -> (TopicDocument, BaselineMeasurement) {
        seal_topic_with(pin, topic, &[])
    }

    fn seal_topic_with(
        pin: &ProofPin,
        mut topic: TopicDocument,
        registered: &[&str],
    ) -> (TopicDocument, BaselineMeasurement) {
        let recs = synthetic_holdout(STRATUM_SIZE, 1);
        topic.holdout_commitment = holdout_commitment(&recs);
        let doc = sim_document(pin, &topic, "base", "base-art", BASELINE_SKILL, true);
        let meas = BaselineMeasurement {
            eval_image_digest: pin.eval_image_digest.clone(),
            topic_id: topic.id.clone(),
            holdout_commitment: topic.holdout_commitment.clone(),
            holdout_nll: doc.harness.holdout_nll,
            split_nll: doc.harness.split_nll.clone(),
            tokens_per_sec: doc.harness.tokens_per_sec,
            step_latency_ms: doc.harness.step_latency_ms,
            custom_value: doc.harness.custom_value,
        };
        topic.baseline.metrics_commitment = meas.commitment();
        topic.signature = topic.sign_with(&sk()).expect("sign");
        topic.validate(pin, registered).expect("valid");
        topic.verify_signature(pin).expect("sig");
        (topic, meas)
    }

    struct StubScorer {
        reproduced: bool,
        skill: f64,
        hits: AtomicUsize,
    }

    impl StubScorer {
        fn win() -> Self {
            Self {
                reproduced: true,
                skill: 0.95,
                hits: AtomicUsize::new(0),
            }
        }
        fn lose() -> Self {
            Self {
                reproduced: false,
                skill: 0.95,
                hits: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait]
    impl LiveScorer for StubScorer {
        async fn score(
            &self,
            pin: &ProofPin,
            topic: &TopicDocument,
            _offer: &InferenceOffer,
            _plan: &ExecutorPlan,
            frozen: &str,
            artifact: &str,
            _artifact_uri: Option<&str>,
            _declared_flops: u64,
            _holdout: &[proof_task::HoldoutRecord],
            _claim: &str,
        ) -> Result<proof_eval::ProofEvalDocument, EvalError> {
            self.hits.fetch_add(1, Ordering::SeqCst);
            Ok(sim_document(
                pin,
                topic,
                frozen,
                artifact,
                self.skill,
                self.reproduced,
            ))
        }
    }

    fn declared_manifest() -> ArtifactManifest {
        ArtifactManifest {
            train_dataset_ids: vec!["public-pretrain-v0".into()],
            ..ArtifactManifest::default()
        }
    }

    fn app_full(
        token: &str,
        backend: EvalBackend,
        eval_digest: &str,
        live: Option<Arc<dyn LiveScorer>>,
        load: bool,
        baseline: bool,
        with_offer: bool,
    ) -> Router {
        let p = pin(eval_digest);
        let store = MemoryStore::new();
        if load {
            let recs = synthetic_holdout(STRATUM_SIZE, 1);
            let (topic, meas) = seal_topic(&p, unsigned_topic(&recs));
            store.put_topic(topic.clone()).expect("topic");
            store.load_holdout(&topic.id, recs).expect("holdout");
            if baseline {
                store
                    .set_baseline(&topic.id, meas.into_sealed())
                    .expect("baseline");
            }
        }
        let judge_api_key =
            (backend == EvalBackend::Lium && live.is_some()).then(|| "test-judge-key".to_owned());
        // Lium + a wired harvest is the live path: it also needs the open 1x
        // executor. Sim rents nothing, so the slot stays empty there.
        let executor = (backend == EvalBackend::Lium && live.is_some()).then(|| test_executor(&p));
        proof_router(AppState {
            store,
            pin: p,
            backend,
            live_scorer: live,
            offer: with_offer.then(offer),
            executor: executor_slot(executor),
            // Lium + a wired harvest is the live path: a missing key is the
            // Testeur blocker. Sim does not call the judge, so it stays None.
            judge_api_key,
            admin_hashes: Arc::new(vec![hash_admin_token(token)]),
            vm_probe: None,
            epoch: 0,
        })
    }

    fn app(token: &str) -> Router {
        app_full(token, EvalBackend::Sim, "", None, true, true, true)
    }

    /// Open `1x` executor on the digest-scoped template of `pin`.
    fn test_executor(pin: &ProofPin) -> EvalExecutorOffer {
        let hex = pin.eval_image_digest.trim_start_matches("sha256:");
        let mut o = EvalExecutorOffer {
            offer_id: "lium-1x-v0".into(),
            lium_template_id: format!("proof-eval-{}", hex.get(..12).unwrap_or("unpinned")),
            machine_shape: "1x".into(),
            max_proof_deadline_s: 3_600,
            eval_image_digest: pin.eval_image_digest.clone(),
            config_commitment: String::new(),
            status: proof_executor::OfferStatus::Open,
        };
        o.config_commitment = o.expected_commitment();
        o
    }

    /// Lium host with a wired harvest whose executor slot holds `executor`.
    fn app_lium_with_executor(executor: Option<EvalExecutorOffer>) -> Router {
        let p = pin(&format!("sha256:{}", "ab".repeat(32)));
        let store = MemoryStore::new();
        let recs = synthetic_holdout(STRATUM_SIZE, 1);
        let (topic, meas) = seal_topic(&p, unsigned_topic(&recs));
        store.put_topic(topic.clone()).expect("topic");
        store.load_holdout(&topic.id, recs).expect("holdout");
        store
            .set_baseline(&topic.id, meas.into_sealed())
            .expect("baseline");
        proof_router(AppState {
            store,
            pin: p,
            backend: EvalBackend::Lium,
            live_scorer: Some(Arc::new(StubScorer::win())),
            offer: Some(offer()),
            executor: executor_slot(executor),
            judge_api_key: Some("test-judge-key".into()),
            admin_hashes: Arc::new(vec![hash_admin_token("op")]),
            vm_probe: None,
            epoch: 0,
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

    fn submit_body(label: &str, extra: &serde_json::Value) -> serde_json::Value {
        let mut v = serde_json::json!({
            "miner_hotkey": digest("miner-hotkey"),
            "artifact_digest": digest(label),
            "claim": "beats the sealed reference under the cap",
            "declared_flops": FLOPS_BUDGET_MAX / 2,
            "topic_id": "dt-no-ib-v0",
            "manifest": {
                "train_dataset_ids": ["public-pretrain-v0"]
            },
        });
        if let Some(obj) = extra.as_object() {
            if let Some(dst) = v.as_object_mut() {
                for (k, val) in obj {
                    dst.insert(k.clone(), val.clone());
                }
            }
        }
        v
    }

    #[tokio::test]
    async fn health_and_status_name_the_challenge() {
        let (st, health) = json_req(app("op"), "GET", "/health", serde_json::json!({}), None).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(health["challenge_id"], "proof");

        let (st, body) =
            json_req(app("op"), "GET", "/v1/status", serde_json::json!({}), None).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(body["eval_backend"], "sim");
        assert_eq!(body["sim_stub_win"], true, "{body}");
        assert_eq!(body["can_score"], true, "{body}");
        assert_eq!(body["baseline_sealed"], true, "{body}");
        assert_eq!(body["open_topics"][0], "dt-no-ib-v0");
        let dump = body.to_string();
        assert!(!dump.contains("synthetic-dev"), "{dump}");
        assert!(!dump.contains("holdout_nll"));
        assert_eq!(body["inference_offer"]["offer_id"], "master-v0");
        assert_eq!(body["inference_offer"]["status"], "open");
        assert_eq!(body["inference"]["provider"], "openai_compatible");
        assert_eq!(body["inference"]["mode"], "chat");
        assert_eq!(body["inference"]["model"], "master-proxy-v0");
        assert!(!dump.contains("8000"), "{dump}");
        assert!(!dump.contains("base_url"), "{dump}");
        assert!(!dump.contains("api_key"), "{dump}");
        assert!(!dump.contains("evil.example"), "{dump}");
        // Sim rents nothing: no executor offer, still scorable; the pin
        // ceilings are public regardless.
        assert!(body["eval_executor"].is_null(), "{body}");
        assert_eq!(body["executor"]["gpu_class"], "1x");
        assert_eq!(body["executor"]["max_proof_deadline_s_ceiling"], 7_200);
    }

    #[tokio::test]
    async fn status_and_executor_route_expose_the_public_executor_contract() {
        let p = pin(&format!("sha256:{}", "ab".repeat(32)));
        let app = app_lium_with_executor(Some(test_executor(&p)));
        let (st, body) = json_req(
            app.clone(),
            "GET",
            "/v1/status",
            serde_json::json!({}),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(body["can_score"], true, "{body}");
        assert_eq!(body["eval_executor"]["offer_id"], "lium-1x-v0");
        assert_eq!(body["eval_executor"]["machine_shape"], "1x");
        assert_eq!(body["eval_executor"]["gpu_count"], 1);
        assert_eq!(body["eval_executor"]["max_proof_deadline_s"], 3_600);
        assert_eq!(
            body["eval_executor"]["lium_template_id"],
            "proof-eval-abababababab"
        );
        assert_eq!(body["eval_executor"]["status"], "open");
        assert_eq!(body["executor"]["gpu_class"], "1x");
        assert_eq!(body["executor"]["schema_version"], 1);

        let (st, view) = json_req(
            app,
            "GET",
            "/v1/proof/executor",
            serde_json::json!({}),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(view["ready"], true, "{view}");
        assert!(view["reason"].is_null(), "{view}");
        assert_eq!(view["eval_executor"]["offer_id"], "lium-1x-v0");
        assert_eq!(view["pin"]["max_proof_deadline_s_ceiling"], 7_200);
        let dump = view.to_string();
        assert!(!dump.contains("api_key"), "{dump}");
        assert!(!dump.contains("/run/base"), "{dump}");
    }

    #[tokio::test]
    async fn missing_closed_or_wide_executor_is_can_score_false_and_submit_503() {
        let p = pin(&format!("sha256:{}", "ab".repeat(32)));
        let mut closed = test_executor(&p);
        closed.status = proof_executor::OfferStatus::Closed;
        let mut wide = test_executor(&p);
        wide.machine_shape = "8x".into();
        wide.config_commitment = wide.expected_commitment();
        for (label, executor, want) in [
            ("missing", None, "executor offer missing"),
            ("closed", Some(closed), "closed"),
            ("8x", Some(wide), "machine_shape"),
        ] {
            let app = app_lium_with_executor(executor);
            let (st, status) = json_req(
                app.clone(),
                "GET",
                "/v1/status",
                serde_json::json!({}),
                None,
            )
            .await;
            assert_eq!(st, StatusCode::OK);
            assert_eq!(status["live_harvest_wired"], true, "{label}: {status}");
            assert_eq!(status["can_score"], false, "{label}: {status}");

            let (st, view) = json_req(
                app.clone(),
                "GET",
                "/v1/proof/executor",
                serde_json::json!({}),
                None,
            )
            .await;
            assert_eq!(st, StatusCode::OK);
            assert_eq!(view["ready"], false, "{label}: {view}");
            assert!(
                view["reason"].as_str().unwrap_or_default().contains(want),
                "{label}: {view}"
            );

            let (st, body) = json_req(
                app.clone(),
                "POST",
                "/v1/submissions",
                submit_body("x", &serde_json::json!({})),
                None,
            )
            .await;
            assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE, "{label}: {body}");
            assert!(
                body["error"].as_str().unwrap_or_default().contains(want),
                "{label}: {body}"
            );
            let (_, list) =
                json_req(app, "GET", "/v1/submissions", serde_json::json!({}), None).await;
            assert!(
                list["items"].as_array().is_some_and(Vec::is_empty),
                "{label} banked rows: {list}"
            );
        }
    }

    #[tokio::test]
    async fn admin_rotate_executor_requires_bearer_validates_and_takes_effect() {
        let p = pin(&format!("sha256:{}", "ab".repeat(32)));
        let app = app_lium_with_executor(None);
        let good = serde_json::to_value(test_executor(&p)).expect("json");

        let (st, _) = json_req(
            app.clone(),
            "POST",
            "/v1/admin/proof/executor",
            good.clone(),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);

        let mut wide = test_executor(&p);
        wide.machine_shape = "8x".into();
        wide.config_commitment = wide.expected_commitment();
        let (st, body) = json_req(
            app.clone(),
            "POST",
            "/v1/admin/proof/executor",
            serde_json::to_value(&wide).expect("json"),
            Some("op"),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
        assert!(
            body["error"]
                .as_str()
                .unwrap_or_default()
                .contains("machine_shape"),
            "{body}"
        );
        let mut forged = test_executor(&p);
        forged.config_commitment = "cd".repeat(32);
        let (st, body) = json_req(
            app.clone(),
            "POST",
            "/v1/admin/proof/executor",
            serde_json::to_value(&forged).expect("json"),
            Some("op"),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
        let (_, status) = json_req(
            app.clone(),
            "GET",
            "/v1/status",
            serde_json::json!({}),
            None,
        )
        .await;
        assert_eq!(
            status["can_score"], false,
            "refused rotations leave the slot empty"
        );

        let (st, created) = json_req(
            app.clone(),
            "POST",
            "/v1/admin/proof/executor",
            good,
            Some("op"),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{created}");
        assert_eq!(created["eval_executor"]["offer_id"], "lium-1x-v0");
        assert_eq!(created["can_score"], true, "{created}");
        let (st, created) = json_req(
            app.clone(),
            "POST",
            "/v1/submissions",
            submit_body("after-rotate", &serde_json::json!({})),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{created}");
        let id = created["id"].as_str().expect("id");
        let (_, row) = json_req(
            app.clone(),
            "GET",
            &format!("/v1/submissions/{id}"),
            serde_json::json!({}),
            None,
        )
        .await;
        assert_eq!(row["executor_offer_id"], "lium-1x-v0", "{row}");
        assert_eq!(
            row["executor_commitment"],
            test_executor(&p).config_commitment,
            "{row}"
        );

        // Closing is the same route: the host stops scoring without a restart.
        let mut closed = test_executor(&p);
        closed.status = proof_executor::OfferStatus::Closed;
        let (st, body) = json_req(
            app.clone(),
            "POST",
            "/v1/admin/proof/executor",
            serde_json::to_value(&closed).expect("json"),
            Some("op"),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{body}");
        assert_eq!(body["can_score"], false, "{body}");
        let (st, body) = json_req(
            app,
            "POST",
            "/v1/submissions",
            submit_body("after-close", &serde_json::json!({})),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    }

    /// What the binary hands the route on a wired host: a canned snapshot.
    struct StubProbe(VmOrchestratorReport);

    #[async_trait]
    impl VmOrchestratorProbe for StubProbe {
        async fn probe(&self) -> VmOrchestratorReport {
            self.0.clone()
        }
    }

    fn app_with_probe(probe: Option<Arc<dyn VmOrchestratorProbe>>, admin: bool) -> Router {
        let p = pin(&format!("sha256:{}", "ab".repeat(32)));
        proof_router(AppState {
            store: MemoryStore::new(),
            pin: p,
            backend: EvalBackend::Lium,
            live_scorer: Some(Arc::new(StubScorer::win())),
            offer: Some(offer()),
            executor: executor_slot(None),
            judge_api_key: Some("test-judge-key".into()),
            admin_hashes: Arc::new(if admin {
                vec![hash_admin_token("op")]
            } else {
                Vec::new()
            }),
            vm_probe: probe,
            epoch: 0,
        })
    }

    /// The operator probe sits behind the admin bearer, is always 200 once
    /// authorised (a broken wire is data), reports the host's own gates next
    /// to the client's snapshot, and never carries a bearer value.
    #[tokio::test]
    async fn admin_vm_orchestrator_probe_is_bearer_gated_and_reports_the_wire() {
        let none = app_with_probe(None, true);
        let (st, body) = json_req(
            none.clone(),
            "GET",
            "/v1/admin/proof/vm-orchestrator",
            serde_json::json!({}),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::UNAUTHORIZED, "{body}");
        let (st, body) = json_req(
            none.clone(),
            "GET",
            "/v1/admin/proof/vm-orchestrator",
            serde_json::json!({}),
            Some("wrong"),
        )
        .await;
        assert_eq!(st, StatusCode::UNAUTHORIZED, "{body}");
        let (st, body) = json_req(
            app_with_probe(None, false),
            "GET",
            "/v1/admin/proof/vm-orchestrator",
            serde_json::json!({}),
            Some("op"),
        )
        .await;
        assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE, "{body}");
        assert_eq!(body["error"], "auth_unconfigured");

        let (st, body) = json_req(
            none,
            "GET",
            "/v1/admin/proof/vm-orchestrator",
            serde_json::json!({}),
            Some("op"),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{body}");
        assert_eq!(body["orchestrator"], "unwired", "{body}");
        assert_eq!(body["ready"], false);
        assert_eq!(body["live_harvest_wired"], true, "{body}");
        assert_eq!(body["registered_custom"], serde_json::json!([]), "{body}");

        let mut wired = VmOrchestratorReport::unwired("", &format!("sha256:{}", "cd".repeat(32)));
        wired.orchestrator = "firecracker".into();
        wired.ready = true;
        wired.vcpus = 4;
        wired.mem_mib = 8_192;
        wired.agent = Some(VmAgentHealth {
            api_version: 1,
            ready: true,
            reason: String::new(),
            hypervisor: "firecracker".into(),
            vms: 2,
        });
        // The probe's own view of the host gates is overwritten by the route.
        wired.live_harvest_wired = false;
        wired.custom_family_wired = true;
        wired.registered_custom = vec!["stale".into()];
        let (st, body) = json_req(
            app_with_probe(Some(Arc::new(StubProbe(wired))), true),
            "GET",
            "/v1/admin/proof/vm-orchestrator",
            serde_json::json!({}),
            Some("op"),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{body}");
        let report: VmOrchestratorReport = serde_json::from_value(body.clone()).expect("typed");
        assert_eq!(report.orchestrator, "firecracker");
        assert!(report.ready);
        assert_eq!((report.vcpus, report.mem_mib), (4, 8_192));
        assert_eq!(
            report.agent.as_ref().map(|a| a.hypervisor.as_str()),
            Some("firecracker")
        );
        assert_eq!(report.agent.as_ref().map(|a| a.vms), Some(2));
        assert!(
            report.live_harvest_wired,
            "Lium-only host gate, not the probe's copy"
        );
        assert!(
            report.registered_custom.is_empty(),
            "host registry, not the probe's copy"
        );
        assert!(
            !report.custom_family_wired,
            "follows the host registry, not the probe's copy"
        );
        let dump = body.to_string();
        for forbidden in ["Bearer ", "\"token\"", "api_key"] {
            assert!(!dump.contains(forbidden), "{forbidden} in {dump}");
        }

        let (st, body) = json_req(
            app_with_probe(
                Some(Arc::new(StubProbe(VmOrchestratorReport::unwired(
                    "PROOF_VM_ORCHESTRATOR_TOKEN_FILE (/run/base/proof/vm_orchestrator_token) missing or empty",
                    "",
                )))),
                true,
            ),
            "GET",
            "/v1/admin/proof/vm-orchestrator",
            serde_json::json!({}),
            Some("op"),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{body}");
        assert_eq!(body["ready"], false);
        assert!(
            body["reason"]
                .as_str()
                .unwrap_or_default()
                .contains("PROOF_VM_ORCHESTRATOR_TOKEN_FILE"),
            "{body}"
        );
        assert_eq!(body["image_digest"], "", "unpinned stays visibly empty");
    }

    #[tokio::test]
    async fn topic_pinning_another_executor_commitment_is_503() {
        let p = pin(&format!("sha256:{}", "ab".repeat(32)));
        let store = MemoryStore::new();
        let recs = synthetic_holdout(STRATUM_SIZE, 1);
        let (mut topic, meas) = seal_topic(&p, unsigned_topic(&recs));
        topic.eval_executor.require_offer_commitment = Some("cd".repeat(32));
        topic.eval_executor.max_proof_deadline_s = Some(900);
        topic.signature = topic.sign_with(&sk()).expect("sign");
        topic.validate(&p, &[]).expect("tightened topic is legal");
        store.put_topic(topic.clone()).expect("topic");
        store.load_holdout(&topic.id, recs).expect("holdout");
        store
            .set_baseline(&topic.id, meas.into_sealed())
            .expect("baseline");
        let scorer = Arc::new(StubScorer::win());
        let app = proof_router(AppState {
            store,
            pin: p.clone(),
            backend: EvalBackend::Lium,
            live_scorer: Some(scorer.clone()),
            offer: Some(offer()),
            executor: executor_slot(Some(test_executor(&p))),
            judge_api_key: Some("test-judge-key".into()),
            admin_hashes: Arc::new(vec![hash_admin_token("op")]),
            vm_probe: None,
            epoch: 0,
        });
        let (st, body) = json_req(
            app,
            "POST",
            "/v1/submissions",
            submit_body("pinned", &serde_json::json!({})),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE, "{body}");
        assert!(
            body["error"]
                .as_str()
                .unwrap_or_default()
                .contains("cannot serve"),
            "{body}"
        );
        assert_eq!(scorer.hits.load(Ordering::SeqCst), 0, "no rent");
    }

    #[tokio::test]
    async fn topics_are_public_documents_without_records() {
        let (st, list) = json_req(
            app("op"),
            "GET",
            "/v1/proof/topics",
            serde_json::json!({}),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(list["items"][0]["id"], "dt-no-ib-v0");
        assert_eq!(list["items"][0]["metric"]["family"], "throughput");
        assert!(list["items"][0]["holdout_commitment"].is_string());
        let dump = list.to_string();
        assert!(!dump.contains("content_sha256"), "{dump}");

        let (st, one) = json_req(
            app("op"),
            "GET",
            "/v1/proof/topics/dt-no-ib-v0",
            serde_json::json!({}),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(one["status"], "open");
    }

    #[tokio::test]
    async fn submit_requires_an_open_topic_id() {
        let app = app("op");
        let (st, body) = json_req(
            app.clone(),
            "POST",
            "/v1/submissions",
            submit_body("x", &serde_json::json!({ "topic_id": "" })),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");

        let (st, body) = json_req(
            app,
            "POST",
            "/v1/submissions",
            submit_body("x", &serde_json::json!({ "topic_id": "unknown-v0" })),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
    }

    #[tokio::test]
    async fn reproduced_true_scores_and_false_zeros() {
        let win = Arc::new(StubScorer::win());
        let lose = Arc::new(StubScorer::lose());
        let digest = format!("sha256:{}", "ab".repeat(32));

        let (st, created) = json_req(
            app_full(
                "op",
                EvalBackend::Lium,
                &digest,
                Some(win),
                true,
                true,
                true,
            ),
            "POST",
            "/v1/submissions",
            submit_body("miner-strong-proof", &serde_json::json!({})),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{created}");
        assert_eq!(created["eval_backend"], "lium");
        assert_eq!(created["eligible"], true, "{created}");
        assert_eq!(created["state"], "awaiting_admin");

        let (st, created) = json_req(
            app_full(
                "op",
                EvalBackend::Lium,
                &digest,
                Some(lose),
                true,
                true,
                true,
            ),
            "POST",
            "/v1/submissions",
            submit_body("miner-unreproduced", &serde_json::json!({})),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{created}");
        assert_eq!(created["eligible"], false, "{created}");
        assert_eq!(created["state"], "rejected");
    }

    #[tokio::test]
    async fn contamination_is_rejected_without_renting() {
        let scorer = Arc::new(StubScorer::win());
        let recs = synthetic_holdout(STRATUM_SIZE, 1);
        let dirty_hash = recs[0].content_sha256.clone();
        let app = app_full(
            "op",
            EvalBackend::Lium,
            &format!("sha256:{}", "ab".repeat(32)),
            Some(scorer.clone()),
            true,
            true,
            true,
        );
        for manifest in [
            serde_json::json!({}),
            serde_json::json!({
                "manifest": { "train_content_hashes": [dirty_hash] }
            }),
        ] {
            let empty_evidence = serde_json::json!({ "manifest": { "train_dataset_ids": [] } });
            let body = if manifest.as_object().is_some_and(serde_json::Map::is_empty) {
                submit_body("junk", &empty_evidence)
            } else {
                submit_body("junk", &manifest)
            };
            let (st, created) = json_req(app.clone(), "POST", "/v1/submissions", body, None).await;
            assert_eq!(st, StatusCode::CREATED, "{created}");
            assert_eq!(created["eligible"], false, "{created}");
            assert_eq!(created["state"], "rejected", "{created}");
        }
        assert_eq!(
            scorer.hits.load(Ordering::SeqCst),
            0,
            "contaminated / empty-evidence must not rent a pod"
        );
        let _ = declared_manifest();
    }

    #[tokio::test]
    async fn empty_digest_and_unwired_harvest_are_503() {
        let (st, body) = json_req(
            app_full("op", EvalBackend::Lium, "", None, true, true, true),
            "POST",
            "/v1/submissions",
            submit_body("x", &serde_json::json!({})),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE, "{body}");
        assert!(body["error"]
            .as_str()
            .unwrap_or_default()
            .contains("eval image digest not pinned"));

        let (st, body) = json_req(
            app_full(
                "op",
                EvalBackend::Lium,
                &format!("sha256:{}", "cd".repeat(32)),
                None,
                true,
                true,
                true,
            ),
            "POST",
            "/v1/submissions",
            submit_body("x", &serde_json::json!({})),
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
    async fn refused_submissions_leave_no_rows() {
        let unpinned = app_full("op", EvalBackend::Lium, "", None, true, true, true);
        let no_baseline = app_full(
            "op",
            EvalBackend::Lium,
            &format!("sha256:{}", "cd".repeat(32)),
            Some(Arc::new(StubScorer::win())),
            true,
            false,
            true,
        );
        let sealed = app_full("op", EvalBackend::Sim, "", None, false, false, true);

        for (label, app) in [
            ("unpinned digest", unpinned),
            ("no baseline", no_baseline),
            ("no open topic", sealed),
        ] {
            for _ in 0..3 {
                let (st, body) = json_req(
                    app.clone(),
                    "POST",
                    "/v1/submissions",
                    submit_body("spam", &serde_json::json!({})),
                    None,
                )
                .await;
                assert!(
                    st == StatusCode::SERVICE_UNAVAILABLE || st == StatusCode::BAD_REQUEST,
                    "{label}: {st} {body}"
                );
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

    #[tokio::test]
    async fn can_score_is_false_until_open_holdout_and_baseline() {
        let (st, body) = json_req(
            app_full("op", EvalBackend::Sim, "", None, false, false, true),
            "GET",
            "/v1/status",
            serde_json::json!({}),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(body["can_score"], false, "{body}");
        assert_eq!(body["baseline_sealed"], false, "{body}");
        // No live scorer at all: neither family is wired.
        assert_eq!(body["live_harvest_wired"], false, "{body}");
        assert_eq!(body["custom_family_wired"], false, "{body}");
        assert_eq!(body["registered_custom"], serde_json::json!([]), "{body}");
        assert_eq!(body["custom_ready"], serde_json::json!([]), "{body}");

        let (st, body) = json_req(
            app_full(
                "op",
                EvalBackend::Lium,
                &format!("sha256:{}", "ab".repeat(32)),
                Some(Arc::new(StubScorer::win())),
                true,
                false,
                true,
            ),
            "GET",
            "/v1/status",
            serde_json::json!({}),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        // A harvest with no custom route: the harvest flag alone is true.
        assert_eq!(body["live_harvest_wired"], true);
        assert_eq!(body["custom_family_wired"], false, "{body}");
        assert_eq!(body["custom_ready"], serde_json::json!([]), "{body}");
        assert_eq!(body["can_score"], false, "{body}");
    }

    #[tokio::test]
    async fn missing_or_closed_offer_is_503() {
        let (st, body) = json_req(
            app_full("op", EvalBackend::Sim, "", None, true, true, false),
            "GET",
            "/v1/status",
            serde_json::json!({}),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(body["can_score"], false, "{body}");
        assert!(body["inference_offer"].is_null(), "{body}");

        let (st, body) = json_req(
            app_full("op", EvalBackend::Sim, "", None, true, true, false),
            "POST",
            "/v1/submissions",
            submit_body("x", &serde_json::json!({})),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE, "{body}");
        assert!(body["error"]
            .as_str()
            .unwrap_or_default()
            .contains("inference offer"));
    }

    #[tokio::test]
    async fn admin_publish_requires_bearer_and_a_valid_signature() {
        let token = "op-test-token";
        let app = app(token);
        let recs = synthetic_holdout(STRATUM_SIZE, 1);
        let p = pin("");
        let (mut doc, _) = seal_topic(&p, unsigned_topic(&recs));
        doc.id = "adamw-beater-v0".into();
        doc.signature = doc.sign_with(&sk()).expect("sign");

        let (st, _) = json_req(
            app.clone(),
            "POST",
            "/v1/admin/proof/topics",
            serde_json::to_value(&doc).expect("json"),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);

        let (st, created) = json_req(
            app,
            "POST",
            "/v1/admin/proof/topics",
            serde_json::to_value(&doc).expect("json"),
            Some(token),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{created}");
        assert_eq!(created["id"], "adamw-beater-v0");
    }

    fn custom_metric(custom_id: &str) -> MetricSpec {
        MetricSpec {
            family: MetricFamily::Custom,
            primary: "primary_value".into(),
            direction: MetricDirection::Max,
            unit: "rate".into(),
            epsilon_rel: 0.05,
            quality_floor_nll: 0.0,
            wall_budget_s: 0,
            custom_id: custom_id.into(),
        }
    }

    /// Custom ids are topic data. With no runner registered on the host an
    /// open custom topic is a publish 400 (nobody can compute it); the same
    /// document drafts fine, and a topic-minted id with a registered runner
    /// opens. Nothing about the id is compiled in.
    #[tokio::test]
    async fn custom_topics_open_only_with_a_registered_runner() {
        let token = "op";
        let recs = synthetic_holdout(STRATUM_SIZE, 1);
        let p = pin("");
        let (mut doc, _) = seal_topic(&p, unsigned_topic(&recs));
        doc.id = "custom-topic-v0".into();
        doc.payout_mode = proof_task::PayoutMode::Discovery;
        doc.metric = custom_metric("topic_minted_metric");
        doc.signature = doc.sign_with(&sk()).expect("sign");
        let (st, body) = json_req(
            app(token),
            "POST",
            "/v1/admin/proof/topics",
            serde_json::to_value(&doc).expect("json"),
            Some(token),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
        assert!(body["error"]
            .as_str()
            .unwrap_or_default()
            .contains("no registered runner"));

        doc.status = TopicStatus::Draft;
        doc.signature = doc.sign_with(&sk()).expect("sign");
        let (st, body) = json_req(
            app(token),
            "POST",
            "/v1/admin/proof/topics",
            serde_json::to_value(&doc).expect("json"),
            Some(token),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{body}");
        assert_eq!(body["status"], "draft");

        let (st, body) = json_req(
            app_with_custom(Arc::new(FamilyStub::win("topic_minted_metric"))),
            "POST",
            "/v1/admin/proof/topics",
            serde_json::to_value(&{
                let mut open = doc.clone();
                open.status = TopicStatus::Open;
                open.signature = open.sign_with(&sk()).expect("sign");
                open
            })
            .expect("json"),
            Some(token),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{body}");
        assert_eq!(body["metric"]["custom_id"], "topic_minted_metric");
    }

    /// A live scorer with one registered custom id. `wired: false` models a
    /// registered runner whose backend (topic VM) is not configured. It
    /// crowns every pass and records persist hooks, so the generic promotion
    /// / artefact plumbing is exercised without the RLM crates.
    struct FamilyStub {
        inner: StubScorer,
        custom_id: String,
        wired: bool,
        persisted: std::sync::Mutex<Vec<(String, String, bool)>>,
    }

    impl FamilyStub {
        fn win(custom_id: &str) -> Self {
            Self {
                inner: StubScorer::win(),
                custom_id: custom_id.into(),
                wired: true,
                persisted: std::sync::Mutex::new(Vec::new()),
            }
        }

        fn unwired(custom_id: &str) -> Self {
            Self {
                wired: false,
                ..Self::win(custom_id)
            }
        }
    }

    #[async_trait]
    impl LiveScorer for FamilyStub {
        async fn score(
            &self,
            pin: &ProofPin,
            topic: &TopicDocument,
            offer: &InferenceOffer,
            plan: &ExecutorPlan,
            frozen: &str,
            artifact: &str,
            artifact_uri: Option<&str>,
            declared_flops: u64,
            holdout: &[proof_task::HoldoutRecord],
            claim: &str,
        ) -> Result<proof_eval::ProofEvalDocument, EvalError> {
            self.ready_for_topic(topic)?;
            if topic.metric.family == MetricFamily::Custom {
                assert!(
                    artifact_uri.is_some_and(|u| !u.trim().is_empty()),
                    "intake must never hand a custom run to the scorer without a locator"
                );
            }
            let mut doc = self
                .inner
                .score(
                    pin,
                    topic,
                    offer,
                    plan,
                    frozen,
                    artifact,
                    artifact_uri,
                    declared_flops,
                    holdout,
                    claim,
                )
                .await?;
            if topic.metric.family == MetricFamily::Custom {
                doc.harness.custom_value = Some(0.7);
            }
            Ok(doc)
        }

        fn ready_for_topic(&self, topic: &TopicDocument) -> Result<(), EvalError> {
            if topic.metric.family != MetricFamily::Custom {
                return Ok(());
            }
            if topic.metric.custom_id != self.custom_id {
                return Err(EvalError::RunnerUnwired {
                    custom_id: topic.metric.custom_id.clone(),
                    detail: "no registered runner".into(),
                });
            }
            if !self.wired {
                return Err(EvalError::RunnerUnwired {
                    custom_id: topic.metric.custom_id.clone(),
                    detail: "topic-vm orchestrator not wired".into(),
                });
            }
            Ok(())
        }

        fn custom_ids(&self) -> Vec<String> {
            vec![self.custom_id.clone()]
        }

        fn ready_custom_ids(&self) -> Vec<String> {
            if self.wired {
                self.custom_ids()
            } else {
                Vec::new()
            }
        }

        async fn auto_promote(
            &self,
            _topic: &TopicDocument,
            _digest: &str,
            pass: bool,
            primary: Option<f64>,
            bar: Option<f64>,
        ) -> bool {
            pass && primary.is_some() && bar.is_some()
        }

        async fn on_persisted(&self, topic_id: &str, _digest: &str, id: &str, promoted: bool) {
            self.persisted
                .lock()
                .expect("persisted")
                .push((topic_id.into(), id.into(), promoted));
        }
    }

    fn unsigned_custom_topic(recs: &[proof_task::HoldoutRecord], custom_id: &str) -> TopicDocument {
        let mut baseline = default_adamw(FLOPS_BUDGET_MAX);
        baseline.optimizer = "reference-placeholder".into();
        baseline.lr = 1.0;
        baseline.schedule = "n/a".into();
        baseline.dtype = "n/a".into();
        baseline.script_sha256 = "11".repeat(32);
        TopicDocument {
            id: "custom-topic-v0".into(),
            statement: "Placeholder problem scored by a topic-minted custom metric.".into(),
            payout_mode: proof_task::PayoutMode::Discovery,
            constraints: Constraints {
                firecracker_required: true,
                model_pin: Some("vendor/model-placeholder".into()),
                task_slice: Some("slice-placeholder".into()),
                ..Constraints::default()
            },
            metric: custom_metric(custom_id),
            checklist: vec![proof_task::ChecklistRule {
                id: "rule_a".into(),
                text: "placeholder rule".into(),
            }],
            baseline,
            holdout_commitment: holdout_commitment(recs),
            holdout_size: HOLDOUT_SIZE,
            status: TopicStatus::Open,
            ..TopicDocument::default()
        }
    }

    /// Live host: the throughput topic scores through the harvest stub, the
    /// custom topic goes through `scorer` (registered id `topic_minted_metric`).
    fn app_with_custom(scorer: Arc<FamilyStub>) -> Router {
        app_with_live_scorer(scorer)
    }

    /// Live host holding the open, sealed throughput topic `dt-no-ib-v0` and
    /// custom topic `custom-topic-v0` (id `topic_minted_metric`, which
    /// `live.custom_ids()` must list), scored by `live`.
    fn app_with_live_scorer(live: Arc<dyn LiveScorer>) -> Router {
        let p = pin(&format!("sha256:{}", "ab".repeat(32)));
        let store = MemoryStore::new();
        let registered = live.custom_ids();
        for draft in [
            unsigned_topic(&[]),
            unsigned_custom_topic(&[], "topic_minted_metric"),
        ] {
            let recs = synthetic_holdout(STRATUM_SIZE, 1);
            let (topic, meas) = seal_topic_with(&p, draft, &custom_ids_ref(&registered));
            store.put_topic(topic.clone()).expect("topic");
            store.load_holdout(&topic.id, recs).expect("holdout");
            let mut sealed = meas.into_sealed();
            if topic.metric.family == MetricFamily::Custom {
                sealed.custom_value = Some(0.5);
            }
            store.set_baseline(&topic.id, sealed).expect("baseline");
        }
        let executor = test_executor(&p);
        proof_router(AppState {
            store,
            pin: p,
            backend: EvalBackend::Lium,
            live_scorer: Some(live),
            offer: Some(offer()),
            executor: executor_slot(Some(executor)),
            judge_api_key: Some("test-judge-key".into()),
            admin_hashes: Arc::new(vec![hash_admin_token("op")]),
            vm_probe: None,
            epoch: 0,
        })
    }

    /// A host whose topic-VM runner is registered but whose Lium harvest is
    /// not wired (`FamilyMux::custom_only`): the host is ready, the custom
    /// topic is scorable and scores, and every `nll` / `throughput` topic is
    /// open but not scorable — a submit there is a 503 with no row and no
    /// scorer call, never an in-process sim. `/v1/status` says so per
    /// family: `live_harvest_wired` stays **false** (Lium only) while the
    /// custom family reports wired and ready on its own fields.
    #[tokio::test]
    async fn a_custom_only_host_scores_custom_and_refuses_the_harvest_families() {
        let scorer = Arc::new(FamilyStub::win("topic_minted_metric"));
        let app = app_with_live_scorer(Arc::new(FamilyMux::custom_only(scorer.clone())));
        let (st, status) = json_req(
            app.clone(),
            "GET",
            "/v1/status",
            serde_json::json!({}),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        let ids = |key: &str| -> Vec<String> {
            status[key]
                .as_array()
                .expect(key)
                .iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        };
        let mut open = ids("open_topics");
        open.sort();
        assert_eq!(open, ["custom-topic-v0", "dt-no-ib-v0"], "{status}");
        assert_eq!(ids("scorable_topics"), ["custom-topic-v0"], "{status}");
        assert_eq!(
            ids("registered_custom"),
            ["topic_minted_metric"],
            "{status}"
        );
        assert_eq!(
            status["live_harvest_wired"], false,
            "no Lium harvest: the harvest flag must not follow the custom mux: {status}"
        );
        assert_eq!(status["custom_family_wired"], true, "{status}");
        assert_eq!(ids("custom_ready"), ["topic_minted_metric"], "{status}");
        assert_eq!(status["can_score"], true, "{status}");

        let (st, body) = json_req(
            app.clone(),
            "POST",
            "/v1/submissions",
            submit_body("harvest-topic", &serde_json::json!({})),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE, "{body}");
        assert_eq!(
            body["error"],
            EvalError::LiveHarvestUnavailable.to_string(),
            "{body}"
        );
        assert_eq!(scorer.inner.hits.load(Ordering::SeqCst), 0, "no run");
        let (st, list) = json_req(
            app.clone(),
            "GET",
            "/v1/submissions",
            serde_json::json!({}),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert!(
            list["items"].as_array().is_some_and(Vec::is_empty),
            "a harvest-family refusal banked a row: {list}"
        );

        let (st, created) = json_req(
            app,
            "POST",
            "/v1/submissions",
            submit_body(
                "custom-without-lium",
                &serde_json::json!({
                    "topic_id": "custom-topic-v0",
                    "artifact_uri": "https://example.invalid/custom-without-lium.zip",
                }),
            ),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{created}");
        assert_eq!(created["state"], "champion", "{created}");
        assert_eq!(scorer.inner.hits.load(Ordering::SeqCst), 1);
        let persisted = scorer.persisted.lock().expect("p").clone();
        assert_eq!(persisted.len(), 1, "{persisted:?}");
        assert_eq!(persisted[0].0, "custom-topic-v0");
    }

    /// A registered runner whose topic VM is not wired: the topic is open
    /// but not scorable, and a submit is a 503 with no row.
    #[tokio::test]
    async fn an_unwired_custom_runner_is_503_with_no_row_and_not_scorable() {
        let scorer = Arc::new(FamilyStub::unwired("topic_minted_metric"));
        let app = app_with_custom(scorer.clone());
        let (st, status) = json_req(
            app.clone(),
            "GET",
            "/v1/status",
            serde_json::json!({}),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        let ids = |key: &str| -> Vec<String> {
            status[key]
                .as_array()
                .expect(key)
                .iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        };
        assert!(
            ids("open_topics").contains(&"custom-topic-v0".to_owned()),
            "{status}"
        );
        assert_eq!(ids("scorable_topics"), ["dt-no-ib-v0"], "{status}");
        assert_eq!(
            ids("registered_custom"),
            ["topic_minted_metric"],
            "{status}"
        );
        // Harvest wired, custom registered but its VM backend not: each
        // family reports its own state.
        assert_eq!(status["live_harvest_wired"], true, "{status}");
        assert_eq!(status["custom_family_wired"], true, "{status}");
        assert!(ids("custom_ready").is_empty(), "{status}");
        assert_eq!(status["can_score"], true, "{status}");

        let (st, body) = json_req(
            app.clone(),
            "POST",
            "/v1/submissions",
            submit_body(
                "custom-artifact",
                &serde_json::json!({
                    "topic_id": "custom-topic-v0",
                    "artifact_uri": "https://example.invalid/custom-artifact.zip",
                }),
            ),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE, "{body}");
        let msg = body["error"].as_str().unwrap_or_default();
        assert!(msg.contains("topic_minted_metric"), "{body}");
        assert!(msg.contains("not wired"), "{body}");
        assert_eq!(scorer.inner.hits.load(Ordering::SeqCst), 0);
        let (st, list) = json_req(app, "GET", "/v1/submissions", serde_json::json!({}), None).await;
        assert_eq!(st, StatusCode::OK);
        assert!(
            list["items"].as_array().is_some_and(Vec::is_empty),
            "unwired runner banked a row: {list}"
        );
        assert!(scorer.persisted.lock().expect("p").is_empty());
    }

    /// A custom-family runner retrieves the artefact from the miner's
    /// locator; a custom submission without one is refused at intake with
    /// no row and no scorer call. `nll` / `throughput` keep it optional.
    #[tokio::test]
    async fn a_custom_submission_without_a_locator_is_400_with_no_row() {
        let scorer = Arc::new(FamilyStub::win("topic_minted_metric"));
        let app = app_with_custom(scorer.clone());
        for missing in [serde_json::Value::Null, serde_json::json!("   ")] {
            let (st, body) = json_req(
                app.clone(),
                "POST",
                "/v1/submissions",
                submit_body(
                    "no-locator",
                    &serde_json::json!({
                        "topic_id": "custom-topic-v0",
                        "artifact_uri": missing,
                    }),
                ),
                None,
            )
            .await;
            assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
            assert_eq!(body["error"], "artifact_uri is required for custom topics");
        }
        assert_eq!(scorer.inner.hits.load(Ordering::SeqCst), 0, "no run");
        let (_, list) = json_req(
            app.clone(),
            "GET",
            "/v1/submissions",
            serde_json::json!({}),
            None,
        )
        .await;
        assert!(
            list["items"].as_array().is_some_and(Vec::is_empty),
            "{list}"
        );
        let (st, created) = json_req(
            app,
            "POST",
            "/v1/submissions",
            submit_body("harvest-topic", &serde_json::json!({})),
            None,
        )
        .await;
        assert_eq!(
            st,
            StatusCode::CREATED,
            "the harvest fetches by digest: {created}"
        );
    }

    /// The generic promotion plumbing: a pass the family scorer crowns is
    /// persisted as `champion`, and the persist hook fires with the row id.
    #[tokio::test]
    async fn a_family_scorer_can_crown_a_pass_and_sees_the_persisted_row() {
        let scorer = Arc::new(FamilyStub::win("topic_minted_metric"));
        let app = app_with_custom(scorer.clone());
        let (st, created) = json_req(
            app.clone(),
            "POST",
            "/v1/submissions",
            submit_body(
                "crowned",
                &serde_json::json!({
                    "topic_id": "custom-topic-v0",
                    "artifact_uri": "https://example.invalid/crowned.zip",
                }),
            ),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{created}");
        assert_eq!(created["eligible"], true, "{created}");
        assert_eq!(created["state"], "champion", "{created}");
        let id = created["id"].as_str().expect("id").to_owned();
        let persisted = scorer.persisted.lock().expect("p").clone();
        assert_eq!(
            persisted,
            vec![("custom-topic-v0".to_owned(), id.clone(), true)]
        );

        // A reject is persisted too, never crowned.
        let lose = Arc::new(FamilyStub {
            inner: StubScorer::lose(),
            ..FamilyStub::win("topic_minted_metric")
        });
        let (st, created) = json_req(
            app_with_custom(lose.clone()),
            "POST",
            "/v1/submissions",
            submit_body("not-crowned", &serde_json::json!({})),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{created}");
        assert_eq!(created["state"], "rejected", "{created}");
        let persisted = lose.persisted.lock().expect("p").clone();
        assert_eq!(persisted.len(), 1);
        assert!(!persisted[0].2, "a reject must not be promoted");
    }

    fn app_lium_missing_judge_key() -> Router {
        let p = pin(&format!("sha256:{}", "ab".repeat(32)));
        let store = MemoryStore::new();
        let recs = synthetic_holdout(STRATUM_SIZE, 1);
        let (topic, meas) = seal_topic(&p, unsigned_topic(&recs));
        store.put_topic(topic.clone()).expect("topic");
        store.load_holdout(&topic.id, recs).expect("holdout");
        store
            .set_baseline(&topic.id, meas.into_sealed())
            .expect("baseline");
        proof_router(AppState {
            store,
            pin: p,
            backend: EvalBackend::Lium,
            live_scorer: Some(Arc::new(StubScorer::win())),
            offer: Some(offer()),
            executor: executor_slot(None),
            judge_api_key: None,
            admin_hashes: Arc::new(vec![hash_admin_token("op")]),
            vm_probe: None,
            epoch: 0,
        })
    }

    #[tokio::test]
    async fn lium_without_judge_api_key_cannot_score() {
        let (st, body) = json_req(
            app_lium_missing_judge_key(),
            "GET",
            "/v1/status",
            serde_json::json!({}),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(body["live_harvest_wired"], true);
        assert_eq!(body["can_score"], false, "{body}");
        let dump = body.to_string();
        assert!(!dump.contains("api_key"), "{dump}");

        let (st, body) = json_req(
            app_lium_missing_judge_key(),
            "POST",
            "/v1/submissions",
            submit_body("no-key", &serde_json::json!({})),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE, "{body}");
        assert!(
            body["error"]
                .as_str()
                .unwrap_or_default()
                .contains("API key"),
            "{body}"
        );
    }

    #[tokio::test]
    async fn lium_with_judge_api_key_can_score_and_status_omits_the_secret() {
        let digest = format!("sha256:{}", "ab".repeat(32));
        let (st, body) = json_req(
            app_full(
                "op",
                EvalBackend::Lium,
                &digest,
                Some(Arc::new(StubScorer::win())),
                true,
                true,
                true,
            ),
            "GET",
            "/v1/status",
            serde_json::json!({}),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(body["can_score"], true, "{body}");
        let dump = body.to_string();
        assert!(!dump.contains("api_key"), "{dump}");
        assert!(!dump.contains("test-judge-key"), "{dump}");
        assert!(!dump.contains("base_url"), "{dump}");
        assert!(!dump.contains("8000"), "{dump}");
    }

    #[tokio::test]
    async fn spoofed_topic_origin_is_503() {
        let p = pin("");
        let store = MemoryStore::new();
        let recs = synthetic_holdout(STRATUM_SIZE, 1);
        let (mut topic, meas) = seal_topic(&p, unsigned_topic(&recs));
        topic.inference.max_input_tokens = Some(4_096);
        topic.inference.base_url = Some("http://evil.example/v1".into());
        topic.signature = topic.sign_with(&sk()).expect("sign");
        store.put_topic(topic.clone()).expect("topic");
        store.load_holdout(&topic.id, recs).expect("holdout");
        store
            .set_baseline(&topic.id, meas.into_sealed())
            .expect("baseline");
        let app = proof_router(AppState {
            store,
            pin: p,
            backend: EvalBackend::Sim,
            live_scorer: None,
            offer: Some(offer()),
            executor: executor_slot(None),
            judge_api_key: None,
            admin_hashes: Arc::new(vec![hash_admin_token("op")]),
            vm_probe: None,
            epoch: 0,
        });
        let (st, body) = json_req(
            app,
            "POST",
            "/v1/submissions",
            submit_body("spoof", &serde_json::json!({})),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE, "{body}");
        assert!(
            body["error"]
                .as_str()
                .unwrap_or_default()
                .contains("committed judge origin"),
            "{body}"
        );
    }

    fn app_staging_sim() -> Router {
        let p = pin("");
        let store = MemoryStore::new();
        for draft in [unsigned_topic(&[]), unsigned_muon_topic(&[])] {
            let recs = synthetic_holdout(STRATUM_SIZE, 1);
            let (topic, meas) = seal_topic(&p, draft);
            store.put_topic(topic.clone()).expect("topic");
            store.load_holdout(&topic.id, recs).expect("holdout");
            store
                .set_baseline(&topic.id, meas.into_sealed())
                .expect("baseline");
        }
        proof_router(AppState {
            store,
            pin: p,
            backend: EvalBackend::Sim,
            live_scorer: None,
            offer: Some(staging_offer()),
            executor: executor_slot(None),
            judge_api_key: None,
            admin_hashes: Arc::new(vec![hash_admin_token("op")]),
            vm_probe: None,
            epoch: 0,
        })
    }

    fn assert_scored_row(created: &serde_json::Value, topic_id: &str) {
        assert!(
            created["id"]
                .as_str()
                .is_some_and(|id| id.starts_with("pf_")),
            "silent empty id: {created}"
        );
        assert_eq!(created["topic_id"], topic_id, "{created}");
        assert_eq!(created["eval_backend"], "sim", "{created}");
        let state = created["state"].as_str().unwrap_or_default();
        assert!(
            state == "awaiting_admin" || state == "rejected",
            "non-terminal or empty state: {created}"
        );
        assert!(created["eligible"].is_boolean(), "{created}");
        assert!(
            created["submission_digest"]
                .as_str()
                .is_some_and(|d| d.len() == 64),
            "{created}"
        );
    }

    #[tokio::test]
    async fn sim_submit_scores_claim_artifact_and_flops() {
        let app = app("op");
        let body = submit_body(
            "staging-e2e-artifact",
            &serde_json::json!({
                "claim": "beats the sealed reference under the cap",
                "declared_flops": 1_000_000_000_000u64,
            }),
        );
        let (st, created) = json_req(app.clone(), "POST", "/v1/submissions", body, None).await;
        assert_eq!(st, StatusCode::CREATED, "{created}");
        assert_scored_row(&created, "dt-no-ib-v0");

        let id = created["id"].as_str().expect("id");
        let (st, row) = json_req(
            app,
            "GET",
            &format!("/v1/submissions/{id}"),
            serde_json::json!({}),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{row}");
        assert_eq!(row["id"], id);
        assert_eq!(row["topic_id"], "dt-no-ib-v0");
        assert_eq!(row["declared_flops"], 1_000_000_000_000u64);
        assert_eq!(row["claim"], "beats the sealed reference under the cap");
        assert!(row["verdict"].is_object(), "judge path missing: {row}");
        assert!(row["verdict"]["agent"].is_object(), "{row}");
        assert!(
            row["verdict"]["harness"]["holdout_nll"].is_number(),
            "{row}"
        );
        assert!(row["verdict"]["lattice"].is_number(), "{row}");
        assert!(
            row["verdict"]["agent"]["rationale"]
                .as_str()
                .is_some_and(|s| !s.is_empty()),
            "notation missing: {row}"
        );
        let receipt = row["receipt_json"].as_str().unwrap_or_default();
        assert!(receipt.contains("sim"), "sim receipt missing: {row}");
        let dump = row.to_string();
        assert!(!dump.contains("content_sha256"), "{dump}");
        assert!(!dump.contains("api_key"), "{dump}");
    }

    #[tokio::test]
    async fn sim_submit_accepts_staging_topic_ids() {
        let app = app_staging_sim();
        let (st, status) = json_req(
            app.clone(),
            "GET",
            "/v1/status",
            serde_json::json!({}),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(status["can_score"], true, "{status}");
        assert_eq!(status["eval_backend"], "sim", "{status}");
        assert_eq!(status["baseline_sealed"], true, "{status}");
        assert_eq!(
            status["inference_offer"]["offer_id"],
            "openrouter-glm53flash-v0"
        );
        let open = status["open_topics"].as_array().expect("open_topics");
        let ids: Vec<&str> = open.iter().filter_map(|v| v.as_str()).collect();
        assert!(ids.contains(&"dt-no-ib-v0"), "{status}");
        assert!(ids.contains(&"muon-vs-adamw-10m-v0"), "{status}");

        let (st, list) = json_req(
            app.clone(),
            "GET",
            "/v1/proof/topics",
            serde_json::json!({}),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        let dump = list.to_string();
        assert!(!dump.contains("content_sha256"), "{dump}");
        assert!(!dump.contains("synthetic-dev"), "{dump}");

        for topic_id in ["dt-no-ib-v0", "muon-vs-adamw-10m-v0"] {
            let (st, created) = json_req(
                app.clone(),
                "POST",
                "/v1/submissions",
                submit_body(
                    topic_id,
                    &serde_json::json!({
                        "topic_id": topic_id,
                        "declared_flops": 42u64,
                    }),
                ),
                None,
            )
            .await;
            assert_eq!(st, StatusCode::CREATED, "{topic_id}: {created}");
            assert_scored_row(&created, topic_id);
        }
    }

    #[tokio::test]
    async fn sim_submit_fail_closed_reasons_are_explicit() {
        let app = app_staging_sim();
        let (st, body) = json_req(
            app.clone(),
            "POST",
            "/v1/submissions",
            submit_body("x", &serde_json::json!({ "topic_id": "" })),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(body["error"], "topic_id is required");

        let (st, body) = json_req(
            app.clone(),
            "POST",
            "/v1/submissions",
            submit_body("x", &serde_json::json!({ "topic_id": "not-a-live-topic" })),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(body["error"], "unknown topic");

        let (st, body) = json_req(
            app,
            "POST",
            "/v1/submissions",
            submit_body("x", &serde_json::json!({ "declared_flops": u64::MAX })),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
        assert!(
            body["error"]
                .as_str()
                .unwrap_or_default()
                .contains("declared_flops"),
            "{body}"
        );
    }

    fn tight_sealed() -> proof_score::SealedBaseline {
        let mut split = std::collections::BTreeMap::new();
        for s in HoldoutSplit::SCORED {
            split.insert(s.as_str().to_owned(), 0.29);
        }
        proof_score::SealedBaseline {
            holdout_nll: 0.29,
            split_nll: split,
            tokens_per_sec: Some(80.0),
            step_latency_ms: None,
            custom_value: None,
        }
    }

    fn app_tight_sim() -> Router {
        let p = pin("");
        let store = MemoryStore::new();
        let recs = synthetic_holdout(STRATUM_SIZE, 1);
        let (topic, _) = seal_topic(&p, unsigned_topic(&recs));
        store.put_topic(topic.clone()).expect("topic");
        store.load_holdout(&topic.id, recs).expect("holdout");
        store
            .set_baseline(&topic.id, tight_sealed())
            .expect("baseline");
        proof_router(AppState {
            store,
            pin: p,
            backend: EvalBackend::Sim,
            live_scorer: None,
            offer: Some(staging_offer()),
            executor: executor_slot(None),
            judge_api_key: None,
            admin_hashes: Arc::new(vec![hash_admin_token("op")]),
            vm_probe: None,
            epoch: 0,
        })
    }

    #[tokio::test]
    async fn sim_stub_win_submit_reaches_awaiting_admin() {
        let app = app_tight_sim();
        let (st, status) = json_req(
            app.clone(),
            "GET",
            "/v1/status",
            serde_json::json!({}),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(status["eval_backend"], "sim");
        assert_eq!(status["sim_stub_win"], true, "{status}");

        let (st, created) = json_req(
            app.clone(),
            "POST",
            "/v1/submissions",
            submit_body("tight-win", &serde_json::json!({})),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{created}");
        assert_eq!(created["state"], "awaiting_admin", "{created}");
        assert_eq!(created["eligible"], true, "{created}");
        assert_eq!(created["eval_backend"], "sim");
        let id = created["id"].as_str().expect("id");
        let (st, row) = json_req(
            app,
            "GET",
            &format!("/v1/submissions/{id}"),
            serde_json::json!({}),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{row}");
        assert_eq!(row["verdict"]["pass"], true, "{row}");
        assert_eq!(row["verdict"]["agent"]["rationale"], "sim stub win");
        assert_eq!(row["verdict"]["failed"].as_array().map(Vec::len), Some(0));
    }
}
