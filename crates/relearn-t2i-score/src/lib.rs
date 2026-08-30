//! Displacement scoring for Relearn T2I.
//!
//! A challenger must beat the champion on the **holdout** prompt split, and it
//! must do so without regressing any single L1 pillar. That second condition is
//! the point of this crate: a large Alignment gain can otherwise hide a Quality
//! collapse, and the total alone would crown it. Every pillar is gated
//! separately with a small tolerance ([`PILLAR_EPSILON`]).
//!
//! The other agentic measures are gates too, not reports:
//!
//! - **Paired A/B** on identical `(prompt_id, seed)` cells — win rate and mean
//!   delta, so the comparison is never confounded by sampler luck.
//! - **Seed replay** — a handful of pinned cells are regenerated and compared
//!   with the artifact's claimed outputs. Drift means non-determinism or
//!   different weights than the ones that were scored.
//! - **Prompt faithfulness** — small agentic spot checks (object counts,
//!   rendered text, spatial relations) must agree with Q-Judger's Alignment
//!   pillar. Disagreement discards the run rather than trusting one of them.
//! - **Contamination** — eval prompt ids appearing in submitted training
//!   metadata reject the submission outright.
//! - **N/A rate** — a judge that declined most items did not produce a score.
//!
//! All series are on the normalized `0..=1` scale (paper points ÷ 100), which
//! makes one `prism_competition` dead-zone unit equal one paper point.

#![forbid(unsafe_code)]
#![allow(
    clippy::doc_markdown,
    clippy::module_name_repetitions,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use std::collections::{BTreeMap, BTreeSet};

use prism_competition::{
    paired_test, Direction, ExampleSeries, PairedInput, PairedOutcome, PairedRefusal, DEADZONE,
};
use relearn_t2i_task::{L1Dimension, SCORE_MAX};
use serde::{Deserialize, Serialize};

/// Largest per-pillar drop tolerated versus the champion (normalized units).
///
/// `0.02` is two paper points: inside judge noise, well below a real collapse.
pub const PILLAR_EPSILON: f64 = 0.02;

/// Largest share of level-3 items the judge may decline before the run is void.
pub const MAX_NA_RATE: f64 = 0.25;

/// Largest tolerated embedding drift on a replayed cell (`1 − cosine`).
pub const MAX_REPLAY_DRIFT: f64 = 0.02;

/// Pinned cells that must be regenerated for the replay check.
pub const REPLAY_CELLS: u32 = 3;

/// Minimum agentic faithfulness spot checks required for a verdict.
pub const MIN_FAITHFULNESS_CHECKS: u32 = 8;

/// Minimum agreement between the agentic spot checks and Q-Judger Alignment.
pub const MIN_FAITHFULNESS_AGREEMENT: f64 = 0.75;

/// Largest tolerated public-minus-holdout gap (overfit / contamination signal).
pub const MAX_PUBLIC_HOLDOUT_GAP: f64 = 0.08;

/// Minimum head-to-head A/B win rate (bps of decided cells).
pub const MIN_AB_WIN_RATE_BPS: u64 = 5_000;

/// Slice id bound into the paired test. Both sides must carry it.
pub const HOLDOUT_SLICE_ID: &str = "relearn-t2i-holdout";

/// Seed-replay evidence for one artifact.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ReplayEvidence {
    /// Cells regenerated from pinned `(prompt_id, seed)` pairs.
    pub cells_checked: u32,
    /// Cells whose image hash matched the artifact's claimed output exactly.
    pub exact_hash_matches: u32,
    /// Worst `1 − cosine` embedding distance across the replayed cells.
    pub max_embedding_drift: f64,
}

impl Default for ReplayEvidence {
    fn default() -> Self {
        Self {
            cells_checked: 0,
            exact_hash_matches: 0,
            max_embedding_drift: 1.0,
        }
    }
}

impl ReplayEvidence {
    /// Whether replay cleared the gate.
    ///
    /// Exact hashes are the fast path. They are not required, because pixel
    /// determinism does not survive a driver change, so an embedding distance
    /// under [`MAX_REPLAY_DRIFT`] is accepted as the same weights.
    #[must_use]
    pub fn passes(&self) -> bool {
        if self.cells_checked < REPLAY_CELLS {
            return false;
        }
        self.exact_hash_matches >= self.cells_checked
            || self.max_embedding_drift <= MAX_REPLAY_DRIFT
    }
}

/// Agentic prompt-faithfulness spot checks (counts, rendered text, relations).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FaithfulnessEvidence {
    /// Spot checks executed.
    pub checks: u32,
    /// Spot checks that agreed with Q-Judger's Alignment pillar.
    pub agreements: u32,
}

impl FaithfulnessEvidence {
    /// Agreement rate, or `0.0` when no checks ran.
    #[must_use]
    pub fn agreement_rate(&self) -> f64 {
        if self.checks == 0 {
            return 0.0;
        }
        f64::from(self.agreements.min(self.checks)) / f64::from(self.checks)
    }

    /// Whether faithfulness cleared the gate.
    #[must_use]
    pub fn passes(&self) -> bool {
        self.checks >= MIN_FAITHFULNESS_CHECKS
            && self.agreement_rate() + DEADZONE >= MIN_FAITHFULNESS_AGREEMENT
    }
}

/// Per-artifact T2I measurements. Series keys are `p{id}#v{variation}`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct T2iSliceScores {
    /// Normalized Q-Judger totals on the holdout split.
    pub holdout: ExampleSeries,
    /// Normalized Q-Judger totals on the published split (informational).
    pub public: ExampleSeries,
    /// Normalized per-pillar series on the holdout split.
    pub holdout_by_pillar: BTreeMap<L1Dimension, ExampleSeries>,
    /// Share of level-3 items the judge declined.
    pub na_rate: f64,
    /// Seed-replay evidence.
    pub replay: ReplayEvidence,
    /// Agentic faithfulness evidence.
    pub faithfulness: FaithfulnessEvidence,
    /// Eval prompt ids found in the submission's training metadata.
    pub contaminated_prompt_ids: Vec<u32>,
}

impl T2iSliceScores {
    /// Mean of a series, or `None` when empty.
    #[must_use]
    pub fn mean(series: &ExampleSeries) -> Option<f64> {
        if series.is_empty() {
            return None;
        }
        let n = series.len() as f64;
        Some(series.by_cluster.values().sum::<f64>() / n)
    }

    /// Mean of one pillar on the holdout split.
    #[must_use]
    pub fn pillar_mean(&self, dim: L1Dimension) -> Option<f64> {
        self.holdout_by_pillar.get(&dim).and_then(Self::mean)
    }
}

/// Head-to-head result on identical `(prompt_id, seed)` cells.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct PairedAb {
    /// Cells present on both sides.
    pub cells: u64,
    /// Cells whose difference cleared the dead zone.
    pub decided: u64,
    /// Decided cells the challenger won.
    pub wins: u64,
    /// Win rate over decided cells (bps).
    pub win_rate_bps: u64,
    /// Mean challenger-minus-champion delta over paired cells (normalized).
    pub mean_delta: f64,
}

/// Paired A/B on the same seeds. Cells missing on either side are ignored.
#[must_use]
pub fn paired_ab(champion: &ExampleSeries, challenger: &ExampleSeries) -> PairedAb {
    let mut deltas = Vec::new();
    for (cell, champ) in &champion.by_cluster {
        if let Some(chal) = challenger.by_cluster.get(cell) {
            deltas.push(chal - champ);
        }
    }
    if deltas.is_empty() {
        return PairedAb::default();
    }
    let decided: Vec<f64> = deltas
        .iter()
        .copied()
        .filter(|d| d.abs() >= DEADZONE)
        .collect();
    let wins = decided.iter().filter(|d| **d > 0.0).count();
    let win_rate_bps = if decided.is_empty() {
        0
    } else {
        ((wins as u128 * 10_000) / decided.len() as u128) as u64
    };
    PairedAb {
        cells: deltas.len() as u64,
        decided: decided.len() as u64,
        wins: wins as u64,
        win_rate_bps,
        mean_delta: deltas.iter().sum::<f64>() / deltas.len() as f64,
    }
}

/// Per-pillar champion / challenger comparison.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PillarDelta {
    /// Champion pillar mean (normalized).
    pub champion: f64,
    /// Challenger pillar mean (normalized).
    pub challenger: f64,
    /// `challenger − champion`.
    pub delta: f64,
}

/// Gate that blocked promotion (or would have).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateFail {
    /// Challenger is not a significant paired win on the holdout.
    NoPairedWin,
    /// Challenger lost or tied the champion (never crown a regression).
    Regression,
    /// One L1 pillar dropped by more than [`PILLAR_EPSILON`].
    PillarRegression {
        /// Pillar that dropped.
        dimension: L1Dimension,
        /// Size of the drop in bps of the normalized scale.
        drop_bps: u64,
    },
    /// Head-to-head A/B win rate below [`MIN_AB_WIN_RATE_BPS`].
    AbWinRate,
    /// The judge declined too many items for the run to mean anything.
    NotApplicableRate,
    /// Regenerated cells did not reproduce the artifact's claimed outputs.
    SeedReplay,
    /// Agentic spot checks disagree with Q-Judger Alignment.
    PromptFaithfulness,
    /// Eval prompt ids appear in the submission's training metadata.
    Contamination,
    /// Public split far above holdout (memorization / contamination).
    PublicHoldoutGap,
    /// Paired test refused (slice mismatch / too thin).
    PairedRefusal,
}

/// Serializable paired-test summary (prism `PairedOutcome` is not serde).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairedSummary {
    /// Overlapping cells.
    pub n_paired: u64,
    /// Cells outside the dead zone.
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
    /// Same-seed head-to-head summary.
    pub ab: PairedAb,
    /// Per-pillar deltas (the anti-hidden-regression report).
    pub pillars: BTreeMap<L1Dimension, PillarDelta>,
    /// Gates that failed (empty ⇒ all clear).
    pub failed: Vec<GateFail>,
    /// Lattice score to emit if this hotkey is the live champion (`0` otherwise).
    pub lattice: u64,
}

/// Eval prompt ids that leaked into a submission's training metadata.
#[must_use]
pub fn contamination(
    train_prompt_ids: &BTreeSet<u32>,
    eval_prompt_ids: &BTreeSet<u32>,
) -> Vec<u32> {
    train_prompt_ids
        .intersection(eval_prompt_ids)
        .copied()
        .collect()
}

/// Judge challenger vs champion. Never returns `eligible` on a regression.
#[must_use]
pub fn judge_challenger(champion: &T2iSliceScores, challenger: &T2iSliceScores) -> PromoteVerdict {
    let mut failed = Vec::new();

    let paired_raw = match paired_test(&PairedInput {
        metric: "relearn_t2i.qjudger_total".into(),
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

    let ab = paired_ab(&champion.holdout, &challenger.holdout);
    if ab.decided == 0 || ab.win_rate_bps < MIN_AB_WIN_RATE_BPS {
        failed.push(GateFail::AbWinRate);
    }

    let pillars = pillar_deltas(champion, challenger);
    for (dim, d) in &pillars {
        let drop = -d.delta;
        if drop > PILLAR_EPSILON + DEADZONE {
            failed.push(GateFail::PillarRegression {
                dimension: *dim,
                drop_bps: (drop * 10_000.0).round().max(0.0) as u64,
            });
        }
    }

    if challenger.na_rate > MAX_NA_RATE {
        failed.push(GateFail::NotApplicableRate);
    }
    if !challenger.replay.passes() {
        failed.push(GateFail::SeedReplay);
    }
    if !challenger.faithfulness.passes() {
        failed.push(GateFail::PromptFaithfulness);
    }
    if !challenger.contaminated_prompt_ids.is_empty() {
        failed.push(GateFail::Contamination);
    }

    if let (Some(pub_m), Some(hold_m)) = (
        T2iSliceScores::mean(&challenger.public),
        T2iSliceScores::mean(&challenger.holdout),
    ) {
        if pub_m - hold_m > MAX_PUBLIC_HOLDOUT_GAP + DEADZONE {
            failed.push(GateFail::PublicHoldoutGap);
        }
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
        ab,
        pillars,
        failed,
        lattice,
    }
}

/// Per-pillar deltas for every pillar both sides scored.
#[must_use]
pub fn pillar_deltas(
    champion: &T2iSliceScores,
    challenger: &T2iSliceScores,
) -> BTreeMap<L1Dimension, PillarDelta> {
    let mut out = BTreeMap::new();
    for dim in L1Dimension::ALL {
        let (Some(c), Some(x)) = (champion.pillar_mean(dim), challenger.pillar_mean(dim)) else {
            continue;
        };
        out.insert(
            dim,
            PillarDelta {
                champion: c,
                challenger: x,
                delta: x - c,
            },
        );
    }
    out
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

    fn series(n: usize, val: f64) -> ExampleSeries {
        ExampleSeries::from_pairs((0..n).map(|i| {
            (
                relearn_t2i_task::cell_key((i / 4) as u32, (i % 4) as u32),
                val,
            )
        }))
    }

    fn pillars(val: f64) -> BTreeMap<L1Dimension, ExampleSeries> {
        L1Dimension::ALL
            .into_iter()
            .map(|d| (d, series(120, val)))
            .collect()
    }

    fn good_replay() -> ReplayEvidence {
        ReplayEvidence {
            cells_checked: REPLAY_CELLS,
            exact_hash_matches: REPLAY_CELLS,
            max_embedding_drift: 0.0,
        }
    }

    fn good_faith() -> FaithfulnessEvidence {
        FaithfulnessEvidence {
            checks: MIN_FAITHFULNESS_CHECKS,
            agreements: MIN_FAITHFULNESS_CHECKS,
        }
    }

    fn slice(total: f64) -> T2iSliceScores {
        T2iSliceScores {
            holdout: series(120, total),
            public: series(120, total),
            holdout_by_pillar: pillars(total),
            na_rate: 0.05,
            replay: good_replay(),
            faithfulness: good_faith(),
            contaminated_prompt_ids: Vec::new(),
        }
    }

    #[test]
    fn clear_win_is_eligible() {
        let v = judge_challenger(&slice(0.50), &slice(0.80));
        assert!(v.eligible, "failed={:?}", v.failed);
        assert!(v.lattice > 0);
        assert_eq!(v.ab.win_rate_bps, 10_000);
        assert!(v.ab.mean_delta > 0.0);
        assert_eq!(v.pillars.len(), 5);
    }

    #[test]
    fn never_crowns_regression() {
        let v = judge_challenger(&slice(0.80), &slice(0.40));
        assert!(!v.eligible);
        assert!(v.failed.contains(&GateFail::Regression));
        assert_eq!(v.lattice, 0);
    }

    #[test]
    fn pillar_collapse_blocks_even_with_a_higher_total() {
        // Alignment jumps, Quality collapses. The total still improves, so only
        // the per-pillar gate can catch this.
        let champ = slice(0.60);
        let mut chal = slice(0.60);
        chal.holdout = series(120, 0.80);
        chal.holdout_by_pillar
            .insert(L1Dimension::Alignment, series(120, 0.95));
        chal.holdout_by_pillar
            .insert(L1Dimension::Quality, series(120, 0.20));
        chal.public = series(120, 0.80);

        let v = judge_challenger(&champ, &chal);
        assert!(!v.eligible, "pillar collapse must block");
        let hit = v.failed.iter().any(|f| {
            matches!(
                f,
                GateFail::PillarRegression {
                    dimension: L1Dimension::Quality,
                    ..
                }
            )
        });
        assert!(
            hit,
            "expected Quality pillar regression, got {:?}",
            v.failed
        );
        assert!(v.pillars[&L1Dimension::Quality].delta < 0.0);
        assert!(v.pillars[&L1Dimension::Alignment].delta > 0.0);
    }

    #[test]
    fn pillar_noise_inside_epsilon_is_tolerated() {
        let champ = slice(0.50);
        let mut chal = slice(0.80);
        chal.holdout_by_pillar
            .insert(L1Dimension::Aesthetics, series(120, 0.49));
        let v = judge_challenger(&champ, &chal);
        assert!(v.eligible, "failed={:?}", v.failed);
    }

    #[test]
    fn high_na_rate_voids_the_run() {
        let mut chal = slice(0.80);
        chal.na_rate = 0.60;
        let v = judge_challenger(&slice(0.50), &chal);
        assert!(v.failed.contains(&GateFail::NotApplicableRate));
        assert!(!v.eligible);
    }

    #[test]
    fn replay_drift_blocks() {
        let mut chal = slice(0.80);
        chal.replay = ReplayEvidence {
            cells_checked: REPLAY_CELLS,
            exact_hash_matches: 0,
            max_embedding_drift: 0.4,
        };
        let v = judge_challenger(&slice(0.50), &chal);
        assert!(v.failed.contains(&GateFail::SeedReplay));

        // Same hardware drift, different weights is the case we must catch;
        // small embedding distance with no exact hash is still accepted.
        let mut ok = slice(0.80);
        ok.replay = ReplayEvidence {
            cells_checked: REPLAY_CELLS,
            exact_hash_matches: 0,
            max_embedding_drift: 0.01,
        };
        assert!(judge_challenger(&slice(0.50), &ok).eligible);
    }

    #[test]
    fn missing_replay_cells_block() {
        let mut chal = slice(0.80);
        chal.replay = ReplayEvidence {
            cells_checked: 1,
            exact_hash_matches: 1,
            max_embedding_drift: 0.0,
        };
        assert!(judge_challenger(&slice(0.50), &chal)
            .failed
            .contains(&GateFail::SeedReplay));
    }

    #[test]
    fn faithfulness_disagreement_blocks() {
        let mut chal = slice(0.80);
        chal.faithfulness = FaithfulnessEvidence {
            checks: MIN_FAITHFULNESS_CHECKS,
            agreements: 2,
        };
        let v = judge_challenger(&slice(0.50), &chal);
        assert!(v.failed.contains(&GateFail::PromptFaithfulness));
    }

    #[test]
    fn too_few_faithfulness_checks_block() {
        let mut chal = slice(0.80);
        chal.faithfulness = FaithfulnessEvidence {
            checks: 2,
            agreements: 2,
        };
        assert!(judge_challenger(&slice(0.50), &chal)
            .failed
            .contains(&GateFail::PromptFaithfulness));
    }

    #[test]
    fn contamination_blocks() {
        let mut chal = slice(0.80);
        chal.contaminated_prompt_ids = vec![902];
        let v = judge_challenger(&slice(0.50), &chal);
        assert!(v.failed.contains(&GateFail::Contamination));
    }

    #[test]
    fn contamination_detects_overlap() {
        let train: BTreeSet<u32> = [1, 2, 900].into_iter().collect();
        let eval: BTreeSet<u32> = [900, 901].into_iter().collect();
        assert_eq!(contamination(&train, &eval), vec![900]);
        assert!(contamination(&BTreeSet::new(), &eval).is_empty());
    }

    #[test]
    fn public_far_above_holdout_blocks() {
        let mut chal = slice(0.80);
        chal.public = series(120, 0.99);
        let v = judge_challenger(&slice(0.50), &chal);
        assert!(v.failed.contains(&GateFail::PublicHoldoutGap));
    }

    #[test]
    fn thin_overlap_refuses_rather_than_promotes() {
        let champ = T2iSliceScores {
            holdout: series(8, 0.5),
            ..slice(0.5)
        };
        let chal = T2iSliceScores {
            holdout: series(8, 0.9),
            ..slice(0.9)
        };
        let v = judge_challenger(&champ, &chal);
        assert!(!v.eligible);
        assert!(v.failed.contains(&GateFail::PairedRefusal));
    }

    #[test]
    fn ab_ignores_cells_missing_on_one_side() {
        let champ = ExampleSeries::from_pairs([("p1#v0", 0.4), ("p1#v1", 0.4)]);
        let chal = ExampleSeries::from_pairs([("p1#v0", 0.9), ("p9#v0", 0.9)]);
        let ab = paired_ab(&champ, &chal);
        assert_eq!(ab.cells, 1);
        assert_eq!(ab.wins, 1);
        assert_eq!(ab.win_rate_bps, 10_000);
    }

    #[test]
    fn lattice_endpoints() {
        assert_eq!(lattice_from_win_rate(0), 0);
        assert_eq!(lattice_from_win_rate(10_000), SCORE_MAX);
        assert_eq!(champion_hold_lattice(), SCORE_MAX / 2);
    }
}
