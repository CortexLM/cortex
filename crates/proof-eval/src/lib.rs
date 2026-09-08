//! Proof eval loop: freeze digest → unseal holdout → RLM agent + harness.
//!
//! The control plane only ever boots a digest-pinned `proof-eval` image. The
//! RLM agent (PrimeIntellect-style tool loop) lives *inside* that image: this
//! crate holds a handle to it and never invents a verdict. Without
//! `PROOF_FORCE_SIM=1` a host needs a `sha256:` pin **and** a wired harvest,
//! and refuses until it has both. Sim is reported on `/v1/status` and is
//! never a fallback for an empty digest or a down agent.
//!
//! The agent never sees holdout records. The request it gets is the claim, the
//! code, the public split, and the constraints. Holdout NLL, throughput, and
//! pass are filled by the harness from the same image's measurement sidecar.

#![forbid(unsafe_code)]
#![allow(
    clippy::missing_errors_doc,
    clippy::doc_markdown,
    clippy::module_name_repetitions,
    clippy::cast_precision_loss,
    clippy::must_use_candidate,
    clippy::too_many_arguments
)]

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use async_trait::async_trait;
use prism_lium_types::{EvalReceipt, NoScoreGate};
use proof_executor::{
    executor_plan, require_open_executor, EvalExecutorOffer, ExecutorOfferError, ExecutorPlan,
    HarvestOverrides,
};
use proof_score::{AgentVerdict, HarnessMetrics, ProofCheatCode, ProofKind, SealedBaseline};
use proof_store::ArtifactManifest;
use proof_task::{
    canonical_json, contamination, require_open_offer, resolve_inference, HoldoutRecord,
    HoldoutSplit, InferenceOffer, MetricDirection, MetricFamily, OfferError, ProofPin,
    TopicDocument, BASELINE_DOMAIN,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Where a Proof eval actually runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvalBackend {
    /// Digest-pinned eval image on a Lium pod (production default).
    Lium,
    /// Deterministic offline scorer. CI / local opt-in only, never a fallback.
    Sim,
}

/// True when the operator explicitly opted into sim scoring.
#[must_use]
pub fn force_sim() -> bool {
    matches!(
        std::env::var("PROOF_FORCE_SIM")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "yes"
    )
}

/// True when this host opted into sim (`PROOF_FORCE_SIM`).
///
/// Under [`EvalBackend::Sim`] a sealed baseline scores with
/// [`sim_win_document`] (harness relative to the seal). Skill-only
/// [`sim_document`] cannot beat a real ~0.29 NLL seal. The Lium path never
/// uses either helper. `PROOF_SIM_STUB_WIN` is a leftover no-op.
#[must_use]
pub fn sim_stub_win() -> bool {
    force_sim()
}

/// Resolve the scoring backend for this host. Sim is never implicit.
#[must_use]
pub fn resolve_eval_backend() -> EvalBackend {
    if force_sim() {
        EvalBackend::Sim
    } else {
        EvalBackend::Lium
    }
}

/// Eval errors.
#[derive(Debug, Error)]
pub enum EvalError {
    /// Holdout was requested before the digest freeze, or none are loaded.
    #[error("holdout still sealed")]
    HoldoutSealed,
    /// No `open` topic with a sealed baseline on this host.
    #[error("no open sealed topic; refuse scoring")]
    NoOpenTopic,
    /// Integrity gate failed.
    #[error("integrity: {0}")]
    Integrity(String),
    /// Provider / eval-image failure.
    #[error("backend: {0}")]
    Backend(String),
    /// A live run was asked for without a digest-pinned eval image.
    #[error("eval image digest not pinned; refuse live scoring (PROOF_FORCE_SIM=1 is CI only)")]
    EvalImageUnpinned,
    /// A live run reached the in-process scorer. It must not silently sim.
    #[error("live proof eval is driven by the digest-pinned proof-eval image; no in-process sim")]
    LiveHarvestUnavailable,
    /// The agent returned nothing parseable. Not a zero — a 503.
    #[error("no agent verdict: {0}")]
    NoVerdict(String),
    /// The operator-recorded baseline does not match the topic/pin.
    #[error("recorded baseline: {0}")]
    Baseline(String),
    /// No live RLM judge InferenceOffer on this host.
    #[error("inference offer missing; refuse scoring")]
    InferenceOfferMissing,
    /// Live InferenceOffer is closed.
    #[error("inference offer is closed; refuse scoring")]
    InferenceOfferClosed,
    /// Live InferenceOffer failed pin validation.
    #[error("inference offer: {0}")]
    InferenceOffer(String),
    /// Live open judge offer needs auth and the key file is missing/unreadable.
    #[error("inference API key missing; refuse scoring")]
    InferenceAuthMissing,
    /// Live score needs operator-staged local measurement weights (no HF bake).
    #[error("PROOF_PROXY_MODEL_DIR missing or empty; refuse live scoring (no HF bake)")]
    ProxyModelMissing,
    /// Live score needs operator-staged holdout shard bytes.
    #[error("PROOF_HOLDOUT_STORE missing or incomplete; refuse scoring")]
    HoldoutStoreMissing,
    /// No live `EvalExecutorOffer` on this host (Lium path).
    #[error("eval executor offer missing; refuse scoring")]
    ExecutorOfferMissing,
    /// Live `EvalExecutorOffer` is closed.
    #[error("eval executor offer is closed; refuse scoring")]
    ExecutorOfferClosed,
    /// Live `EvalExecutorOffer` failed pin validation or cannot serve the topic.
    #[error("eval executor offer: {0}")]
    ExecutorOffer(String),
    /// The eval run was cut at the executor proof deadline. Not a zero — a 503
    /// whose body carries the pod's log tail.
    #[error("proof deadline of {deadline_s}s exceeded; stdout_tail: {stdout_tail}")]
    ProofDeadlineExceeded {
        /// Deadline the run was held to (offer, topic tighten, operator override).
        deadline_s: u64,
        /// Last bytes of pod stdout (run log tail), for the operator.
        stdout_tail: String,
    },
    /// A custom metric family has no registered runner on this host.
    #[error("custom metric {custom_id:?} has no registered runner ({detail}); refuse scoring")]
    RunnerUnwired {
        /// Custom metric id the topic names.
        custom_id: String,
        /// Which piece is missing (never a secret).
        detail: String,
    },
}

/// Map an executor refusal onto the eval error the HTTP layer answers 503 with.
pub fn map_executor_err(e: ExecutorOfferError) -> EvalError {
    match e {
        ExecutorOfferError::Missing => EvalError::ExecutorOfferMissing,
        ExecutorOfferError::Closed => EvalError::ExecutorOfferClosed,
        other => EvalError::ExecutorOffer(other.to_string()),
    }
}

/// Schema version of the metrics+verdict document the eval image emits.
pub const PROOF_METRICS_SCHEMA: u32 = 1;

/// Custom metric ids with a registered runner, as reported by the host's
/// live scorer. There is **no** compiled-in list: a topic mints its
/// `custom_id`, a runner registered under that id makes it scorable, and an
/// open topic whose id is absent here answers 503.
#[must_use]
pub fn registered_custom(live: Option<&dyn LiveScorer>) -> Vec<String> {
    live.map(LiveScorer::custom_ids).unwrap_or_default()
}

/// Borrow a registered-id list as the `&[&str]` the topic validator takes.
#[must_use]
pub fn custom_ids_ref(ids: &[String]) -> Vec<&str> {
    ids.iter().map(String::as_str).collect()
}

/// The document `proof-eval` must print for one scored artifact.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProofEvalDocument {
    /// Must equal [`PROOF_METRICS_SCHEMA`].
    pub schema_version: u32,
    /// Frozen submission digest the run was asked for.
    pub submission_digest: String,
    /// Artifact digest the run was asked for.
    pub artifact_digest: String,
    /// Topic the run was asked for.
    pub topic_id: String,
    /// Eval image digest that produced these numbers.
    pub eval_image_digest: String,
    /// Holdout commitment measured against.
    pub holdout_commitment: String,
    /// Agent envelope. Missing → [`EvalError::NoVerdict`].
    pub agent: AgentVerdict,
    /// Harness-owned metric values. The agent must not fill holdout NLL.
    pub harness: HarnessMetrics,
}

impl ProofEvalDocument {
    /// Parse a metrics document emitted by the eval image.
    pub fn from_json(body: &str) -> Result<Self, EvalError> {
        serde_json::from_str(body).map_err(|e| EvalError::NoVerdict(e.to_string()))
    }

    /// Bind the document to the run that was requested.
    pub fn verify(
        &self,
        pin: &ProofPin,
        topic: &TopicDocument,
        frozen_digest: &str,
        artifact_digest: &str,
    ) -> Result<(), EvalError> {
        if self.schema_version != PROOF_METRICS_SCHEMA {
            return Err(EvalError::Baseline(format!(
                "metrics schema_version {}, expected {PROOF_METRICS_SCHEMA}",
                self.schema_version
            )));
        }
        if self.submission_digest.trim() != frozen_digest.trim() {
            return Err(EvalError::Baseline(
                "metrics submission_digest is not the frozen run".into(),
            ));
        }
        if !self
            .artifact_digest
            .trim()
            .eq_ignore_ascii_case(artifact_digest.trim())
        {
            return Err(EvalError::Baseline(
                "metrics artifact_digest is not the scored artifact".into(),
            ));
        }
        if self.topic_id.trim() != topic.id {
            return Err(EvalError::Baseline(
                "metrics topic_id is not the scored topic".into(),
            ));
        }
        if self.eval_image_digest.trim() != pin.eval_image_digest.trim() {
            return Err(EvalError::Baseline(format!(
                "measured by eval image {:?}, pin is {:?}",
                self.eval_image_digest, pin.eval_image_digest
            )));
        }
        if !self
            .holdout_commitment
            .trim()
            .eq_ignore_ascii_case(topic.holdout_commitment.trim())
        {
            return Err(EvalError::Baseline(
                "holdout commitment does not match the topic".into(),
            ));
        }
        if self.agent.topic_id.trim() != topic.id {
            return Err(EvalError::NoVerdict(
                "agent verdict topic_id mismatch".into(),
            ));
        }
        if self.agent.family != topic.metric.family {
            return Err(EvalError::NoVerdict("agent verdict family mismatch".into()));
        }
        Ok(())
    }
}

/// Handle to the digest-pinned eval image's harvest.
#[async_trait]
pub trait LiveScorer: Send + Sync {
    /// Resolve what one rent is allowed to do for `topic` on `executor`:
    /// template, exact width, deadline, and the commitment of that resolved
    /// configuration. The harvest applies its `PROOF_HARVEST_*` overrides
    /// here; the default applies none. Called before [`Self::score`] so the
    /// caller can persist the plan the run was actually held to.
    fn plan(
        &self,
        pin: &ProofPin,
        topic: &TopicDocument,
        executor: &EvalExecutorOffer,
    ) -> Result<ExecutorPlan, EvalError> {
        executor_plan(pin, Some(executor), topic, &HarvestOverrides::default())
            .map_err(map_executor_err)
    }

    /// Score one artifact on one topic's verified holdout.
    ///
    /// `offer` is the RLM judge the image calls; `plan` is the resolved `1x`
    /// rent ([`Self::plan`]) the image is run under. Both are host state.
    /// `artifact_uri` is the miner-supplied locator of the bytes behind
    /// `artifact_digest` (the runner fetches and digest-checks them); the
    /// digest alone is not enough to retrieve an artefact. `declared_flops`
    /// is the miner's declaration (already `<=` the topic budget at intake):
    /// a scorer that measures usage must fail a run that exceeds it.
    #[allow(clippy::too_many_arguments)]
    async fn score(
        &self,
        pin: &ProofPin,
        topic: &TopicDocument,
        offer: &InferenceOffer,
        plan: &ExecutorPlan,
        frozen_digest: &str,
        artifact_digest: &str,
        artifact_uri: Option<&str>,
        declared_flops: u64,
        holdout: &[HoldoutRecord],
        claim: &str,
    ) -> Result<ProofEvalDocument, EvalError>;

    /// Whether this scorer could run right now.
    fn ready(&self) -> Result<(), EvalError> {
        Ok(())
    }

    /// Whether this scorer could score `topic` right now. A custom family
    /// whose runner is not registered refuses here, before any row or rent.
    fn ready_for_topic(&self, topic: &TopicDocument) -> Result<(), EvalError> {
        let _ = topic;
        self.ready()
    }

    /// Custom metric ids this scorer has a registered runner for. Default:
    /// none — the digest-pinned harvest scores `nll` / `throughput` only.
    fn custom_ids(&self) -> Vec<String> {
        Vec::new()
    }

    /// Whether the run for `submission_digest` on `topic` should be crowned
    /// champion automatically.
    ///
    /// `pass` is the harness verdict, `primary` its primary metric, `bar` the
    /// current novelty bar (sealed baseline vs reigning champion,
    /// direction-aware). Default: never — promotion stays an operator action.
    async fn auto_promote(
        &self,
        topic: &TopicDocument,
        submission_digest: &str,
        pass: bool,
        primary: Option<f64>,
        bar: Option<f64>,
    ) -> bool {
        let _ = (topic, submission_digest, pass, primary, bar);
        false
    }

    /// Called once the scored row is persisted and has its `pf_…` id, so a
    /// scorer can write per-submission artefacts and promotion events.
    /// Failures are the scorer's to log; the row is already final.
    async fn on_persisted(
        &self,
        topic_id: &str,
        submission_digest: &str,
        submission_id: &str,
        promoted: bool,
    ) {
        let _ = (topic_id, submission_digest, submission_id, promoted);
    }
}

/// Route scoring by metric family: `custom` topics go to the registered
/// custom-family scorer (which resolves the runner by `custom_id`, fail-closed);
/// `nll` / `throughput` go to the default digest-pinned harvest. Planning,
/// readiness, promotion, and artefact hooks follow the same route, so an
/// unregistered custom id can never fall back to the harvest, and — on a
/// host with no harvest ([`Self::custom_only`]) — no `nll` / `throughput`
/// topic can ever reach the custom scorer or an in-process sim.
pub struct FamilyMux {
    /// Digest-pinned harvest for `nll` / `throughput`. `None` on a host with
    /// no Lium harvest: those families refuse per topic.
    default: Option<Arc<dyn LiveScorer>>,
    custom: Option<Arc<dyn LiveScorer>>,
}

impl FamilyMux {
    /// Mux over the default harvest with no custom-family scorer: every
    /// custom topic is unscorable (503).
    #[must_use]
    pub fn new(default: Arc<dyn LiveScorer>) -> Self {
        Self {
            default: Some(default),
            custom: None,
        }
    }

    /// Mux with **no** default harvest: the `custom` family routes to
    /// `scorer`; every `nll` / `throughput` topic refuses with
    /// [`EvalError::LiveHarvestUnavailable`] at readiness, plan, and score
    /// (503, no row, no rent). For a host whose topic-VM orchestrator and
    /// custom runners are wired but whose Lium harvest is not.
    #[must_use]
    pub fn custom_only(scorer: Arc<dyn LiveScorer>) -> Self {
        Self {
            default: None,
            custom: Some(scorer),
        }
    }

    /// Route the whole `custom` family to `scorer`.
    #[must_use]
    pub fn with_custom_family(mut self, scorer: Arc<dyn LiveScorer>) -> Self {
        self.custom = Some(scorer);
        self
    }

    fn route(&self, topic: &TopicDocument) -> Result<&dyn LiveScorer, EvalError> {
        if topic.metric.family != MetricFamily::Custom {
            return self
                .default
                .as_deref()
                .ok_or(EvalError::LiveHarvestUnavailable);
        }
        self.custom
            .as_deref()
            .ok_or_else(|| EvalError::RunnerUnwired {
                custom_id: topic.metric.custom_id.trim().to_owned(),
                detail: "no custom-family scorer on this host".into(),
            })
    }
}

#[async_trait]
impl LiveScorer for FamilyMux {
    fn plan(
        &self,
        pin: &ProofPin,
        topic: &TopicDocument,
        executor: &EvalExecutorOffer,
    ) -> Result<ExecutorPlan, EvalError> {
        self.route(topic)?.plan(pin, topic, executor)
    }

    async fn score(
        &self,
        pin: &ProofPin,
        topic: &TopicDocument,
        offer: &InferenceOffer,
        plan: &ExecutorPlan,
        frozen_digest: &str,
        artifact_digest: &str,
        artifact_uri: Option<&str>,
        declared_flops: u64,
        holdout: &[HoldoutRecord],
        claim: &str,
    ) -> Result<ProofEvalDocument, EvalError> {
        self.route(topic)?
            .score(
                pin,
                topic,
                offer,
                plan,
                frozen_digest,
                artifact_digest,
                artifact_uri,
                declared_flops,
                holdout,
                claim,
            )
            .await
    }

    /// Host-wide gate: the default harvest's readiness. With no harvest
    /// there is no host-wide blocker — the host still scores its `custom`
    /// family — and the `nll` / `throughput` refusal lands per topic in the
    /// route, where `ready_for_topic`, `plan`, and `score` look.
    fn ready(&self) -> Result<(), EvalError> {
        self.default.as_deref().map_or(Ok(()), LiveScorer::ready)
    }

    fn ready_for_topic(&self, topic: &TopicDocument) -> Result<(), EvalError> {
        self.route(topic)?.ready_for_topic(topic)
    }

    fn custom_ids(&self) -> Vec<String> {
        self.custom
            .as_deref()
            .map_or_else(Vec::new, LiveScorer::custom_ids)
    }

    async fn auto_promote(
        &self,
        topic: &TopicDocument,
        submission_digest: &str,
        pass: bool,
        primary: Option<f64>,
        bar: Option<f64>,
    ) -> bool {
        match self.route(topic) {
            Ok(s) => {
                s.auto_promote(topic, submission_digest, pass, primary, bar)
                    .await
            }
            Err(_) => false,
        }
    }

    /// The persist hook arrives without the document, so both routes are
    /// told; only the scorer holding a pending bundle for this digest acts.
    async fn on_persisted(
        &self,
        topic_id: &str,
        submission_digest: &str,
        submission_id: &str,
        promoted: bool,
    ) {
        if let Some(d) = &self.default {
            d.on_persisted(topic_id, submission_digest, submission_id, promoted)
                .await;
        }
        if let Some(c) = &self.custom {
            c.on_persisted(topic_id, submission_digest, submission_id, promoted)
                .await;
        }
    }
}

/// Operator-recorded sealed baseline for one topic.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BaselineMeasurement {
    /// Eval image digest that produced these numbers.
    pub eval_image_digest: String,
    /// Topic id measured.
    pub topic_id: String,
    /// Holdout commitment measured against.
    pub holdout_commitment: String,
    /// Mean holdout NLL.
    pub holdout_nll: f64,
    /// Per-split NLL.
    pub split_nll: BTreeMap<String, f64>,
    /// Throughput primary.
    pub tokens_per_sec: Option<f64>,
    /// Latency primary.
    pub step_latency_ms: Option<f64>,
    /// Custom value.
    pub custom_value: Option<f64>,
}

impl BaselineMeasurement {
    /// Parse an operator baseline file body.
    pub fn from_json(body: &str) -> Result<Self, EvalError> {
        serde_json::from_str(body).map_err(|e| EvalError::Baseline(e.to_string()))
    }

    /// Commitment over the metric vector (not the image digest).
    #[must_use]
    pub fn commitment(&self) -> String {
        metrics_commitment(
            &self.split_nll,
            self.holdout_nll,
            self.tokens_per_sec,
            self.step_latency_ms,
            self.custom_value,
        )
    }

    /// Check the measurement against the pin and topic before it can score.
    pub fn verify(&self, pin: &ProofPin, topic: &TopicDocument) -> Result<(), EvalError> {
        if self.eval_image_digest.trim() != pin.eval_image_digest.trim() {
            return Err(EvalError::Baseline(format!(
                "measured by eval image {:?}, pin is {:?}",
                self.eval_image_digest, pin.eval_image_digest
            )));
        }
        if self.topic_id.trim() != topic.id {
            return Err(EvalError::Baseline(
                "baseline topic_id is not this topic".into(),
            ));
        }
        if !self
            .holdout_commitment
            .trim()
            .eq_ignore_ascii_case(topic.holdout_commitment.trim())
        {
            return Err(EvalError::Baseline(
                "baseline holdout commitment does not match the topic".into(),
            ));
        }
        if self.split_nll.len() != HoldoutSplit::SCORED.len() {
            return Err(EvalError::Baseline(format!(
                "{} split scores, need {}",
                self.split_nll.len(),
                HoldoutSplit::SCORED.len()
            )));
        }
        let got = self.commitment();
        if !got.eq_ignore_ascii_case(topic.baseline.metrics_commitment.trim()) {
            return Err(EvalError::Baseline(
                "metrics_commitment does not match the measured vector".into(),
            ));
        }
        Ok(())
    }

    /// Convert to the score crate's sealed baseline.
    #[must_use]
    pub fn into_sealed(self) -> SealedBaseline {
        SealedBaseline {
            holdout_nll: self.holdout_nll,
            split_nll: self.split_nll,
            tokens_per_sec: self.tokens_per_sec,
            step_latency_ms: self.step_latency_ms,
            custom_value: self.custom_value,
        }
    }
}

/// Domain-separated commitment over a baseline metric vector.
#[must_use]
pub fn metrics_commitment(
    split_nll: &BTreeMap<String, f64>,
    holdout_nll: f64,
    tokens_per_sec: Option<f64>,
    step_latency_ms: Option<f64>,
    custom_value: Option<f64>,
) -> String {
    let mut obj = serde_json::Map::new();
    obj.insert("holdout_nll".into(), serde_json::json!(holdout_nll));
    obj.insert("split_nll".into(), serde_json::json!(split_nll));
    if let Some(v) = tokens_per_sec {
        obj.insert("tokens_per_sec".into(), serde_json::json!(v));
    }
    if let Some(v) = step_latency_ms {
        obj.insert("step_latency_ms".into(), serde_json::json!(v));
    }
    if let Some(v) = custom_value {
        obj.insert("custom_value".into(), serde_json::json!(v));
    }
    let body = canonical_json(&serde_json::Value::Object(obj));
    let mut h = Sha256::new();
    h.update(BASELINE_DOMAIN);
    h.update([0xff]);
    h.update(body.as_bytes());
    hex::encode(h.finalize())
}

fn map_offer_err(e: OfferError) -> EvalError {
    match e {
        OfferError::Missing => EvalError::InferenceOfferMissing,
        OfferError::Closed => EvalError::InferenceOfferClosed,
        other => EvalError::InferenceOffer(other.to_string()),
    }
}

/// Whether this host can produce a verdict at all.
///
/// The RLM judge offer is required on every backend. The eval executor offer
/// is a Lium-path requirement: sim rents nothing, so there is no machine to
/// bind; a live host with no open `1x` executor cannot score.
pub fn scoring_readiness(
    pin: &ProofPin,
    backend: EvalBackend,
    live: Option<&dyn LiveScorer>,
    has_open_sealed_topic: bool,
    offer: Option<&InferenceOffer>,
    executor: Option<&EvalExecutorOffer>,
    judge_api_key: Option<&str>,
) -> Result<(), EvalError> {
    if !has_open_sealed_topic {
        return Err(EvalError::NoOpenTopic);
    }
    require_open_offer(offer, pin).map_err(map_offer_err)?;
    match backend {
        EvalBackend::Sim => Ok(()),
        EvalBackend::Lium => {
            if !pin.can_rent() {
                return Err(EvalError::EvalImageUnpinned);
            }
            let scorer = live.ok_or(EvalError::LiveHarvestUnavailable)?;
            scorer.ready()?;
            judge_api_key_ready(judge_api_key)?;
            require_open_executor(executor, pin).map_err(map_executor_err)?;
            Ok(())
        }
    }
}

/// Live Lium scoring needs a readable non-empty judge API key.
///
/// # Errors
///
/// [`EvalError::InferenceAuthMissing`] when the key is absent.
pub fn judge_api_key_ready(judge_api_key: Option<&str>) -> Result<(), EvalError> {
    if judge_api_key.map(str::trim).is_some_and(|s| !s.is_empty()) {
        Ok(())
    } else {
        Err(EvalError::InferenceAuthMissing)
    }
}

/// `PROOF_INFERENCE_BASE_URL`, else first non-empty line of `PROOF_INFERENCE_BASE_URL_FILE`.
pub fn secret_backed_base_url() -> Option<String> {
    let env = std::env::var("PROOF_INFERENCE_BASE_URL")
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty());
    if env.is_some() {
        return env;
    }
    std::fs::read_to_string(std::env::var("PROOF_INFERENCE_BASE_URL_FILE").ok()?)
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

/// One finished eval.
#[derive(Debug, Clone)]
pub struct EvalOutcome {
    /// Agent envelope.
    pub agent: AgentVerdict,
    /// Harness metrics.
    pub harness: HarnessMetrics,
    /// Integrity receipt.
    pub receipt: EvalReceipt,
    /// Backend that produced the scores.
    pub backend: EvalBackend,
    /// Executor plan the live run was held to (template, `1x`, deadline,
    /// commitment of that resolved configuration). `None` on sim.
    pub executor: Option<ExecutorPlan>,
}

/// Declared training metadata plus the holdout fingerprints inside it.
#[must_use]
pub fn contamination_evidence(
    manifest: &ArtifactManifest,
    holdout: &[HoldoutRecord],
) -> (bool, Vec<String>) {
    let hashes: BTreeSet<String> = manifest
        .train_content_hashes
        .iter()
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .collect();
    let datasets: BTreeSet<String> = manifest
        .train_dataset_ids
        .iter()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect();
    (
        manifest.is_declared(),
        contamination(&hashes, &datasets, holdout),
    )
}

fn unit(parts: &[&str], index: u32) -> f64 {
    let mut h = Sha256::new();
    for p in parts {
        h.update(p.as_bytes());
        h.update([0xff]);
    }
    h.update(index.to_le_bytes());
    f64::from(h.finalize()[0]) / 255.0
}

/// Deterministic sim scores. Only used when the host opted into sim.
///
/// Holdout NLL is `(3.10 - 0.40 * skill).max(1.0)`. Skill=1.0 still yields
/// NLL ≥ 1.0, so this **cannot** clear `quality_floor` against a real sealed
/// baseline near 0.29. Test wins against sim-derived baselines
/// ([`BASELINE_SKILL`]) do not apply on staging. Use [`sim_win_document`].
#[must_use]
pub fn sim_document(
    pin: &ProofPin,
    topic: &TopicDocument,
    frozen: &str,
    artifact: &str,
    skill: f64,
    reproduced: bool,
) -> ProofEvalDocument {
    let nll = (3.10 - 0.40 * skill.clamp(0.0, 1.0)).max(1.0);
    let mut split = BTreeMap::new();
    for (i, s) in HoldoutSplit::SCORED.iter().enumerate() {
        let jitter = 0.01 * unit(&[artifact, s.as_str()], u32::try_from(i).unwrap_or(0));
        split.insert(s.as_str().to_owned(), nll + jitter);
    }
    let mean = split.values().sum::<f64>() / split.len() as f64;
    let tps = 100.0 * (1.0 + 0.20 * skill.clamp(0.0, 1.0));
    ProofEvalDocument {
        schema_version: PROOF_METRICS_SCHEMA,
        submission_digest: frozen.to_owned(),
        artifact_digest: artifact.to_owned(),
        topic_id: topic.id.clone(),
        eval_image_digest: pin.eval_image_digest.clone(),
        holdout_commitment: topic.holdout_commitment.clone(),
        agent: AgentVerdict {
            verdict: if reproduced {
                ProofKind::Clean
            } else {
                ProofKind::Reject
            },
            reproduced,
            claim_holds_public: reproduced,
            contamination: false,
            canary_hit: false,
            flops_used: topic.flops_budget / 2,
            flops_budget: topic.flops_budget,
            cheat_codes: if reproduced {
                Vec::new()
            } else {
                vec![ProofCheatCode::UnreproducedClaim]
            },
            rationale: if reproduced {
                "sim reproduced".into()
            } else {
                "sim unreproduced".into()
            },
            topic_id: topic.id.clone(),
            family: topic.metric.family,
        },
        harness: HarnessMetrics {
            holdout_nll: mean,
            split_nll: split,
            public_nll: Some(mean),
            tokens_per_sec: (topic.metric.family == MetricFamily::Throughput).then_some(tps),
            step_latency_ms: None,
            wall_s: (topic.metric.family == MetricFamily::Throughput)
                .then_some(topic.metric.wall_budget_s / 2),
            custom_value: None,
            canary_nll: None,
        },
    }
}

/// Skill of the sealed AdamW / comms reference in sim (so a strong miner wins).
pub const BASELINE_SKILL: f64 = 0.40;

fn beat(baseline: f64, direction: MetricDirection, epsilon: f64) -> f64 {
    let margin = 0.01;
    match direction {
        MetricDirection::Max => baseline * (1.0 + epsilon + margin),
        MetricDirection::Min => (baseline * (1.0 - epsilon - margin)).max(0.0),
    }
}

/// Sim harness **relative to `sealed`**, not a higher [`sim_document`] skill.
///
/// Inequalities (option A): holdout NLL ≤ baseline + quality floor, each
/// scored split ≤ baseline + `epsilon_topic_max_regress`, and
/// `tokens_per_sec` ≥ ref × (1 + `epsilon_rel`) when that is the primary.
/// Used for every [`EvalBackend::Sim`] score that has a sealed baseline.
/// Never called from the Lium path.
#[must_use]
pub fn sim_win_document(
    pin: &ProofPin,
    topic: &TopicDocument,
    frozen: &str,
    artifact: &str,
    sealed: &SealedBaseline,
) -> ProofEvalDocument {
    let nll_eps = topic.epsilon_nll.max(0.01);
    let holdout = match topic.metric.family {
        MetricFamily::Nll => (sealed.holdout_nll - nll_eps).max(0.0),
        _ => sealed.holdout_nll,
    };
    let mut split = BTreeMap::new();
    for s in HoldoutSplit::SCORED {
        let b = sealed.split_nll.get(s.as_str()).copied().unwrap_or(holdout);
        let v = match topic.metric.family {
            MetricFamily::Nll => (b - nll_eps).max(0.0),
            _ => b,
        };
        split.insert(s.as_str().to_owned(), v);
    }
    let tps = match topic.metric.primary.as_str() {
        proof_task::METRIC_TOKENS_PER_SEC => Some(beat(
            sealed.tokens_per_sec.unwrap_or(100.0),
            topic.metric.direction,
            topic.metric.epsilon_rel,
        )),
        _ => sealed.tokens_per_sec,
    };
    let latency = match topic.metric.primary.as_str() {
        proof_task::METRIC_STEP_LATENCY_MS => Some(beat(
            sealed.step_latency_ms.unwrap_or(100.0),
            topic.metric.direction,
            topic.metric.epsilon_rel,
        )),
        _ => sealed.step_latency_ms,
    };
    let custom = sealed
        .custom_value
        .map(|b| beat(b, topic.metric.direction, topic.metric.epsilon_rel));
    ProofEvalDocument {
        schema_version: PROOF_METRICS_SCHEMA,
        submission_digest: frozen.to_owned(),
        artifact_digest: artifact.to_owned(),
        topic_id: topic.id.clone(),
        eval_image_digest: pin.eval_image_digest.clone(),
        holdout_commitment: topic.holdout_commitment.clone(),
        agent: AgentVerdict {
            verdict: ProofKind::Clean,
            reproduced: true,
            claim_holds_public: true,
            contamination: false,
            canary_hit: false,
            flops_used: topic.flops_budget / 2,
            flops_budget: topic.flops_budget,
            cheat_codes: Vec::new(),
            rationale: "sim stub win".into(),
            topic_id: topic.id.clone(),
            family: topic.metric.family,
        },
        harness: HarnessMetrics {
            holdout_nll: holdout,
            split_nll: split,
            public_nll: Some(holdout),
            tokens_per_sec: tps,
            step_latency_ms: latency,
            wall_s: (topic.metric.family == MetricFamily::Throughput)
                .then_some(topic.metric.wall_budget_s / 2),
            custom_value: custom,
            canary_nll: None,
        },
    }
}

/// Score only after the submission digest is frozen and a topic is open.
///
/// `artifact_uri` and `declared_flops` travel to the live scorer untouched:
/// the miner's locator for the bytes behind `artifact_digest` (never trusted
/// beyond that) and the miner's FLOP declaration the measured run is held to.
#[allow(clippy::too_many_arguments)]
pub async fn eval_after_freeze(
    pin: &ProofPin,
    topic: &TopicDocument,
    offer: &InferenceOffer,
    executor: Option<&EvalExecutorOffer>,
    frozen_digest: &str,
    artifact_digest: &str,
    artifact_uri: Option<&str>,
    declared_flops: u64,
    holdout: &[HoldoutRecord],
    claim: &str,
    backend: EvalBackend,
    live: Option<&dyn LiveScorer>,
    judge_api_key: Option<&str>,
    sealed: Option<&SealedBaseline>,
) -> Result<EvalOutcome, EvalError> {
    if frozen_digest.trim().is_empty() || holdout.is_empty() {
        return Err(EvalError::HoldoutSealed);
    }
    scoring_readiness(
        pin,
        backend,
        live,
        true,
        Some(offer),
        executor,
        judge_api_key,
    )?;
    offer
        .serves_topic(pin, topic)
        .map_err(|e| EvalError::InferenceOffer(e.to_string()))?;
    if backend == EvalBackend::Lium {
        executor
            .ok_or(EvalError::ExecutorOfferMissing)?
            .serves_topic(topic)
            .map_err(map_executor_err)?;
    }
    let resolved = resolve_inference(
        pin,
        Some(&topic.inference),
        secret_backed_base_url().as_deref(),
        Some(offer),
    );
    if !resolved.ready_to_score() {
        return Err(EvalError::InferenceOffer(
            OfferError::Incomplete.to_string(),
        ));
    }
    if resolved.base_url.trim() != offer.provider.base_url.trim() {
        return Err(EvalError::InferenceOffer(
            OfferError::OriginMismatch.to_string(),
        ));
    }
    let mut plan = None;
    let doc = match backend {
        EvalBackend::Sim => {
            if let Some(sealed) = sealed {
                sim_win_document(pin, topic, frozen_digest, artifact_digest, sealed)
            } else {
                let skill = unit(&[artifact_digest, "skill"], 0);
                sim_document(pin, topic, frozen_digest, artifact_digest, skill, true)
            }
        }
        EvalBackend::Lium => {
            let scorer = live.ok_or(EvalError::LiveHarvestUnavailable)?;
            let executor = executor.ok_or(EvalError::ExecutorOfferMissing)?;
            // Resolve the rent before anything runs so the row records the
            // configuration the run was actually held to.
            let resolved = scorer.plan(pin, topic, executor)?;
            let doc = scorer
                .score(
                    pin,
                    topic,
                    offer,
                    &resolved,
                    frozen_digest,
                    artifact_digest,
                    artifact_uri,
                    declared_flops,
                    holdout,
                    claim,
                )
                .await?;
            plan = Some(resolved);
            doc
        }
    };
    doc.verify(pin, topic, frozen_digest, artifact_digest)?;
    let metrics = serde_json::to_vec(&serde_json::json!({
        "topic": doc.topic_id,
        "holdout_nll": doc.harness.holdout_nll,
        "reproduced": doc.agent.reproduced,
    }))
    .unwrap_or_default();
    let receipt = EvalReceipt {
        provider: match backend {
            EvalBackend::Sim => "sim".into(),
            EvalBackend::Lium => "lium".into(),
        },
        pod_id: format!("proof-{}", &frozen_digest[..8.min(frozen_digest.len())]),
        image_digest: pin.eval_image_digest.clone(),
        submission_hash: frozen_digest.to_owned(),
        metrics_hash: EvalReceipt::hash_metrics_bytes(&metrics),
        termination_verified: true,
    };
    NoScoreGate::check(&receipt, backend == EvalBackend::Lium)
        .map_err(|e| EvalError::Integrity(e.to_string()))?;
    Ok(EvalOutcome {
        agent: doc.agent.truncated(),
        harness: doc.harness,
        receipt,
        backend,
        executor: plan,
    })
}

/// Parse the agent envelope from a JSON object, ignoring any `holdout_nll`.
pub fn parse_agent_verdict(body: &str) -> Result<AgentVerdict, EvalError> {
    let mut value: serde_json::Value =
        serde_json::from_str(body).map_err(|e| EvalError::NoVerdict(e.to_string()))?;
    if let Some(obj) = value.as_object_mut() {
        obj.remove("holdout_nll");
        obj.remove("baseline_nll");
        obj.remove("delta");
        obj.remove("pass");
    }
    serde_json::from_value(value).map_err(|e| EvalError::NoVerdict(e.to_string()))
}

#[cfg(test)]
mod tests {
    use proof_task::{
        default_adamw, holdout_commitment, inference_config_commitment, synthetic_holdout,
        InferenceConfig, InferenceMode, InferenceOffer, InferenceProvider, InferenceProviderKind,
        OfferStatus, TopicDocument, TopicStatus, FLOPS_BUDGET_MAX, STRATUM_SIZE,
    };

    use super::*;

    fn pin(digest: &str) -> ProofPin {
        let mut p = ProofPin {
            eval_image_digest: digest.to_owned(),
            topic_pubkey: "ab".repeat(32),
            ..ProofPin::default()
        };
        p.inference.model = "master-proxy-v0".into();
        p
    }

    fn offer() -> InferenceOffer {
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
            offer_id: "master-v0".into(),
            provider: InferenceProvider {
                kind: InferenceProviderKind::OpenaiCompatible,
                base_url: "http://127.0.0.1:8000/v1".into(),
            },
            config_commitment: inference_config_commitment(&config, "http://127.0.0.1:8000/v1"),
            config,
            status: OfferStatus::Open,
        }
    }

    fn topic() -> TopicDocument {
        let recs = synthetic_holdout(STRATUM_SIZE, 1);
        let mut b = default_adamw(FLOPS_BUDGET_MAX);
        b.script_sha256 = "11".repeat(32);
        b.metrics_commitment = "22".repeat(32);
        TopicDocument {
            id: "adamw-beater-v0".into(),
            statement: "beat adamw".into(),
            baseline: b,
            holdout_commitment: holdout_commitment(&recs),
            status: TopicStatus::Open,
            ..TopicDocument::default()
        }
    }

    /// Open `1x` executor bound to `pin`'s digest (template name carries the
    /// digest prefix, as the digest-scoped harvest template does).
    fn executor(pin: &ProofPin) -> EvalExecutorOffer {
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

    struct Harvest {
        reproduced: bool,
    }

    #[async_trait]
    impl LiveScorer for Harvest {
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
            _holdout: &[HoldoutRecord],
            _claim: &str,
        ) -> Result<ProofEvalDocument, EvalError> {
            Ok(sim_document(
                pin,
                topic,
                frozen,
                artifact,
                0.9,
                self.reproduced,
            ))
        }
    }

    #[test]
    fn sim_is_opt_in_only() {
        assert!(!force_sim());
        assert_eq!(resolve_eval_backend(), EvalBackend::Lium);
    }

    #[tokio::test]
    async fn a_live_host_refuses_rather_than_simming() {
        let t = topic();
        let recs = synthetic_holdout(STRATUM_SIZE, 1);
        let unpinned = eval_after_freeze(
            &pin(""),
            &t,
            &offer(),
            Some(&executor(&pin(""))),
            "d",
            "art",
            None,
            1,
            &recs,
            "claim",
            EvalBackend::Lium,
            None,
            None,
            None,
        )
        .await
        .expect_err("no digest");
        assert!(
            matches!(unpinned, EvalError::EvalImageUnpinned),
            "{unpinned}"
        );

        let unwired = eval_after_freeze(
            &pin(&format!("sha256:{}", "ab".repeat(32))),
            &t,
            &offer(),
            Some(&executor(&pin(&format!("sha256:{}", "ab".repeat(32))))),
            "d",
            "art",
            None,
            1,
            &recs,
            "claim",
            EvalBackend::Lium,
            None,
            None,
            None,
        )
        .await
        .expect_err("no harvest");
        assert!(
            matches!(unwired, EvalError::LiveHarvestUnavailable),
            "{unwired}"
        );
    }

    #[tokio::test]
    async fn a_live_host_uses_the_wired_harvest() {
        let t = topic();
        let recs = synthetic_holdout(STRATUM_SIZE, 1);
        let p = pin(&format!("sha256:{}", "ab".repeat(32)));
        let out = eval_after_freeze(
            &p,
            &t,
            &offer(),
            Some(&executor(&p)),
            "digest-a",
            "art",
            None,
            1,
            &recs,
            "claim",
            EvalBackend::Lium,
            Some(&Harvest { reproduced: true }),
            Some("test-judge-key"),
            None,
        )
        .await
        .expect("live");
        assert_eq!(out.backend, EvalBackend::Lium);
        assert!(out.agent.reproduced);
        assert_eq!(out.agent.topic_id, t.id);
        assert_eq!(out.receipt.provider, "lium");
        let plan = out
            .executor
            .expect("live outcome carries the resolved plan");
        assert_eq!(plan.offer_id, "lium-1x-v0");
        assert_eq!(plan.topic_id, t.id);
        assert_eq!(plan.gpu_count, 1);
        assert_eq!(plan.deadline_s, 3_600);
        assert_eq!(plan.offer_commitment, executor(&p).config_commitment);
        assert_eq!(plan.config_commitment, plan.offer_commitment);
    }

    #[test]
    fn agent_holdout_fields_are_stripped() {
        let body = r#"{
            "verdict": "clean", "reproduced": true, "claim_holds_public": true,
            "contamination": false, "canary_hit": false, "flops_used": 1,
            "flops_budget": 2, "cheat_codes": [], "rationale": "ok",
            "topic_id": "dt-no-ib-v0", "family": "throughput",
            "holdout_nll": 1.23, "pass": true
        }"#;
        let v = parse_agent_verdict(body).expect("parse");
        assert!(v.reproduced);
        assert_eq!(v.topic_id, "dt-no-ib-v0");
        assert_eq!(v.family, MetricFamily::Throughput);
    }

    #[test]
    fn missing_agent_envelope_is_no_verdict() {
        assert!(parse_agent_verdict("{}").is_err());
        assert!(parse_agent_verdict("not json").is_err());
    }

    #[test]
    fn readiness_names_the_root_cause() {
        let live = pin(&format!("sha256:{}", "ab".repeat(32)));
        let o = offer();
        assert!(matches!(
            scoring_readiness(
                &live,
                EvalBackend::Sim,
                None,
                false,
                Some(&o),
                Some(&executor(&live)),
                None
            ),
            Err(EvalError::NoOpenTopic)
        ));
        scoring_readiness(
            &ProofPin::default(),
            EvalBackend::Sim,
            None,
            true,
            Some(&o),
            Some(&executor(&ProofPin::default())),
            None,
        )
        .expect("sim");
        assert!(matches!(
            scoring_readiness(
                &ProofPin::default(),
                EvalBackend::Sim,
                None,
                true,
                None,
                Some(&executor(&ProofPin::default())),
                None,
            ),
            Err(EvalError::InferenceOfferMissing)
        ));
        assert!(matches!(
            scoring_readiness(
                &ProofPin::default(),
                EvalBackend::Lium,
                None,
                true,
                Some(&o),
                Some(&executor(&ProofPin::default())),
                None,
            ),
            Err(EvalError::EvalImageUnpinned)
        ));
        assert!(matches!(
            scoring_readiness(
                &live,
                EvalBackend::Lium,
                None,
                true,
                Some(&o),
                Some(&executor(&live)),
                None
            ),
            Err(EvalError::LiveHarvestUnavailable)
        ));
        scoring_readiness(
            &live,
            EvalBackend::Lium,
            Some(&Harvest { reproduced: true }),
            true,
            Some(&o),
            Some(&executor(&live)),
            Some("test-judge-key"),
        )
        .expect("ready");
        assert!(matches!(
            scoring_readiness(
                &live,
                EvalBackend::Lium,
                Some(&Harvest { reproduced: true }),
                true,
                Some(&o),
                Some(&executor(&live)),
                None,
            ),
            Err(EvalError::InferenceAuthMissing)
        ));
    }

    /// Lium: missing / closed / non-`1x` executor is a refusal after every
    /// other live prerequisite holds. Sim never rents, so it does not care.
    #[test]
    fn readiness_requires_an_open_one_gpu_executor_on_lium_only() {
        let live = pin(&format!("sha256:{}", "ab".repeat(32)));
        let o = offer();
        let harvest = Harvest { reproduced: true };
        assert!(matches!(
            scoring_readiness(
                &live,
                EvalBackend::Lium,
                Some(&harvest),
                true,
                Some(&o),
                None,
                Some("test-judge-key"),
            ),
            Err(EvalError::ExecutorOfferMissing)
        ));
        let mut closed = executor(&live);
        closed.status = proof_executor::OfferStatus::Closed;
        assert!(matches!(
            scoring_readiness(
                &live,
                EvalBackend::Lium,
                Some(&harvest),
                true,
                Some(&o),
                Some(&closed),
                Some("test-judge-key"),
            ),
            Err(EvalError::ExecutorOfferClosed)
        ));
        let mut wide = executor(&live);
        wide.machine_shape = "8x".into();
        wide.config_commitment = wide.expected_commitment();
        let err = scoring_readiness(
            &live,
            EvalBackend::Lium,
            Some(&harvest),
            true,
            Some(&o),
            Some(&wide),
            Some("test-judge-key"),
        )
        .expect_err("8x cannot score");
        assert!(
            matches!(err, EvalError::ExecutorOffer(ref m) if m.contains("machine_shape")),
            "{err}"
        );
        scoring_readiness(
            &ProofPin::default(),
            EvalBackend::Sim,
            None,
            true,
            Some(&o),
            None,
            None,
        )
        .expect("sim rents nothing");
    }

    #[tokio::test]
    async fn live_eval_refuses_without_an_executor_that_serves_the_topic() {
        let recs = synthetic_holdout(STRATUM_SIZE, 1);
        let p = pin(&format!("sha256:{}", "ab".repeat(32)));
        let mut t = topic();
        let missing = eval_after_freeze(
            &p,
            &t,
            &offer(),
            None,
            "digest-a",
            "art",
            None,
            1,
            &recs,
            "claim",
            EvalBackend::Lium,
            Some(&Harvest { reproduced: true }),
            Some("test-judge-key"),
            None,
        )
        .await
        .expect_err("no executor");
        assert!(
            matches!(missing, EvalError::ExecutorOfferMissing),
            "{missing}"
        );

        t.eval_executor.require_offer_commitment = Some("cd".repeat(32));
        let pinned_elsewhere = eval_after_freeze(
            &p,
            &t,
            &offer(),
            Some(&executor(&p)),
            "digest-a",
            "art",
            None,
            1,
            &recs,
            "claim",
            EvalBackend::Lium,
            Some(&Harvest { reproduced: true }),
            Some("test-judge-key"),
            None,
        )
        .await
        .expect_err("topic pins another executor");
        assert!(
            matches!(pinned_elsewhere, EvalError::ExecutorOffer(ref m) if m.contains("cannot serve")),
            "{pinned_elsewhere}"
        );
        assert!(EvalError::ProofDeadlineExceeded {
            deadline_s: 600,
            stdout_tail: "exit=124".into(),
        }
        .to_string()
        .contains("600s"),);
    }

    #[test]
    fn baseline_commitment_is_bound_to_the_vector() {
        let mut splits = BTreeMap::new();
        for s in HoldoutSplit::SCORED {
            splits.insert(s.as_str().to_owned(), 3.0);
        }
        let a = metrics_commitment(&splits, 3.0, Some(100.0), None, None);
        let b = metrics_commitment(&splits, 3.0, Some(100.0), None, None);
        assert_eq!(a, b);
        assert_ne!(a, metrics_commitment(&splits, 3.1, Some(100.0), None, None));
        assert_eq!(a.len(), 64);
    }

    #[test]
    fn custom_ids_come_from_the_live_scorer_and_sim_never_invents_a_value() {
        assert!(registered_custom(None).is_empty());
        assert!(registered_custom(Some(&Harvest { reproduced: true })).is_empty());
        let t = topic();
        let pin = pin("");
        let doc = sim_document(&pin, &t, "f", "art", 1.0, true);
        assert!(doc.harness.custom_value.is_none());
    }

    /// A custom-family scorer with a registry of one id: that id is
    /// scorable, another id refuses, and no custom topic ever reaches the
    /// default harvest.
    struct OneRunner;

    #[async_trait]
    impl LiveScorer for OneRunner {
        async fn score(
            &self,
            _pin: &ProofPin,
            topic: &TopicDocument,
            _offer: &InferenceOffer,
            _plan: &ExecutorPlan,
            _frozen: &str,
            _artifact: &str,
            _artifact_uri: Option<&str>,
            _declared_flops: u64,
            _holdout: &[HoldoutRecord],
            _claim: &str,
        ) -> Result<ProofEvalDocument, EvalError> {
            self.ready_for_topic(topic)?;
            Err(EvalError::Backend("would run the registered runner".into()))
        }

        fn ready_for_topic(&self, topic: &TopicDocument) -> Result<(), EvalError> {
            if topic.metric.custom_id == "registered_metric" {
                Ok(())
            } else {
                Err(EvalError::RunnerUnwired {
                    custom_id: topic.metric.custom_id.clone(),
                    detail: "not in registry".into(),
                })
            }
        }

        fn custom_ids(&self) -> Vec<String> {
            vec!["registered_metric".into()]
        }

        async fn auto_promote(
            &self,
            _t: &TopicDocument,
            _digest: &str,
            pass: bool,
            p: Option<f64>,
            b: Option<f64>,
        ) -> bool {
            pass && p > b
        }
    }

    fn custom_topic(id: &str) -> TopicDocument {
        let mut t = topic();
        t.metric.family = MetricFamily::Custom;
        t.metric.custom_id = id.into();
        t
    }

    #[tokio::test]
    async fn family_mux_routes_custom_to_the_registry_and_never_to_the_harvest() {
        let bare = FamilyMux::new(Arc::new(Harvest { reproduced: true }));
        bare.ready_for_topic(&topic())
            .expect("nll routes to the harvest");
        assert!(bare.custom_ids().is_empty());
        assert!(matches!(
            bare.ready_for_topic(&custom_topic("anything")),
            Err(EvalError::RunnerUnwired { .. })
        ));

        let mux = FamilyMux::new(Arc::new(Harvest { reproduced: true }))
            .with_custom_family(Arc::new(OneRunner));
        assert_eq!(mux.custom_ids(), vec!["registered_metric".to_owned()]);
        assert_eq!(
            registered_custom(Some(&mux)),
            vec!["registered_metric".to_owned()]
        );
        assert_eq!(custom_ids_ref(&mux.custom_ids()), vec!["registered_metric"]);
        mux.ready_for_topic(&custom_topic("registered_metric"))
            .expect("registered id");
        let err = mux
            .ready_for_topic(&custom_topic("unknown_metric"))
            .expect_err("unregistered id");
        assert!(matches!(err, EvalError::RunnerUnwired { .. }), "{err}");
        assert!(
            !mux.auto_promote(&topic(), "d", true, Some(1.0), Some(0.5))
                .await
        );
        assert!(
            mux.auto_promote(
                &custom_topic("registered_metric"),
                "d",
                true,
                Some(1.0),
                Some(0.5)
            )
            .await
        );
        assert!(
            !mux.auto_promote(
                &custom_topic("registered_metric"),
                "d",
                false,
                Some(1.0),
                Some(0.5)
            )
            .await
        );

        let p = pin(&format!("sha256:{}", "ab".repeat(32)));
        let recs = synthetic_holdout(STRATUM_SIZE, 1);
        let exec = executor(&p);
        let plan = mux.plan(&p, &topic(), &exec).expect("harvest plan");
        let err = mux
            .score(
                &p,
                &custom_topic("unknown_metric"),
                &offer(),
                &plan,
                "d",
                "a",
                None,
                1,
                &recs,
                "c",
            )
            .await
            .expect_err("unregistered family must not sim or harvest");
        assert!(matches!(err, EvalError::RunnerUnwired { .. }), "{err}");
        // With no custom-family scorer at all, even planning a custom topic
        // refuses: nothing may rent for a family nobody can score.
        assert!(matches!(
            bare.plan(&p, &custom_topic("unknown_metric"), &exec),
            Err(EvalError::RunnerUnwired { .. })
        ));
        mux.on_persisted("t", "d", "pf_0", false).await;
    }

    /// A host with a registered custom runner but no Lium harvest: the mux
    /// passes the host-wide gate, `custom` routes to the registry, and every
    /// `nll` / `throughput` topic refuses at readiness, plan, and score with
    /// `LiveHarvestUnavailable` — never the custom scorer, never a sim, no
    /// plan to rent under.
    #[tokio::test]
    async fn custom_only_mux_scores_custom_and_refuses_the_harvest_families() {
        let mux = FamilyMux::custom_only(Arc::new(OneRunner));
        mux.ready().expect("no host-wide blocker without a harvest");
        assert_eq!(mux.custom_ids(), vec!["registered_metric".to_owned()]);
        mux.ready_for_topic(&custom_topic("registered_metric"))
            .expect("registered id");
        assert!(matches!(
            mux.ready_for_topic(&custom_topic("unknown_metric")),
            Err(EvalError::RunnerUnwired { .. })
        ));

        let p = pin(&format!("sha256:{}", "ab".repeat(32)));
        let exec = executor(&p);
        let recs = synthetic_holdout(STRATUM_SIZE, 1);
        for t in [topic(), throughput_topic()] {
            assert!(
                matches!(
                    mux.ready_for_topic(&t),
                    Err(EvalError::LiveHarvestUnavailable)
                ),
                "{}",
                t.id
            );
            assert!(matches!(
                mux.plan(&p, &t, &exec),
                Err(EvalError::LiveHarvestUnavailable)
            ));
            let plan = executor_plan(&p, Some(&exec), &t, &HarvestOverrides::default())
                .expect("plan resolved outside the mux");
            let err = mux
                .score(&p, &t, &offer(), &plan, "d", "a", None, 1, &recs, "c")
                .await
                .expect_err("no harvest to score on");
            assert!(matches!(err, EvalError::LiveHarvestUnavailable), "{err}");
            assert!(!mux.auto_promote(&t, "d", true, Some(1.0), Some(0.5)).await);
        }
        mux.on_persisted("t", "d", "pf_0", false).await;

        // The whole live path: the host-wide gate passes, an `nll` run
        // refuses before any plan, a registered custom run reaches its runner.
        scoring_readiness(
            &p,
            EvalBackend::Lium,
            Some(&mux),
            true,
            Some(&offer()),
            Some(&exec),
            Some("test-judge-key"),
        )
        .expect("custom-only host is ready host-wide");
        let err = eval_after_freeze(
            &p,
            &topic(),
            &offer(),
            Some(&exec),
            "digest-a",
            "art",
            None,
            1,
            &recs,
            "claim",
            EvalBackend::Lium,
            Some(&mux),
            Some("test-judge-key"),
            None,
        )
        .await
        .expect_err("nll needs the harvest");
        assert!(matches!(err, EvalError::LiveHarvestUnavailable), "{err}");
        let err = eval_after_freeze(
            &p,
            &custom_topic("registered_metric"),
            &offer(),
            Some(&exec),
            "digest-a",
            "art",
            Some("https://example.invalid/a.zip"),
            1,
            &recs,
            "claim",
            EvalBackend::Lium,
            Some(&mux),
            Some("test-judge-key"),
            None,
        )
        .await
        .expect_err("the stub runner refuses after routing");
        assert!(
            matches!(err, EvalError::Backend(ref m) if m.contains("registered runner")),
            "{err}"
        );
    }

    fn tight_sealed() -> SealedBaseline {
        let mut split = BTreeMap::new();
        for s in HoldoutSplit::SCORED {
            split.insert(s.as_str().to_owned(), 0.29);
        }
        SealedBaseline {
            holdout_nll: 0.29,
            split_nll: split,
            tokens_per_sec: Some(80.0),
            step_latency_ms: None,
            custom_value: None,
        }
    }

    fn throughput_topic() -> TopicDocument {
        let mut t = topic();
        t.id = "dt-no-ib-v0".into();
        t.metric.family = MetricFamily::Throughput;
        t.metric.primary = proof_task::METRIC_TOKENS_PER_SEC.into();
        t.metric.direction = MetricDirection::Max;
        t.metric.epsilon_rel = 0.05;
        t.metric.quality_floor_nll = 0.02;
        t.metric.wall_budget_s = 14_400;
        t
    }

    #[test]
    fn stub_win_clears_quality_floor_against_a_tight_sealed_baseline() {
        let pin = pin("");
        let t = throughput_topic();
        let sealed = tight_sealed();
        let floor = sealed.holdout_nll + t.metric.quality_floor_nll;
        let stub_win_skill = sim_document(&pin, &t, "f", "art", 0.95, true);
        let max_skill = sim_document(&pin, &t, "f", "art", 1.0, true);
        assert!(
            max_skill.harness.holdout_nll >= 1.0,
            "skill=1.0 must not dip below the sim NLL floor: {}",
            max_skill.harness.holdout_nll
        );
        assert!(
            stub_win_skill.harness.holdout_nll >= 1.0,
            "StubScorer::win skill=0.95 is still NLL≥1.0: {}",
            stub_win_skill.harness.holdout_nll
        );
        for skill_doc in [&stub_win_skill, &max_skill] {
            let reject = proof_score::judge_topic(
                &t,
                &skill_doc.agent,
                &skill_doc.harness,
                &sealed,
                &[],
                &[],
            );
            assert!(!reject.pass, "{reject:?}");
            assert!(
                reject
                    .failed
                    .iter()
                    .any(|g| matches!(g, proof_score::GateFail::QualityFloor { .. })),
                "{reject:?}"
            );
        }

        let win = sim_win_document(&pin, &t, "f", "art", &sealed);
        assert_eq!(win.agent.rationale, "sim stub win");
        assert!(
            win.harness.holdout_nll <= floor,
            "holdout {} > baseline+floor {}",
            win.harness.holdout_nll,
            floor
        );
        for s in HoldoutSplit::SCORED {
            let h = win.harness.split_nll[s.as_str()];
            let b = sealed.split_nll[s.as_str()];
            assert!(
                h <= b + t.epsilon_topic_max_regress,
                "split {} {h} > {b}+eps",
                s.as_str()
            );
        }
        let tps = win.harness.tokens_per_sec.expect("tps");
        let ref_tps = sealed.tokens_per_sec.expect("ref tps");
        assert!(
            tps >= ref_tps * (1.0 + t.metric.epsilon_rel),
            "tps {tps} < ref*(1+eps) {}",
            ref_tps * (1.0 + t.metric.epsilon_rel)
        );
        let verdict = proof_score::judge_topic(&t, &win.agent, &win.harness, &sealed, &[], &[]);
        assert!(verdict.pass, "{verdict:?}");
        assert!(verdict.failed.is_empty(), "{verdict:?}");
    }

    #[tokio::test]
    async fn sim_plus_sealed_uses_relative_harness() {
        let t = throughput_topic();
        let recs = synthetic_holdout(STRATUM_SIZE, 1);
        let p = pin("");
        let sealed = tight_sealed();
        let out = eval_after_freeze(
            &p,
            &t,
            &offer(),
            Some(&executor(&p)),
            "digest-a",
            "art",
            None,
            1,
            &recs,
            "claim",
            EvalBackend::Sim,
            None,
            None,
            Some(&sealed),
        )
        .await
        .expect("sim");
        assert_eq!(out.backend, EvalBackend::Sim);
        assert_eq!(out.receipt.provider, "sim");
        assert!(out.executor.is_none(), "sim rents nothing");
        assert_eq!(out.agent.rationale, "sim stub win");
        assert!(out.harness.holdout_nll <= sealed.holdout_nll + t.metric.quality_floor_nll);
        assert!(
            out.harness.tokens_per_sec.expect("tps")
                >= sealed.tokens_per_sec.expect("ref") * (1.0 + t.metric.epsilon_rel)
        );
        let verdict = proof_score::judge_topic(&t, &out.agent, &out.harness, &sealed, &[], &[]);
        assert!(verdict.pass, "{verdict:?}");
    }

    #[tokio::test]
    async fn stub_win_is_ignored_on_the_lium_path() {
        let t = throughput_topic();
        let recs = synthetic_holdout(STRATUM_SIZE, 1);
        let p = pin(&format!("sha256:{}", "ab".repeat(32)));
        let out = eval_after_freeze(
            &p,
            &t,
            &offer(),
            Some(&executor(&p)),
            "digest-a",
            "art",
            None,
            1,
            &recs,
            "claim",
            EvalBackend::Lium,
            Some(&Harvest { reproduced: true }),
            Some("test-judge-key"),
            Some(&tight_sealed()),
        )
        .await
        .expect("live");
        assert_eq!(out.backend, EvalBackend::Lium);
        assert_eq!(out.receipt.provider, "lium");
        assert!(out.harness.holdout_nll > 1.0, "must not emit stub-win NLL");
    }
}
