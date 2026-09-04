//! Proof live harvest: score an artifact on a digest-pinned `proof-eval` image.
//!
//! The RLM agent and the metric harness live inside the image. This crate
//! boots that image on a Lium pod, hands it the run request (including the
//! topic constraints the image must enforce — e.g. a 12.5 Gbit/s cap), reads
//! back the metrics document, and tears the pod down. Nothing here computes
//! a score. There is no sim fallback.

#![forbid(unsafe_code)]
#![allow(
    clippy::missing_errors_doc,
    clippy::doc_markdown,
    clippy::too_many_arguments
)]

use std::sync::Arc;

use async_trait::async_trait;
use harvest_pod::{harvest_template_name, EvalPod, PodProgram};
use prism_lium_types::InstanceSpec;
use proof_eval::{
    secret_backed_base_url, EvalError, LiveScorer, ProofEvalDocument, PROOF_METRICS_SCHEMA,
};
use proof_task::{
    resolve_inference, HoldoutRecord, InferenceOffer, ProofPin, TopicDocument, CHALLENGE_ID,
};
use serde::{Deserialize, Serialize};

/// Prefix the eval image prints before its metrics document.
pub const METRICS_MARKER: &str = "PROOF_METRICS=";

/// Marker the eval image prints on a completed run.
pub const OK_MARKER: &str = "PROOF_EVAL_OK";

/// Directory the request and metrics sidecar live in, on the pod.
pub const POD_WORKDIR: &str = "/tmp/proof_eval";

/// Lium SSH key name the harvest registers its public key under.
pub const SSH_KEY_NAME: &str = "proof-eval-worker";

/// Image contract for the Proof eval entrypoint.
pub const PROGRAM: PodProgram = PodProgram {
    workdir: POD_WORKDIR,
    entrypoint: "proof-eval score",
    metrics_marker: METRICS_MARKER,
    ok_marker: OK_MARKER,
    score_binary: "/usr/bin/proof-eval",
};

/// What the eval image is asked to score.
///
/// The request carries the **private holdout** and the topic constraints.
/// The image enforces the comms cap; it does not trust the miner's claim.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarvestRequest {
    /// Must equal [`PROOF_METRICS_SCHEMA`].
    pub schema_version: u32,
    /// Must be `proof`.
    pub challenge_id: String,
    /// Frozen submission digest.
    pub submission_digest: String,
    /// Artifact to score.
    pub artifact_digest: String,
    /// Topic id.
    pub topic_id: String,
    /// Metric family wire name.
    pub family: String,
    /// Live judge offer id the eval image must call.
    pub inference_offer_id: String,
    /// Provider kind wire name.
    pub provider_kind: String,
    /// Judge origin (operator state; not a public status field).
    pub base_url: String,
    /// Serving mode.
    pub mode: String,
    /// Provider model id (not an HF bake).
    pub model_ref: String,
    /// Input token cap for this run (min of offer and topic).
    pub max_input_tokens: u32,
    /// Output token cap for this run.
    pub max_output_tokens: u32,
    /// Judge config commitment.
    pub config_commitment: String,
    /// Eval image digest, so the image can stamp its own provenance.
    pub eval_image_digest: String,
    /// Commitment the records below must hash to.
    pub holdout_commitment: String,
    /// Topic constraints the image enforces (12.5 Gbit/s, no IB, …).
    pub constraints: proof_task::Constraints,
    /// FLOP budget.
    pub flops_budget: u64,
    /// Wall budget (throughput).
    pub wall_budget_s: u64,
    /// Miner claim string.
    pub claim: String,
    /// Verified holdout records. Rotate the set if a pod is suspected of exfil.
    pub holdout: Vec<HoldoutRecord>,
}

/// Rent limits for one harvest.
#[derive(Debug, Clone)]
pub struct HarvestLimits {
    /// Max pod lifetime hours.
    pub max_lifetime_hours: f64,
    /// Max USD per GPU-hour.
    pub max_price_per_hour: f64,
    /// GPUs requested.
    pub gpu_count: u32,
}

impl Default for HarvestLimits {
    fn default() -> Self {
        Self {
            max_lifetime_hours: 6.0,
            max_price_per_hour: 12.0,
            gpu_count: 1,
        }
    }
}

/// [`LiveScorer`] over a digest-pinned eval image on a Lium pod.
pub struct LiumProofHarvest {
    pod: Arc<dyn EvalPod>,
    limits: HarvestLimits,
    ssh_public_keys: Vec<String>,
}

impl LiumProofHarvest {
    /// Wrap a pod transport.
    #[must_use]
    pub fn new(pod: Arc<dyn EvalPod>, limits: HarvestLimits, ssh_public_keys: Vec<String>) -> Self {
        Self {
            pod,
            limits,
            ssh_public_keys,
        }
    }

    fn spec(&self, pin: &ProofPin, frozen_digest: &str) -> InstanceSpec {
        InstanceSpec {
            name: format!("proof-{}", &frozen_digest[..12.min(frozen_digest.len())]),
            max_lifetime_hours: self.limits.max_lifetime_hours,
            max_price_per_hour: self.limits.max_price_per_hour,
            gpu_count: self.limits.gpu_count,
            image_digest: Some(pin.eval_image_digest.clone()),
            docker_image: Some(pin.eval_image.clone()),
            startup_commands: None,
            ssh_public_keys: self.ssh_public_keys.clone(),
            ssh_key_name: Some(SSH_KEY_NAME.to_owned()),
            preferred_offer_id: None,
            template_id: None,
            template_name: Some(harvest_template_name(
                &pin.eval_image,
                &pin.eval_image_digest,
            )),
        }
    }
}

#[async_trait]
impl LiveScorer for LiumProofHarvest {
    async fn score(
        &self,
        pin: &ProofPin,
        topic: &TopicDocument,
        offer: &InferenceOffer,
        frozen_digest: &str,
        artifact_digest: &str,
        holdout: &[HoldoutRecord],
        claim: &str,
    ) -> Result<ProofEvalDocument, EvalError> {
        if !pin.can_rent() {
            return Err(EvalError::EvalImageUnpinned);
        }
        if holdout.is_empty() {
            return Err(EvalError::HoldoutSealed);
        }
        self.ready()?;
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
                proof_task::OfferError::Incomplete.to_string(),
            ));
        }
        let max_in = resolved.max_input_tokens.min(offer.config.max_input_tokens);
        let max_out = resolved
            .max_output_tokens
            .min(offer.config.max_output_tokens);
        let request = HarvestRequest {
            schema_version: PROOF_METRICS_SCHEMA,
            challenge_id: CHALLENGE_ID.to_owned(),
            submission_digest: frozen_digest.to_owned(),
            artifact_digest: artifact_digest.to_owned(),
            topic_id: topic.id.clone(),
            family: topic.metric.family.as_str().to_owned(),
            inference_offer_id: offer.offer_id.clone(),
            provider_kind: resolved.provider.as_str().to_owned(),
            base_url: resolved.base_url.clone(),
            mode: resolved.mode.as_str().to_owned(),
            model_ref: resolved.model.clone(),
            max_input_tokens: max_in,
            max_output_tokens: max_out,
            config_commitment: offer.config_commitment.clone(),
            eval_image_digest: pin.eval_image_digest.clone(),
            holdout_commitment: topic.holdout_commitment.clone(),
            constraints: topic.constraints,
            flops_budget: topic.flops_budget,
            wall_budget_s: topic.metric.wall_budget_s,
            claim: claim.to_owned(),
            holdout: holdout.to_vec(),
        };
        let body = serde_json::to_vec(&request)
            .map_err(|e| EvalError::Backend(format!("encode request: {e}")))?;

        let instance = self
            .pod
            .boot(&self.spec(pin, frozen_digest))
            .await
            .map_err(EvalError::Backend)?;
        let run = self.pod.run(&instance, &body, &[]).await;
        let shutdown = self.pod.shutdown(&instance).await;
        match shutdown {
            Ok(true) => {}
            Ok(false) => {
                return Err(EvalError::Integrity(format!(
                    "pod {instance} terminate not verified"
                )))
            }
            Err(e) => return Err(EvalError::Backend(e)),
        }
        let stdout = run.map_err(EvalError::Backend)?;
        if !PROGRAM.ran_to_completion(&stdout) {
            tracing::warn!(instance, "eval image did not print {OK_MARKER}; refusing");
            return Err(EvalError::Backend(format!(
                "eval image did not print {OK_MARKER}"
            )));
        }
        let body = PROGRAM.extract_document(&stdout).ok_or_else(|| {
            EvalError::NoVerdict(format!("eval image printed no {METRICS_MARKER} document"))
        })?;
        let doc = ProofEvalDocument::from_json(body)?;
        doc.verify(pin, topic, frozen_digest, artifact_digest)?;
        Ok(doc)
    }

    fn ready(&self) -> Result<(), EvalError> {
        if self.ssh_public_keys.iter().all(|k| k.trim().is_empty()) {
            return Err(EvalError::Backend(
                "no master SSH public key; the eval pod would be unreachable".into(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use proof_task::{default_adamw, holdout_commitment, synthetic_holdout, STRATUM_SIZE};

    use super::*;

    #[test]
    fn program_is_proof_not_relearn() {
        assert_eq!(PROGRAM.metrics_marker, "PROOF_METRICS=");
        assert_eq!(PROGRAM.ok_marker, "PROOF_EVAL_OK");
        assert!(PROGRAM.entrypoint.contains("proof-eval"));
    }

    #[test]
    fn request_carries_constraints_the_image_must_enforce() {
        let recs = synthetic_holdout(STRATUM_SIZE, 1);
        let mut b = default_adamw(proof_task::FLOPS_BUDGET_MAX);
        b.script_sha256 = "11".repeat(32);
        b.metrics_commitment = "22".repeat(32);
        let topic = TopicDocument {
            id: "dt-no-ib-v0".into(),
            statement: "no IB".into(),
            constraints: proof_task::Constraints {
                no_infiniband: true,
                no_nvlink: true,
                no_nccl_fast_fabric: true,
                max_inter_node_gbps: Some(12.5),
            },
            baseline: b,
            holdout_commitment: holdout_commitment(&recs),
            status: proof_task::TopicStatus::Open,
            ..TopicDocument::default()
        };
        let req = HarvestRequest {
            schema_version: PROOF_METRICS_SCHEMA,
            challenge_id: CHALLENGE_ID.into(),
            submission_digest: "d".into(),
            artifact_digest: "a".into(),
            topic_id: topic.id.clone(),
            family: topic.metric.family.as_str().into(),
            inference_offer_id: "master-v0".into(),
            provider_kind: "openai_compatible".into(),
            base_url: "http://127.0.0.1:8000/v1".into(),
            mode: "chat".into(),
            model_ref: "master-proxy-v0".into(),
            max_input_tokens: 4_096,
            max_output_tokens: 256,
            config_commitment: "ab".repeat(32),
            eval_image_digest: String::new(),
            holdout_commitment: topic.holdout_commitment.clone(),
            constraints: topic.constraints,
            flops_budget: topic.flops_budget,
            wall_budget_s: topic.metric.wall_budget_s,
            claim: String::new(),
            holdout: recs,
        };
        let v = serde_json::to_value(&req).expect("json");
        assert_eq!(v["constraints"]["max_inter_node_gbps"], 12.5);
        assert_eq!(v["constraints"]["no_infiniband"], true);
        assert_eq!(v["challenge_id"], "proof");
        assert_eq!(v["provider_kind"], "openai_compatible");
        assert_eq!(v["mode"], "chat");
        assert!(v.get("proxy_model").is_none());
        assert!(v.get("api_key").is_none());
    }
}
