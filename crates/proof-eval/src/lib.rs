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

use async_trait::async_trait;
use prism_lium_types::{EvalReceipt, NoScoreGate};
use proof_score::{AgentVerdict, HarnessMetrics, ProofCheatCode, ProofKind, SealedBaseline};
use proof_store::ArtifactManifest;
use proof_task::{
    canonical_json, contamination, require_open_offer, resolve_inference, HoldoutRecord,
    HoldoutSplit, InferenceOffer, MetricFamily, OfferError, ProofPin, TopicDocument,
    BASELINE_DOMAIN,
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
}

/// Schema version of the metrics+verdict document the eval image emits.
pub const PROOF_METRICS_SCHEMA: u32 = 1;

/// Custom metric ids this control-plane build can score.
///
/// `harness_success_rate` is listed so an operator can publish the agent-harness
/// topic. The real GPU harness is not in this image yet: scoring fail-closes
/// (`EvidenceMissing` on `custom_value`) until that sidecar exists. Do not
/// invent a success rate.
#[must_use]
pub fn supported_custom() -> Vec<&'static str> {
    vec![proof_task::CUSTOM_HARNESS_SUCCESS_RATE]
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
    /// Score one artifact on one topic's verified holdout.
    #[allow(clippy::too_many_arguments)]
    async fn score(
        &self,
        pin: &ProofPin,
        topic: &TopicDocument,
        offer: &InferenceOffer,
        frozen_digest: &str,
        artifact_digest: &str,
        holdout: &[HoldoutRecord],
        claim: &str,
    ) -> Result<ProofEvalDocument, EvalError>;

    /// Whether this scorer could run right now.
    fn ready(&self) -> Result<(), EvalError> {
        Ok(())
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
pub fn scoring_readiness(
    pin: &ProofPin,
    backend: EvalBackend,
    live: Option<&dyn LiveScorer>,
    has_open_sealed_topic: bool,
    offer: Option<&InferenceOffer>,
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
            judge_api_key_ready(judge_api_key)
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

/// Score only after the submission digest is frozen and a topic is open.
#[allow(clippy::too_many_arguments)]
pub async fn eval_after_freeze(
    pin: &ProofPin,
    topic: &TopicDocument,
    offer: &InferenceOffer,
    frozen_digest: &str,
    artifact_digest: &str,
    holdout: &[HoldoutRecord],
    claim: &str,
    backend: EvalBackend,
    live: Option<&dyn LiveScorer>,
    judge_api_key: Option<&str>,
) -> Result<EvalOutcome, EvalError> {
    if frozen_digest.trim().is_empty() || holdout.is_empty() {
        return Err(EvalError::HoldoutSealed);
    }
    scoring_readiness(pin, backend, live, true, Some(offer), judge_api_key)?;
    offer
        .serves_topic(pin, topic)
        .map_err(|e| EvalError::InferenceOffer(e.to_string()))?;
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
    let doc = match backend {
        EvalBackend::Sim => {
            let skill = unit(&[artifact_digest, "skill"], 0);
            sim_document(pin, topic, frozen_digest, artifact_digest, skill, true)
        }
        EvalBackend::Lium => {
            let scorer = live.ok_or(EvalError::LiveHarvestUnavailable)?;
            scorer
                .score(
                    pin,
                    topic,
                    offer,
                    frozen_digest,
                    artifact_digest,
                    holdout,
                    claim,
                )
                .await?
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
            frozen: &str,
            artifact: &str,
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
            "d",
            "art",
            &recs,
            "claim",
            EvalBackend::Lium,
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
            "d",
            "art",
            &recs,
            "claim",
            EvalBackend::Lium,
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
            "digest-a",
            "art",
            &recs,
            "claim",
            EvalBackend::Lium,
            Some(&Harvest { reproduced: true }),
            Some("test-judge-key"),
        )
        .await
        .expect("live");
        assert_eq!(out.backend, EvalBackend::Lium);
        assert!(out.agent.reproduced);
        assert_eq!(out.agent.topic_id, t.id);
        assert_eq!(out.receipt.provider, "lium");
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
            scoring_readiness(&live, EvalBackend::Sim, None, false, Some(&o), None),
            Err(EvalError::NoOpenTopic)
        ));
        scoring_readiness(
            &ProofPin::default(),
            EvalBackend::Sim,
            None,
            true,
            Some(&o),
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
                None
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
                None,
            ),
            Err(EvalError::EvalImageUnpinned)
        ));
        assert!(matches!(
            scoring_readiness(&live, EvalBackend::Lium, None, true, Some(&o), None),
            Err(EvalError::LiveHarvestUnavailable)
        ));
        scoring_readiness(
            &live,
            EvalBackend::Lium,
            Some(&Harvest { reproduced: true }),
            true,
            Some(&o),
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
                None,
            ),
            Err(EvalError::InferenceAuthMissing)
        ));
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
    fn harness_success_rate_is_listed_and_sim_does_not_invent_a_value() {
        assert!(supported_custom().contains(&proof_task::CUSTOM_HARNESS_SUCCESS_RATE));
        let t = topic();
        let pin = pin("");
        let doc = sim_document(&pin, &t, "f", "art", 1.0, true);
        assert!(doc.harness.custom_value.is_none());
    }
}
