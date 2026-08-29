//! Displacement scoring for Relearn.
//!
//! Score is challenger vs the previous champion on a shared holdout slice.
//! A regression is never crowned. Promotion additionally requires the
//! operator-audited paired win plus retention / overfit gates.

#![forbid(unsafe_code)]
#![allow(
    clippy::missing_errors_doc,
    clippy::doc_markdown,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation
)]

use prism_competition::{
    paired_test, Direction, ExampleSeries, PairedInput, PairedOutcome, PairedRefusal, DEADZONE,
};
use relearn_challenge_task::SCORE_MAX;
use serde::{Deserialize, Serialize};

/// Maximum allowed public-private accuracy gap (absolute).
pub const MAX_PUBLIC_PRIVATE_GAP: f64 = 0.08;

/// Maximum allowed drop under input perturbation (absolute).
pub const MAX_PERTURB_DROP: f64 = 0.05;

/// Minimum canary accuracy (known-answer items the base model already solves).
pub const MIN_CANARY_ACCURACY: f64 = 0.95;

/// Minimum agent-trace score (first-class; 0..1).
pub const MIN_AGENT_TRACE: f64 = 0.5;

/// Per-example holdout measurements for one submission.
#[derive(Debug, Clone, PartialEq)]
pub struct SliceScores {
    /// Holdout items (scored artifact). Higher is better.
    pub holdout: ExampleSeries,
    /// Public / training-adjacent canary slice (overfit detector).
    pub public: ExampleSeries,
    /// Same holdout items after a pinned perturbation.
    pub perturbed: ExampleSeries,
    /// Known-answer canaries (base-model already-correct items).
    pub canaries: ExampleSeries,
    /// Agent-trace quality in `[0, 1]` (first-class; not a side channel).
    pub agent_trace: f64,
}

impl SliceScores {
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
    /// Challenger is not a significant paired win.
    NoPairedWin,
    /// Challenger lost or tied the champion (never crown a regression).
    Regression,
    /// Public-private gap too large (memorization / contamination).
    PublicPrivateGap,
    /// Perturbed holdout collapsed (brittle / overfit).
    Perturbation,
    /// Canaries failed (catastrophic forgetting of base competence).
    Canaries,
    /// Agent-trace score below floor.
    AgentTrace,
    /// Paired test refused (slice mismatch / too thin).
    PairedRefusal,
}

/// Serializable paired-test summary (prism `PairedOutcome` is not serde).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairedSummary {
    /// Overlapping examples.
    pub n_paired: u64,
    /// Decided examples (outside the dead zone).
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
    /// Gates that failed (empty ⇒ all clear).
    pub failed: Vec<GateFail>,
    /// Lattice score to emit if this hotkey is the live champion (`0` otherwise).
    pub lattice: u64,
}

/// Judge challenger vs champion. Never returns `eligible` on a regression.
#[must_use]
pub fn judge_challenger(champion: &SliceScores, challenger: &SliceScores) -> PromoteVerdict {
    let mut failed = Vec::new();

    let input = PairedInput {
        metric: "relearn.holdout".into(),
        direction: Direction::HigherBetter,
        slice_id: "holdout".into(),
        champion: champion.holdout.clone(),
        challenger: challenger.holdout.clone(),
    };
    let paired_raw = match paired_test(&input) {
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

    if let (Some(pub_m), Some(priv_m)) = (
        SliceScores::mean(&challenger.public),
        SliceScores::mean(&challenger.holdout),
    ) {
        if (pub_m - priv_m).abs() > MAX_PUBLIC_PRIVATE_GAP + DEADZONE {
            failed.push(GateFail::PublicPrivateGap);
        }
    }

    if let (Some(h), Some(p)) = (
        SliceScores::mean(&challenger.holdout),
        SliceScores::mean(&challenger.perturbed),
    ) {
        if h - p > MAX_PERTURB_DROP + DEADZONE {
            failed.push(GateFail::Perturbation);
        }
    }

    if let Some(c) = SliceScores::mean(&challenger.canaries) {
        if c + DEADZONE < MIN_CANARY_ACCURACY {
            failed.push(GateFail::Canaries);
        }
    }

    if challenger.agent_trace + DEADZONE < MIN_AGENT_TRACE {
        failed.push(GateFail::AgentTrace);
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

/// Champion row always keeps a positive lattice so emission does not burn
/// solely because a challenger was rejected.
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

    fn slice(hold: f64, public: f64, pert: f64, canary: f64, trace: f64) -> SliceScores {
        SliceScores {
            holdout: series("h", 120, hold),
            public: series("p", 120, public),
            perturbed: series("x", 120, pert),
            canaries: series("c", 40, canary),
            agent_trace: trace,
        }
    }

    #[test]
    fn never_crowns_regression() {
        let champ = slice(0.80, 0.80, 0.79, 0.99, 0.9);
        let worse = slice(0.40, 0.40, 0.39, 0.99, 0.9);
        let v = judge_challenger(&champ, &worse);
        assert!(!v.eligible);
        assert!(v.failed.contains(&GateFail::Regression));
        assert_eq!(v.lattice, 0);
    }

    #[test]
    fn significant_win_plus_gates_is_eligible() {
        let champ = slice(0.50, 0.50, 0.49, 0.99, 0.9);
        let better = slice(0.80, 0.80, 0.79, 0.99, 0.9);
        let v = judge_challenger(&champ, &better);
        assert!(v.eligible, "expected eligible, failed={:?}", v.failed);
        assert!(v.lattice > 0);
    }

    #[test]
    fn public_private_gap_blocks() {
        let champ = slice(0.50, 0.50, 0.49, 0.99, 0.9);
        let leak = slice(0.80, 0.99, 0.79, 0.99, 0.9);
        let v = judge_challenger(&champ, &leak);
        assert!(v.failed.contains(&GateFail::PublicPrivateGap));
        assert!(!v.eligible);
    }

    #[test]
    fn canary_collapse_blocks() {
        let champ = slice(0.50, 0.50, 0.49, 0.99, 0.9);
        let forget = slice(0.80, 0.80, 0.79, 0.20, 0.9);
        let v = judge_challenger(&champ, &forget);
        assert!(v.failed.contains(&GateFail::Canaries));
        assert!(!v.eligible);
    }

    #[test]
    fn lattice_is_zero_for_zero_bps() {
        assert_eq!(lattice_from_win_rate(0), 0);
        assert_eq!(lattice_from_win_rate(10_000), SCORE_MAX);
    }
}
