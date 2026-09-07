//! Trusted, controller-side measurement of a finished scientific run.
//!
//! Trust boundary: the workload sandbox never talks to the observer. After a
//! run exits, the controller captures the bytes the workload left in
//! `/work/artifact`, hashes them, and hands that tar plus the committed run
//! identity to a [`MeasurementObserver`]. Only the observer's output becomes
//! `HarnessMetrics`; script stdout is never promoted to a measurement.
//!
//! FLOPs: `proof_research::Measurement::flops_used` is a `u64` that must be
//! non-zero and within the topic budget for every family. An observer therefore
//! reports `flops_used: Some(n)` only when its document carries a measured
//! `n > 0`; a declared `0` becomes `None`, and the executor treats such a run
//! as unobserved instead of inventing a number. No topic family relaxes this.
//!
//! [`DockerObserver`] runs the digest-pinned `proof-eval` image locally. By
//! default it has no network at all. Because that image scores by loading the
//! miner artifact with `trust_remote_code=True` — arbitrary code next to the
//! operator holdout — it never receives plain internet access.
//!
//! The live image also refuses to score without an RLM judge call. Configuring
//! a [`JudgeEgress`] satisfies both: the scoring container joins an `internal`
//! Docker network with no route off the host, and the only reachable peer is a
//! controller-owned proxy that forwards exactly one upstream origin and injects
//! the API key itself. Untrusted code gets the judge and nothing else, and never
//! sees the credential.

#![forbid(unsafe_code)]

mod docker;
mod judge;

pub use docker::{DockerObserver, JudgeEgress};
pub use judge::{JudgeUpstream, JUDGE_HOST};

use async_trait::async_trait;
use proof_autonomy::{commitment, is_digest};
use proof_eval::ProofEvalDocument;
use proof_research::artifact_digest;
use proof_score::{AgentVerdict, HarnessMetrics};
use proof_task::{HoldoutRecord, HoldoutSplit, ProofPin, TopicDocument};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Largest artifact tar an observer accepts.
pub const MAX_ARTIFACT_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "snake_case")]
pub enum ObserverError {
    #[error("no trusted measurement observer is configured")]
    Unavailable,
    #[error("measurement request is unbounded or inconsistent")]
    Request,
    #[error("verified holdout for this topic is not loaded")]
    Holdout,
    #[error("pinned observer image or daemon unavailable")]
    Target,
    #[error("observer deadline expired")]
    Deadline,
    #[error("observer log limit exceeded")]
    LogLimit,
    #[error("observer produced no admissible metrics document")]
    Document,
}

/// Everything the observer needs, bound to one committed run. The controller
/// fills every field; nothing here comes from the workload's stdout.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeasurementRequest {
    pub experiment_id: Uuid,
    pub intent_id: Uuid,
    pub run_index: usize,
    pub seed: u64,
    pub script_digest: String,
    /// Tar of `/work/artifact` captured by the controller after the run.
    pub artifact: Vec<u8>,
    pub artifact_digest: String,
    pub topic: TopicDocument,
    pub pin: ProofPin,
    /// Absolute Unix milliseconds; a clock step never extends it.
    pub deadline_ms: i64,
    pub timeout_ms: u64,
}

impl MeasurementRequest {
    /// # Errors
    /// Unbounded artifact, digest mismatch, unpinned image or empty identity.
    pub fn validate(&self) -> Result<(), ObserverError> {
        if self.experiment_id.is_nil()
            || self.intent_id.is_nil()
            || self.run_index >= 64
            || !is_digest(&self.script_digest)
            || !is_digest(&self.artifact_digest)
            || self.artifact.is_empty()
            || self.artifact.len() > MAX_ARTIFACT_BYTES
            || artifact_digest(&self.artifact) != self.artifact_digest
            || !(1..=86_400_000).contains(&self.timeout_ms)
            || self.deadline_ms <= 0
            || !self.pin.can_rent()
            || self.topic.id.is_empty()
        {
            return Err(ObserverError::Request);
        }
        Ok(())
    }

    /// Digest the image must echo as `submission_digest`, binding the document
    /// to this exact run rather than to any artifact with the same bytes.
    ///
    /// # Errors
    /// Encoding failure only.
    pub fn submission_digest(&self) -> Result<String, ObserverError> {
        commitment(&(
            self.experiment_id,
            self.intent_id,
            self.run_index,
            self.seed,
            &self.script_digest,
            &self.artifact_digest,
        ))
        .map_err(|_| ObserverError::Request)
    }
}

/// One trusted measurement. `flops_used` is `None` unless the observer measured
/// a positive count; the executor never fills it in.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    pub metrics: HarnessMetrics,
    pub flops_used: Option<u64>,
    pub verdict: AgentVerdict,
    pub observer_image: String,
    pub log_digest: String,
    /// Bounded observer stdout/stderr, retained by the caller.
    pub log: Vec<u8>,
}

#[async_trait]
pub trait MeasurementObserver: Send + Sync {
    /// Verified holdout records for the topic, from operator state.
    ///
    /// # Errors
    /// No verified holdout loaded for this topic.
    fn holdout(&self, topic: &TopicDocument) -> Result<Vec<HoldoutRecord>, ObserverError>;

    /// # Errors
    /// Any failure is a refusal; callers must not substitute a measurement.
    async fn measure(&self, request: MeasurementRequest) -> Result<Observation, ObserverError>;
}

/// Explicit absence of an observer: every measurement is refused.
pub struct NoObserver;

#[async_trait]
impl MeasurementObserver for NoObserver {
    fn holdout(&self, _: &TopicDocument) -> Result<Vec<HoldoutRecord>, ObserverError> {
        Err(ObserverError::Unavailable)
    }
    async fn measure(&self, _: MeasurementRequest) -> Result<Observation, ObserverError> {
        Err(ObserverError::Unavailable)
    }
}

/// Bind an image document to the request and check every scored stratum.
///
/// # Errors
/// Any identity mismatch, missing split, or nonfinite/negative NLL.
pub fn validate_document(
    document: &ProofEvalDocument,
    request: &MeasurementRequest,
) -> Result<(), ObserverError> {
    document
        .verify(
            &request.pin,
            &request.topic,
            &request.submission_digest()?,
            &request.artifact_digest,
        )
        .map_err(|_| ObserverError::Document)?;
    let metrics = &document.harness;
    let finite = |v: f64| v.is_finite() && v >= 0.0;
    if !finite(metrics.holdout_nll)
        || metrics.split_nll.len() != HoldoutSplit::SCORED.len()
        || !HoldoutSplit::SCORED.iter().all(|s| {
            metrics
                .split_nll
                .get(s.as_str())
                .is_some_and(|v| finite(*v))
        })
        || document.agent.flops_budget != request.topic.flops_budget
    {
        return Err(ObserverError::Document);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use proof_score::ProofKind;
    use proof_task::{MetricFamily, TopicStatus};

    use super::*;

    fn request() -> MeasurementRequest {
        let artifact = b"artifact tar bytes".to_vec();
        MeasurementRequest {
            experiment_id: Uuid::from_u128(1),
            intent_id: Uuid::from_u128(2),
            run_index: 0,
            seed: 7,
            script_digest: "a".repeat(64),
            artifact_digest: artifact_digest(&artifact),
            artifact,
            topic: TopicDocument {
                id: "unit".into(),
                status: TopicStatus::Open,
                holdout_commitment: "c".repeat(64),
                ..TopicDocument::default()
            },
            pin: ProofPin {
                eval_image_digest: format!("sha256:{}", "b".repeat(64)),
                ..ProofPin::default()
            },
            deadline_ms: 1,
            timeout_ms: 1000,
        }
    }

    fn document(request: &MeasurementRequest) -> ProofEvalDocument {
        ProofEvalDocument {
            schema_version: proof_eval::PROOF_METRICS_SCHEMA,
            submission_digest: request.submission_digest().unwrap_or_default(),
            artifact_digest: request.artifact_digest.clone(),
            topic_id: request.topic.id.clone(),
            eval_image_digest: request.pin.eval_image_digest.clone(),
            holdout_commitment: request.topic.holdout_commitment.clone(),
            agent: AgentVerdict {
                verdict: ProofKind::Clean,
                reproduced: true,
                claim_holds_public: true,
                contamination: false,
                canary_hit: false,
                flops_used: 0,
                flops_budget: request.topic.flops_budget,
                cheat_codes: vec![],
                rationale: String::new(),
                topic_id: request.topic.id.clone(),
                family: MetricFamily::Nll,
            },
            harness: HarnessMetrics {
                holdout_nll: 1.5,
                split_nll: HoldoutSplit::SCORED
                    .iter()
                    .map(|s| (s.as_str().into(), 1.5))
                    .collect(),
                ..HarnessMetrics::default()
            },
        }
    }

    #[test]
    fn request_rejects_digest_mismatch_and_unpinned_image() {
        let good = request();
        assert!(good.validate().is_ok());
        let mut bad = good.clone();
        bad.artifact.push(0);
        assert_eq!(bad.validate(), Err(ObserverError::Request));
        let mut bad = good.clone();
        bad.pin.eval_image_digest.clear();
        assert_eq!(bad.validate(), Err(ObserverError::Request));
        let mut bad = good;
        bad.artifact.clear();
        assert_eq!(bad.validate(), Err(ObserverError::Request));
    }

    #[test]
    fn submission_digest_binds_run_identity() {
        let a = request();
        let mut b = request();
        b.run_index = 1;
        assert_ne!(a.submission_digest().ok(), b.submission_digest().ok());
    }

    #[test]
    fn document_must_echo_request_and_cover_every_split() {
        let request = request();
        let good = document(&request);
        assert!(validate_document(&good, &request).is_ok());
        let mut bad = good.clone();
        bad.submission_digest = "d".repeat(64);
        assert_eq!(
            validate_document(&bad, &request),
            Err(ObserverError::Document)
        );
        let mut bad = good.clone();
        bad.harness.split_nll.remove(HoldoutSplit::Longctx.as_str());
        assert_eq!(
            validate_document(&bad, &request),
            Err(ObserverError::Document)
        );
        let mut bad = good.clone();
        bad.harness.holdout_nll = f64::NAN;
        assert_eq!(
            validate_document(&bad, &request),
            Err(ObserverError::Document)
        );
        let mut bad = good;
        bad.eval_image_digest = format!("sha256:{}", "e".repeat(64));
        assert_eq!(
            validate_document(&bad, &request),
            Err(ObserverError::Document)
        );
    }

    #[tokio::test]
    async fn no_observer_refuses_everything() {
        assert_eq!(
            NoObserver.holdout(&request().topic).err(),
            Some(ObserverError::Unavailable)
        );
        assert_eq!(
            NoObserver.measure(request()).await.err(),
            Some(ObserverError::Unavailable)
        );
    }
}
