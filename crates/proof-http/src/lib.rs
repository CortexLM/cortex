//! Proof HTTP API (master-only).
//!
//! ```text
//! GET  /health
//! GET  /v1/status
//! GET  /v1/proof/topics
//! GET  /v1/proof/topics/{id}
//! GET  /v1/proof/executor         public EvalExecutorOffer + pin ceilings
//! POST /v1/submissions            miner submit (topic_id + hotkey_signature + submit_nonce required)
//! GET  /v1/submissions            ?state=queued&topic_id=… filters
//! GET  /v1/submissions/{id}
//! POST /v1/admin/proof/topics     operator publish (signed document)
//! POST /v1/admin/proof/executor   operator rotate the live executor offer
//! GET  /v1/admin/proof/vm-orchestrator   operator probe: topic-VM orchestrator readiness + agent health
//! POST /v1/admin/proof/queue/drain       operator: score `queued` rows of one topic, in order
//! POST /v1/admin/proof/submissions/{id}/score   operator: score one `queued` row
//! ```
//!
//! **Deferred scoring.** An open topic whose signed document carries
//! `constraints.params.defer_scoring = "true"` accepts submissions the same
//! way (every intake gate applies) but persists them as `queued` (**201**)
//! instead of evaluating: no readiness check, no harvest rent, no topic VM,
//! no judge call, no verdict, no topic mass. The rows wait until the operator
//! re-publishes the topic without the flag and the queue is drained (the
//! admin route here, or the binary's poll loop) — one row at a time, oldest
//! first, through the exact path a live submit takes. A drain on a topic
//! that still defers is a **409**; a host that cannot score leaves the rows
//! `queued` (**503**, nothing rented), never a reject.

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
use axum::extract::{DefaultBodyLimit, Path, Query, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};

mod submit;

use proof_canon::MinerEnv;
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
    freeze_submission_digest, is_staged_artefact_uri, staged_artefact_uri, ArtifactManifest,
    Enqueued, MemoryStore, StoreError, Submission, SubmissionState, MAX_ARTEFACT_BYTES,
};
use proof_submit::{
    is_lowercase_hex, parse_hotkey_hex, parse_signature_hex, parse_submit_nonce_hex, verify_submit,
    SubmitFields,
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
    /// runner is not registered is open but not scorable). A topic that
    /// defers scoring is open — it accepts and queues — but is not scored
    /// right now, so it is not listed here ([`Self::deferred_topics`]).
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
                    && !t.constraints.defer_scoring()
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

    /// Open topics whose signed document defers scoring: a submit there is
    /// a **201** `queued` row, whatever the host can score right now.
    fn deferred_topics(&self) -> Vec<String> {
        self.store
            .topics()
            .unwrap_or_default()
            .iter()
            .filter(|t| t.is_open_at(self.epoch) && t.constraints.defer_scoring())
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
        .route("/v1/admin/proof/queue/drain", post(drain_queue))
        .route("/v1/admin/proof/submissions/{id}/score", post(score_queued))
        .layer(DefaultBodyLimit::max(submit::SUBMIT_BODY_LIMIT))
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
        "deferred_topics": st.deferred_topics(),
        "queued_submissions": st.store.queued(None).map_or(0, |q| q.len()),
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
    /// sr25519 signature over [`SubmitFields::signing_payload`] (128 lowercase hex).
    #[serde(default)]
    hotkey_signature: Option<String>,
    /// Client anti-replay nonce (64 lowercase hex), bound into the signature
    /// and accepted once per hotkey.
    #[serde(default)]
    submit_nonce: Option<String>,
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
    /// Miner BYOK: `{"<NAME>": "<value>"}` for the variables this topic's
    /// signed document declares (`constraints.params.miner_byok` /
    /// `miner_env_allowlist`). Never signed (v1 of the submit payload is
    /// unchanged), never persisted on the row, never echoed back. A name the
    /// topic does not declare is a **400** before the signature is checked,
    /// so a mistake here never burns the single-use `submit_nonce`.
    #[serde(default)]
    env: MinerEnv,
}

/// What a submit — or a drain of one row — answers.
#[derive(Debug, Clone, Serialize)]
pub struct SubmitResp {
    /// Row id (`pf_…`).
    pub id: String,
    /// Frozen digest.
    pub submission_digest: String,
    /// Topic scored (or queued) against.
    pub topic_id: String,
    /// Lifecycle; `queued` when the topic defers scoring.
    pub state: SubmissionState,
    /// Backend that scored — the host's when nothing has run yet.
    pub eval_backend: EvalBackend,
    /// Clean pass. Always `false` on a `queued` row.
    pub eligible: bool,
    /// Why the row is where it is: the deferred-scoring note on a `queued`
    /// row, the failed gates on a reject. `None` on a clean pass.
    pub detail: Option<String>,
}

impl SubmitResp {
    fn of(row: &Submission, backend: EvalBackend, eligible: bool) -> Self {
        Self {
            id: row.id.clone(),
            submission_digest: row.submission_digest.clone(),
            topic_id: row.topic_id.clone(),
            state: row.state,
            eval_backend: backend,
            eligible,
            detail: row.detail.clone(),
        }
    }
}

/// `detail` of every `queued` row: what the miner sees while the operator
/// finishes the topic, and why nothing has run.
pub const DEFERRED_DETAIL: &str = "scoring deferred until the topic is ready: the signed topic sets constraints.params.defer_scoring, so this row is queued (no eval, no rent, no judge call yet) and is scored in order once the operator lifts the flag and drains the queue";

type ErrResp = (StatusCode, Json<serde_json::Value>);

/// Exactly 64 lowercase hex, no `0x`, no whitespace. The wire form **is** the
/// signed form: the host never normalises a hex field before verifying it,
/// so a value a miner did not sign byte for byte is refused as their
/// request, not silently rewritten into a signature mismatch.
fn parse_hex64(s: &str, field: &str) -> Result<String, (StatusCode, Json<serde_json::Value>)> {
    if !is_lowercase_hex(s, 64) {
        return Err(err(
            StatusCode::BAD_REQUEST,
            &format!("invalid {field}: exactly 64 lowercase hex, no 0x"),
        ));
    }
    Ok(s.to_owned())
}

/// A digest of **nothing** is not an artefact digest: the sha256 of zero
/// bytes and of an empty tar archive (`tar cf - -T /dev/null`, 10240 zero
/// bytes). Staging's happy path once matched exactly such a digest because
/// the RLM guest could not fetch the artefact and fell back to an empty
/// tree; refusing it at submit keeps that stub out of every row and rent.
fn is_digest_of_nothing(hex64: &str) -> bool {
    let empty_input = hex::encode(Sha256::digest(b""));
    let empty_tar = hex::encode(Sha256::digest([0u8; 10_240]));
    hex64.eq_ignore_ascii_case(&empty_input) || hex64.eq_ignore_ascii_case(&empty_tar)
}

fn parse_artifact_digest(s: &str) -> Result<String, (StatusCode, Json<serde_json::Value>)> {
    // Named before the encoding check so a pasted empty-tar digest gets the
    // useful answer whatever its case.
    if is_digest_of_nothing(&proof_submit::canonical_hex(s)) {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "artifact_digest is the sha256 of empty input (or of an empty tar archive): hash the recipe bytes you upload (or ship at artifact_uri)",
        ));
    }
    parse_hex64(s, "artifact_digest")
}

/// Miner identity for one submit: `hotkey_signature` over every gate input
/// (topic, artefact, FLOPs, claim, manifest) plus the client `submit_nonce`.
/// Verified over the strings exactly as posted (`hotkey` and `artifact` are
/// already in their only accepted form). Returns the nonce the caller must
/// reserve before any row or rent.
fn authenticate_submit(
    hotkey: &str,
    topic_id: &str,
    artifact: &str,
    body: &SubmitBody,
) -> Result<String, (StatusCode, Json<serde_json::Value>)> {
    let sig_hex = body.hotkey_signature.as_deref().filter(|s| !s.is_empty());
    let Some(sig_hex) = sig_hex else {
        return Err(err(StatusCode::UNAUTHORIZED, "hotkey_signature required"));
    };
    let sig = parse_signature_hex(sig_hex)
        .map_err(|_| err(StatusCode::UNAUTHORIZED, "hotkey_signature invalid"))?;
    let nonce = body.submit_nonce.as_deref().filter(|s| !s.is_empty());
    let Some(nonce) = nonce else {
        return Err(err(StatusCode::UNAUTHORIZED, "submit_nonce required"));
    };
    parse_submit_nonce_hex(nonce)
        .map_err(|_| err(StatusCode::UNAUTHORIZED, "submit_nonce invalid"))?;
    let pk = parse_hotkey_hex(hotkey)
        .map_err(|_| err(StatusCode::BAD_REQUEST, "invalid miner_hotkey"))?;
    verify_submit(
        &pk,
        &SubmitFields {
            hotkey_hex: hotkey,
            topic_id,
            artifact_digest: artifact,
            declared_flops: body.declared_flops,
            claim: &body.claim,
            train_content_hashes: &body.manifest.train_content_hashes,
            train_dataset_ids: &body.manifest.train_dataset_ids,
            submit_nonce_hex: nonce,
        },
        &sig,
    )
    .map_err(|_| err(StatusCode::UNAUTHORIZED, "hotkey_signature invalid"))?;
    Ok(nonce.to_owned())
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
    req: Request,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let parsed = submit::parse_submit(&headers, req).await?;
    let body = parsed.body;
    let uploaded = parsed.artifact;
    let hotkey = parse_hex64(&body.miner_hotkey, "miner_hotkey")?;
    let artifact = parse_artifact_digest(&body.artifact_digest)?;
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
    // Miner BYOK, held to what the signed topic declares. Checked before the
    // signature so a body with the wrong variable names is a plain 400 the
    // miner can fix and re-post: the `submit_nonce` they signed is still
    // unspent. `env` is not part of the signed payload, so nothing here
    // weakens the identity check that follows.
    let miner_env = body
        .env
        .accept(&topic.constraints)
        .map_err(|e| err(StatusCode::BAD_REQUEST, &e.to_string()))?;
    let miner_uri = body
        .artifact_uri
        .as_deref()
        .map(str::trim)
        .filter(|u| !u.is_empty() && !is_staged_artefact_uri(u))
        .map(str::to_owned);
    if let Some(bytes) = uploaded.as_deref() {
        submit::accept_uploaded(bytes, &artifact, is_digest_of_nothing)?;
    }
    // Custom family: upload (preferred) or a miner-hosted URI (compat).
    // Neither is 400 before the signature so a forgotten artefact does not
    // burn the single-use nonce. Both: bytes win after reserve.
    if topic.metric.family == MetricFamily::Custom && uploaded.is_none() && miner_uri.is_none() {
        return Err(err(StatusCode::BAD_REQUEST, "artifact required"));
    }
    let submit_nonce = authenticate_submit(&hotkey, &body.topic_id, &artifact, &body)?;
    // A verified request is single-use, whatever happens to it next: a
    // replay must never reach evaluation or a second row.
    if !st
        .store
        .reserve_submit_nonce(&hotkey, &submit_nonce)
        .map_err(|e| store_err(&e))?
    {
        return Err(err(StatusCode::UNAUTHORIZED, "submit_nonce reused"));
    }
    // Harvest (`nll` / `throughput`) still refuses a declaration over the
    // topic budget. Custom / agent topics ignore `declared_flops` as a gate
    // — it stays on the wire for signature compat and is never a reject.
    if topic.metric.family != MetricFamily::Custom && body.declared_flops > topic.flops_budget {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "declared_flops exceeds the topic budget",
        ));
    }

    let nonce = nonce_from(&hotkey, &topic_id, &artifact);
    let submission_digest = freeze_submission_digest(&hotkey, &topic_id, &artifact, &nonce);
    // In-flight ref on this frozen digest. Concurrent retries share the
    // vault entry; abort releases one ref and deletes only if nothing
    // queued still names it.
    if !miner_env.is_empty() {
        st.store
            .claim_miner_env(&submission_digest, &miner_env)
            .map_err(|e| err(StatusCode::SERVICE_UNAVAILABLE, &e.to_string()))?;
    }
    let mut artifact_staged = None;
    if let Some(bytes) = uploaded.as_ref() {
        artifact_staged = Some(
            st.store
                .stash_artefact(&artifact, &hotkey, &submit_nonce, bytes)
                .map_err(|e| {
                    let _ = st.store.release_miner_env(&submission_digest);
                    err(StatusCode::SERVICE_UNAVAILABLE, &e.to_string())
                })?,
        );
    }
    // Bytes win: a staged upload is identified by the internal locator so
    // PR B can inject from the vault. A miner URI is ignored for identity
    // when bytes were posted. URI-only keeps the miner locator (compat).
    let artifact_uri = if artifact_staged.is_some() {
        Some(staged_artefact_uri(&artifact))
    } else {
        miner_uri
    };
    // The intake row: every miner-supplied field, frozen digest, no host
    // stamps yet. It is either queued as-is or scored right now.
    let row = Submission {
        id: String::new(),
        topic_id: topic_id.clone(),
        miner_hotkey: hotkey,
        artifact_digest: artifact,
        artifact_uri,
        artifact_staged,
        claim: body.claim,
        declared_flops: body.declared_flops,
        architecture: body.architecture,
        inference_offer_id: String::new(),
        config_commitment: String::new(),
        executor_offer_id: String::new(),
        executor_commitment: String::new(),
        manifest: body.manifest,
        nonce,
        submit_nonce,
        submission_digest,
        state: SubmissionState::Queued,
        receipt_json: None,
        verdict: None,
        detail: None,
    };

    // A topic that defers scoring accepts the artefact now and scores it
    // later: the row is persisted `queued` before any readiness check, so a
    // host still installing its baseline / harness never rents, boots, or
    // judges for it — and never refuses it either.
    if topic.constraints.defer_scoring() {
        return queue_deferred(&st, row);
    }
    // Live submit: a 503/401 after staging must not leave bytes on disk.
    // The nonce is already spent; a retry uses a new nonce (new vault key).
    // Drain keeps staging on error so a queued row can still be scored.
    let staged_digest = row.artifact_digest.clone();
    let staged_hotkey = row.miner_hotkey.clone();
    let staged_nonce = row.submit_nonce.clone();
    let staged_env = row.submission_digest.clone();
    match score_intake(&st, row, &topic).await {
        Ok(resp) => Ok((StatusCode::CREATED, Json(resp))),
        Err(e) => {
            let _ = st
                .store
                .forget_artefact(&staged_digest, &staged_hotkey, &staged_nonce);
            let _ = st.store.release_miner_env(&staged_env);
            Err(e)
        }
    }
}

/// Persist an intake row as `queued` — one row per frozen digest per topic,
/// in a single store step. A retry of the same artefact by the same hotkey
/// (same frozen digest) finds its row (**200**) — still queued, or already
/// scored after a drain — instead of queueing a second paid run; two
/// identical submits racing each other yield one row.
fn queue_deferred(
    st: &AppState,
    mut row: Submission,
) -> Result<(StatusCode, Json<SubmitResp>), ErrResp> {
    row.detail = Some(DEFERRED_DETAIL.to_owned());
    let digest = row.artifact_digest.clone();
    let hotkey = row.miner_hotkey.clone();
    let nonce = row.submit_nonce.clone();
    let env_digest = row.submission_digest.clone();
    let enqueued = st.store.enqueue(row).map_err(|e| store_err(&e))?;
    let _ = st.store.release_miner_env(&env_digest);
    match enqueued {
        Enqueued::Inserted(row) => Ok((
            StatusCode::CREATED,
            Json(SubmitResp::of(&row, st.backend, false)),
        )),
        Enqueued::Existing(existing) => {
            let _ = st.store.forget_artefact(&digest, &hotkey, &nonce);
            let eligible = existing.verdict.as_ref().is_some_and(|v| v.pass);
            let mut resp = SubmitResp::of(&existing, st.backend, eligible);
            resp.detail = Some(if existing.state == SubmissionState::Queued {
                format!("already queued as {}; {DEFERRED_DETAIL}", existing.id)
            } else {
                format!(
                    "already submitted as {} and scored ({}); one run per artefact per topic",
                    existing.id,
                    state_name(existing.state)
                )
            });
            Ok((StatusCode::OK, Json(resp)))
        }
    }
}

/// Wire name of a state (`queued`, `awaiting_admin`, …).
fn state_name(state: SubmissionState) -> String {
    serde_json::to_value(state)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// Score one intake row on `topic` and persist the result: host readiness,
/// the judge / executor offers, the sealed baseline, the holdout unseal, the
/// contamination gate (a persisted reject with no rent), then the eval and
/// the judge. The same path serves a live submit (a fresh row) and a drain
/// (a `queued` row that keeps its id). Every refusal is an error with no
/// row change — the caller decides whether that means "no row" (submit) or
/// "still queued" (drain).
async fn score_intake(
    st: &AppState,
    row: Submission,
    topic: &TopicDocument,
) -> Result<SubmitResp, ErrResp> {
    // The miner's own key, read back from the vault it was put in at intake.
    // A topic that demands one and cannot get it here is a **503** with the
    // row untouched — a drain leaves it `queued` and nothing is rented. The
    // alternative, running the miner's evaluation on the operator's
    // credentials, is the one outcome this whole path exists to prevent.
    let miner_env = st
        .store
        .miner_env(&row.submission_digest)
        .map_err(|e| err(StatusCode::SERVICE_UNAVAILABLE, &e.to_string()))?;
    if let Some(missing) = topic
        .constraints
        .miner_env_required()
        .into_iter()
        .find(|n| miner_env.names().iter().all(|have| have != n))
    {
        return Err(err(
            StatusCode::SERVICE_UNAVAILABLE,
            &format!(
                "this topic runs on the miner's own {missing}, and this host no longer holds the one that was submitted (vault {}); the row is untouched — nothing was rented and the operator key is never substituted",
                st.store
                    .miner_byok_vault()
                    .root()
                    .map_or_else(|| "in-process".to_owned(), |r| r.display().to_string())
            ),
        ));
    }
    let miner_env = &miner_env;
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
        live.ready_for_topic(topic).map_err(|e| eval_err(&e))?;
    }
    let Some(offer) = st.offer.as_ref() else {
        return Err(eval_err(&EvalError::InferenceOfferMissing));
    };
    offer
        .serves_topic(&st.pin, topic)
        .map_err(|e| offer_err(&e))?;
    if st.backend == EvalBackend::Lium {
        executor
            .as_ref()
            .ok_or(EvalError::ExecutorOfferMissing)
            .and_then(|x| x.serves_topic(topic).map_err(proof_eval::map_executor_err))
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
        .baseline(&topic.id)
        .map_err(|e| store_err(&e))?
        .ok_or_else(|| {
            err(
                StatusCode::SERVICE_UNAVAILABLE,
                "no sealed baseline recorded for this topic",
            )
        })?;

    let holdout = st
        .store
        .unseal_holdout(&topic.id, &row.submission_digest)
        .map_err(|e| err(StatusCode::SERVICE_UNAVAILABLE, &e.to_string()))?;

    let (declared, hits) = contamination_evidence(&row.manifest, &holdout);
    // Holdout overlap in a declared manifest is always contamination. An empty
    // manifest is EvidenceMissing only when the topic requires training
    // evidence (harvest default; custom / agent skip unless the signed
    // document sets require_training_evidence = "true").
    if !hits.is_empty() || (topic.requires_training_evidence() && !declared) {
        let failed = if declared {
            vec![GateFail::Contamination]
        } else {
            vec![GateFail::EvidenceMissing {
                field: "contamination_evidence".into(),
            }]
        };
        // A persisted reject is terminal: the row will never be drained again.
        let _ = st.store.forget_miner_env(&row.submission_digest);
        let _ =
            st.store
                .forget_artefact(&row.artifact_digest, &row.miner_hotkey, &row.submit_nonce);
        return persist_pre_eval_reject(st, executor.as_ref(), row, topic, &failed);
    }

    // Upload path: read the vault and hand the exact bytes to evaluate so
    // the host can inject them over vsock. Missing / empty / oversize /
    // digest mismatch is 503 with the row untouched — never invent bytes.
    // URI-only (`https://…`) skips the vault and the guest still fetches.
    let artifact_tar = staged_artefact_bytes(st, &row)?;
    let eval = eval_after_freeze(
        &st.pin,
        topic,
        offer,
        executor.as_ref(),
        &row.submission_digest,
        &row.artifact_digest,
        row.artifact_uri.as_deref(),
        row.declared_flops,
        &holdout,
        &row.claim,
        st.backend,
        st.live(),
        st.judge_api_key.as_deref(),
        Some(&sealed),
        miner_env,
        artifact_tar.as_deref(),
    )
    .await
    .map_err(|e| eval_err(&e))?;

    let registered = st.registered_custom();
    let verdict = judge_topic(
        topic,
        &eval.agent,
        &eval.harness,
        &sealed,
        &hits,
        &custom_ids_ref(&registered),
    );
    let receipt_json = serde_json::to_string(&eval.receipt).unwrap_or_default();
    // The run is over: whatever BYOK a `queued` row was holding is spent.
    let _ = st.store.forget_miner_env(&row.submission_digest);
    let _ = st
        .store
        .forget_artefact(&row.artifact_digest, &row.miner_hotkey, &row.submit_nonce);
    persist_scored(
        st,
        executor.as_ref(),
        eval.executor.as_ref(),
        row,
        topic,
        verdict,
        receipt_json,
        eval.backend,
    )
    .await
}

/// Gateway-vault bytes for a `proof-artefact://` locator. URI-only is `Ok(None)`.
///
/// # Errors
///
/// **503** when the locator is staged but the vault cannot produce matching
/// bytes (missing, empty, oversize, digest mismatch). The row is untouched.
fn staged_artefact_bytes(st: &AppState, row: &Submission) -> Result<Option<Vec<u8>>, ErrResp> {
    let Some(uri) = row
        .artifact_uri
        .as_deref()
        .map(str::trim)
        .filter(|u| !u.is_empty())
    else {
        return Ok(None);
    };
    if !is_staged_artefact_uri(uri) {
        return Ok(None);
    }
    let vault = st
        .store
        .artefact_bytes(
            &row.artifact_digest,
            &row.miner_hotkey,
            &row.submit_nonce,
        )
        .map_err(|e| err(StatusCode::SERVICE_UNAVAILABLE, &e.to_string()))?;
    let Some(bytes) = vault else {
        return Err(err(
            StatusCode::SERVICE_UNAVAILABLE,
            "this submission's artefact was uploaded and this host no longer holds the staged bytes; the row is untouched — nothing was rented and no bytes are invented",
        ));
    };
    if bytes.is_empty() || bytes.len() > MAX_ARTEFACT_BYTES {
        return Err(err(
            StatusCode::SERVICE_UNAVAILABLE,
            "staged artefact is empty or oversize; refusing to invent bytes",
        ));
    }
    let got = hex::encode(Sha256::digest(&bytes));
    if !got.eq_ignore_ascii_case(row.artifact_digest.trim()) {
        return Err(err(
            StatusCode::SERVICE_UNAVAILABLE,
            "staged artefact digest mismatch; refusing to invent bytes",
        ));
    }
    Ok(Some(bytes))
}

/// Stamp the judge offer and the executor offer the run was held to.
fn stamp_offers(
    st: &AppState,
    executor: Option<&EvalExecutorOffer>,
    plan: Option<&ExecutorPlan>,
    row: &mut Submission,
) {
    row.inference_offer_id = st
        .offer
        .as_ref()
        .map(|o| o.offer_id.clone())
        .unwrap_or_default();
    row.config_commitment = st
        .offer
        .as_ref()
        .map(|o| o.config_commitment.clone())
        .unwrap_or_default();
    row.executor_offer_id = executor.map(|x| x.offer_id.clone()).unwrap_or_default();
    // A live run stamps the configuration it was actually held to (template,
    // 1x, effective deadline); sim has no plan and no rent, and a pre-eval
    // reject stamps the offer's own commitment.
    row.executor_commitment = plan
        .map(|p| p.config_commitment.clone())
        .or_else(|| executor.map(|x| x.config_commitment.clone()))
        .unwrap_or_default();
}

fn persist_pre_eval_reject(
    st: &AppState,
    executor: Option<&EvalExecutorOffer>,
    mut row: Submission,
    topic: &TopicDocument,
    failed: &[GateFail],
) -> Result<SubmitResp, ErrResp> {
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
    stamp_offers(st, executor, None, &mut row);
    row.state = SubmissionState::Rejected;
    row.receipt_json = None;
    row.verdict = Some(verdict);
    row.detail = Some(format!("gates={failed:?}"));
    let row = st
        .store
        .insert(row)
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
    Ok(SubmitResp::of(&row, st.backend, false))
}

async fn persist_scored(
    st: &AppState,
    executor: Option<&EvalExecutorOffer>,
    plan: Option<&ExecutorPlan>,
    mut row: Submission,
    topic: &TopicDocument,
    verdict: ProofVerdict,
    receipt_json: String,
    backend: EvalBackend,
) -> Result<SubmitResp, ErrResp> {
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
                .auto_promote(topic, &row.submission_digest, pass, primary, bar)
                .await;
        }
    }
    stamp_offers(st, executor, plan, &mut row);
    row.state = if promoted {
        SubmissionState::Champion
    } else if pass {
        SubmissionState::AwaitingAdmin
    } else {
        SubmissionState::Rejected
    };
    row.receipt_json = Some(receipt_json);
    row.detail = if pass {
        None
    } else {
        Some(format!("gates={:?}", verdict.failed))
    };
    row.verdict = Some(verdict);
    let row = st
        .store
        .insert(row)
        .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "store"))?;
    let _ = st.store.record_topic_run(
        &row.miner_hotkey,
        &topic_id,
        MinerTopicRun {
            pass,
            primary,
            artifact_digest: row.artifact_digest.clone(),
            near_duplicate: false,
        },
    );
    if let Some(live) = st.live() {
        live.on_persisted(&topic_id, &row.submission_digest, &row.id, promoted)
            .await;
    }
    Ok(SubmitResp::of(&row, backend, pass))
}

/// `GET /v1/submissions` filters. Both optional; `state` is the wire name
/// (`queued`, `awaiting_admin`, `rejected`, `champion`).
#[derive(Debug, Default, Deserialize)]
struct ListQuery {
    #[serde(default)]
    state: Option<SubmissionState>,
    #[serde(default)]
    topic_id: Option<String>,
}

/// Newest first. An unknown `state` value is axum's **400**.
async fn list_subs(State(st): State<AppState>, Query(q): Query<ListQuery>) -> impl IntoResponse {
    let topic = q
        .topic_id
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty());
    let rows: Vec<Submission> = st
        .store
        .list()
        .unwrap_or_default()
        .into_iter()
        .filter(|r| q.state.is_none_or(|s| r.state == s))
        .filter(|r| topic.is_none_or(|t| r.topic_id == t))
        .collect();
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

/// `POST /v1/admin/proof/queue/drain` body.
#[derive(Debug, Deserialize)]
struct DrainBody {
    /// Topic whose `queued` rows to score.
    topic_id: String,
    /// Rows to score this call, oldest first (default 1, floor 1). The reply's
    /// `remaining` says what is still queued.
    #[serde(default)]
    limit: Option<usize>,
}

/// What one drain pass over one topic did.
#[derive(Debug, Clone, Serialize)]
pub struct DrainReport {
    /// Topic drained.
    pub topic_id: String,
    /// Rows scored this pass, in queue order, each in its final state.
    pub drained: Vec<SubmitResp>,
    /// Rows still `queued` on the topic after this pass.
    pub remaining: usize,
    /// Why the pass stopped with rows still queued: the host refused the next
    /// row (it went back to the queue unscored, nothing rented) or the topic
    /// was re-published to defer / close mid-pass. `None` when the pass ran
    /// to its limit or emptied the queue.
    pub stopped: Option<String>,
}

impl AppState {
    /// The topic whose queue may be drained right now: published, open at
    /// this epoch, and not deferring. A topic that still defers is a **409**
    /// (lift `constraints.params.defer_scoring` by re-publishing first);
    /// unknown is **400**, not open **409**. Nothing is touched on refusal.
    fn drainable_topic(&self, topic_id: &str) -> Result<TopicDocument, ErrResp> {
        let topic = self
            .store
            .topic(topic_id)
            .map_err(|_| err(StatusCode::BAD_REQUEST, "unknown topic"))?;
        if !topic.is_open_at(self.epoch) {
            return Err(err(
                StatusCode::CONFLICT,
                "topic is not open; its queued rows stay queued",
            ));
        }
        if topic.constraints.defer_scoring() {
            return Err(err(
                StatusCode::CONFLICT,
                "topic still defers scoring (constraints.params.defer_scoring = \"true\"); re-publish the signed topic without it, then drain",
            ));
        }
        Ok(topic)
    }

    /// Score up to `limit` `queued` rows of `topic_id`, oldest first, each
    /// through the exact path a live submit takes ([`score_intake`]), and
    /// land each result on its row. The topic is re-read before every row so
    /// a re-publish that defers or closes it mid-pass stops the pass. The
    /// first host refusal stops the pass too: that row is released back to
    /// the queue unscored (nothing was rented) and later rows are not tried.
    /// Another drain already holding the topic's claim stops it as well
    /// (`stopped` names the row in flight) — a topic scores one row at a time.
    pub async fn drain_queue(&self, topic_id: &str, limit: usize) -> DrainReport {
        let mut report = DrainReport {
            topic_id: topic_id.to_owned(),
            drained: Vec::new(),
            remaining: 0,
            stopped: None,
        };
        for _ in 0..limit {
            let topic = match self.drainable_topic(topic_id) {
                Ok(t) => t,
                Err((_, body)) => {
                    report.stopped = Some(error_text(&body));
                    break;
                }
            };
            let row = match self.store.claim_next_queued(topic_id) {
                Ok(Some(row)) => row,
                Ok(None) => break,
                Err(e) => {
                    report.stopped = Some(e.to_string());
                    break;
                }
            };
            let claim = ClaimGuard::new(self.store.clone(), &row);
            match score_intake(self, row, &topic).await {
                Ok(resp) => {
                    claim.landed();
                    report.drained.push(resp);
                }
                Err((code, body)) => {
                    drop(claim);
                    report.stopped = Some(format!("{code}: {}", error_text(&body)));
                    break;
                }
            }
        }
        report.remaining = self.store.queued(Some(topic_id)).map_or(0, |q| q.len());
        report
    }

    /// One pass for a poll loop: drain every open topic that has `queued`
    /// rows and no longer defers scoring, until each queue is empty or the
    /// host refuses (those rows stay queued for the next pass). Topics that
    /// still defer are skipped — lifting the flag is the operator's
    /// "score now". Returns one report per topic that had queued rows.
    pub async fn drain_ready_queues(&self) -> Vec<DrainReport> {
        let mut reports = Vec::new();
        for topic in self.store.topics().unwrap_or_default() {
            if !topic.is_open_at(self.epoch) || topic.constraints.defer_scoring() {
                continue;
            }
            let queued = self.store.queued(Some(&topic.id)).map_or(0, |q| q.len());
            if queued == 0 {
                continue;
            }
            reports.push(self.drain_queue(&topic.id, queued).await);
        }
        reports
    }
}

fn error_text(body: &Json<serde_json::Value>) -> String {
    body.0["error"]
        .as_str()
        .map_or_else(|| body.0.to_string(), str::to_owned)
}

/// Holds a topic's queue claim for one row while it scores, and gives it
/// back on **every** exit that did not land the row: the host refused, the
/// scorer panicked, or the drain future was dropped (an operator's HTTP
/// call cut mid-eval, the poll task cancelled). Without it an interrupted
/// drain would leave the topic "busy" until a restart. [`Self::landed`]
/// disarms it once the scored row is persisted (which settled the claim
/// itself); a release is keyed by `(topic, row)`, so a late one can never
/// drop a claim a later drain took.
struct ClaimGuard {
    store: MemoryStore,
    topic_id: String,
    id: String,
    armed: bool,
}

impl ClaimGuard {
    fn new(store: MemoryStore, row: &Submission) -> Self {
        Self {
            store,
            topic_id: row.topic_id.clone(),
            id: row.id.clone(),
            armed: true,
        }
    }

    /// The row landed scored: nothing to give back.
    fn landed(mut self) {
        self.armed = false;
    }
}

impl Drop for ClaimGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ = self.store.release_claim(&self.topic_id, &self.id);
        }
    }
}

fn claim_err(e: StoreError) -> ErrResp {
    match e {
        StoreError::NotFound(_) => err(StatusCode::NOT_FOUND, "not_found"),
        StoreError::Illegal(why) => err(StatusCode::CONFLICT, &why),
        busy @ StoreError::Busy { .. } => err(StatusCode::CONFLICT, &busy.to_string()),
        other => store_err(&other),
    }
}

/// Operator drain: score the oldest `queued` rows of one topic through the
/// live path. **200** with a [`DrainReport`]; **503** carrying the report
/// (plus `error`) when the host refused before a single row scored — the
/// rows are still queued, nothing was rented. Same bearer as every admin
/// route; the topic must be open and no longer deferring (**409**).
async fn drain_queue(
    State(st): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<DrainBody>,
) -> Result<impl IntoResponse, ErrResp> {
    if st.admin_hashes.is_empty() {
        return Err(err(StatusCode::SERVICE_UNAVAILABLE, "auth_unconfigured"));
    }
    if !admin_ok(&headers, &st.admin_hashes) {
        return Err(err(StatusCode::UNAUTHORIZED, "unauthorized"));
    }
    let topic_id = body.topic_id.trim().to_owned();
    st.drainable_topic(&topic_id)?;
    if let Some(id) = st.store.scoring_row(&topic_id).map_err(|e| store_err(&e))? {
        return Err(claim_err(StoreError::Busy { topic_id, id }));
    }
    let report = st
        .drain_queue(&topic_id, body.limit.unwrap_or(1).max(1))
        .await;
    let mut view = serde_json::to_value(&report)
        .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "report"))?;
    if report.drained.is_empty() {
        if let Some(why) = &report.stopped {
            view["error"] = serde_json::Value::String(why.clone());
            return Ok((StatusCode::SERVICE_UNAVAILABLE, Json(view)));
        }
    }
    Ok((StatusCode::OK, Json(view)))
}

/// Operator: score one `queued` row now (same rules as a drain of one). The
/// row must be the **head** of its topic's queue — the queue drains oldest
/// first, and promotion compares each run against the best at that moment.
/// **200** with the scored row's reply; **404** unknown; **409** when the
/// row is not `queued`, is not the head (the error names the head), its
/// topic already has a row in flight, or its topic still defers / is not
/// open; a host refusal is that refusal (**503**) with the row released
/// back to the queue.
async fn score_queued(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ErrResp> {
    if st.admin_hashes.is_empty() {
        return Err(err(StatusCode::SERVICE_UNAVAILABLE, "auth_unconfigured"));
    }
    if !admin_ok(&headers, &st.admin_hashes) {
        return Err(err(StatusCode::UNAUTHORIZED, "unauthorized"));
    }
    let row = st.store.claim_queued(&id).map_err(claim_err)?;
    let claim = ClaimGuard::new(st.store.clone(), &row);
    let topic = st.drainable_topic(&row.topic_id)?;
    let resp = score_intake(&st, row, &topic).await?;
    claim.landed();
    Ok((StatusCode::OK, Json(resp)))
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

    /// Uncompressed ustar with file content — the shape intake accepts.
    fn recipe_tar() -> Vec<u8> {
        use proof_vm_proto::tar::fixtures::{archive, member};
        archive(&[member("recipe/run.sh", b'0', b"echo recipe\n")])
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
        /// The miner BYOK environment each `score` call was handed.
        envs: std::sync::Mutex<Vec<MinerEnv>>,
        /// Length of staged artefact bytes each `score` call was handed (`None` = URI-only).
        tars: std::sync::Mutex<Vec<Option<usize>>>,
    }

    impl StubScorer {
        fn win() -> Self {
            Self {
                reproduced: true,
                skill: 0.95,
                hits: AtomicUsize::new(0),
                envs: std::sync::Mutex::new(Vec::new()),
                tars: std::sync::Mutex::new(Vec::new()),
            }
        }
        fn lose() -> Self {
            Self {
                reproduced: false,
                ..Self::win()
            }
        }

        /// What reached the scorer, newest last.
        fn envs(&self) -> Vec<MinerEnv> {
            self.envs.lock().expect("envs").clone()
        }

        /// Staged artefact lengths that reached the scorer, newest last.
        fn tars(&self) -> Vec<Option<usize>> {
            self.tars.lock().expect("tars").clone()
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
            miner_env: &MinerEnv,
            artifact_tar: Option<&[u8]>,
        ) -> Result<proof_eval::ProofEvalDocument, EvalError> {
            self.hits.fetch_add(1, Ordering::SeqCst);
            self.envs.lock().expect("envs").push(miner_env.clone());
            self.tars
                .lock()
                .expect("tars")
                .push(artifact_tar.map(<[u8]>::len));
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

    async fn multipart_req(
        app: Router,
        fields: &serde_json::Value,
        artifact: Option<&[u8]>,
    ) -> (StatusCode, serde_json::Value) {
        let boundary = "----ProofTestBoundary";
        let mut raw = Vec::new();
        if let Some(obj) = fields.as_object() {
            for (k, v) in obj {
                let text = match v {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                raw.extend_from_slice(
                    format!(
                        "--{boundary}\r\nContent-Disposition: form-data; name=\"{k}\"\r\n\r\n{text}\r\n"
                    )
                    .as_bytes(),
                );
            }
        }
        if let Some(bytes) = artifact {
            raw.extend_from_slice(
                format!(
                    "--{boundary}\r\nContent-Disposition: form-data; name=\"artifact\"; filename=\"recipe.tar\"\r\nContent-Type: application/octet-stream\r\n\r\n"
                )
                .as_bytes(),
            );
            raw.extend_from_slice(bytes);
            raw.extend_from_slice(b"\r\n");
        }
        raw.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
        let req = Request::builder()
            .method("POST")
            .uri("/v1/submissions")
            .header(
                "content-type",
                format!("multipart/form-data; boundary={boundary}"),
            )
            .body(Body::from(raw))
            .expect("req");
        let resp = app.oneshot(req).await.expect("resp");
        let status = resp.status();
        let bytes = resp.into_body().collect().await.expect("body").to_bytes();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or(serde_json::json!({}));
        (status, v)
    }

    fn fixture_sk() -> [u8; 32] {
        let mut s = [0x11u8; 32];
        s[0] = 0x42;
        s
    }

    fn submit_body(label: &str, extra: &serde_json::Value) -> serde_json::Value {
        let mut v = serde_json::json!({
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
        if extra.get("hotkey_signature").is_none() {
            proof_submit::attach_to_json(&mut v, &fixture_sk()).expect("sign");
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
    async fn submit_requires_a_hotkey_signature() {
        let app = app("op");
        let mut unsigned = submit_body("x", &serde_json::json!({}));
        unsigned
            .as_object_mut()
            .expect("obj")
            .remove("hotkey_signature");
        let (st, body) = json_req(app.clone(), "POST", "/v1/submissions", unsigned, None).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED, "{body}");
        assert_eq!(body["error"], "hotkey_signature required");
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
            "unsigned must not insert: {list}"
        );

        let mut zero_sig = submit_body("x", &serde_json::json!({}));
        zero_sig["hotkey_signature"] = serde_json::json!("00".repeat(64));
        let (st, body) = json_req(app.clone(), "POST", "/v1/submissions", zero_sig, None).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED, "{body}");
        assert_eq!(body["error"], "hotkey_signature invalid");

        // The wire format is exactly 128 lowercase hex: no 0x, no uppercase.
        for mangle in [
            |s: &str| format!("0x{s}"),
            |s: &str| s.to_ascii_uppercase(),
            |s: &str| format!(" {s}"),
        ] {
            let mut body_v = submit_body("x", &serde_json::json!({}));
            let good = body_v["hotkey_signature"].as_str().expect("sig").to_owned();
            body_v["hotkey_signature"] = serde_json::json!(mangle(&good));
            let (st, body) = json_req(app.clone(), "POST", "/v1/submissions", body_v, None).await;
            assert_eq!(st, StatusCode::UNAUTHORIZED, "{body}");
            assert_eq!(body["error"], "hotkey_signature invalid");
        }

        let mut other = [0x22u8; 32];
        other[0] = 0x43;
        let other_pk = crypto::public_key_from_mini_secret(&other).expect("other");
        let mut wrong_key = submit_body("x", &serde_json::json!({}));
        wrong_key["miner_hotkey"] = serde_json::json!(hex::encode(other_pk));
        let (st, body) = json_req(app.clone(), "POST", "/v1/submissions", wrong_key, None).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED, "{body}");
        assert_eq!(body["error"], "hotkey_signature invalid");
        let (_, list) = json_req(app, "GET", "/v1/submissions", serde_json::json!({}), None).await;
        assert!(
            list["items"].as_array().is_some_and(Vec::is_empty),
            "wrong key must not insert: {list}"
        );
    }

    async fn row_count(app: Router) -> usize {
        let (_, list) = json_req(app, "GET", "/v1/submissions", serde_json::json!({}), None).await;
        list["items"].as_array().map_or(usize::MAX, Vec::len)
    }

    #[tokio::test]
    async fn submit_nonce_is_required_single_use_and_the_manifest_is_signed() {
        let app = app_tight_sim();

        let mut no_nonce = submit_body("nonce", &serde_json::json!({}));
        no_nonce
            .as_object_mut()
            .expect("obj")
            .remove("submit_nonce");
        let (st, body) = json_req(app.clone(), "POST", "/v1/submissions", no_nonce, None).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED, "{body}");
        assert_eq!(body["error"], "submit_nonce required");

        for bad in [
            "AB".repeat(32),
            "ab".repeat(31),
            format!("0x{}", "ab".repeat(31)),
        ] {
            let mut body_v = submit_body("nonce", &serde_json::json!({}));
            body_v["submit_nonce"] = serde_json::json!(bad);
            let (st, body) = json_req(app.clone(), "POST", "/v1/submissions", body_v, None).await;
            assert_eq!(st, StatusCode::UNAUTHORIZED, "{body}");
            assert_eq!(body["error"], "submit_nonce invalid");
        }

        // A signature over one manifest does not authorise another.
        let mut swapped = submit_body("nonce", &serde_json::json!({}));
        swapped["manifest"] = serde_json::json!({ "train_dataset_ids": ["leaked-holdout-v0"] });
        let (st, body) = json_req(app.clone(), "POST", "/v1/submissions", swapped, None).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED, "{body}");
        assert_eq!(body["error"], "hotkey_signature invalid");
        let mut reordered = submit_body(
            "nonce",
            &serde_json::json!({ "manifest": { "train_dataset_ids": ["b-mix", "a-mix"] } }),
        );
        assert_eq!(
            row_count(app.clone()).await,
            0,
            "nothing stored before a valid submit"
        );

        // The same bytes, once: the first is scored, the replay is refused
        // before evaluation and leaves no second row.
        let signed = submit_body("nonce", &serde_json::json!({}));
        let (st, created) =
            json_req(app.clone(), "POST", "/v1/submissions", signed.clone(), None).await;
        assert_eq!(st, StatusCode::CREATED, "{created}");
        let (st, body) = json_req(app.clone(), "POST", "/v1/submissions", signed, None).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED, "{body}");
        assert_eq!(body["error"], "submit_nonce reused");
        assert_eq!(row_count(app.clone()).await, 1, "replay must not add a row");
        let id = created["id"].as_str().expect("id");
        let (_, row) = json_req(
            app.clone(),
            "GET",
            &format!("/v1/submissions/{id}"),
            serde_json::json!({}),
            None,
        )
        .await;
        assert_eq!(
            row["submit_nonce"].as_str().map(str::len),
            Some(64),
            "row stamps the client nonce: {row}"
        );

        // A fresh nonce from the same key is a new submission; the canonical
        // manifest is order-independent so the client may list in any order.
        reordered["manifest"]["train_dataset_ids"] = serde_json::json!(["a-mix", "b-mix"]);
        let (st, body) = json_req(app.clone(), "POST", "/v1/submissions", reordered, None).await;
        assert_eq!(st, StatusCode::CREATED, "{body}");
        assert_eq!(row_count(app).await, 2);
    }

    /// The host verifies the bytes that were posted. Hex fields have one
    /// accepted spelling, so a value the signer did not see byte for byte is
    /// a 400 about the request, never a 401 from silent normalisation; and a
    /// `topic_id` is signed verbatim (the host trims only to look it up).
    #[tokio::test]
    async fn hex_fields_have_one_wire_form_and_strings_are_verified_verbatim() {
        let app = app_tight_sim();
        let signed = submit_body("wire", &serde_json::json!({}));
        let hotkey = signed["miner_hotkey"].as_str().expect("hk").to_owned();
        let wire_digest = signed["artifact_digest"]
            .as_str()
            .expect("digest")
            .to_owned();
        for (field, value) in [
            ("miner_hotkey", hotkey.to_ascii_uppercase()),
            ("miner_hotkey", format!("0x{hotkey}")),
            ("artifact_digest", wire_digest.to_ascii_uppercase()),
            ("artifact_digest", format!("0x{wire_digest}")),
            ("artifact_digest", format!(" {wire_digest}")),
        ] {
            let mut body_v = signed.clone();
            body_v[field] = serde_json::json!(value);
            let (st, body) = json_req(app.clone(), "POST", "/v1/submissions", body_v, None).await;
            assert_eq!(st, StatusCode::BAD_REQUEST, "{field}: {body}");
            let error = body["error"].as_str().unwrap_or_default();
            assert!(
                error.starts_with(&format!("invalid {field}")) && error.contains("lowercase"),
                "{field}: {body}"
            );
        }
        assert_eq!(row_count(app.clone()).await, 0);

        // Signed with whitespace around the topic id, posted the same way.
        let padded = submit_body(
            "padded",
            &serde_json::json!({ "topic_id": "  dt-no-ib-v0\n" }),
        );
        assert_eq!(padded["topic_id"], "  dt-no-ib-v0\n");
        let (st, body) = json_req(app.clone(), "POST", "/v1/submissions", padded, None).await;
        assert_eq!(st, StatusCode::CREATED, "{body}");
        assert_eq!(body["topic_id"], "dt-no-ib-v0");

        // A loosely pasted digest is canonicalised by the signing helper
        // before signing, so what it posts verifies.
        let loose = submit_body(
            "loose",
            &serde_json::json!({ "artifact_digest": format!("0x{}", digest("loose").to_ascii_uppercase()) }),
        );
        assert_eq!(loose["artifact_digest"], digest("loose"));
        let (st, body) = json_req(app.clone(), "POST", "/v1/submissions", loose, None).await;
        assert_eq!(st, StatusCode::CREATED, "{body}");
        assert_eq!(row_count(app).await, 2);
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

    fn empty_training_extra(topic_id: &str, uri: Option<&str>) -> serde_json::Value {
        let mut extra = serde_json::json!({
            "topic_id": topic_id,
            "manifest": { "train_content_hashes": [], "train_dataset_ids": [] },
        });
        if let Some(uri) = uri {
            extra["artifact_uri"] = serde_json::json!(uri);
        }
        extra
    }

    #[tokio::test]
    async fn custom_family_accepts_an_empty_training_manifest() {
        let scorer = Arc::new(FamilyStub::win("topic_minted_metric"));
        let app = app_with_custom(scorer.clone());
        let (st, created) = json_req(
            app,
            "POST",
            "/v1/submissions",
            submit_body(
                "agent-no-train",
                &empty_training_extra(
                    "custom-topic-v0",
                    Some("https://example.invalid/agent-no-train.tar"),
                ),
            ),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{created}");
        assert_ne!(created["state"], "rejected", "{created}");
        assert!(
            scorer.inner.hits.load(Ordering::SeqCst) >= 1,
            "empty custom manifest must still reach eval"
        );
    }

    #[tokio::test]
    async fn custom_topic_can_require_training_evidence() {
        let scorer = Arc::new(FamilyStub::win(CUSTOM_ID));
        let app = proof_router(state_with_custom_params(
            scorer.clone(),
            false,
            true,
            &[(proof_task::PARAM_REQUIRE_TRAINING_EVIDENCE, "true")],
        ));
        let (st, created) = json_req(
            app,
            "POST",
            "/v1/submissions",
            submit_body(
                "tight-empty",
                &empty_training_extra(CUSTOM, Some("https://example.invalid/tight.tar")),
            ),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{created}");
        assert_eq!(created["state"], "rejected", "{created}");
        let dump = created.to_string();
        assert!(
            dump.contains("contamination_evidence"),
            "tightened custom must still refuse an empty manifest: {created}"
        );
        assert_eq!(scorer.inner.hits.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn custom_family_still_rejects_holdout_overlap() {
        let scorer = Arc::new(FamilyStub::win("topic_minted_metric"));
        let recs = synthetic_holdout(STRATUM_SIZE, 1);
        let dirty = recs[0].content_sha256.clone();
        let app = app_with_custom(scorer.clone());
        let (st, created) = json_req(
            app,
            "POST",
            "/v1/submissions",
            submit_body(
                "agent-dirty",
                &serde_json::json!({
                    "topic_id": "custom-topic-v0",
                    "artifact_uri": "https://example.invalid/agent-dirty.tar",
                    "manifest": { "train_content_hashes": [dirty] },
                }),
            ),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{created}");
        assert_eq!(created["state"], "rejected", "{created}");
        assert_eq!(scorer.inner.hits.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn harvest_topic_can_skip_training_evidence() {
        let scorer = Arc::new(StubScorer::win());
        let p = pin(&format!("sha256:{}", "ab".repeat(32)));
        let recs = synthetic_holdout(STRATUM_SIZE, 1);
        let mut draft = unsigned_topic(&recs);
        draft.constraints.params.insert(
            proof_task::PARAM_REQUIRE_TRAINING_EVIDENCE.to_owned(),
            "false".to_owned(),
        );
        let store = MemoryStore::new();
        let (topic, meas) = seal_topic(&p, draft);
        store.put_topic(topic.clone()).expect("topic");
        store.load_holdout(&topic.id, recs).expect("holdout");
        store
            .set_baseline(&topic.id, meas.into_sealed())
            .expect("baseline");
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
        let (st, created) = json_req(
            app,
            "POST",
            "/v1/submissions",
            submit_body(
                "harvest-no-train",
                &empty_training_extra("dt-no-ib-v0", None),
            ),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{created}");
        assert_ne!(created["state"], "rejected", "{created}");
        assert!(scorer.hits.load(Ordering::SeqCst) >= 1);
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

    /// The sha256 of nothing — zero bytes, or an empty tar archive — is not
    /// a recipe digest. It is refused as the miner's request (400, no row)
    /// on a fully scorable host, before readiness, rent, or a topic VM.
    #[tokio::test]
    async fn a_digest_of_nothing_is_a_400_before_anything_runs() {
        let app = app_tight_sim();
        let empty_input = hex::encode(Sha256::digest(b""));
        let empty_tar = hex::encode(Sha256::digest([0u8; 10_240]));
        assert!(is_digest_of_nothing(&empty_input) && is_digest_of_nothing(&empty_tar));
        assert!(!is_digest_of_nothing(&digest("a real recipe")));
        for nothing in [empty_input, empty_tar.to_ascii_uppercase()] {
            let (st, body) = json_req(
                app.clone(),
                "POST",
                "/v1/submissions",
                submit_body("x", &serde_json::json!({ "artifact_digest": nothing })),
                None,
            )
            .await;
            assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
            let error = body["error"].as_str().unwrap_or_default();
            assert!(error.contains("sha256 of empty input"), "{body}");
            assert!(error.contains("artifact_uri"), "{body}");
        }
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
            "no row: {list}"
        );
        // The same host scores a real digest.
        let (st, body) = json_req(
            app,
            "POST",
            "/v1/submissions",
            submit_body("a real recipe", &serde_json::json!({})),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{body}");
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
            miner_env: &MinerEnv,
            artifact_tar: Option<&[u8]>,
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
                    miner_env,
                    artifact_tar,
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
            assert_eq!(body["error"], "artifact required");
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

    #[tokio::test]
    async fn upload_only_custom_submit_stages_bytes_and_records_internal_locator() {
        let scorer = Arc::new(FamilyStub::win("topic_minted_metric"));
        let app = app_with_custom(scorer.clone());
        let bytes = recipe_tar();
        let artifact_digest = hex::encode(Sha256::digest(&bytes));
        let fields = submit_body(
            "upload-only",
            &serde_json::json!({
                "topic_id": "custom-topic-v0",
                "artifact_digest": artifact_digest,
            }),
        );
        let (st, created) = multipart_req(app.clone(), &fields, Some(&bytes)).await;
        assert_eq!(st, StatusCode::CREATED, "{created}");
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
        assert_eq!(row["artifact_digest"], artifact_digest);
        assert_eq!(
            row["artifact_uri"],
            format!("proof-artefact://{artifact_digest}"),
            "{row}"
        );
        assert!(
            row.get("artifact_staged").is_none(),
            "host path stays off GET: {row}"
        );
        assert_eq!(scorer.inner.hits.load(Ordering::SeqCst), 1);
        assert_eq!(
            scorer.inner.tars(),
            vec![Some(bytes.len())],
            "evaluate must receive the vault bytes for vsock inject"
        );

        let scorer = Arc::new(FamilyStub::win(CUSTOM_ID));
        let app = proof_router(state_with_deferred_custom(scorer.clone(), true, true));
        let fields = submit_body(
            "upload-queued",
            &serde_json::json!({
                "topic_id": CUSTOM,
                "artifact_digest": artifact_digest,
                "manifest": { "train_content_hashes": [], "train_dataset_ids": [] },
            }),
        );
        let (st, created) = multipart_req(app.clone(), &fields, Some(&bytes)).await;
        assert_eq!(st, StatusCode::CREATED, "{created}");
        assert_eq!(created["state"], "queued", "{created}");
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
        assert_eq!(row["artifact_digest"], artifact_digest);
        assert_eq!(
            row["artifact_uri"],
            format!("proof-artefact://{artifact_digest}"),
            "{row}"
        );
        assert!(
            row.get("artifact_staged").is_none(),
            "host path stays off GET: {row}"
        );
        assert_eq!(scorer.inner.hits.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn uri_only_custom_submit_still_works() {
        let scorer = Arc::new(FamilyStub::win("topic_minted_metric"));
        let app = app_with_custom(scorer.clone());
        let (st, created) = json_req(
            app,
            "POST",
            "/v1/submissions",
            submit_body(
                "uri-only",
                &serde_json::json!({
                    "topic_id": "custom-topic-v0",
                    "artifact_uri": "https://example.invalid/uri-only.tar",
                }),
            ),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{created}");
        assert_eq!(scorer.inner.hits.load(Ordering::SeqCst), 1);
        assert_eq!(
            scorer.inner.tars(),
            vec![None],
            "URI-only must not invent vault bytes"
        );
    }

    #[tokio::test]
    async fn upload_wins_over_miner_uri_and_mismatch_or_oversize_or_empty_is_400() {
        let scorer = Arc::new(FamilyStub::win(CUSTOM_ID));
        let app = proof_router(state_with_deferred_custom(scorer.clone(), true, true));
        let bytes = recipe_tar();
        let artifact_digest = hex::encode(Sha256::digest(&bytes));
        let fields = submit_body(
            "both",
            &serde_json::json!({
                "topic_id": CUSTOM,
                "artifact_digest": artifact_digest,
                "artifact_uri": "https://example.invalid/ignored.tar",
                "manifest": { "train_content_hashes": [], "train_dataset_ids": [] },
            }),
        );
        let (st, created) = multipart_req(app.clone(), &fields, Some(&bytes)).await;
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
        assert_eq!(
            row["artifact_uri"],
            format!("proof-artefact://{artifact_digest}")
        );

        let live = app_with_custom(Arc::new(FamilyStub::win("topic_minted_metric")));
        let mismatch = submit_body(
            "mismatch",
            &serde_json::json!({
                "topic_id": "custom-topic-v0",
                "artifact_digest": digest("mismatch"),
            }),
        );
        let (st, body) = multipart_req(live.clone(), &mismatch, Some(&bytes)).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(
            body["error"],
            "artifact_digest does not match uploaded bytes"
        );

        let empty = submit_body(
            "empty-bytes",
            &serde_json::json!({
                "topic_id": "custom-topic-v0",
                "artifact_digest": digest("empty-bytes"),
            }),
        );
        let (st, body) = multipart_req(live.clone(), &empty, Some(b"")).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(body["error"], "artifact is empty");

        let empty_tar = [0u8; 10_240];
        let empty_tar_digest = hex::encode(Sha256::digest(empty_tar));
        let nothing = submit_body(
            "empty-tar",
            &serde_json::json!({
                "topic_id": "custom-topic-v0",
                "artifact_digest": empty_tar_digest,
            }),
        );
        let (st, body) = multipart_req(live.clone(), &nothing, Some(&empty_tar)).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
        assert!(
            body["error"]
                .as_str()
                .unwrap_or_default()
                .contains("sha256 of empty input"),
            "{body}"
        );

        let not_tar = b"recipe-tar-not-empty";
        let not_tar_digest = hex::encode(Sha256::digest(not_tar));
        let garbage = submit_body(
            "not-a-tar",
            &serde_json::json!({
                "topic_id": "custom-topic-v0",
                "artifact_digest": not_tar_digest,
            }),
        );
        let (st, body) = multipart_req(app.clone(), &garbage, Some(not_tar)).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(body["error"], "artifact is not a tar archive");

        let gzip = [0x1fu8, 0x8b, 0x08, 0x00, 0, 0, 0, 0];
        let gzip_digest = hex::encode(Sha256::digest(gzip));
        let gz = submit_body(
            "gzip",
            &serde_json::json!({
                "topic_id": "custom-topic-v0",
                "artifact_digest": gzip_digest,
            }),
        );
        let (st, body) = multipart_req(app.clone(), &gz, Some(&gzip)).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(
            body["error"],
            "artifact is gzip-compressed; upload an uncompressed tar"
        );

        let hollow = {
            use proof_vm_proto::tar::fixtures::{archive, member};
            archive(&[member("recipe/empty", b'0', b"")])
        };
        let hollow_digest = hex::encode(Sha256::digest(&hollow));
        let empty_files = submit_body(
            "hollow",
            &serde_json::json!({
                "topic_id": "custom-topic-v0",
                "artifact_digest": hollow_digest,
            }),
        );
        let (st, body) = multipart_req(app.clone(), &empty_files, Some(&hollow)).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(body["error"], "artifact carries no file content");

        let over = vec![b'x'; proof_store::MAX_ARTEFACT_BYTES + 1];
        let over_digest = hex::encode(Sha256::digest(&over));
        let oversize = submit_body(
            "oversize",
            &serde_json::json!({
                "topic_id": "custom-topic-v0",
                "artifact_digest": over_digest,
            }),
        );
        let (st, body) = multipart_req(live, &oversize, Some(&over)).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
        assert!(
            body["error"].as_str().unwrap_or_default().contains("5 MiB"),
            "{body}"
        );
        assert_eq!(
            scorer.inner.hits.load(Ordering::SeqCst),
            0,
            "deferred bytes-win must not score"
        );
        assert_eq!(
            scorer.inner.tars(),
            vec![Some(bytes.len())],
            "upload wins: evaluate receives the staged bytes, not a miner fetch"
        );
    }

    /// A queued upload whose vault file is gone must 503 with the row
    /// untouched — never invent bytes, never rent.
    #[tokio::test]
    async fn a_queued_upload_with_missing_vault_is_503_row_untouched() {
        let scorer = Arc::new(FamilyStub::win(CUSTOM_ID));
        let state = state_with_deferred_custom(scorer.clone(), true, true);
        let pin = state.pin.clone();
        let app = proof_router(state.clone());
        let bytes = recipe_tar();
        let artifact_digest = hex::encode(Sha256::digest(&bytes));
        let fields = submit_body(
            "queued-upload",
            &serde_json::json!({
                "topic_id": CUSTOM,
                "artifact_digest": artifact_digest,
            }),
        );
        let nonce = fields["submit_nonce"].as_str().expect("nonce").to_owned();
        let hotkey = fields["miner_hotkey"].as_str().expect("hotkey").to_owned();
        let (st, created) = multipart_req(app.clone(), &fields, Some(&bytes)).await;
        assert_eq!(st, StatusCode::CREATED, "{created}");
        assert_eq!(created["state"], "queued", "{created}");
        let id = created["id"].as_str().expect("id").to_owned();
        assert_eq!(scorer.inner.hits.load(Ordering::SeqCst), 0, "deferred");
        assert_eq!(
            state
                .store
                .artefact_bytes(&artifact_digest, &hotkey, &nonce)
                .expect("vault")
                .as_deref(),
            Some(bytes.as_slice()),
            "intake staged the upload"
        );
        state
            .store
            .forget_artefact(&artifact_digest, &hotkey, &nonce)
            .expect("drop");
        republish_custom(app.clone(), &pin, None).await;
        let (st, body) = json_req(
            app.clone(),
            "POST",
            &format!("/v1/admin/proof/submissions/{id}/score"),
            serde_json::json!({}),
            Some("op"),
        )
        .await;
        assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE, "{body}");
        let why = body["error"].as_str().unwrap_or_default();
        assert!(
            why.contains("no longer holds") || why.contains("refusing to invent"),
            "{body}"
        );
        let (st, row) = json_req(
            app,
            "GET",
            &format!("/v1/submissions/{id}"),
            serde_json::json!({}),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{row}");
        assert_eq!(row["state"], "queued", "row untouched: {row}");
        assert_eq!(
            scorer.inner.hits.load(Ordering::SeqCst),
            0,
            "nothing rented"
        );
        assert!(scorer.inner.tars().is_empty(), "no invented bytes");
    }

    #[tokio::test]
    async fn invalid_tar_gzip_or_contentless_upload_is_400_with_no_row() {
        let scorer = Arc::new(FamilyStub::win("topic_minted_metric"));
        let app = app_with_custom(scorer.clone());
        let junk = vec![0x41u8; 1024];
        let junk_digest = hex::encode(Sha256::digest(&junk));
        let fields = submit_body(
            "not-tar",
            &serde_json::json!({
                "topic_id": "custom-topic-v0",
                "artifact_digest": junk_digest,
            }),
        );
        let (st, body) = multipart_req(app.clone(), &fields, Some(&junk)).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
        assert!(
            body["error"]
                .as_str()
                .unwrap_or_default()
                .contains("not a tar archive"),
            "{body}"
        );

        let gzip = vec![0x1f, 0x8b, 0x08, 0x00, 0, 0, 0, 0, 0, 0];
        let gzip_digest = hex::encode(Sha256::digest(&gzip));
        let fields = submit_body(
            "gzip",
            &serde_json::json!({
                "topic_id": "custom-topic-v0",
                "artifact_digest": gzip_digest,
            }),
        );
        let (st, body) = multipart_req(app.clone(), &fields, Some(&gzip)).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
        assert!(
            body["error"]
                .as_str()
                .unwrap_or_default()
                .contains("gzip-compressed"),
            "{body}"
        );

        let hollow = proof_vm_proto::tar::fixtures::archive(&[
            proof_vm_proto::tar::fixtures::member("recipe/", b'5', b""),
            proof_vm_proto::tar::fixtures::member("recipe/run.sh", b'0', b""),
        ]);
        let hollow_digest = hex::encode(Sha256::digest(&hollow));
        let fields = submit_body(
            "hollow",
            &serde_json::json!({
                "topic_id": "custom-topic-v0",
                "artifact_digest": hollow_digest,
            }),
        );
        let (st, body) = multipart_req(app.clone(), &fields, Some(&hollow)).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
        assert!(
            body["error"]
                .as_str()
                .unwrap_or_default()
                .contains("no file content"),
            "{body}"
        );

        let (_, list) = json_req(app, "GET", "/v1/submissions", serde_json::json!({}), None).await;
        assert!(
            list["items"].as_array().is_some_and(Vec::is_empty),
            "invalid tar must not persist a row: {list}"
        );
        assert_eq!(scorer.inner.hits.load(Ordering::SeqCst), 0);
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

    // ----- deferred scoring: `queued` rows and the drain -----

    const CUSTOM: &str = "custom-topic-v0";
    const CUSTOM_ID: &str = "topic_minted_metric";

    /// The signed custom topic of [`app_with_live_scorer`], with
    /// `constraints.params.defer_scoring` set to `value` when given.
    fn custom_topic_with_defer(pin: &ProofPin, value: Option<&str>) -> TopicDocument {
        let mut draft = unsigned_custom_topic(&[], CUSTOM_ID);
        if let Some(v) = value {
            draft
                .constraints
                .params
                .insert(proof_task::PARAM_DEFER_SCORING.to_owned(), v.to_owned());
        }
        let (topic, _) = seal_topic_with(pin, draft, &[CUSTOM_ID]);
        topic
    }

    /// Live Lium host like [`app_with_live_scorer`] — open sealed throughput
    /// topic `dt-no-ib-v0` plus the custom topic — where the custom topic
    /// carries `defer_scoring = "true"` when `defer` is set. `custom_baseline`
    /// false models a host still installing that topic's baseline (no sealed
    /// vector recorded, so scoring it would be a 503).
    fn state_with_deferred_custom(
        live: Arc<dyn LiveScorer>,
        defer: bool,
        custom_baseline: bool,
    ) -> AppState {
        state_with_custom_params(live, defer, custom_baseline, &[])
    }

    /// [`state_with_deferred_custom`] with extra `constraints.params` on the
    /// custom topic's signed document (the BYOK knobs, in these tests).
    fn state_with_custom_params(
        live: Arc<dyn LiveScorer>,
        defer: bool,
        custom_baseline: bool,
        params: &[(&str, &str)],
    ) -> AppState {
        let p = pin(&format!("sha256:{}", "ab".repeat(32)));
        let store = MemoryStore::new();
        let recs = synthetic_holdout(STRATUM_SIZE, 1);
        let (harvest, meas) = seal_topic(&p, unsigned_topic(&recs));
        store.put_topic(harvest.clone()).expect("topic");
        store.load_holdout(&harvest.id, recs).expect("holdout");
        store
            .set_baseline(&harvest.id, meas.into_sealed())
            .expect("baseline");

        let mut draft = unsigned_custom_topic(&[], CUSTOM_ID);
        if defer {
            draft.constraints.params.insert(
                proof_task::PARAM_DEFER_SCORING.to_owned(),
                "true".to_owned(),
            );
        }
        for (k, v) in params {
            draft
                .constraints
                .params
                .insert((*k).to_owned(), (*v).to_owned());
        }
        let recs = synthetic_holdout(STRATUM_SIZE, 1);
        let (custom, meas) = seal_topic_with(&p, draft, &[CUSTOM_ID]);
        assert_eq!(custom.constraints.defer_scoring(), defer);
        store.put_topic(custom.clone()).expect("topic");
        store.load_holdout(&custom.id, recs).expect("holdout");
        if custom_baseline {
            let mut sealed = meas.into_sealed();
            sealed.custom_value = Some(0.5);
            store.set_baseline(&custom.id, sealed).expect("baseline");
        }
        let executor = test_executor(&p);
        AppState {
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
        }
    }

    fn custom_submit(label: &str) -> serde_json::Value {
        submit_body(
            label,
            &serde_json::json!({
                "topic_id": CUSTOM,
                "artifact_uri": format!("https://example.invalid/{label}.tar"),
            }),
        )
    }

    fn ids_of(v: &serde_json::Value, key: &str) -> Vec<String> {
        v[key]
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .filter_map(|x| x.as_str().map(str::to_owned))
            .collect()
    }

    async fn status_of(app: Router) -> serde_json::Value {
        let (st, status) = json_req(app, "GET", "/v1/status", serde_json::json!({}), None).await;
        assert_eq!(st, StatusCode::OK);
        status
    }

    async fn queued_ids(app: Router, topic: Option<&str>) -> Vec<String> {
        let uri = match topic {
            Some(t) => format!("/v1/submissions?state=queued&topic_id={t}"),
            None => "/v1/submissions?state=queued".to_owned(),
        };
        let (st, list) = json_req(app, "GET", &uri, serde_json::json!({}), None).await;
        assert_eq!(st, StatusCode::OK, "{list}");
        // The public list is newest first; intake order is what the queue
        // assertions compare against.
        let mut ids: Vec<String> = list["items"]
            .as_array()
            .expect("items")
            .iter()
            .filter_map(|r| r["id"].as_str().map(str::to_owned))
            .collect();
        ids.sort();
        ids
    }

    /// Re-publish the custom topic through the admin route with the flag set
    /// to `value` (`None` removes it). Re-signed under the test topic key.
    async fn republish_custom(
        app: Router,
        pin: &ProofPin,
        value: Option<&str>,
    ) -> serde_json::Value {
        let doc = custom_topic_with_defer(pin, value);
        let (st, body) = json_req(
            app,
            "POST",
            "/v1/admin/proof/topics",
            serde_json::to_value(&doc).expect("json"),
            Some("op"),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{body}");
        body
    }

    /// An open topic whose signed document defers scoring accepts the
    /// submission under every intake gate and persists it `queued`: **201**,
    /// no scorer call, no persist hook, no stamps, no verdict, no mass. The
    /// public list and status show it; a retry of the same artefact finds its
    /// row (**200**) instead of queueing twice; the other open topic scores
    /// as before.
    #[tokio::test]
    async fn a_deferring_topic_queues_submissions_without_scoring() {
        let scorer = Arc::new(FamilyStub::win(CUSTOM_ID));
        let app = proof_router(state_with_deferred_custom(scorer.clone(), true, true));

        let status = status_of(app.clone()).await;
        assert_eq!(ids_of(&status, "deferred_topics"), [CUSTOM], "{status}");
        assert_eq!(
            ids_of(&status, "scorable_topics"),
            ["dt-no-ib-v0"],
            "a deferring topic is open but not scored right now: {status}"
        );
        let mut open = ids_of(&status, "open_topics");
        open.sort();
        assert_eq!(open, [CUSTOM, "dt-no-ib-v0"], "{status}");
        assert_eq!(status["queued_submissions"], 0, "{status}");

        let (st, created) = json_req(
            app.clone(),
            "POST",
            "/v1/submissions",
            custom_submit("deferred-artifact"),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{created}");
        assert_eq!(created["state"], "queued", "{created}");
        assert_eq!(created["eligible"], false, "{created}");
        assert_eq!(created["topic_id"], CUSTOM);
        assert_eq!(created["eval_backend"], "lium");
        let detail = created["detail"].as_str().unwrap_or_default();
        assert!(detail.contains("scoring deferred"), "{created}");
        assert!(detail.contains("defer_scoring"), "{created}");
        let id = created["id"].as_str().expect("id").to_owned();
        assert!(id.starts_with("pf_"), "{created}");
        assert_eq!(scorer.inner.hits.load(Ordering::SeqCst), 0, "no run");
        assert!(scorer.persisted.lock().expect("p").is_empty(), "no hook");

        let (st, row) = json_req(
            app.clone(),
            "GET",
            &format!("/v1/submissions/{id}"),
            serde_json::json!({}),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{row}");
        assert_eq!(row["state"], "queued", "{row}");
        assert!(row["verdict"].is_null(), "{row}");
        assert!(row["receipt_json"].is_null(), "{row}");
        assert_eq!(row["inference_offer_id"], "", "no host stamp yet: {row}");
        assert_eq!(row["executor_offer_id"], "", "no host stamp yet: {row}");
        assert_eq!(
            row["artifact_uri"],
            "https://example.invalid/deferred-artifact.tar"
        );
        assert_eq!(row["declared_flops"], FLOPS_BUDGET_MAX / 2, "{row}");
        assert!(
            row["detail"]
                .as_str()
                .unwrap_or_default()
                .contains("scoring deferred"),
            "{row}"
        );

        assert_eq!(
            queued_ids(app.clone(), None).await,
            std::slice::from_ref(&id)
        );
        assert_eq!(
            queued_ids(app.clone(), Some(CUSTOM)).await,
            std::slice::from_ref(&id)
        );
        assert!(queued_ids(app.clone(), Some("dt-no-ib-v0"))
            .await
            .is_empty());
        let (st, list) = json_req(
            app.clone(),
            "GET",
            "/v1/submissions?state=rejected",
            serde_json::json!({}),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert!(
            list["items"].as_array().is_some_and(Vec::is_empty),
            "{list}"
        );
        let (st, _) = json_req(
            app.clone(),
            "GET",
            "/v1/submissions?state=bogus",
            serde_json::json!({}),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "an unknown state filter");
        assert_eq!(status_of(app.clone()).await["queued_submissions"], 1);

        // The same artefact again is the same queued row, not a second run.
        let (st, again) = json_req(
            app.clone(),
            "POST",
            "/v1/submissions",
            custom_submit("deferred-artifact"),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{again}");
        assert_eq!(again["id"], id, "{again}");
        assert_eq!(again["state"], "queued", "{again}");
        assert!(
            again["detail"]
                .as_str()
                .unwrap_or_default()
                .contains("already queued"),
            "{again}"
        );
        assert_eq!(queued_ids(app.clone(), None).await.len(), 1);
        // A different artefact by the same hotkey queues behind it.
        let (st, second) = json_req(
            app.clone(),
            "POST",
            "/v1/submissions",
            custom_submit("second-artifact"),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{second}");
        assert_ne!(second["id"], id);
        assert_eq!(queued_ids(app.clone(), None).await.len(), 2);
        assert_eq!(scorer.inner.hits.load(Ordering::SeqCst), 0, "still no run");

        // Intake gates still refuse a custom row without an artefact.
        // FLOP declarations are ignored on custom topics, even u64::MAX.
        let (st, body) = json_req(
            app.clone(),
            "POST",
            "/v1/submissions",
            submit_body(
                "gated",
                &serde_json::json!({ "topic_id": CUSTOM, "artifact_uri": "" }),
            ),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "no locator: {body}");
        let (st, huge) = json_req(
            app.clone(),
            "POST",
            "/v1/submissions",
            submit_body(
                "huge-flops",
                &serde_json::json!({
                    "topic_id": CUSTOM,
                    "artifact_uri": "https://example.invalid/x.tar",
                    "declared_flops": u64::MAX,
                }),
            ),
            None,
        )
        .await;
        assert_eq!(
            st,
            StatusCode::CREATED,
            "custom ignores declared_flops: {huge}"
        );
        assert_eq!(huge["state"], "queued", "{huge}");
        assert_eq!(
            queued_ids(app.clone(), None).await.len(),
            3,
            "over-budget declaration still queued on custom"
        );

        // The other open topic is not deferred and scores right now.
        let (st, harvest_row) = json_req(
            app,
            "POST",
            "/v1/submissions",
            submit_body("harvest-artifact", &serde_json::json!({})),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{harvest_row}");
        assert_ne!(harvest_row["state"], "queued", "{harvest_row}");
        assert!(harvest_row["detail"].is_null() || harvest_row["detail"].is_string());
        assert_eq!(scorer.inner.hits.load(Ordering::SeqCst), 1);
    }

    /// The reason the flag exists: a host whose topic cannot be scored yet
    /// (runner registered but its VM backend not wired, no sealed baseline
    /// recorded) answers **503** with no row without the flag — and **201**
    /// `queued` with it. Nothing about host readiness is consulted on the
    /// queued path.
    #[tokio::test]
    async fn a_deferring_topic_queues_even_when_the_host_cannot_score_it() {
        let refusing = proof_router(state_with_deferred_custom(
            Arc::new(FamilyStub::unwired(CUSTOM_ID)),
            false,
            false,
        ));
        let (st, body) = json_req(
            refusing.clone(),
            "POST",
            "/v1/submissions",
            custom_submit("not-yet"),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE, "{body}");
        assert!(
            queued_ids(refusing, None).await.is_empty(),
            "no row on a 503"
        );

        let scorer = Arc::new(FamilyStub::unwired(CUSTOM_ID));
        let app = proof_router(state_with_deferred_custom(scorer.clone(), true, false));
        let status = status_of(app.clone()).await;
        assert_eq!(ids_of(&status, "deferred_topics"), [CUSTOM], "{status}");
        assert_eq!(
            ids_of(&status, "scorable_topics"),
            ["dt-no-ib-v0"],
            "{status}"
        );
        assert!(ids_of(&status, "custom_ready").is_empty(), "{status}");
        let (st, created) = json_req(
            app.clone(),
            "POST",
            "/v1/submissions",
            custom_submit("not-yet"),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{created}");
        assert_eq!(created["state"], "queued", "{created}");
        assert_eq!(scorer.inner.hits.load(Ordering::SeqCst), 0);
        assert_eq!(queued_ids(app, Some(CUSTOM)).await.len(), 1);
    }

    /// A malformed flag is a publish **400** naming the param; a `draft`
    /// topic that defers is still a submit **400** (deferring is not a
    /// way around `open`).
    #[tokio::test]
    async fn a_malformed_defer_flag_is_a_publish_400_and_draft_stays_400() {
        let state = state_with_deferred_custom(Arc::new(FamilyStub::win(CUSTOM_ID)), false, true);
        let pin = state.pin.clone();
        let app = proof_router(state);
        // Built past the ceremony helper (which validates): a signed document
        // whose flag is not a boolean word.
        let mut bad = custom_topic_with_defer(&pin, None);
        bad.constraints.params.insert(
            proof_task::PARAM_DEFER_SCORING.to_owned(),
            "maybe".to_owned(),
        );
        bad.signature = bad.sign_with(&sk()).expect("sign");
        let (st, body) = json_req(
            app.clone(),
            "POST",
            "/v1/admin/proof/topics",
            serde_json::to_value(&bad).expect("json"),
            Some("op"),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
        let msg = body["error"].as_str().unwrap_or_default();
        assert!(msg.contains("constraints.params.defer_scoring"), "{body}");
        assert!(msg.contains("\"true\" or \"false\""), "{body}");

        let mut draft = custom_topic_with_defer(&pin, Some("true"));
        draft.status = TopicStatus::Draft;
        draft.signature = draft.sign_with(&sk()).expect("sign");
        let (st, body) = json_req(
            app.clone(),
            "POST",
            "/v1/admin/proof/topics",
            serde_json::to_value(&draft).expect("json"),
            Some("op"),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{body}");
        let (st, body) = json_req(
            app.clone(),
            "POST",
            "/v1/submissions",
            custom_submit("draft-artifact"),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(body["error"], "topic is not open");
        let status = status_of(app.clone()).await;
        assert!(ids_of(&status, "deferred_topics").is_empty(), "{status}");
        assert!(queued_ids(app, None).await.is_empty());
    }

    /// The operator path: a drain on a topic that still defers is a **409**
    /// and touches nothing; once the topic is re-published without the flag
    /// the queue drains oldest first through the live path — each row keeps
    /// its id, lands scored with the host stamps and the persist hook fires
    /// — under the caller's `limit`, until the queue is empty.
    #[tokio::test]
    async fn drain_refuses_while_deferred_then_scores_the_queue_in_order() {
        let scorer = Arc::new(FamilyStub::win(CUSTOM_ID));
        let state = state_with_deferred_custom(scorer.clone(), true, true);
        let pin = state.pin.clone();
        let app = proof_router(state);
        let mut queued = Vec::new();
        for label in ["first", "second", "third"] {
            let (st, created) = json_req(
                app.clone(),
                "POST",
                "/v1/submissions",
                custom_submit(label),
                None,
            )
            .await;
            assert_eq!(st, StatusCode::CREATED, "{created}");
            queued.push(created["id"].as_str().expect("id").to_owned());
        }
        let drain = serde_json::json!({ "topic_id": CUSTOM });

        let (st, body) = json_req(
            app.clone(),
            "POST",
            "/v1/admin/proof/queue/drain",
            drain.clone(),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::UNAUTHORIZED, "{body}");
        let (st, body) = json_req(
            app.clone(),
            "POST",
            "/v1/admin/proof/queue/drain",
            drain.clone(),
            Some("op"),
        )
        .await;
        assert_eq!(st, StatusCode::CONFLICT, "{body}");
        assert!(
            body["error"]
                .as_str()
                .unwrap_or_default()
                .contains("still defers scoring"),
            "{body}"
        );
        let (st, body) = json_req(
            app.clone(),
            "POST",
            &format!("/v1/admin/proof/submissions/{}/score", queued[0]),
            serde_json::json!({}),
            Some("op"),
        )
        .await;
        assert_eq!(st, StatusCode::CONFLICT, "{body}");
        assert_eq!(scorer.inner.hits.load(Ordering::SeqCst), 0, "nothing ran");
        assert_eq!(queued_ids(app.clone(), None).await, queued);
        let (st, body) = json_req(
            app.clone(),
            "POST",
            "/v1/admin/proof/queue/drain",
            serde_json::json!({ "topic_id": "nope-v0" }),
            Some("op"),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");

        // Lift the flag: the re-signed document replaces the topic.
        republish_custom(app.clone(), &pin, None).await;
        let status = status_of(app.clone()).await;
        assert!(ids_of(&status, "deferred_topics").is_empty(), "{status}");
        assert!(
            ids_of(&status, "scorable_topics").contains(&CUSTOM.to_owned()),
            "{status}"
        );
        assert_eq!(
            status["queued_submissions"], 3,
            "the queue survives a re-publish"
        );

        let (st, report) = json_req(
            app.clone(),
            "POST",
            "/v1/admin/proof/queue/drain",
            serde_json::json!({ "topic_id": CUSTOM, "limit": 1 }),
            Some("op"),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{report}");
        assert_eq!(report["topic_id"], CUSTOM);
        assert_eq!(
            report["drained"].as_array().map(Vec::len),
            Some(1),
            "{report}"
        );
        assert_eq!(
            report["drained"][0]["id"], queued[0],
            "oldest first: {report}"
        );
        assert_eq!(report["drained"][0]["state"], "champion", "{report}");
        assert_eq!(report["drained"][0]["eligible"], true, "{report}");
        assert_eq!(report["remaining"], 2, "{report}");
        assert!(report["stopped"].is_null(), "{report}");
        assert_eq!(scorer.inner.hits.load(Ordering::SeqCst), 1);
        assert_eq!(
            scorer.persisted.lock().expect("p").clone(),
            vec![(CUSTOM.to_owned(), queued[0].clone(), true)]
        );
        let (st, row) = json_req(
            app.clone(),
            "GET",
            &format!("/v1/submissions/{}", queued[0]),
            serde_json::json!({}),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{row}");
        assert_eq!(row["state"], "champion", "{row}");
        assert!(row["verdict"]["pass"].as_bool().unwrap_or(false), "{row}");
        assert_eq!(row["inference_offer_id"], "master-v0", "judge stamp: {row}");
        assert_eq!(
            row["executor_offer_id"], "lium-1x-v0",
            "executor stamp: {row}"
        );
        assert!(
            row["detail"].is_null(),
            "a clean pass carries no detail: {row}"
        );
        assert_eq!(queued_ids(app.clone(), None).await, queued[1..]);

        // The single-row route scores only the head of the queue: naming the
        // third row while the second is still queued is a 409 that names
        // the head; naming the head scores exactly that row.
        let (st, body) = json_req(
            app.clone(),
            "POST",
            &format!("/v1/admin/proof/submissions/{}/score", queued[2]),
            serde_json::json!({}),
            Some("op"),
        )
        .await;
        assert_eq!(st, StatusCode::CONFLICT, "out of order: {body}");
        let msg = body["error"].as_str().unwrap_or_default();
        assert!(
            msg.contains("not the head") && msg.contains(&queued[1]),
            "{body}"
        );
        assert_eq!(queued_ids(app.clone(), None).await, queued[1..]);
        let (st, second) = json_req(
            app.clone(),
            "POST",
            &format!("/v1/admin/proof/submissions/{}/score", queued[1]),
            serde_json::json!({}),
            Some("op"),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{second}");
        assert_eq!(second["id"], queued[1], "{second}");
        assert_ne!(second["state"], "queued", "{second}");
        assert_eq!(queued_ids(app.clone(), None).await, [queued[2].clone()]);
        let (st, body) = json_req(
            app.clone(),
            "POST",
            &format!("/v1/admin/proof/submissions/{}/score", queued[0]),
            serde_json::json!({}),
            Some("op"),
        )
        .await;
        assert_eq!(
            st,
            StatusCode::CONFLICT,
            "a scored row is not queued: {body}"
        );
        let (st, _) = json_req(
            app.clone(),
            "POST",
            "/v1/admin/proof/submissions/pf_missing/score",
            serde_json::json!({}),
            Some("op"),
        )
        .await;
        assert_eq!(st, StatusCode::NOT_FOUND);

        // Default limit is one row; an empty queue is a 200 with nothing drained.
        let (st, report) = json_req(
            app.clone(),
            "POST",
            "/v1/admin/proof/queue/drain",
            drain.clone(),
            Some("op"),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{report}");
        assert_eq!(report["drained"][0]["id"], queued[2], "{report}");
        assert_eq!(report["remaining"], 0, "{report}");
        let (st, report) = json_req(
            app.clone(),
            "POST",
            "/v1/admin/proof/queue/drain",
            drain,
            Some("op"),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{report}");
        assert!(
            report["drained"].as_array().is_some_and(Vec::is_empty),
            "{report}"
        );
        assert_eq!(report["remaining"], 0);
        assert_eq!(scorer.inner.hits.load(Ordering::SeqCst), 3);
        assert_eq!(status_of(app).await["queued_submissions"], 0);
    }

    /// Fail closed on the drain: a host that cannot score the topic leaves
    /// every row `queued` (**503**, the report says why, nothing rented) —
    /// on the drain route, on the single-row route, and in the poll pass —
    /// and a topic re-published to defer again is skipped by the pass.
    #[tokio::test]
    async fn a_drain_the_host_refuses_leaves_the_rows_queued() {
        let scorer = Arc::new(FamilyStub::unwired(CUSTOM_ID));
        let state = state_with_deferred_custom(scorer.clone(), true, true);
        let pin = state.pin.clone();
        let app = proof_router(state.clone());
        let (st, created) = json_req(
            app.clone(),
            "POST",
            "/v1/submissions",
            custom_submit("waiting"),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{created}");
        let id = created["id"].as_str().expect("id").to_owned();

        // Still deferring: the poll pass skips the topic entirely.
        assert!(state.drain_ready_queues().await.is_empty());
        republish_custom(app.clone(), &pin, None).await;

        let (st, report) = json_req(
            app.clone(),
            "POST",
            "/v1/admin/proof/queue/drain",
            serde_json::json!({ "topic_id": CUSTOM, "limit": 5 }),
            Some("op"),
        )
        .await;
        assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE, "{report}");
        let why = report["error"].as_str().unwrap_or_default();
        assert!(why.contains("503") && why.contains("not wired"), "{report}");
        assert_eq!(report["stopped"], report["error"], "{report}");
        assert!(
            report["drained"].as_array().is_some_and(Vec::is_empty),
            "{report}"
        );
        assert_eq!(report["remaining"], 1, "{report}");

        let (st, body) = json_req(
            app.clone(),
            "POST",
            &format!("/v1/admin/proof/submissions/{id}/score"),
            serde_json::json!({}),
            Some("op"),
        )
        .await;
        assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE, "{body}");

        let reports = state.drain_ready_queues().await;
        assert_eq!(reports.len(), 1, "{reports:?}");
        assert!(reports[0].drained.is_empty());
        assert_eq!(reports[0].remaining, 1);
        assert!(
            reports[0]
                .stopped
                .as_deref()
                .is_some_and(|s| s.contains("not wired")),
            "{reports:?}"
        );

        assert_eq!(
            scorer.inner.hits.load(Ordering::SeqCst),
            0,
            "nothing rented"
        );
        assert_eq!(
            queued_ids(app.clone(), None).await,
            std::slice::from_ref(&id)
        );
        let (_, row) = json_req(
            app.clone(),
            "GET",
            &format!("/v1/submissions/{id}"),
            serde_json::json!({}),
            None,
        )
        .await;
        assert_eq!(row["state"], "queued", "released back to the queue: {row}");

        // A topic with no queue is a no-op drain; deferring again pauses the pass.
        let (st, report) = json_req(
            app.clone(),
            "POST",
            "/v1/admin/proof/queue/drain",
            serde_json::json!({ "topic_id": "dt-no-ib-v0" }),
            Some("op"),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{report}");
        assert_eq!(report["remaining"], 0);
        republish_custom(app.clone(), &pin, Some("true")).await;
        assert!(state.drain_ready_queues().await.is_empty());
        let (st, body) = json_req(
            app,
            "POST",
            "/v1/admin/proof/queue/drain",
            serde_json::json!({ "topic_id": CUSTOM }),
            Some("op"),
        )
        .await;
        assert_eq!(st, StatusCode::CONFLICT, "{body}");
    }

    /// The poll pass the binary runs: it scores the whole queue of a lifted
    /// topic in order and returns one report per topic that had rows.
    #[tokio::test]
    async fn the_poll_pass_drains_a_lifted_topic_in_order() {
        let scorer = Arc::new(FamilyStub::win(CUSTOM_ID));
        let state = state_with_deferred_custom(scorer.clone(), true, true);
        let pin = state.pin.clone();
        let app = proof_router(state.clone());
        let mut queued = Vec::new();
        for label in ["a", "b"] {
            let (st, created) = json_req(
                app.clone(),
                "POST",
                "/v1/submissions",
                custom_submit(label),
                None,
            )
            .await;
            assert_eq!(st, StatusCode::CREATED, "{created}");
            queued.push(created["id"].as_str().expect("id").to_owned());
        }
        assert!(
            state.drain_ready_queues().await.is_empty(),
            "deferred: skipped"
        );
        republish_custom(app.clone(), &pin, None).await;
        let reports = state.drain_ready_queues().await;
        assert_eq!(reports.len(), 1, "{reports:?}");
        let drained: Vec<String> = reports[0].drained.iter().map(|r| r.id.clone()).collect();
        assert_eq!(drained, queued);
        assert_eq!(reports[0].remaining, 0);
        assert!(reports[0].stopped.is_none());
        assert_eq!(scorer.inner.hits.load(Ordering::SeqCst), 2);
        assert!(state.drain_ready_queues().await.is_empty(), "nothing left");
        assert!(queued_ids(app, None).await.is_empty());
    }

    /// A topic's queue is scored one row at a time: while one drain holds
    /// the topic's claim (its head is mid-eval), a second drain — the admin
    /// route, the single-row route, or the poll pass — gets **409** / a
    /// `stopped` report and scores nothing, so two rows of one topic never
    /// run side by side and promotion stays oldest-first. Another topic is
    /// unaffected. Once the claim is back, the drain proceeds in order.
    #[tokio::test]
    async fn a_topic_with_a_row_in_flight_refuses_a_second_drain() {
        let scorer = Arc::new(FamilyStub::win(CUSTOM_ID));
        let state = state_with_deferred_custom(scorer.clone(), true, true);
        let pin = state.pin.clone();
        let app = proof_router(state.clone());
        let mut queued = Vec::new();
        for label in ["head", "next"] {
            let (st, created) = json_req(
                app.clone(),
                "POST",
                "/v1/submissions",
                custom_submit(label),
                None,
            )
            .await;
            assert_eq!(st, StatusCode::CREATED, "{created}");
            queued.push(created["id"].as_str().expect("id").to_owned());
        }
        republish_custom(app.clone(), &pin, None).await;

        // Another drain is mid-eval on the head: it holds the topic's claim.
        let in_flight = state
            .store
            .claim_next_queued(CUSTOM)
            .expect("claim")
            .expect("head");
        assert_eq!(in_flight.id, queued[0]);

        let (st, body) = json_req(
            app.clone(),
            "POST",
            "/v1/admin/proof/queue/drain",
            serde_json::json!({ "topic_id": CUSTOM, "limit": 5 }),
            Some("op"),
        )
        .await;
        assert_eq!(st, StatusCode::CONFLICT, "{body}");
        let msg = body["error"].as_str().unwrap_or_default();
        assert!(msg.contains("already has a row being scored"), "{body}");
        assert!(msg.contains(&queued[0]), "names the row in flight: {body}");
        for id in &queued {
            let (st, body) = json_req(
                app.clone(),
                "POST",
                &format!("/v1/admin/proof/submissions/{id}/score"),
                serde_json::json!({}),
                Some("op"),
            )
            .await;
            assert_eq!(st, StatusCode::CONFLICT, "{id}: {body}");
        }
        let reports = state.drain_ready_queues().await;
        assert_eq!(reports.len(), 1, "{reports:?}");
        assert!(reports[0].drained.is_empty(), "{reports:?}");
        assert!(
            reports[0]
                .stopped
                .as_deref()
                .is_some_and(|s| s.contains("already has a row being scored")),
            "{reports:?}"
        );
        assert_eq!(
            scorer.inner.hits.load(Ordering::SeqCst),
            0,
            "nothing else ran"
        );
        assert_eq!(queued_ids(app.clone(), None).await, queued);

        // The other topic's queue is independent of this topic's claim.
        let (st, report) = json_req(
            app.clone(),
            "POST",
            "/v1/admin/proof/queue/drain",
            serde_json::json!({ "topic_id": "dt-no-ib-v0" }),
            Some("op"),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{report}");

        // The in-flight drain gives the head back (it was refused): the next
        // drain scores head then next, in that order.
        state
            .store
            .release_claim(CUSTOM, &in_flight.id)
            .expect("release");
        let (st, report) = json_req(
            app.clone(),
            "POST",
            "/v1/admin/proof/queue/drain",
            serde_json::json!({ "topic_id": CUSTOM, "limit": 5 }),
            Some("op"),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{report}");
        let drained: Vec<String> = report["drained"]
            .as_array()
            .expect("drained")
            .iter()
            .filter_map(|r| r["id"].as_str().map(str::to_owned))
            .collect();
        assert_eq!(drained, queued, "oldest first, one at a time");
        assert_eq!(report["remaining"], 0);
        assert_eq!(state.store.scoring_row(CUSTOM).expect("q"), None);
        assert!(queued_ids(app, None).await.is_empty());
    }

    /// An interrupted drain must not leave its topic busy until a restart:
    /// dropping the claim guard without landing (a panic, a cancelled poll
    /// task, an operator HTTP call cut mid-eval) releases the claim; landing
    /// disarms it; and a stale guard from an earlier drain never drops the
    /// claim a later drain holds.
    #[tokio::test]
    async fn the_claim_guard_releases_on_interruption_and_only_its_own_claim() {
        let state = state_with_deferred_custom(Arc::new(FamilyStub::win(CUSTOM_ID)), true, true);
        let app = proof_router(state.clone());
        for label in ["one", "two"] {
            let (st, created) = json_req(
                app.clone(),
                "POST",
                "/v1/submissions",
                custom_submit(label),
                None,
            )
            .await;
            assert_eq!(st, StatusCode::CREATED, "{created}");
        }
        let store = state.store.clone();

        // Interrupted: the guard drops armed and the head is claimable again.
        let head = store
            .claim_next_queued(CUSTOM)
            .expect("claim")
            .expect("head");
        assert_eq!(store.scoring_row(CUSTOM).expect("q"), Some(head.id.clone()));
        {
            let _guard = ClaimGuard::new(store.clone(), &head);
            assert!(matches!(
                store.claim_next_queued(CUSTOM),
                Err(StoreError::Busy { .. })
            ));
        }
        assert_eq!(
            store.scoring_row(CUSTOM).expect("q"),
            None,
            "released on drop"
        );
        assert!(std::panic::catch_unwind(|| {
            let head = store
                .claim_next_queued(CUSTOM)
                .expect("claim")
                .expect("head");
            let _guard = ClaimGuard::new(store.clone(), &head);
            panic!("scorer blew up mid-eval");
        })
        .is_err());
        assert_eq!(
            store.scoring_row(CUSTOM).expect("q"),
            None,
            "released on unwind"
        );

        // Landed: the guard disarms; the claim was settled by the insert.
        let head = store
            .claim_next_queued(CUSTOM)
            .expect("claim")
            .expect("head");
        let guard = ClaimGuard::new(store.clone(), &head);
        let mut scored = head.clone();
        scored.state = SubmissionState::AwaitingAdmin;
        store.insert(scored).expect("land");
        assert_eq!(store.scoring_row(CUSTOM).expect("q"), None);
        // A later drain takes the next row before the old guard is gone.
        let next = store
            .claim_next_queued(CUSTOM)
            .expect("claim")
            .expect("next");
        assert_ne!(next.id, head.id);
        guard.landed();
        assert_eq!(
            store.scoring_row(CUSTOM).expect("q"),
            Some(next.id.clone()),
            "the later claim is untouched"
        );
        // Even a stale *armed* guard for the old row cannot drop the new claim.
        drop(ClaimGuard::new(store.clone(), &head));
        assert_eq!(store.scoring_row(CUSTOM).expect("q"), Some(next.id));
    }

    /// Deduplication is one atomic store step: two identical submits racing
    /// each other yield one queued row (one **201**, one **200**), and a retry
    /// after the row was drained finds the *scored* row (**200**, its final
    /// state) — never a second queued row and never a second paid run.
    #[tokio::test]
    async fn identical_submits_yield_one_row_for_the_rows_whole_life() {
        let scorer = Arc::new(FamilyStub::win(CUSTOM_ID));
        let state = state_with_deferred_custom(scorer.clone(), true, true);
        let pin = state.pin.clone();
        let app = proof_router(state.clone());

        let (a, b) = tokio::join!(
            json_req(
                app.clone(),
                "POST",
                "/v1/submissions",
                custom_submit("raced"),
                None,
            ),
            json_req(
                app.clone(),
                "POST",
                "/v1/submissions",
                custom_submit("raced"),
                None,
            ),
        );
        let mut codes = [a.0, b.0];
        codes.sort();
        assert_eq!(
            codes,
            [StatusCode::OK, StatusCode::CREATED],
            "{} {}",
            a.1,
            b.1
        );
        assert_eq!(a.1["id"], b.1["id"], "one row: {} {}", a.1, b.1);
        let id = a.1["id"].as_str().expect("id").to_owned();
        assert_eq!(
            queued_ids(app.clone(), None).await,
            std::slice::from_ref(&id)
        );

        republish_custom(app.clone(), &pin, None).await;
        let (st, report) = json_req(
            app.clone(),
            "POST",
            "/v1/admin/proof/queue/drain",
            serde_json::json!({ "topic_id": CUSTOM }),
            Some("op"),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{report}");
        assert_eq!(report["drained"][0]["id"], id, "{report}");
        assert_eq!(scorer.inner.hits.load(Ordering::SeqCst), 1);

        // Defer again and retry the same artefact: the scored row answers.
        republish_custom(app.clone(), &pin, Some("true")).await;
        let (st, again) = json_req(
            app.clone(),
            "POST",
            "/v1/submissions",
            custom_submit("raced"),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{again}");
        assert_eq!(again["id"], id, "{again}");
        assert_eq!(again["state"], "champion", "{again}");
        assert_eq!(again["eligible"], true, "{again}");
        assert!(
            again["detail"]
                .as_str()
                .unwrap_or_default()
                .contains("already submitted"),
            "{again}"
        );
        assert!(
            queued_ids(app.clone(), None).await.is_empty(),
            "no second row"
        );
        assert_eq!(status_of(app.clone()).await["queued_submissions"], 0);
        republish_custom(app.clone(), &pin, None).await;
        assert!(state.drain_ready_queues().await.is_empty());
        assert_eq!(
            scorer.inner.hits.load(Ordering::SeqCst),
            1,
            "never scored twice"
        );
    }
    // ----- miner BYOK: `env` on the submit body -----

    /// The variable the BYOK topics below declare. A name, never a vendor:
    /// which variable a topic wants is topic data, not a constant of this
    /// repository.
    const BYOK: &str = "MINER_PROVIDED_API_KEY";
    /// The value a miner posts in these tests. Asserted absent from every
    /// public answer.
    const BYOK_VALUE: &str = "miner-supplied-value-not-a-real-key";

    fn byok_submit(label: &str, env: Option<&serde_json::Value>) -> serde_json::Value {
        let mut extra = serde_json::json!({
            "topic_id": CUSTOM,
            "artifact_uri": format!("https://example.invalid/{label}.tar"),
        });
        if let Some(env) = env {
            extra["env"] = env.clone();
        }
        submit_body(label, &extra)
    }

    /// A topic that declares `miner_byok` refuses a submission that omits it
    /// — with **no row**, **no rent**, and, because `env` is outside the
    /// signed payload, **without spending the signed `submit_nonce`**: the
    /// same body plus the key is accepted right after.
    #[tokio::test]
    async fn a_topic_that_asks_for_a_key_refuses_a_body_without_one() {
        let scorer = Arc::new(FamilyStub::win(CUSTOM_ID));
        let app = proof_router(state_with_custom_params(
            scorer.clone(),
            false,
            true,
            &[(proof_canon::PARAM_MINER_BYOK, BYOK)],
        ));

        let body = byok_submit("a", None);
        let (st, out) = json_req(app.clone(), "POST", "/v1/submissions", body.clone(), None).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{out}");
        let why = out["error"].as_str().unwrap_or_default();
        assert!(why.contains(BYOK) && why.contains("required"), "{why}");
        assert!(why.contains("miner_byok"), "names the knob to read: {why}");
        assert_eq!(scorer.inner.hits.load(Ordering::SeqCst), 0, "nothing ran");
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
            list["items"].as_array().expect("items").is_empty(),
            "no row"
        );

        // Same signature, same nonce, now with the key: accepted.
        let mut retry = body;
        retry["env"] = serde_json::json!({ BYOK: BYOK_VALUE });
        let (st, out) = json_req(app, "POST", "/v1/submissions", retry, None).await;
        assert_eq!(
            st,
            StatusCode::CREATED,
            "the refusal must not have burnt the nonce: {out}"
        );
    }

    /// Only what the signed topic declares gets through, and the refusal says
    /// what the topic does accept without echoing what was sent.
    #[tokio::test]
    async fn an_undeclared_variable_is_refused_by_name() {
        let scorer = Arc::new(FamilyStub::win(CUSTOM_ID));
        let app = proof_router(state_with_custom_params(
            scorer.clone(),
            false,
            true,
            &[(proof_canon::PARAM_MINER_BYOK, BYOK)],
        ));
        for env in [
            serde_json::json!({ BYOK: BYOK_VALUE, "SOME_OTHER_TOKEN": "another-value" }),
            serde_json::json!({ "SOME_OTHER_TOKEN": "another-value" }),
            serde_json::json!({ "PATH": "/evil/bin" }),
            serde_json::json!({ "PROOF_SECRETS_DIR": "/tmp/evil" }),
            serde_json::json!({ BYOK: "" }),
        ] {
            let (st, out) = json_req(
                app.clone(),
                "POST",
                "/v1/submissions",
                byok_submit("a", Some(&env)),
                None,
            )
            .await;
            assert_eq!(st, StatusCode::BAD_REQUEST, "{env} -> {out}");
            let why = out.to_string();
            assert!(
                !why.contains("another-value") && !why.contains("/evil/bin"),
                "a refusal never echoes what was sent: {why}"
            );
        }
        assert_eq!(scorer.inner.hits.load(Ordering::SeqCst), 0);

        // A topic that declares nothing accepts nothing, and says so.
        let silent = proof_router(state_with_deferred_custom(
            Arc::new(FamilyStub::win(CUSTOM_ID)),
            false,
            true,
        ));
        let (st, out) = json_req(
            silent,
            "POST",
            "/v1/submissions",
            byok_submit("a", Some(&serde_json::json!({ BYOK: BYOK_VALUE }))),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{out}");
        assert!(
            out["error"].as_str().unwrap_or_default().contains("(none)"),
            "{out}"
        );
    }

    /// The happy path: the declared value reaches the scorer that runs the
    /// miner's code, and appears in no answer the host serves.
    #[tokio::test]
    async fn the_declared_key_reaches_the_scorer_and_never_a_public_answer() {
        let scorer = Arc::new(FamilyStub::win(CUSTOM_ID));
        let app = proof_router(state_with_custom_params(
            scorer.clone(),
            false,
            true,
            &[(proof_canon::PARAM_MINER_BYOK, BYOK)],
        ));
        let (st, out) = json_req(
            app.clone(),
            "POST",
            "/v1/submissions",
            byok_submit("a", Some(&serde_json::json!({ BYOK: BYOK_VALUE }))),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{out}");
        let id = out["id"].as_str().expect("id").to_owned();

        let envs = scorer.inner.envs();
        assert_eq!(envs.len(), 1, "one scored run");
        assert_eq!(
            envs[0].iter().collect::<Vec<_>>(),
            vec![(BYOK, BYOK_VALUE)],
            "the miner's own key reached the run"
        );

        for uri in [
            "/v1/submissions".to_owned(),
            format!("/v1/submissions/{id}"),
            "/v1/status".to_owned(),
            "/v1/proof/topics".to_owned(),
        ] {
            let (st, body) = json_req(app.clone(), "GET", &uri, serde_json::json!({}), None).await;
            assert_eq!(st, StatusCode::OK, "{uri}");
            let dump = body.to_string();
            assert!(!dump.contains(BYOK_VALUE), "{uri} leaked the key: {dump}");
        }
        // The topic document is public and names the *variable*, never a value.
        let (_, topics) = json_req(
            app,
            "GET",
            &format!("/v1/proof/topics/{CUSTOM}"),
            serde_json::json!({}),
            None,
        )
        .await;
        let dump = topics.to_string();
        assert!(dump.contains(BYOK), "the topic declares the name: {dump}");
        assert!(!dump.contains(BYOK_VALUE), "{dump}");
    }

    /// A deferring topic still takes the key: it waits beside the `queued`
    /// row (never on it) and is handed to the drain that finally scores it.
    #[tokio::test]
    async fn a_queued_row_keeps_its_key_for_the_drain() {
        let scorer = Arc::new(FamilyStub::win(CUSTOM_ID));
        let state = state_with_custom_params(
            scorer.clone(),
            true,
            true,
            &[(proof_canon::PARAM_MINER_BYOK, BYOK)],
        );
        let store = state.store.clone();
        let p = state.pin.clone();
        let app = proof_router(state);

        let (st, out) = json_req(
            app.clone(),
            "POST",
            "/v1/submissions",
            byok_submit("a", Some(&serde_json::json!({ BYOK: BYOK_VALUE }))),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{out}");
        assert_eq!(out["state"], "queued", "{out}");
        let digest = out["submission_digest"]
            .as_str()
            .expect("digest")
            .to_owned();
        assert_eq!(scorer.inner.hits.load(Ordering::SeqCst), 0, "nothing ran");

        // Held beside the row, not on it.
        assert_eq!(
            store
                .miner_env(&digest)
                .expect("stash")
                .iter()
                .collect::<Vec<_>>(),
            vec![(BYOK, BYOK_VALUE)]
        );
        let (_, list) = json_req(
            app.clone(),
            "GET",
            "/v1/submissions",
            serde_json::json!({}),
            None,
        )
        .await;
        assert!(!list.to_string().contains(BYOK_VALUE), "{list}");

        // Lift the flag and drain: the key the miner sent is what scores.
        let mut relisted = custom_topic_with_defer(&p, None);
        relisted
            .constraints
            .params
            .insert(proof_canon::PARAM_MINER_BYOK.to_owned(), BYOK.to_owned());
        relisted.signature = relisted.sign_with(&sk()).expect("sign");
        let (st, body) = json_req(
            app.clone(),
            "POST",
            "/v1/admin/proof/topics",
            serde_json::to_value(&relisted).expect("json"),
            Some("op"),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{body}");
        let (st, report) = json_req(
            app,
            "POST",
            "/v1/admin/proof/queue/drain",
            serde_json::json!({ "topic_id": CUSTOM, "limit": 4 }),
            Some("op"),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{report}");
        assert_eq!(report["drained"].as_array().expect("drained").len(), 1);
        let envs = scorer.inner.envs();
        assert_eq!(
            envs.last().map(|e| e.iter().collect::<Vec<_>>()),
            Some(vec![(BYOK, BYOK_VALUE)]),
            "the drain scored with the key the miner posted"
        );
        assert!(!report.to_string().contains(BYOK_VALUE), "{report}");
        assert!(
            store.miner_env(&digest).expect("stash").is_empty(),
            "a scored row does not keep the key"
        );
    }

    /// Staging 503 after BYOK is held rolls the key back: no row, vault empty.
    #[tokio::test]
    async fn a_failed_artefact_stage_does_not_orphan_byok() {
        let scorer = Arc::new(FamilyStub::win(CUSTOM_ID));
        let pid = std::process::id();
        let byok_root = std::env::temp_dir().join(format!("proof-byok-orphan-{pid}"));
        let art_file = std::env::temp_dir().join(format!("proof-art-notdir-{pid}"));
        let _ = std::fs::remove_dir_all(&byok_root);
        let _ = std::fs::remove_file(&art_file);
        std::fs::write(&art_file, b"not-a-dir").expect("file not a dir");
        let mut state = state_with_custom_params(
            scorer.clone(),
            false,
            true,
            &[(proof_canon::PARAM_MINER_BYOK, BYOK)],
        );
        state.store = state
            .store
            .with_miner_byok_vault(proof_store::MinerEnvVault::at(&byok_root))
            .with_artefact_vault(proof_store::ArtefactVault::at(&art_file));
        let app = proof_router(state);
        let bytes = recipe_tar();
        let artifact_digest = hex::encode(Sha256::digest(&bytes));
        let fields = submit_body(
            "stage-fail",
            &serde_json::json!({
                "topic_id": CUSTOM,
                "artifact_digest": artifact_digest,
                "env": { BYOK: BYOK_VALUE },
            }),
        );
        let (st, out) = multipart_req(app.clone(), &fields, Some(&bytes)).await;
        assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE, "{out}");
        assert_eq!(scorer.inner.hits.load(Ordering::SeqCst), 0, "nothing ran");
        let (st, list) = json_req(app, "GET", "/v1/submissions", serde_json::json!({}), None).await;
        assert_eq!(st, StatusCode::OK);
        assert!(
            list["items"].as_array().expect("items").is_empty(),
            "no row: {list}"
        );
        let leftover =
            std::fs::read_dir(&byok_root).map_or(0, |d| d.filter_map(Result::ok).count());
        assert_eq!(leftover, 0, "BYOK must not remain after a staging 503");
        let _ = std::fs::remove_dir_all(&byok_root);
        let _ = std::fs::remove_file(&art_file);
    }

    /// Live evaluate of an upload now scores. BYOK that staging wrote must
    /// still be dropped after the run (spent), not left on disk.
    #[tokio::test]
    async fn live_upload_scores_and_forgets_byok() {
        let scorer = Arc::new(FamilyStub::win(CUSTOM_ID));
        let pid = std::process::id();
        let byok_root = std::env::temp_dir().join(format!("proof-byok-live-{pid}"));
        let _ = std::fs::remove_dir_all(&byok_root);
        let mut state = state_with_custom_params(
            scorer.clone(),
            false,
            true,
            &[(proof_canon::PARAM_MINER_BYOK, BYOK)],
        );
        state.store = state
            .store
            .with_miner_byok_vault(proof_store::MinerEnvVault::at(&byok_root));
        let app = proof_router(state);
        let bytes = recipe_tar();
        let artifact_digest = hex::encode(Sha256::digest(&bytes));
        let fields = submit_body(
            "byok-live-upload",
            &serde_json::json!({
                "topic_id": CUSTOM,
                "artifact_digest": artifact_digest,
                "env": { BYOK: BYOK_VALUE },
            }),
        );
        let (st, created) = multipart_req(app.clone(), &fields, Some(&bytes)).await;
        assert_eq!(st, StatusCode::CREATED, "{created}");
        let (st, list) = json_req(app, "GET", "/v1/submissions", serde_json::json!({}), None).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(
            list["items"].as_array().expect("items").len(),
            1,
            "upload-only evaluate persists a row: {list}"
        );
        let leftover =
            std::fs::read_dir(&byok_root).map_or(0, |d| d.filter_map(Result::ok).count());
        assert_eq!(leftover, 0, "BYOK must not remain after a scored run");
        let _ = std::fs::remove_dir_all(&byok_root);
        assert_eq!(scorer.inner.hits.load(Ordering::SeqCst), 1);
        assert_eq!(
            scorer.inner.tars(),
            vec![Some(bytes.len())],
            "evaluate received the vault bytes"
        );
    }

    /// A deferred retry of the same artefact (new nonce, same frozen digest)
    /// must not wipe the BYOK the queued row already holds when this retry's
    /// upload cannot be staged.
    #[tokio::test]
    async fn a_retry_staging_503_does_not_erase_queued_byok() {
        let scorer = Arc::new(FamilyStub::win(CUSTOM_ID));
        let pid = std::process::id();
        let art_file = std::env::temp_dir().join(format!("proof-art-retry-{pid}"));
        let _ = std::fs::remove_file(&art_file);
        std::fs::write(&art_file, b"not-a-dir").expect("file not a dir");
        let mut state = state_with_custom_params(
            scorer.clone(),
            true,
            true,
            &[(proof_canon::PARAM_MINER_BYOK, BYOK)],
        );
        state.store = state
            .store
            .with_artefact_vault(proof_store::ArtefactVault::at(&art_file));
        let store = state.store.clone();
        let app = proof_router(state);
        let bytes = recipe_tar();
        let artifact_digest = hex::encode(Sha256::digest(&bytes));
        let env = serde_json::json!({ BYOK: BYOK_VALUE });
        let (st, out) = json_req(
            app.clone(),
            "POST",
            "/v1/submissions",
            submit_body(
                "queued",
                &serde_json::json!({
                    "topic_id": CUSTOM,
                    "artifact_digest": artifact_digest,
                    "artifact_uri": "https://example.invalid/queued.tar",
                    "env": env,
                }),
            ),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{out}");
        assert_eq!(out["state"], "queued", "{out}");
        let digest = out["submission_digest"]
            .as_str()
            .expect("digest")
            .to_owned();
        assert_eq!(
            store
                .miner_env(&digest)
                .expect("held")
                .iter()
                .collect::<Vec<_>>(),
            vec![(BYOK, BYOK_VALUE)]
        );

        let retry = submit_body(
            "retry",
            &serde_json::json!({
                "topic_id": CUSTOM,
                "artifact_digest": artifact_digest,
                "env": env,
            }),
        );
        let (st, out) = multipart_req(app.clone(), &retry, Some(&bytes)).await;
        assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE, "{out}");
        assert_eq!(
            store
                .miner_env(&digest)
                .expect("kept")
                .iter()
                .collect::<Vec<_>>(),
            vec![(BYOK, BYOK_VALUE)],
            "retry staging 503 must not drop the queued row's key"
        );
        let (st, list) = json_req(app, "GET", "/v1/submissions", serde_json::json!({}), None).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(list["items"].as_array().expect("items").len(), 1, "{list}");
        let _ = std::fs::remove_file(&art_file);
    }
    /// A topic that runs on the miner's own key and a host that no longer
    /// holds the one that was submitted (a restart with nothing on disk, an
    /// operator who cleared the vault) is a **503** with the row untouched.
    /// The alternative — scoring the miner's evaluation on the operator's
    /// credentials — is what this whole path exists to prevent, so it fails
    /// closed and the drain leaves the row `queued`.
    #[tokio::test]
    async fn a_lost_key_refuses_the_run_instead_of_spending_the_owners() {
        let scorer = Arc::new(FamilyStub::win(CUSTOM_ID));
        let state = state_with_custom_params(
            scorer.clone(),
            true,
            true,
            &[(proof_canon::PARAM_MINER_BYOK, BYOK)],
        );
        let store = state.store.clone();
        let p = state.pin.clone();
        let app = proof_router(state);

        let (st, out) = json_req(
            app.clone(),
            "POST",
            "/v1/submissions",
            byok_submit("a", Some(&serde_json::json!({ BYOK: BYOK_VALUE }))),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{out}");
        let digest = out["submission_digest"]
            .as_str()
            .expect("digest")
            .to_owned();
        let id = out["id"].as_str().expect("id").to_owned();

        // The key is gone before the drain reaches it.
        store.forget_miner_env(&digest).expect("cleared");
        let mut relisted = custom_topic_with_defer(&p, None);
        relisted
            .constraints
            .params
            .insert(proof_canon::PARAM_MINER_BYOK.to_owned(), BYOK.to_owned());
        relisted.signature = relisted.sign_with(&sk()).expect("sign");
        let (st, body) = json_req(
            app.clone(),
            "POST",
            "/v1/admin/proof/topics",
            serde_json::to_value(&relisted).expect("json"),
            Some("op"),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{body}");

        let (st, report) = json_req(
            app.clone(),
            "POST",
            "/v1/admin/proof/queue/drain",
            serde_json::json!({ "topic_id": CUSTOM, "limit": 4 }),
            Some("op"),
        )
        .await;
        assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE, "{report}");
        let why = report.to_string();
        assert!(why.contains(BYOK), "names what is missing: {why}");
        assert!(why.contains("operator key is never substituted"), "{why}");
        assert_eq!(scorer.inner.hits.load(Ordering::SeqCst), 0, "nothing ran");
        assert_eq!(
            queued_ids(app.clone(), Some(CUSTOM)).await,
            vec![id.clone()],
            "the row is still queued, nothing was rented"
        );

        // Scoring the head directly refuses the same way and releases it.
        let (st, one) = json_req(
            app.clone(),
            "POST",
            &format!("/v1/admin/proof/submissions/{id}/score"),
            serde_json::json!({}),
            Some("op"),
        )
        .await;
        assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE, "{one}");
        assert_eq!(queued_ids(app, Some(CUSTOM)).await, vec![id]);
    }
}
