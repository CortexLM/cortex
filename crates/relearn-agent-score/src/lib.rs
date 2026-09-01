//! Displacement scoring for Relearn Agent.
//!
//! The visible lattice is the bootstrap paired test on holdout **task
//! success**, and nothing else. Every other measurement here is a gate: it can
//! zero a run, and it can never raise one. Miners overfit whatever pays.
//!
//! What the gates are actually defending is the difference between an agent
//! and a model that memorised the answer. Task success alone cannot tell those
//! apart, so a verdict needs three pieces of counter-evidence, all produced by
//! the same eval run:
//!
//! - **Trace replay** — the emitted tool calls are re-executed against the
//!   episode's environment. A call whose arguments are not derivable from the
//!   goal or an earlier observation, or a final answer that appears before the
//!   observation that supports it, is not a grounded solve.
//! - **Tool ablation** — the same episodes with the tools stubbed out. If
//!   success barely moves, the environment was never load-bearing.
//! - **Observation shuffle** — the same episodes with another episode's
//!   observation. If success barely moves, the model answered the prompt, not
//!   the task.
//!
//! Missing counter-evidence is a failure, not a pass: an eval that returned no
//! ablation arm cannot show the model used its tools, and this challenge does
//! not crown "unproven".

#![forbid(unsafe_code)]
#![allow(
    clippy::doc_markdown,
    clippy::module_name_repetitions,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use prism_competition::{
    paired_test, Direction, ExampleSeries, PairedInput, PairedOutcome, PairedRefusal, DEADZONE,
};
use relearn_agent_task::{HOLDOUT_SLICE_ID, SCORE_MAX};
use serde::{Deserialize, Serialize};

/// Minimum mean trace validity over the holdout.
///
/// Below this the run reached answers the harness could not re-derive, which
/// is indistinguishable from recall plus a plausible-looking transcript.
pub const MIN_TRACE_VALIDITY: f64 = 0.80;

/// Minimum success drop when the episode's tools are stubbed out.
pub const MIN_ABLATION_DROP: f64 = 0.20;

/// Minimum success drop when the episode's observation is swapped.
pub const MIN_SHUFFLE_DROP: f64 = 0.15;

/// Largest tolerated public-minus-holdout gap (memorization signal).
pub const MAX_PUBLIC_HOLDOUT_GAP: f64 = 0.08;

/// Largest tolerated drop vs the champion on the capability canary.
///
/// The canary is **off** the visible score. A regression past this epsilon is
/// a hard zero, not a reduced lattice.
pub const CANARY_EPSILON: f64 = 0.02;

/// Counter-evidence from re-running an episode set under a changed condition.
///
/// `episodes == 0` means the arm did not run. That is fail-closed everywhere:
/// the whole point of the arm is to prove a negative.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct AblationEvidence {
    /// Episodes measured under the changed condition.
    pub episodes: u32,
    /// Mean success with the environment intact (`0..=1`).
    pub score: f64,
    /// Mean success under the changed condition (`0..=1`).
    pub ablated_score: f64,
}

impl AblationEvidence {
    /// How much success fell when the condition changed.
    #[must_use]
    pub fn drop(&self) -> f64 {
        self.score - self.ablated_score
    }

    /// Whether the arm ran and the drop clears `min_drop`.
    #[must_use]
    pub fn shows_dependence(&self, min_drop: f64) -> bool {
        self.episodes > 0 && self.drop() >= min_drop - DEADZONE
    }
}

/// What a submission declared about its training data, plus what leaked.
///
/// An empty manifest is *absence of evidence*, not a clean bill of health.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContaminationEvidence {
    /// Distinct episode ids the submission declared training on.
    pub declared_episode_ids: usize,
    /// Distinct observation hashes the submission declared.
    pub declared_observation_hashes: usize,
    /// Distinct environment ids the submission declared.
    pub declared_environment_ids: usize,
    /// Holdout fingerprints found inside the declared metadata.
    pub hits: Vec<String>,
}

impl ContaminationEvidence {
    /// Whether the submission declared anything the gate could check.
    #[must_use]
    pub fn is_declared(&self) -> bool {
        self.declared_episode_ids > 0
            || self.declared_observation_hashes > 0
            || self.declared_environment_ids > 0
    }
}

/// Per-artifact Agent measurements. Series keys are `e{episode_id}`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AgentSliceScores {
    /// Holdout task success (the only series that may enter the lattice).
    pub holdout: ExampleSeries,
    /// Published episode split (memorization detector; never lattice).
    pub public: ExampleSeries,
    /// Per-episode trace validity from the replay.
    pub trace_valid: ExampleSeries,
    /// General instruction-following slice. Off the visible score path.
    pub capability_canary: ExampleSeries,
    /// Success with the episode's tools stubbed out.
    pub tool_ablation: AblationEvidence,
    /// Success with another episode's observation.
    pub observation_shuffle: AblationEvidence,
    /// Declared training metadata and the holdout fingerprints inside it.
    pub contamination: ContaminationEvidence,
}

impl AgentSliceScores {
    /// Mean of a series, or `None` when empty.
    #[must_use]
    pub fn mean(series: &ExampleSeries) -> Option<f64> {
        if series.is_empty() {
            return None;
        }
        let n = series.len() as f64;
        Some(series.by_cluster.values().sum::<f64>() / n)
    }
}

/// Gate that blocked promotion (or would have).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateFail {
    /// Challenger is not a significant paired win on holdout success.
    NoPairedWin,
    /// Challenger lost or tied the champion (never crown a regression).
    Regression,
    /// Replayed traces do not re-derive the answers the run claimed.
    TraceInvalid {
        /// Observed mean validity in bps.
        validity_bps: u64,
    },
    /// The trace replay did not run.
    TraceEvidenceMissing,
    /// Stubbing the tools barely changed the score: the model is not using
    /// the environment it is paid to use.
    AnswersWithoutTools {
        /// Observed drop in bps.
        drop_bps: u64,
    },
    /// The tool-ablation arm did not run.
    ToolAblationEvidenceMissing,
    /// Swapping the observation barely changed the score.
    IgnoresTheObservation {
        /// Observed drop in bps.
        drop_bps: u64,
    },
    /// The observation-shuffle arm did not run.
    ObservationShuffleEvidenceMissing,
    /// Public split far above holdout (memorization / contamination).
    PublicHoldoutGap,
    /// Public split evidence was missing (fail-closed).
    PublicEvidenceMissing,
    /// Capability canary regressed past [`CANARY_EPSILON`].
    CapabilityCanaryRegression {
        /// Size of the drop in bps.
        drop_bps: u64,
    },
    /// The capability canary did not run on one of the two sides.
    CapabilityCanaryEvidenceMissing,
    /// Holdout episode ids / observation hashes appear in training metadata.
    Contamination,
    /// The submission declared no training metadata, so the contamination
    /// gate had nothing to check (fail-closed).
    ContaminationEvidenceMissing,
    /// Paired test refused (slice mismatch / too thin).
    PairedRefusal,
}

/// Serializable paired-test summary (prism `PairedOutcome` is not serde).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairedSummary {
    /// Overlapping episodes.
    pub n_paired: u64,
    /// Episodes outside the dead zone.
    pub n_decided: u64,
    /// Bootstrap LCB win-rate (bps).
    pub win_rate_lcb_bps: u64,
    /// Challenger displaces champion.
    pub displaces: bool,
}

impl PairedSummary {
    fn from_outcome(o: &PairedOutcome) -> Self {
        Self {
            n_paired: u64::try_from(o.n_paired).unwrap_or(u64::MAX),
            n_decided: u64::try_from(o.n_decided).unwrap_or(u64::MAX),
            win_rate_lcb_bps: o.win_rate_lcb_bps,
            displaces: o.displaces,
        }
    }
}

/// Full promote / reject verdict. Consensus-critical once leaves are signed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromoteVerdict {
    /// Whether this submission may become champion after operator audit.
    pub eligible: bool,
    /// Paired-test outcome when the slices lined up.
    pub paired: Option<PairedSummary>,
    /// Tool-ablation evidence, reported whether or not it gated.
    pub tool_ablation: AblationEvidence,
    /// Observation-shuffle evidence.
    pub observation_shuffle: AblationEvidence,
    /// Gates that failed (empty ⇒ all clear).
    pub failed: Vec<GateFail>,
    /// Lattice score to emit if this hotkey is the live champion (`0` else).
    pub lattice: u64,
}

fn bps(x: f64) -> u64 {
    (x * 10_000.0).round().max(0.0) as u64
}

/// Judge challenger vs champion. Never returns `eligible` on a regression.
///
/// The lattice is holdout-only. Trace replay, ablation, shuffle, public gap,
/// canary, and contamination can only zero a run. Missing evidence — no trace
/// replay, no ablation arm, no declared training metadata — is a fail.
#[must_use]
pub fn judge_challenger(
    champion: &AgentSliceScores,
    challenger: &AgentSliceScores,
) -> PromoteVerdict {
    let mut failed = Vec::new();

    let paired_raw = match paired_test(&PairedInput {
        metric: "relearn_agent.task_success".into(),
        direction: Direction::HigherBetter,
        slice_id: HOLDOUT_SLICE_ID.into(),
        champion: champion.holdout.clone(),
        challenger: challenger.holdout.clone(),
    }) {
        Ok(o) => Some(o),
        Err(
            PairedRefusal::NotEnoughDecided
            | PairedRefusal::NoOverlap
            | PairedRefusal::SliceMismatch,
        ) => {
            failed.push(GateFail::PairedRefusal);
            None
        }
    };

    match paired_raw {
        Some(ref o) if o.displaces => {}
        Some(_) => {
            failed.push(GateFail::NoPairedWin);
            failed.push(GateFail::Regression);
        }
        None => failed.push(GateFail::NoPairedWin),
    }

    match AgentSliceScores::mean(&challenger.trace_valid) {
        None => failed.push(GateFail::TraceEvidenceMissing),
        Some(v) => {
            if v + DEADZONE < MIN_TRACE_VALIDITY {
                failed.push(GateFail::TraceInvalid {
                    validity_bps: bps(v),
                });
            }
        }
    }

    if challenger.tool_ablation.episodes == 0 {
        failed.push(GateFail::ToolAblationEvidenceMissing);
    } else if !challenger.tool_ablation.shows_dependence(MIN_ABLATION_DROP) {
        failed.push(GateFail::AnswersWithoutTools {
            drop_bps: bps(challenger.tool_ablation.drop().max(0.0)),
        });
    }

    if challenger.observation_shuffle.episodes == 0 {
        failed.push(GateFail::ObservationShuffleEvidenceMissing);
    } else if !challenger
        .observation_shuffle
        .shows_dependence(MIN_SHUFFLE_DROP)
    {
        failed.push(GateFail::IgnoresTheObservation {
            drop_bps: bps(challenger.observation_shuffle.drop().max(0.0)),
        });
    }

    match (
        AgentSliceScores::mean(&challenger.public),
        AgentSliceScores::mean(&challenger.holdout),
    ) {
        (None, _) => failed.push(GateFail::PublicEvidenceMissing),
        (Some(pub_m), Some(hold_m)) => {
            if pub_m - hold_m > MAX_PUBLIC_HOLDOUT_GAP + DEADZONE {
                failed.push(GateFail::PublicHoldoutGap);
            }
        }
        (Some(_), None) => failed.push(GateFail::PairedRefusal),
    }

    match (
        AgentSliceScores::mean(&champion.capability_canary),
        AgentSliceScores::mean(&challenger.capability_canary),
    ) {
        (None, _) | (_, None) => failed.push(GateFail::CapabilityCanaryEvidenceMissing),
        (Some(champ_c), Some(chal_c)) => {
            let drop = champ_c - chal_c;
            if drop > CANARY_EPSILON + DEADZONE {
                failed.push(GateFail::CapabilityCanaryRegression {
                    drop_bps: bps(drop),
                });
            }
        }
    }

    if challenger.contamination.is_declared() {
        if !challenger.contamination.hits.is_empty() {
            failed.push(GateFail::Contamination);
        }
    } else {
        failed.push(GateFail::ContaminationEvidenceMissing);
    }

    failed.sort_by(|a, b| format!("{a:?}").cmp(&format!("{b:?}")));
    failed.dedup();

    let eligible = failed.is_empty();
    let lattice = if eligible {
        paired_raw
            .as_ref()
            .map_or(0, |o| lattice_from_win_rate(o.win_rate_lcb_bps))
    } else {
        0
    };

    PromoteVerdict {
        eligible,
        paired: paired_raw.as_ref().map(PairedSummary::from_outcome),
        tool_ablation: challenger.tool_ablation,
        observation_shuffle: challenger.observation_shuffle,
        failed,
        lattice,
    }
}

/// Map bootstrap LCB win-rate (bps) onto the lattice. Champion-hold → 0.
#[must_use]
pub fn lattice_from_win_rate(win_rate_lcb_bps: u64) -> u64 {
    let clamped = win_rate_lcb_bps.min(10_000);
    u64::from(u32::try_from((u128::from(SCORE_MAX) * u128::from(clamped)) / 10_000).unwrap_or(0))
}

/// Champion row keeps a positive lattice so emission does not burn solely
/// because a challenger was rejected.
#[must_use]
pub fn champion_hold_lattice() -> u64 {
    SCORE_MAX / 2
}

#[cfg(test)]
mod tests {
    use super::*;

    fn series(prefix: &str, n: usize, val: f64) -> ExampleSeries {
        ExampleSeries::from_pairs((0..n).map(|i| (format!("{prefix}{i}"), val)))
    }

    fn declared_clean() -> ContaminationEvidence {
        ContaminationEvidence {
            declared_episode_ids: 40,
            declared_observation_hashes: 12,
            declared_environment_ids: 2,
            hits: Vec::new(),
        }
    }

    fn grounded(success: f64) -> AblationEvidence {
        AblationEvidence {
            episodes: 40,
            score: success,
            ablated_score: (success - 0.45).max(0.0),
        }
    }

    fn slice(success: f64) -> AgentSliceScores {
        AgentSliceScores {
            holdout: series("e", 120, success),
            public: series("p", 120, success),
            trace_valid: series("e", 120, 0.95),
            capability_canary: series("c", 40, 0.97),
            tool_ablation: grounded(success),
            observation_shuffle: grounded(success),
            contamination: declared_clean(),
        }
    }

    #[test]
    fn a_grounded_win_is_eligible_and_pays_from_the_holdout_only() {
        let v = judge_challenger(&slice(0.40), &slice(0.80));
        assert!(v.eligible, "failed={:?}", v.failed);
        assert!(v.lattice > 0);
        assert!(v.paired.expect("paired").displaces);
    }

    #[test]
    fn never_crowns_a_regression() {
        let v = judge_challenger(&slice(0.80), &slice(0.40));
        assert!(!v.eligible);
        assert!(v.failed.contains(&GateFail::Regression));
        assert_eq!(v.lattice, 0);
    }

    /// The headline gate: a model that answers just as well with the tools
    /// stubbed out did not use them, however high its task success is.
    #[test]
    fn answering_without_the_tools_is_a_hard_zero() {
        let mut chal = slice(0.90);
        chal.tool_ablation = AblationEvidence {
            episodes: 40,
            score: 0.90,
            ablated_score: 0.88,
        };
        let v = judge_challenger(&slice(0.40), &chal);
        assert!(
            v.failed
                .iter()
                .any(|f| matches!(f, GateFail::AnswersWithoutTools { .. })),
            "{:?}",
            v.failed
        );
        assert!(!v.eligible);
        assert_eq!(v.lattice, 0);
        assert!(
            v.paired.expect("holdout still ran").displaces,
            "the ablation arm must not be mixed into the holdout win"
        );
    }

    /// Same shape for the observation: solving the task without looking at
    /// what the episode handed you is answering the prompt, not the task.
    #[test]
    fn ignoring_the_observation_is_a_hard_zero() {
        let mut chal = slice(0.90);
        chal.observation_shuffle = AblationEvidence {
            episodes: 40,
            score: 0.90,
            ablated_score: 0.89,
        };
        let v = judge_challenger(&slice(0.40), &chal);
        assert!(
            v.failed
                .iter()
                .any(|f| matches!(f, GateFail::IgnoresTheObservation { .. })),
            "{:?}",
            v.failed
        );
        assert_eq!(v.lattice, 0);
    }

    #[test]
    fn an_arm_that_did_not_run_is_fail_closed_not_a_pass() {
        for break_it in 0..3 {
            let mut chal = slice(0.80);
            let expect = match break_it {
                0 => {
                    chal.tool_ablation = AblationEvidence::default();
                    GateFail::ToolAblationEvidenceMissing
                }
                1 => {
                    chal.observation_shuffle = AblationEvidence::default();
                    GateFail::ObservationShuffleEvidenceMissing
                }
                _ => {
                    chal.trace_valid = ExampleSeries::default();
                    GateFail::TraceEvidenceMissing
                }
            };
            let v = judge_challenger(&slice(0.40), &chal);
            assert!(v.failed.contains(&expect), "{:?}", v.failed);
            assert!(!v.eligible);
        }
    }

    /// A run that reached the answers but could not re-derive them is not
    /// distinguishable from recall plus a plausible transcript.
    #[test]
    fn an_unreplayable_trace_blocks() {
        let mut chal = slice(0.85);
        chal.trace_valid = series("e", 120, 0.40);
        let v = judge_challenger(&slice(0.40), &chal);
        assert!(
            v.failed
                .iter()
                .any(|f| matches!(f, GateFail::TraceInvalid { .. })),
            "{:?}",
            v.failed
        );
    }

    #[test]
    fn capability_canary_regression_is_off_the_lattice_and_fatal() {
        let mut chal = slice(0.85);
        chal.capability_canary = series("c", 40, 0.60);
        let v = judge_challenger(&slice(0.40), &chal);
        assert!(
            v.failed
                .iter()
                .any(|f| matches!(f, GateFail::CapabilityCanaryRegression { .. })),
            "{:?}",
            v.failed
        );
        assert_eq!(v.lattice, 0);

        let mut missing = slice(0.85);
        missing.capability_canary = ExampleSeries::default();
        assert!(judge_challenger(&slice(0.40), &missing)
            .failed
            .contains(&GateFail::CapabilityCanaryEvidenceMissing));
    }

    #[test]
    fn canary_noise_inside_epsilon_is_tolerated() {
        let mut chal = slice(0.85);
        chal.capability_canary = series("c", 40, 0.96);
        assert!(judge_challenger(&slice(0.40), &chal).eligible);
    }

    #[test]
    fn public_far_above_holdout_blocks_and_an_empty_public_is_fail_closed() {
        let mut leak = slice(0.80);
        leak.public = series("p", 120, 0.99);
        assert!(judge_challenger(&slice(0.40), &leak)
            .failed
            .contains(&GateFail::PublicHoldoutGap));

        let mut blind = slice(0.80);
        blind.public = ExampleSeries::default();
        assert!(judge_challenger(&slice(0.40), &blind)
            .failed
            .contains(&GateFail::PublicEvidenceMissing));
    }

    #[test]
    fn contamination_and_undeclared_metadata_both_block() {
        let mut dirty = slice(0.80);
        dirty.contamination.hits = vec!["episode:900".into()];
        assert!(judge_challenger(&slice(0.40), &dirty)
            .failed
            .contains(&GateFail::Contamination));

        let mut silent = slice(0.80);
        silent.contamination = ContaminationEvidence::default();
        let v = judge_challenger(&slice(0.40), &silent);
        assert!(v.failed.contains(&GateFail::ContaminationEvidenceMissing));
        assert_eq!(v.lattice, 0);
    }

    #[test]
    fn thin_overlap_refuses_rather_than_promotes() {
        let champ = AgentSliceScores {
            holdout: series("e", 8, 0.4),
            ..slice(0.4)
        };
        let chal = AgentSliceScores {
            holdout: series("e", 8, 0.9),
            ..slice(0.9)
        };
        let v = judge_challenger(&champ, &chal);
        assert!(!v.eligible);
        assert!(v.failed.contains(&GateFail::PairedRefusal));
    }

    #[test]
    fn lattice_endpoints() {
        assert_eq!(lattice_from_win_rate(0), 0);
        assert_eq!(lattice_from_win_rate(10_000), SCORE_MAX);
        assert_eq!(champion_hold_lattice(), SCORE_MAX / 2);
    }
}
