//! Displacement scoring for Relearn.
//!
//! Score is challenger vs the previous champion on a shared **holdout** slice.
//! A regression is never crowned. Promotion additionally requires the
//! operator-audited paired win plus retention / overfit / contamination gates.
//!
//! The visible lattice is computed from the holdout paired test only. Public
//! split, general-bench canaries, and pixel-shuffle are gates: they can zero
//! a run but they never enter the lattice. Miners overfit anything that pays.

#![forbid(unsafe_code)]
#![allow(
    clippy::missing_errors_doc,
    clippy::doc_markdown,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::module_name_repetitions
)]

use std::collections::{BTreeMap, BTreeSet};

use prism_competition::{
    paired_test, Direction, ExampleSeries, PairedInput, PairedOutcome, PairedRefusal, DEADZONE,
};
use relearn_challenge_task::{contamination, HoldoutItem, HoldoutTask, SCORE_MAX};
use serde::{Deserialize, Serialize};

/// Maximum allowed public-minus-holdout gap (absolute).
pub const MAX_PUBLIC_PRIVATE_GAP: f64 = 0.08;

/// Maximum allowed drop under input perturbation (absolute).
pub const MAX_PERTURB_DROP: f64 = 0.05;

/// Minimum canary accuracy (known-answer items the base model already solves).
pub const MIN_CANARY_ACCURACY: f64 = 0.95;

/// Largest tolerated drop vs the champion on the general-bench canary.
///
/// MMLU / MMMU / similar stay **off** the visible score. A regression past
/// this epsilon is a hard zero, not a reduced lattice.
pub const CANARY_EPSILON: f64 = 0.02;

/// Minimum score drop required when vision-item pixels are shuffled.
pub const MIN_SHUFFLE_DROP: f64 = 0.10;

/// Minimum agent-trace score (first-class; 0..1).
pub const MIN_AGENT_TRACE: f64 = 0.5;

/// Slice id bound into the paired test.
pub const HOLDOUT_SLICE_ID: &str = "relearn-holdout";

/// What a submission declared about its training data, plus what leaked.
///
/// The contamination gate reads miner-declared metadata, so an empty manifest
/// is *absence of evidence*, not a clean bill of health. Keeping the declared
/// counts next to the hits is what lets [`judge_challenger`] tell "declared
/// nothing" apart from "declared data with no holdout overlap".
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContaminationEvidence {
    /// Distinct train item ids the submission declared.
    pub declared_ids: usize,
    /// Distinct train image hashes the submission declared.
    pub declared_image_hashes: usize,
    /// Distinct train dataset ids the submission declared.
    pub declared_dataset_ids: usize,
    /// Holdout fingerprints found inside the declared metadata.
    pub hits: Vec<String>,
}

impl ContaminationEvidence {
    /// Whether the submission declared anything the gate could check.
    #[must_use]
    pub fn is_declared(&self) -> bool {
        self.declared_ids > 0 || self.declared_image_hashes > 0 || self.declared_dataset_ids > 0
    }

    /// Whether eval would be wasted: undeclared metadata or a holdout hit.
    #[must_use]
    pub fn blocks_eval(&self) -> bool {
        !self.is_declared() || !self.hits.is_empty()
    }
}

/// Pixel-shuffle evidence for one vision family.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct ShuffleEvidence {
    /// Items scored in this family.
    pub items: u32,
    /// Mean score with the real image (`0..=1`).
    pub score: f64,
    /// Mean score with the image pixels shuffled (`0..=1`).
    pub shuffled_score: f64,
}

impl ShuffleEvidence {
    /// How much the score fell when the image was destroyed.
    #[must_use]
    pub fn shuffle_drop(&self) -> f64 {
        self.score - self.shuffled_score
    }

    /// Whether the model demonstrably used the image.
    #[must_use]
    pub fn uses_the_image(&self) -> bool {
        self.items > 0 && self.shuffle_drop() >= MIN_SHUFFLE_DROP - DEADZONE
    }
}

/// Per-example holdout measurements for one submission.
#[derive(Debug, Clone, PartialEq)]
pub struct SliceScores {
    /// Holdout items (the only series that may enter the lattice).
    pub holdout: ExampleSeries,
    /// Public / training-adjacent split (overfit detector; never lattice).
    pub public: ExampleSeries,
    /// Same holdout items after a pinned perturbation.
    pub perturbed: ExampleSeries,
    /// Known-answer canaries (base-model already-correct items).
    pub canaries: ExampleSeries,
    /// General benches (MMLU / MMMU / …). Off the visible score path.
    pub general_canary: ExampleSeries,
    /// Agent-trace quality in `[0, 1]` (first-class; not a side channel).
    pub agent_trace: f64,
    /// Pixel-shuffle control, one entry per vision family that had images.
    pub vision_shuffle: BTreeMap<HoldoutTask, ShuffleEvidence>,
    /// Declared training metadata and the holdout fingerprints inside it.
    pub contamination: ContaminationEvidence,
}

impl Default for SliceScores {
    fn default() -> Self {
        Self {
            holdout: ExampleSeries::default(),
            public: ExampleSeries::default(),
            perturbed: ExampleSeries::default(),
            canaries: ExampleSeries::default(),
            general_canary: ExampleSeries::default(),
            agent_trace: 0.0,
            vision_shuffle: BTreeMap::new(),
            contamination: ContaminationEvidence::default(),
        }
    }
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
    /// Public split evidence was missing (fail-closed).
    PublicEvidenceMissing,
    /// Perturbed holdout collapsed (brittle / overfit).
    Perturbation,
    /// The perturbed rerun was missing, so the brittleness gate had nothing to
    /// read (fail-closed; omitting the series is not a way past it).
    PerturbationEvidenceMissing,
    /// Base-competence canaries failed.
    Canaries,
    /// Known-answer canaries were missing, so the base-competence gate had
    /// nothing to read (fail-closed).
    BaseCanaryEvidenceMissing,
    /// General-bench canary regressed past [`CANARY_EPSILON`].
    CanaryRegression {
        /// Size of the drop in bps.
        drop_bps: u64,
    },
    /// General-bench canary did not run.
    CanaryEvidenceMissing,
    /// Agent-trace score below floor.
    AgentTrace,
    /// Eval item ids / image hashes appear in training metadata.
    Contamination,
    /// The submission declared no training metadata, so the contamination
    /// gate had nothing to check (fail-closed; an empty manifest is not a pass).
    ContaminationEvidenceMissing,
    /// Shuffling the image pixels barely changed the score.
    IgnoresTheImage {
        /// Family that ignored the pixels.
        task: HoldoutTask,
    },
    /// The champion took the shuffle control on this family and the challenger
    /// did not, so the gate had nothing to read (fail-closed).
    ShuffleEvidenceMissing {
        /// Family the challenger left unmeasured.
        task: HoldoutTask,
    },
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

/// Declared training metadata plus the eval fingerprints that leaked into it.
///
/// Returns the declared counts as well as the hits so a submission that
/// declared nothing cannot be read as a clean run.
#[must_use]
pub fn contamination_evidence(
    train_ids: &BTreeSet<u32>,
    train_image_hashes: &BTreeSet<String>,
    train_dataset_ids: &BTreeSet<String>,
    holdout: &[HoldoutItem],
) -> ContaminationEvidence {
    ContaminationEvidence {
        declared_ids: train_ids.len(),
        declared_image_hashes: train_image_hashes.len(),
        declared_dataset_ids: train_dataset_ids.len(),
        hits: contamination(train_ids, train_image_hashes, train_dataset_ids, holdout),
    }
}

/// Verdict for a submission that must not be scored: contaminated or silent.
///
/// Returns `None` when the evidence is declared and clean — those still need
/// a real eval. Used before a Lium rent so junk cannot spend the pod.
#[must_use]
pub fn pre_eval_contamination_verdict(ev: &ContaminationEvidence) -> Option<PromoteVerdict> {
    if !ev.blocks_eval() {
        return None;
    }
    let failed = if ev.is_declared() {
        vec![GateFail::Contamination]
    } else {
        vec![GateFail::ContaminationEvidenceMissing]
    };
    Some(PromoteVerdict {
        eligible: false,
        paired: None,
        failed,
        lattice: 0,
    })
}

/// Judge challenger vs champion. Never returns `eligible` on a regression.
///
/// The lattice is holdout-only. General-bench canary, public gap, shuffle, and
/// contamination can only zero a run. Missing evidence — no public split, no
/// general canary, no declared training metadata — is a fail, not a pass.
#[must_use]
pub fn judge_challenger(champion: &SliceScores, challenger: &SliceScores) -> PromoteVerdict {
    let mut failed = Vec::new();

    let input = PairedInput {
        metric: "relearn.holdout".into(),
        direction: Direction::HigherBetter,
        slice_id: HOLDOUT_SLICE_ID.into(),
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

    match (
        SliceScores::mean(&challenger.public),
        SliceScores::mean(&challenger.holdout),
    ) {
        (None, _) => failed.push(GateFail::PublicEvidenceMissing),
        (Some(pub_m), Some(priv_m)) => {
            if pub_m - priv_m > MAX_PUBLIC_PRIVATE_GAP + DEADZONE {
                failed.push(GateFail::PublicPrivateGap);
            }
        }
        (Some(_), None) => failed.push(GateFail::PairedRefusal),
    }

    // Missing evidence is a fail, the same as the public split and the general
    // canary: a run that simply omits the series would otherwise walk past the
    // gate, and omitting it is exactly what an overfit artifact wants.
    match (
        SliceScores::mean(&challenger.holdout),
        SliceScores::mean(&challenger.perturbed),
    ) {
        (_, None) => failed.push(GateFail::PerturbationEvidenceMissing),
        (Some(h), Some(p)) => {
            if h - p > MAX_PERTURB_DROP + DEADZONE {
                failed.push(GateFail::Perturbation);
            }
        }
        (None, Some(_)) => {}
    }

    match SliceScores::mean(&challenger.canaries) {
        None => failed.push(GateFail::BaseCanaryEvidenceMissing),
        Some(c) => {
            if c + DEADZONE < MIN_CANARY_ACCURACY {
                failed.push(GateFail::Canaries);
            }
        }
    }

    match (
        SliceScores::mean(&champion.general_canary),
        SliceScores::mean(&challenger.general_canary),
    ) {
        (None, _) | (_, None) => failed.push(GateFail::CanaryEvidenceMissing),
        (Some(champ_c), Some(chal_c)) => {
            let drop = champ_c - chal_c;
            if drop > CANARY_EPSILON + DEADZONE {
                failed.push(GateFail::CanaryRegression {
                    drop_bps: (drop * 10_000.0).round().max(0.0) as u64,
                });
            }
        }
    }

    if challenger.agent_trace + DEADZONE < MIN_AGENT_TRACE {
        failed.push(GateFail::AgentTrace);
    }

    if challenger.contamination.is_declared() {
        if !challenger.contamination.hits.is_empty() {
            failed.push(GateFail::Contamination);
        }
    } else {
        failed.push(GateFail::ContaminationEvidenceMissing);
    }

    // The champion is the reference for which families the holdout actually
    // has images in. A challenger that skips a family the champion measured is
    // not a text-only holdout, it is a run that declined the control.
    for task in HoldoutTask::VISION {
        match (
            champion.vision_shuffle.get(&task),
            challenger.vision_shuffle.get(&task),
        ) {
            (_, Some(ev)) if !ev.uses_the_image() => {
                failed.push(GateFail::IgnoresTheImage { task });
            }
            (Some(_), None) => failed.push(GateFail::ShuffleEvidenceMissing { task }),
            _ => {}
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

    fn vision_ok() -> BTreeMap<HoldoutTask, ShuffleEvidence> {
        HoldoutTask::VISION
            .into_iter()
            .map(|t| {
                (
                    t,
                    ShuffleEvidence {
                        items: 40,
                        score: 0.70,
                        shuffled_score: 0.40,
                    },
                )
            })
            .collect()
    }

    fn declared_clean() -> ContaminationEvidence {
        ContaminationEvidence {
            declared_ids: 40,
            declared_image_hashes: 12,
            declared_dataset_ids: 2,
            hits: Vec::new(),
        }
    }

    fn slice(hold: f64, public: f64, pert: f64, canary: f64, trace: f64) -> SliceScores {
        SliceScores {
            holdout: series("h", 120, hold),
            public: series("p", 120, public),
            perturbed: series("x", 120, pert),
            canaries: series("c", 40, canary),
            general_canary: series("g", 40, 0.97),
            agent_trace: trace,
            vision_shuffle: vision_ok(),
            contamination: declared_clean(),
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
    fn public_far_above_holdout_blocks() {
        let champ = slice(0.50, 0.50, 0.49, 0.99, 0.9);
        let leak = slice(0.80, 0.99, 0.79, 0.99, 0.9);
        let v = judge_challenger(&champ, &leak);
        assert!(v.failed.contains(&GateFail::PublicPrivateGap));
        assert!(!v.eligible);
        assert_eq!(v.lattice, 0);
    }

    #[test]
    fn empty_public_is_fail_closed() {
        let champ = slice(0.50, 0.50, 0.49, 0.99, 0.9);
        let mut chal = slice(0.80, 0.80, 0.79, 0.99, 0.9);
        chal.public = ExampleSeries::default();
        let v = judge_challenger(&champ, &chal);
        assert!(v.failed.contains(&GateFail::PublicEvidenceMissing));
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
    fn general_canary_regression_is_a_hard_zero_off_the_lattice() {
        let champ = slice(0.50, 0.50, 0.49, 0.99, 0.9);
        let mut chal = slice(0.80, 0.80, 0.79, 0.99, 0.9);
        chal.general_canary = series("g", 40, 0.70);
        let v = judge_challenger(&champ, &chal);
        assert!(
            v.failed
                .iter()
                .any(|f| matches!(f, GateFail::CanaryRegression { .. })),
            "{:?}",
            v.failed
        );
        assert!(!v.eligible);
        assert_eq!(v.lattice, 0);
        assert!(
            v.paired.expect("holdout still ran").displaces,
            "canary must not be mixed into the holdout win"
        );
    }

    #[test]
    fn missing_general_canary_is_fail_closed() {
        let champ = slice(0.50, 0.50, 0.49, 0.99, 0.9);
        let mut chal = slice(0.80, 0.80, 0.79, 0.99, 0.9);
        chal.general_canary = ExampleSeries::default();
        let v = judge_challenger(&champ, &chal);
        assert!(v.failed.contains(&GateFail::CanaryEvidenceMissing));
        assert_eq!(v.lattice, 0);
    }

    #[test]
    fn noise_inside_canary_epsilon_is_tolerated() {
        let champ = slice(0.50, 0.50, 0.49, 0.99, 0.9);
        let mut chal = slice(0.80, 0.80, 0.79, 0.99, 0.9);
        chal.general_canary = series("g", 40, 0.96);
        assert!(judge_challenger(&champ, &chal).eligible);
    }

    #[test]
    fn contamination_blocks_promotion() {
        let champ = slice(0.50, 0.50, 0.49, 0.99, 0.9);
        let mut chal = slice(0.80, 0.80, 0.79, 0.99, 0.9);
        chal.contamination.hits = vec!["id:900".into()];
        let v = judge_challenger(&champ, &chal);
        assert!(v.failed.contains(&GateFail::Contamination));
        assert!(!v.eligible);
        assert_eq!(v.lattice, 0);
    }

    #[test]
    fn empty_training_metadata_is_fail_closed_not_a_clean_run() {
        let champ = slice(0.50, 0.50, 0.49, 0.99, 0.9);
        let mut chal = slice(0.80, 0.80, 0.79, 0.99, 0.9);
        chal.contamination = ContaminationEvidence::default();
        let v = judge_challenger(&champ, &chal);
        assert!(
            v.failed.contains(&GateFail::ContaminationEvidenceMissing),
            "{:?}",
            v.failed
        );
        assert!(!v.eligible);
        assert_eq!(v.lattice, 0);
    }

    #[test]
    fn declaring_only_dataset_ids_still_takes_the_gate() {
        let champ = slice(0.50, 0.50, 0.49, 0.99, 0.9);
        let mut chal = slice(0.80, 0.80, 0.79, 0.99, 0.9);
        chal.contamination = ContaminationEvidence {
            declared_dataset_ids: 1,
            ..ContaminationEvidence::default()
        };
        assert!(chal.contamination.is_declared());
        let v = judge_challenger(&champ, &chal);
        assert!(!v.failed.contains(&GateFail::ContaminationEvidenceMissing));
        assert!(v.eligible, "{:?}", v.failed);
    }

    #[test]
    fn evidence_counts_declared_metadata_and_hits() {
        let ids: BTreeSet<u32> = [900, 901].into_iter().collect();
        let images: BTreeSet<String> = ["ab".repeat(32)].into_iter().collect();
        let datasets: BTreeSet<String> = ["dev".to_owned()].into_iter().collect();
        let holdout = vec![HoldoutItem {
            id: 900,
            prompt: "a holdout prompt with several words in it".into(),
            dataset_id: "dev".into(),
            task: HoldoutTask::Text,
            image_hash: String::new(),
        }];
        let ev = contamination_evidence(&ids, &images, &datasets, &holdout);
        assert_eq!(ev.declared_ids, 2);
        assert_eq!(ev.declared_image_hashes, 1);
        assert_eq!(ev.declared_dataset_ids, 1);
        assert!(ev.is_declared());
        assert!(ev.hits.iter().any(|h| h == "id:900"), "{:?}", ev.hits);

        let empty = contamination_evidence(
            &BTreeSet::new(),
            &BTreeSet::new(),
            &BTreeSet::new(),
            &holdout,
        );
        assert!(!empty.is_declared());
        assert!(empty.hits.is_empty());
    }

    #[test]
    fn pixel_shuffle_is_required_on_every_vision_family() {
        let champ = slice(0.50, 0.50, 0.49, 0.99, 0.9);
        for task in HoldoutTask::VISION {
            let mut chal = slice(0.80, 0.80, 0.79, 0.99, 0.9);
            chal.vision_shuffle.insert(
                task,
                ShuffleEvidence {
                    items: 40,
                    score: 0.80,
                    shuffled_score: 0.79,
                },
            );
            let v = judge_challenger(&champ, &chal);
            assert!(
                v.failed.contains(&GateFail::IgnoresTheImage { task }),
                "family {task:?} must take the shuffle control, failed={:?}",
                v.failed
            );
            assert_eq!(v.lattice, 0);
        }
    }

    #[test]
    fn text_only_holdout_does_not_require_shuffle() {
        let mut champ = slice(0.50, 0.50, 0.49, 0.99, 0.9);
        let mut chal = slice(0.80, 0.80, 0.79, 0.99, 0.9);
        champ.vision_shuffle.clear();
        chal.vision_shuffle.clear();
        assert!(
            judge_challenger(&champ, &chal).eligible,
            "no images ⇒ no shuffle gate"
        );
    }

    /// Dropping the family is not the same as there being no images in it: the
    /// champion measured it, so the holdout has them and the control applies.
    #[test]
    fn a_challenger_cannot_skip_a_family_the_champion_measured() {
        let champ = slice(0.50, 0.50, 0.49, 0.99, 0.9);
        for task in HoldoutTask::VISION {
            let mut chal = slice(0.80, 0.80, 0.79, 0.99, 0.9);
            chal.vision_shuffle.remove(&task);
            let v = judge_challenger(&champ, &chal);
            assert!(
                v.failed.contains(&GateFail::ShuffleEvidenceMissing { task }),
                "family {task:?} must not be droppable, failed={:?}",
                v.failed
            );
            assert!(!v.eligible);
            assert_eq!(v.lattice, 0);
        }
    }

    /// Both retention floors used to be `if let Some(..)`, so a run that shipped
    /// no perturbed rerun and no known-answer canaries took neither gate.
    #[test]
    fn omitting_the_retention_series_is_fail_closed_not_a_skipped_gate() {
        let champ = slice(0.50, 0.50, 0.49, 0.99, 0.9);

        let mut no_perturb = slice(0.80, 0.80, 0.79, 0.99, 0.9);
        no_perturb.perturbed = ExampleSeries::default();
        let v = judge_challenger(&champ, &no_perturb);
        assert!(
            v.failed.contains(&GateFail::PerturbationEvidenceMissing),
            "{:?}",
            v.failed
        );
        assert!(!v.eligible);
        assert_eq!(v.lattice, 0);

        let mut no_canaries = slice(0.80, 0.80, 0.79, 0.99, 0.9);
        no_canaries.canaries = ExampleSeries::default();
        let v = judge_challenger(&champ, &no_canaries);
        assert!(
            v.failed.contains(&GateFail::BaseCanaryEvidenceMissing),
            "{:?}",
            v.failed
        );
        assert!(!v.eligible);
        assert_eq!(v.lattice, 0);

        // A brittle run that *does* report the rerun still fails on the drop,
        // so the new variant did not replace the floor it guards.
        let brittle = slice(0.80, 0.80, 0.40, 0.99, 0.9);
        let v = judge_challenger(&champ, &brittle);
        assert!(v.failed.contains(&GateFail::Perturbation), "{:?}", v.failed);
        assert!(!v.failed.contains(&GateFail::PerturbationEvidenceMissing));
    }

    /// The whole point of the gates: a run cannot pick which ones apply.
    /// Every one of these zeroes the lattice on its own.
    #[test]
    fn no_single_gate_can_be_dodged_by_dropping_its_evidence() {
        let champ = slice(0.50, 0.50, 0.49, 0.99, 0.9);
        let drops: [(&str, fn(&mut SliceScores)); 6] = [
            ("public", |s| s.public = ExampleSeries::default()),
            ("perturbed", |s| s.perturbed = ExampleSeries::default()),
            ("canaries", |s| s.canaries = ExampleSeries::default()),
            ("general_canary", |s| {
                s.general_canary = ExampleSeries::default();
            }),
            ("contamination", |s| {
                s.contamination = ContaminationEvidence::default();
            }),
            ("vision_shuffle", |s| s.vision_shuffle.clear()),
        ];
        for (label, drop) in drops {
            let mut chal = slice(0.80, 0.80, 0.79, 0.99, 0.9);
            drop(&mut chal);
            let v = judge_challenger(&champ, &chal);
            assert!(!v.eligible, "{label} was droppable: {:?}", v.failed);
            assert_eq!(v.lattice, 0, "{label}");
        }
    }

    #[test]
    fn lattice_is_zero_for_zero_bps() {
        assert_eq!(lattice_from_win_rate(0), 0);
        assert_eq!(lattice_from_win_rate(10_000), SCORE_MAX);
    }
}
