use std::collections::{BTreeMap, BTreeSet};

use proof_autonomy::{commitment, is_digest};
use proof_eval::BaselineMeasurement;
use proof_score::{judge_topic, AgentVerdict, HarnessMetrics, SealedBaseline};
use proof_task::{HoldoutSplit, MetricFamily, ProofPin, TopicDocument, TopicStatus};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::ResearchError;

/// Entire signed topic and sealed baseline are committed with the recipe.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScientificRecipe {
    pub schema_version: u32,
    pub topic: TopicDocument,
    pub baseline: BaselineMeasurement,
    pub candidate_script_digest: String,
    pub seeds: Vec<u64>,
    pub maximum_wall_ms: u64,
}

impl ScientificRecipe {
    /// # Errors
    /// Unsigned/not-open topic, weakened gates, unsealed baseline or unbounded run.
    pub fn validate(&self, pin: &ProofPin) -> Result<(), ResearchError> {
        pin.validate().map_err(|_| ResearchError::Evidence)?;
        if self.schema_version != 1
            || !is_digest(&self.candidate_script_digest)
            || !(3..=20).contains(&self.seeds.len())
            || self.seeds.iter().collect::<BTreeSet<_>>().len() != self.seeds.len()
            || !(1..=86_400_000).contains(&self.maximum_wall_ms)
            || self.topic.status != TopicStatus::Open
            || !pin.can_rent()
        {
            return Err(ResearchError::Evidence);
        }
        self.topic
            .validate(pin, &proof_eval::supported_custom())
            .map_err(|_| ResearchError::Evidence)?;
        self.topic
            .verify_signature(pin)
            .map_err(|_| ResearchError::Evidence)?;
        self.baseline
            .verify(pin, &self.topic)
            .map_err(|_| ResearchError::Evidence)?;
        if self.topic.metric.wall_budget_s > 0
            && self.maximum_wall_ms > self.topic.metric.wall_budget_s.saturating_mul(1000)
        {
            return Err(ResearchError::Evidence);
        }
        Ok(())
    }
}

/// Populated by the controller's execution adapter, not an agent tool response.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Measurement {
    pub seed: u64,
    pub script_digest: String,
    pub log_digest: String,
    pub exit_code: i32,
    pub wall_ms: u64,
    pub flops_used: u64,
    pub metrics: HarnessMetrics,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairedMeasurement {
    pub baseline: Measurement,
    pub candidate: Measurement,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScientificEvidence {
    pub schema_version: u32,
    pub experiment_id: Uuid,
    pub chain_epoch: u64,
    pub recipe_digest: String,
    pub resource_id: String,
    pub measurements: Vec<PairedMeasurement>,
    /// Controller scanner output, not the agent's contamination opinion.
    pub contamination_hits: Vec<String>,
    pub verdict: AgentVerdict,
}

/// Deliberate public allowlist. No free text, resource/account id, raw log,
/// holdout record, environment, endpoint, or model-supplied metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicEvidence {
    pub schema_version: u32,
    pub evidence_digest: String,
    pub recipe_digest: String,
    pub repetitions: u32,
    pub primary_mean: f64,
    pub primary_standard_error: f64,
    pub passed: bool,
}

/// Raw bytes live outside the rented machine and never in the public summary.
pub type RetainedArtifacts = BTreeMap<String, Vec<u8>>;

#[must_use]
pub fn artifact_digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

impl ScientificEvidence {
    /// Apply the existing topic gates to every observed candidate, both against
    /// the sealed baseline and against its same-seed baseline reproduction.
    ///
    /// # Errors
    /// Missing repetitions, failed/altered executions, nonfinite metrics or logs.
    #[allow(clippy::cast_precision_loss)]
    pub fn evaluate(
        &self,
        recipe: &ScientificRecipe,
        artifacts: &RetainedArtifacts,
    ) -> Result<PublicEvidence, ResearchError> {
        if self.schema_version != 1
            || self.experiment_id.is_nil()
            || self.recipe_digest != commitment(recipe)?
            || !recipe.topic.is_open_at(self.chain_epoch)
            || !(3..=20).contains(&recipe.seeds.len())
            || self.measurements.len() != recipe.seeds.len()
            || !artifacts.contains_key(&recipe.candidate_script_digest)
            || !artifacts.contains_key(&recipe.topic.baseline.script_sha256)
            || self.verdict.topic_id != recipe.topic.id
            || self.verdict.family != recipe.topic.metric.family
            || artifacts.len() > 64
            || artifacts.values().map(Vec::len).sum::<usize>() > 16 * 1024 * 1024
            || artifacts.iter().any(|(digest, bytes)| {
                bytes.is_empty() || bytes.len() > 1024 * 1024 || artifact_digest(bytes) != *digest
            })
        {
            return Err(ResearchError::Evidence);
        }
        let sealed = recipe.baseline.clone().into_sealed();
        let mut primary = Vec::new();
        let mut passed = self.verdict.claim_holds_public;
        for (pair, seed) in self.measurements.iter().zip(&recipe.seeds) {
            for (observation, script) in [
                (&pair.baseline, &recipe.topic.baseline.script_sha256),
                (&pair.candidate, &recipe.candidate_script_digest),
            ] {
                if observation.seed != *seed
                    || observation.script_digest != *script
                    || observation.exit_code != 0
                    || observation.wall_ms == 0
                    || observation.wall_ms > recipe.maximum_wall_ms
                    || observation.flops_used == 0
                    || observation.flops_used > recipe.topic.flops_budget
                    || !artifacts.contains_key(&observation.log_digest)
                    || !valid_metrics(&observation.metrics)
                {
                    return Err(ResearchError::Evidence);
                }
            }
            let observed = &pair.candidate;
            let mut verdict = self.verdict.clone();
            verdict.flops_used = observed.flops_used;
            verdict.flops_budget = recipe.topic.flops_budget;
            let mut metrics = observed.metrics.clone();
            metrics.wall_s = Some(observed.wall_ms.div_ceil(1000));
            let baseline = as_baseline(&pair.baseline.metrics);
            if !reproduces_baseline(recipe, &baseline, &sealed) {
                return Err(ResearchError::Evidence);
            }
            let custom = proof_eval::supported_custom();
            passed &= judge_topic(
                &recipe.topic,
                &verdict,
                &metrics,
                &sealed,
                &self.contamination_hits,
                &custom,
            )
            .pass;
            passed &= judge_topic(
                &recipe.topic,
                &verdict,
                &metrics,
                &baseline,
                &self.contamination_hits,
                &custom,
            )
            .pass;
            let value = proof_score::primary_from_harness(&recipe.topic, &metrics)
                .ok_or(ResearchError::Evidence)?;
            if !value.is_finite() {
                return Err(ResearchError::Evidence);
            }
            primary.push(value);
        }
        let count = primary.len() as f64;
        let mean = primary.iter().sum::<f64>() / count;
        let variance = primary.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (count - 1.0);
        let standard_error = (variance / count).sqrt();
        if !mean.is_finite() || !standard_error.is_finite() {
            return Err(ResearchError::Evidence);
        }
        // Uncertainty remains visible. It never creates credit absent the gates.
        Ok(PublicEvidence {
            schema_version: 1,
            evidence_digest: commitment(self)?,
            recipe_digest: self.recipe_digest.clone(),
            repetitions: u32::try_from(primary.len()).map_err(|_| ResearchError::Evidence)?,
            primary_mean: mean,
            primary_standard_error: standard_error,
            passed,
        })
    }
}

fn valid_metrics(metrics: &HarnessMetrics) -> bool {
    let optional = [
        metrics.public_nll,
        metrics.tokens_per_sec,
        metrics.step_latency_ms,
        metrics.custom_value,
        metrics.canary_nll,
    ];
    metrics.holdout_nll.is_finite()
        && metrics.holdout_nll >= 0.0
        && metrics.split_nll.len() == HoldoutSplit::SCORED.len()
        && HoldoutSplit::SCORED
            .iter()
            .all(|split| metrics.split_nll.contains_key(split.as_str()))
        && metrics
            .split_nll
            .values()
            .all(|v| v.is_finite() && *v >= 0.0)
        && optional
            .into_iter()
            .flatten()
            .all(|v| v.is_finite() && v.abs() <= 1e18)
}

fn reproduces_baseline(
    recipe: &ScientificRecipe,
    measured: &SealedBaseline,
    sealed: &SealedBaseline,
) -> bool {
    let topic = &recipe.topic;
    if !sealed.holdout_nll.is_finite()
        || (measured.holdout_nll - sealed.holdout_nll).abs() > topic.epsilon_nll
        || !measured.split_nll.iter().all(|(name, value)| {
            sealed.split_nll.get(name).is_some_and(|expected| {
                expected.is_finite() && (value - expected).abs() <= topic.epsilon_topic_max_regress
            })
        })
    {
        return false;
    }
    match (
        proof_score::sealed_primary(topic, measured),
        proof_score::sealed_primary(topic, sealed),
    ) {
        (Some(a), Some(b)) if topic.metric.family == MetricFamily::Nll => {
            (a - b).abs() <= topic.epsilon_nll
        }
        (Some(a), Some(b)) => b != 0.0 && (a - b).abs() <= b.abs() * topic.metric.epsilon_rel,
        _ => false,
    }
}

fn as_baseline(metrics: &HarnessMetrics) -> SealedBaseline {
    SealedBaseline {
        holdout_nll: metrics.holdout_nll,
        split_nll: metrics.split_nll.clone(),
        tokens_per_sec: metrics.tokens_per_sec,
        step_latency_ms: metrics.step_latency_ms,
        custom_value: metrics.custom_value,
    }
}
