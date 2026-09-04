//! Proof scoring: the agent verdict envelope and the harness pass rule.
//!
//! The RLM agent never sees holdout records. It emits a structured verdict
//! about whether the recipe reproduced, whether the public claim holds, and
//! which cheat codes it saw. The harness fills the metric values and decides
//! `pass`. A missing or unparseable verdict is not a zero — it is a 503 at
//! the eval layer, because inventing a score would pay for a run nobody
//! judged.

#![forbid(unsafe_code)]
#![allow(
    clippy::doc_markdown,
    clippy::module_name_repetitions,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

mod payout;

pub use payout::{
    payout_lattices, primary_from_harness, primary_metric, sealed_primary, topic_masses,
    topic_share_bps, MinerTopicRun, PrimaryExtras, PROOF_SHARE_BPS,
};

use std::collections::BTreeMap;

use proof_task::{
    HoldoutSplit, MetricDirection, MetricFamily, TopicDocument, SCORE_MAX, STRATUM_SIZE,
};
use serde::{Deserialize, Serialize};

/// Agent `verdict` field. Distinct from Design/Prism `VerdictKind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProofKind {
    /// Eligible for the harness pass rule.
    Clean,
    /// Policy: treat as lattice 0; rationale kept for admin.
    Suspicious,
    /// Hard lattice 0.
    Reject,
}

/// Cheat taxonomy for Proof. Do not overload Design/Prism codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProofCheatCode {
    /// The agent could not re-run the claimed recipe to the claimed result.
    UnreproducedClaim,
    /// The run spent more FLOPs than the topic budget.
    FlopsOverBudget,
    /// Compared against a weaker/different AdamW than the sealed recipe.
    StrawmanAdamw,
    /// Optimizer named Muon/TSP (etc.) but the code is AdamW.
    FakeOptimizer,
    /// Training data overlapped the holdout.
    Contamination,
    /// Claimed public numbers do not match the harness public split.
    PublicMetricMismatch,
    /// Anything else the agent named; never a parse error.
    Other,
}

/// Structured final verdict the agent MUST emit via `submit_verdict`.
///
/// `holdout_nll` is not a field. If the agent emits one, the parser ignores
/// it: the agent never sees the holdout, so a number it invented is not a
/// measurement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[allow(clippy::struct_excessive_bools)]
pub struct AgentVerdict {
    /// clean | suspicious | reject.
    pub verdict: ProofKind,
    /// Whether the agent re-ran the recipe under the topic constraints.
    pub reproduced: bool,
    /// Whether the public-split claim matches what the agent measured.
    pub claim_holds_public: bool,
    /// Whether the agent saw holdout fingerprints in the recipe/data.
    pub contamination: bool,
    /// Off-score canary. Recorded, never added to NLL, never a fail by itself.
    pub canary_hit: bool,
    /// FLOPs the agent observed the run consume.
    pub flops_used: u64,
    /// Budget the agent was told. Must equal the topic's.
    pub flops_budget: u64,
    /// Zero or more cheat codes.
    pub cheat_codes: Vec<ProofCheatCode>,
    /// Audit rationale (truncated on ingest).
    pub rationale: String,
    /// Topic this verdict is about. Must echo the submission's `topic_id`.
    pub topic_id: String,
    /// Metric family this verdict is about. Must echo the topic's family.
    pub family: MetricFamily,
}

impl AgentVerdict {
    /// Truncate rationale so a miner cannot stuff the store.
    #[must_use]
    pub fn truncated(mut self) -> Self {
        const MAX: usize = 4_096;
        if self.rationale.len() > MAX {
            self.rationale.truncate(MAX);
        }
        self.cheat_codes.sort();
        self.cheat_codes.dedup();
        self
    }
}

/// Per-split (and overall) numbers the harness measured. Never agent-authored.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HarnessMetrics {
    /// Mean holdout NLL over the 120 scored records.
    pub holdout_nll: f64,
    /// Per-split mean NLL (`web_ood` … `multilingual_ood`).
    pub split_nll: BTreeMap<String, f64>,
    /// Public-split NLL (informational; never promotion).
    pub public_nll: Option<f64>,
    /// Throughput primary, when the family is throughput.
    pub tokens_per_sec: Option<f64>,
    /// Latency primary, when the family is throughput.
    pub step_latency_ms: Option<f64>,
    /// Wall seconds used (throughput).
    pub wall_s: Option<u64>,
    /// Custom metric value, when the family is custom.
    pub custom_value: Option<f64>,
    /// Off-score canary NLL, recorded and never mixed into `holdout_nll`.
    pub canary_nll: Option<f64>,
}

impl HarnessMetrics {
    /// NLL for one scored split.
    #[must_use]
    pub fn split(&self, name: &str) -> Option<f64> {
        self.split_nll.get(name).copied()
    }
}

/// Gate that blocked a pass (or would have).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateFail {
    /// Agent did not reproduce the recipe.
    Unreproduced,
    /// Agent marked contamination, or the harness fingerprint gate fired.
    Contamination,
    /// FLOPs used exceeded the topic budget.
    FlopsOverBudget {
        /// Observed.
        used: u64,
        /// Topic budget.
        budget: u64,
    },
    /// Wall time exceeded the topic budget.
    WallOverBudget {
        /// Observed seconds.
        used: u64,
        /// Topic budget.
        budget: u64,
    },
    /// Mean holdout NLL did not beat the sealed baseline by ε.
    NllMiss {
        /// Challenger mean.
        holdout: f64,
        /// Sealed mean.
        baseline: f64,
        /// Required absolute win.
        epsilon: f64,
    },
    /// One scored split regressed more than the topic allows.
    SplitRegress {
        /// Split name.
        split: String,
        /// Challenger NLL.
        holdout: f64,
        /// Sealed NLL.
        baseline: f64,
        /// Allowed regression.
        epsilon: f64,
    },
    /// Throughput/latency did not beat the sealed reference by the relative ε.
    ThroughputMiss {
        /// Challenger primary.
        value: f64,
        /// Sealed primary.
        baseline: f64,
        /// Required relative win.
        epsilon_rel: f64,
    },
    /// Speed traded away too much quality.
    QualityFloor {
        /// Challenger holdout NLL.
        holdout: f64,
        /// Sealed holdout NLL.
        baseline: f64,
        /// Allowed extra NLL.
        floor: f64,
    },
    /// A series the gate reads was missing (fail-closed).
    EvidenceMissing {
        /// Which series.
        field: String,
    },
    /// Agent verdict was suspicious or reject.
    AgentReject,
    /// Agent cheat codes that zero a run even when the harness numbers pass.
    Cheat {
        /// Code the agent named.
        code: ProofCheatCode,
    },
    /// Custom metric this host cannot compute.
    UnknownCustom,
}

/// Full pass / reject verdict. Consensus-critical once leaves are signed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProofVerdict {
    /// Whether this submission pays on this topic.
    pub pass: bool,
    /// Agent envelope (echoed).
    pub agent: AgentVerdict,
    /// Harness-owned numbers.
    pub harness: HarnessMetrics,
    /// Gates that failed (empty ⇒ pass).
    pub failed: Vec<GateFail>,
    /// Lattice for this topic (`SCORE_MAX` on pass, else 0).
    pub lattice: u64,
}

/// Sealed baseline numbers the harness compares against.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SealedBaseline {
    /// Mean holdout NLL.
    pub holdout_nll: f64,
    /// Per-split NLL.
    pub split_nll: BTreeMap<String, f64>,
    /// Sealed tokens/sec, when the family is throughput.
    pub tokens_per_sec: Option<f64>,
    /// Sealed step latency, when the family is throughput.
    pub step_latency_ms: Option<f64>,
    /// Sealed custom value.
    pub custom_value: Option<f64>,
}

fn finite(x: f64) -> bool {
    x.is_finite()
}

fn rel_win(challenger: f64, baseline: f64, direction: MetricDirection, epsilon: f64) -> bool {
    if !finite(challenger) || !finite(baseline) || baseline.abs() < 1e-12 {
        return false;
    }
    match direction {
        MetricDirection::Max => challenger >= baseline * (1.0 + epsilon),
        MetricDirection::Min => challenger <= baseline * (1.0 - epsilon),
    }
}

fn nll_gates(
    topic: &TopicDocument,
    harness: &HarnessMetrics,
    sealed: &SealedBaseline,
    failed: &mut Vec<GateFail>,
) {
    if !finite(harness.holdout_nll) || !finite(sealed.holdout_nll) {
        failed.push(GateFail::EvidenceMissing {
            field: "holdout_nll".into(),
        });
        return;
    }
    if harness.holdout_nll > sealed.holdout_nll - topic.epsilon_nll {
        failed.push(GateFail::NllMiss {
            holdout: harness.holdout_nll,
            baseline: sealed.holdout_nll,
            epsilon: topic.epsilon_nll,
        });
    }
    split_regress(topic, harness, sealed, failed);
}

fn split_regress(
    topic: &TopicDocument,
    harness: &HarnessMetrics,
    sealed: &SealedBaseline,
    failed: &mut Vec<GateFail>,
) {
    for split in HoldoutSplit::SCORED {
        let name = split.as_str();
        let (Some(h), Some(b)) = (harness.split(name), sealed.split_nll.get(name).copied()) else {
            failed.push(GateFail::EvidenceMissing {
                field: "split_nll".into(),
            });
            continue;
        };
        if !finite(h) || !finite(b) {
            failed.push(GateFail::EvidenceMissing {
                field: "split_nll".into(),
            });
            continue;
        }
        if h > b + topic.epsilon_topic_max_regress {
            failed.push(GateFail::SplitRegress {
                split: name.into(),
                holdout: h,
                baseline: b,
                epsilon: topic.epsilon_topic_max_regress,
            });
        }
    }
}

fn throughput_gates(
    topic: &TopicDocument,
    harness: &HarnessMetrics,
    sealed: &SealedBaseline,
    failed: &mut Vec<GateFail>,
) {
    let (value, baseline, field) = match topic.metric.primary.as_str() {
        proof_task::METRIC_TOKENS_PER_SEC => (
            harness.tokens_per_sec,
            sealed.tokens_per_sec,
            "tokens_per_sec",
        ),
        proof_task::METRIC_STEP_LATENCY_MS => (
            harness.step_latency_ms,
            sealed.step_latency_ms,
            "step_latency_ms",
        ),
        _ => {
            failed.push(GateFail::EvidenceMissing {
                field: "throughput_primary".into(),
            });
            return;
        }
    };
    match (value, baseline) {
        (Some(v), Some(b)) if finite(v) && finite(b) => {
            if !rel_win(v, b, topic.metric.direction, topic.metric.epsilon_rel) {
                failed.push(GateFail::ThroughputMiss {
                    value: v,
                    baseline: b,
                    epsilon_rel: topic.metric.epsilon_rel,
                });
            }
        }
        _ => failed.push(GateFail::EvidenceMissing {
            field: field.into(),
        }),
    }
    if !finite(harness.holdout_nll) || !finite(sealed.holdout_nll) {
        failed.push(GateFail::EvidenceMissing {
            field: "holdout_nll".into(),
        });
    } else if harness.holdout_nll > sealed.holdout_nll + topic.metric.quality_floor_nll {
        failed.push(GateFail::QualityFloor {
            holdout: harness.holdout_nll,
            baseline: sealed.holdout_nll,
            floor: topic.metric.quality_floor_nll,
        });
    }
    split_regress(topic, harness, sealed, failed);
    match (harness.wall_s, topic.metric.wall_budget_s) {
        (Some(used), budget) if used > budget => {
            failed.push(GateFail::WallOverBudget { used, budget });
        }
        (None, _) => failed.push(GateFail::EvidenceMissing {
            field: "wall_s".into(),
        }),
        _ => {}
    }
}

fn custom_gates(
    topic: &TopicDocument,
    harness: &HarnessMetrics,
    sealed: &SealedBaseline,
    supported_custom: &[&str],
    failed: &mut Vec<GateFail>,
) {
    if !supported_custom.contains(&topic.metric.custom_id.as_str()) {
        failed.push(GateFail::UnknownCustom);
        return;
    }
    match (harness.custom_value, sealed.custom_value) {
        (Some(v), Some(b)) if finite(v) && finite(b) => {
            if !rel_win(v, b, topic.metric.direction, topic.metric.epsilon_rel) {
                failed.push(GateFail::ThroughputMiss {
                    value: v,
                    baseline: b,
                    epsilon_rel: topic.metric.epsilon_rel,
                });
            }
        }
        _ => failed.push(GateFail::EvidenceMissing {
            field: "custom_value".into(),
        }),
    }
}

/// Judge one submission against one topic's sealed baseline.
///
/// The lattice is binary: [`SCORE_MAX`] on a clean pass, else 0. Promotion
/// is holdout-vs-sealed-baseline only; the public split never enters.
#[must_use]
pub fn judge_topic(
    topic: &TopicDocument,
    agent: &AgentVerdict,
    harness: &HarnessMetrics,
    sealed: &SealedBaseline,
    contamination_hits: &[String],
    supported_custom: &[&str],
) -> ProofVerdict {
    let mut failed = Vec::new();
    if !agent.reproduced {
        failed.push(GateFail::Unreproduced);
    }
    if agent.contamination || !contamination_hits.is_empty() {
        failed.push(GateFail::Contamination);
    }
    if agent.flops_used > topic.flops_budget {
        failed.push(GateFail::FlopsOverBudget {
            used: agent.flops_used,
            budget: topic.flops_budget,
        });
    }
    match agent.verdict {
        ProofKind::Clean => {}
        ProofKind::Suspicious | ProofKind::Reject => failed.push(GateFail::AgentReject),
    }
    for code in &agent.cheat_codes {
        if *code != ProofCheatCode::Other {
            failed.push(GateFail::Cheat { code: *code });
        }
    }
    match topic.metric.family {
        MetricFamily::Nll => nll_gates(topic, harness, sealed, &mut failed),
        MetricFamily::Throughput => throughput_gates(topic, harness, sealed, &mut failed),
        MetricFamily::Custom => {
            custom_gates(topic, harness, sealed, supported_custom, &mut failed);
        }
    }
    failed.sort_by(|a, b| format!("{a:?}").cmp(&format!("{b:?}")));
    failed.dedup_by(|a, b| format!("{a:?}") == format!("{b:?}"));
    let pass = failed.is_empty();
    ProofVerdict {
        pass,
        agent: agent.clone(),
        harness: harness.clone(),
        failed,
        lattice: if pass { SCORE_MAX } else { 0 },
    }
}

/// Sum of per-topic lattices over currently open topics, capped at [`SCORE_MAX`].
///
/// Paid emission uses [`payout_lattices`] (WTA / discovery). This helper is
/// the binary fallback: each topic is 0 or [`SCORE_MAX`], then averaged so a
/// skipped open topic still pulls the miner down. An empty open set is a host
/// problem (503), not a miner score of 0 — callers must not emit this as a paid
/// leaf.
#[must_use]
pub fn mean_lattice(per_topic: &BTreeMap<String, u64>, open_ids: &[String]) -> u64 {
    if open_ids.is_empty() {
        return 0;
    }
    let mut sum: u128 = 0;
    for id in open_ids {
        sum = sum.saturating_add(u128::from(
            per_topic.get(id).copied().unwrap_or(0).min(SCORE_MAX),
        ));
    }
    u64::try_from(sum / u128::from(open_ids.len() as u64)).unwrap_or(0)
}

/// Empty split map with one slot per scored stratum (tests / sim).
#[must_use]
pub fn empty_splits() -> BTreeMap<String, f64> {
    HoldoutSplit::SCORED
        .iter()
        .map(|s| (s.as_str().to_owned(), 0.0))
        .collect()
}

/// Fixture: a sealed NLL vector at `nll` on every scored split.
#[must_use]
pub fn flat_nll(nll: f64) -> SealedBaseline {
    SealedBaseline {
        holdout_nll: nll,
        split_nll: HoldoutSplit::SCORED
            .iter()
            .map(|s| (s.as_str().to_owned(), nll))
            .collect(),
        tokens_per_sec: None,
        step_latency_ms: None,
        custom_value: None,
    }
}

/// The 24-per-split floor is part of the pass rule's evidence, not the
/// problem list. Exposed so tests can name it without reaching into task.
pub const SCORED_PER_SPLIT: usize = STRATUM_SIZE;

#[cfg(test)]
mod tests {
    use proof_task::{
        default_adamw, holdout_commitment, synthetic_holdout, Constraints, MetricDirection,
        MetricFamily, MetricSpec, TopicDocument, TopicStatus, FLOPS_BUDGET_MAX,
        METRIC_TOKENS_PER_SEC,
    };

    use super::*;

    fn sealed_adamw() -> proof_task::Baseline {
        let mut b = default_adamw(FLOPS_BUDGET_MAX);
        b.script_sha256 = "11".repeat(32);
        b.metrics_commitment = "22".repeat(32);
        b
    }

    fn nll_topic() -> TopicDocument {
        TopicDocument {
            id: "adamw-beater-v0".into(),
            statement: "Beat sealed AdamW holdout NLL.".into(),
            baseline: sealed_adamw(),
            holdout_commitment: holdout_commitment(&synthetic_holdout(24, 1)),
            status: TopicStatus::Open,
            ..TopicDocument::default()
        }
    }

    fn throughput_topic() -> TopicDocument {
        let mut baseline = sealed_adamw();
        baseline.optimizer = "nccl-ib-reference".into();
        baseline.wall_budget_s = 14_400;
        TopicDocument {
            id: "dt-no-ib-v0".into(),
            statement: "No IB/NVLink; 12.5 Gbit/s cap; beat sealed comms baseline.".into(),
            constraints: Constraints {
                no_infiniband: true,
                no_nvlink: true,
                no_nccl_fast_fabric: true,
                max_inter_node_gbps: Some(12.5),
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
            holdout_commitment: holdout_commitment(&synthetic_holdout(24, 1)),
            status: TopicStatus::Open,
            ..TopicDocument::default()
        }
    }

    fn clean_agent(topic: &str, family: MetricFamily, flops: u64) -> AgentVerdict {
        AgentVerdict {
            verdict: ProofKind::Clean,
            reproduced: true,
            claim_holds_public: true,
            contamination: false,
            canary_hit: false,
            flops_used: flops,
            flops_budget: FLOPS_BUDGET_MAX,
            cheat_codes: Vec::new(),
            rationale: "reproduced under the cap".into(),
            topic_id: topic.into(),
            family,
        }
    }

    fn nll_harness(nll: f64) -> HarnessMetrics {
        HarnessMetrics {
            holdout_nll: nll,
            split_nll: HoldoutSplit::SCORED
                .iter()
                .map(|s| (s.as_str().to_owned(), nll))
                .collect(),
            public_nll: Some(nll),
            tokens_per_sec: None,
            step_latency_ms: None,
            wall_s: None,
            custom_value: None,
            canary_nll: None,
        }
    }

    #[test]
    fn a_clean_nll_win_pays_and_a_miss_is_zero() {
        let topic = nll_topic();
        let sealed = flat_nll(3.0);
        let agent = clean_agent(&topic.id, MetricFamily::Nll, 1_000);
        let win = judge_topic(&topic, &agent, &nll_harness(2.97), &sealed, &[], &[]);
        assert!(win.pass, "{:?}", win.failed);
        assert_eq!(win.lattice, SCORE_MAX);

        let miss = judge_topic(&topic, &agent, &nll_harness(2.99), &sealed, &[], &[]);
        assert!(!miss.pass);
        assert_eq!(miss.lattice, 0);
        assert!(miss
            .failed
            .iter()
            .any(|f| matches!(f, GateFail::NllMiss { .. })));
    }

    #[test]
    fn one_split_regressing_past_epsilon_zeros_the_run() {
        let topic = nll_topic();
        let sealed = flat_nll(3.0);
        let mut harness = nll_harness(2.90);
        harness.split_nll.insert("longctx".into(), 3.10);
        let v = judge_topic(
            &topic,
            &clean_agent(&topic.id, MetricFamily::Nll, 1),
            &harness,
            &sealed,
            &[],
            &[],
        );
        assert!(v
            .failed
            .iter()
            .any(|f| matches!(f, GateFail::SplitRegress { split, .. } if split == "longctx")));
        assert_eq!(v.lattice, 0);
    }

    #[test]
    fn canary_hit_is_recorded_and_does_not_fail() {
        let topic = nll_topic();
        let mut agent = clean_agent(&topic.id, MetricFamily::Nll, 1);
        agent.canary_hit = true;
        let mut harness = nll_harness(2.90);
        harness.canary_nll = Some(9.9);
        let v = judge_topic(&topic, &agent, &harness, &flat_nll(3.0), &[], &[]);
        assert!(v.pass, "{:?}", v.failed);
        assert!(v.agent.canary_hit);
        assert_eq!(v.harness.canary_nll, Some(9.9));
    }

    #[test]
    fn unreproduced_or_contaminated_is_a_hard_zero() {
        let topic = nll_topic();
        let mut agent = clean_agent(&topic.id, MetricFamily::Nll, 1);
        agent.reproduced = false;
        let v = judge_topic(&topic, &agent, &nll_harness(1.0), &flat_nll(3.0), &[], &[]);
        assert!(v.failed.contains(&GateFail::Unreproduced));
        assert_eq!(v.lattice, 0);

        agent.reproduced = true;
        let dirty = judge_topic(
            &topic,
            &agent,
            &nll_harness(1.0),
            &flat_nll(3.0),
            &["shard:aa".into()],
            &[],
        );
        assert!(dirty.failed.contains(&GateFail::Contamination));
    }

    /// dt-no-ib-v0: must beat tokens/sec by 5% AND keep NLL within the quality
    /// floor. Speed that wrecks quality is not a proof.
    #[test]
    fn throughput_requires_a_relative_win_and_the_quality_floor() {
        let topic = throughput_topic();
        let mut sealed = flat_nll(3.0);
        sealed.tokens_per_sec = Some(100.0);
        let mut harness = nll_harness(3.01);
        harness.tokens_per_sec = Some(106.0);
        harness.wall_s = Some(10_000);
        let agent = clean_agent(&topic.id, MetricFamily::Throughput, 1);
        let win = judge_topic(&topic, &agent, &harness, &sealed, &[], &[]);
        assert!(win.pass, "{:?}", win.failed);

        harness.tokens_per_sec = Some(101.0);
        let slow = judge_topic(&topic, &agent, &harness, &sealed, &[], &[]);
        assert!(slow
            .failed
            .iter()
            .any(|f| matches!(f, GateFail::ThroughputMiss { .. })));

        harness.tokens_per_sec = Some(120.0);
        harness.holdout_nll = 3.05;
        let junk = judge_topic(&topic, &agent, &harness, &sealed, &[], &[]);
        assert!(junk
            .failed
            .iter()
            .any(|f| matches!(f, GateFail::QualityFloor { .. })));
    }

    #[test]
    fn an_unimplemented_custom_metric_is_fail_closed_not_a_pass() {
        let mut topic = nll_topic();
        topic.metric.family = MetricFamily::Custom;
        topic.metric.custom_id = "bits_per_joule".into();
        topic.metric.primary = "bits_per_joule".into();
        topic.metric.epsilon_rel = 0.1;
        let v = judge_topic(
            &topic,
            &clean_agent(&topic.id, MetricFamily::Custom, 1),
            &nll_harness(1.0),
            &flat_nll(3.0),
            &[],
            &[],
        );
        assert!(v.failed.contains(&GateFail::UnknownCustom));
        assert_eq!(v.lattice, 0);
    }

    #[test]
    fn harness_success_rate_is_listed_and_fail_closes_without_a_harness_value() {
        let mut topic = nll_topic();
        topic.metric.family = MetricFamily::Custom;
        topic.metric.custom_id = proof_task::CUSTOM_HARNESS_SUCCESS_RATE.into();
        topic.metric.primary = "success_rate".into();
        topic.metric.direction = MetricDirection::Max;
        topic.metric.epsilon_rel = 0.05;
        let mut harness = nll_harness(1.0);
        harness.custom_value = None;
        let v = judge_topic(
            &topic,
            &clean_agent(&topic.id, MetricFamily::Custom, 1),
            &harness,
            &flat_nll(3.0),
            &[],
            &[proof_task::CUSTOM_HARNESS_SUCCESS_RATE],
        );
        assert!(v
            .failed
            .iter()
            .any(|f| matches!(f, GateFail::EvidenceMissing { field } if field == "custom_value")));
        assert_eq!(v.lattice, 0);
    }

    #[test]
    fn skipped_open_topics_pull_the_mean_to_zero() {
        let mut scores = BTreeMap::new();
        scores.insert("dt-no-ib-v0".into(), SCORE_MAX);
        let open = ["dt-no-ib-v0".into(), "other-v0".into()];
        assert_eq!(mean_lattice(&scores, &open), SCORE_MAX / 2);
        assert_eq!(mean_lattice(&scores, &[]), 0);
        assert_eq!(topic_share_bps(2), 4_000);
        assert_eq!(topic_share_bps(0), 0);
        assert_eq!(PROOF_SHARE_BPS, 8_000);
    }

    #[test]
    fn agent_cheat_codes_zero_even_when_numbers_pass() {
        let topic = nll_topic();
        let mut agent = clean_agent(&topic.id, MetricFamily::Nll, 1);
        agent.cheat_codes = vec![ProofCheatCode::FakeOptimizer];
        let v = judge_topic(&topic, &agent, &nll_harness(1.0), &flat_nll(3.0), &[], &[]);
        assert!(v.failed.iter().any(|f| matches!(
            f,
            GateFail::Cheat {
                code: ProofCheatCode::FakeOptimizer
            }
        )));
        assert_eq!(v.lattice, 0);
    }
}
