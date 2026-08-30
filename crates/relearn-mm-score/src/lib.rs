//! Scoring for Relearn Multimodal. Two gates, both mandatory.
//!
//! **Gate 1 — the LLM is intact.** The submitted language model is run on the
//! existing Relearn text holdout with the vision modules ignored. If it scores
//! below `champion − ε` the submission is worth zero on this challenge no matter
//! how good the vision numbers are. This is the whole point of the challenge
//! design: attaching an encoder must not be a way to get paid for damaging the
//! champion. For an encoder-only submission the LM weights must additionally
//! hash-match the champion, which is the proof that nothing on the text side
//! moved at all.
//!
//! **Gate 2 — the vision side actually improved.** A frozen image holdout
//! (captioning, VQA, OCR, spatial relations — deliberately not ImageNet or COCO
//! test, which are in every candidate encoder's pretraining mix) plus agentic
//! traces where the model has to look at a screenshot or diagram before calling
//! a tool. Displacement is measured against the champion with the same paired
//! test the text challenge uses.
//!
//! The agentic traces carry one extra check: the same traces are replayed with
//! the image pixels shuffled. A model that is really reading the image must get
//! materially worse; one that is pattern-matching the text prompt will not
//! move, and a flat shuffle delta means the vision win is not real.

#![forbid(unsafe_code)]
#![allow(
    clippy::doc_markdown,
    clippy::module_name_repetitions,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use std::collections::BTreeMap;

use prism_competition::{
    paired_test, Direction, ExampleSeries, PairedInput, PairedOutcome, PairedRefusal, DEADZONE,
};
use relearn_mm_task::{SubmissionKind, VisionTask, VisionTaskWeights, SCORE_MAX};
use serde::{Deserialize, Serialize};

/// Largest text-holdout drop tolerated versus the champion (absolute).
///
/// Small on purpose. The challenge pays for vision, and the text side is a
/// floor rather than a budget to spend.
pub const LM_EPSILON: f64 = 0.01;

/// Minimum score drop required when the agentic images are pixel-shuffled.
///
/// A model that ignores the image scores the same on shuffled pixels. Requiring
/// a real drop is what separates seeing from guessing from the prompt.
pub const MIN_SHUFFLE_DROP: f64 = 0.10;

/// Slice id for the text-intact comparison.
pub const TEXT_SLICE_ID: &str = "relearn-mm-text-holdout";

/// Slice id for the vision holdout comparison.
pub const VISION_SLICE_ID: &str = "relearn-mm-vision-holdout";

/// Slice id for the agentic image-tool comparison.
pub const AGENTIC_SLICE_ID: &str = "relearn-mm-agentic-holdout";

/// Agentic trace evidence, including the pixel-shuffle control.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct AgenticEvidence {
    /// Traces executed.
    pub traces: u32,
    /// Mean score with the real image (`0..=1`).
    pub score: f64,
    /// Mean score with the image pixels shuffled (`0..=1`).
    pub shuffled_score: f64,
}

impl AgenticEvidence {
    /// How much the score fell when the image was destroyed.
    #[must_use]
    pub fn shuffle_drop(&self) -> f64 {
        self.score - self.shuffled_score
    }

    /// Whether the model demonstrably used the image.
    #[must_use]
    pub fn uses_the_image(&self) -> bool {
        self.traces > 0 && self.shuffle_drop() >= MIN_SHUFFLE_DROP - DEADZONE
    }
}

/// Per-artifact multimodal measurements.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MmSliceScores {
    /// Relearn text holdout with vision modules ignored (gate 1).
    pub text_holdout: ExampleSeries,
    /// Frozen image holdout, all task families pooled (gate 2).
    pub vision_holdout: ExampleSeries,
    /// Per-task-family vision series.
    pub vision_by_task: BTreeMap<VisionTask, ExampleSeries>,
    /// Agentic image-tool traces on the holdout.
    pub agentic: AgenticEvidence,
    /// Per-example agentic scores, for the paired comparison.
    pub agentic_series: ExampleSeries,
    /// Public / training-adjacent vision split (informational).
    pub vision_public: ExampleSeries,
    /// SHA-256 hex of the submitted LM weights.
    pub lm_weights_hash: String,
    /// What the miner submitted.
    pub kind: SubmissionKind,
}

impl MmSliceScores {
    /// Mean of a series, or `None` when empty.
    #[must_use]
    pub fn mean(series: &ExampleSeries) -> Option<f64> {
        if series.is_empty() {
            return None;
        }
        let n = series.len() as f64;
        Some(series.by_cluster.values().sum::<f64>() / n)
    }

    /// Weighted vision score across task families.
    ///
    /// Task families with no items are dropped and the remaining weights are
    /// renormalized, so a missing family cannot silently score as zero.
    #[must_use]
    pub fn weighted_vision(&self, weights: &VisionTaskWeights) -> Option<f64> {
        let mut acc = 0.0;
        let mut total_w = 0.0;
        for task in VisionTask::ALL {
            let Some(series) = self.vision_by_task.get(&task) else {
                continue;
            };
            let Some(m) = Self::mean(series) else {
                continue;
            };
            let w = weights.weight(task);
            acc += w * m;
            total_w += w;
        }
        if total_w <= 0.0 {
            return None;
        }
        Some(acc / total_w)
    }
}

/// Gate that blocked promotion (or would have).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateFail {
    /// The submitted LM regressed on the Relearn text holdout.
    LmRegression {
        /// Size of the drop in bps.
        drop_bps: u64,
    },
    /// An encoder-only submission changed the LM weights.
    LmWeightsChanged,
    /// The text holdout comparison could not run.
    LmEvidenceMissing,
    /// Challenger is not a significant paired win on the vision holdout.
    NoVisionWin,
    /// Challenger lost or tied the champion on vision.
    VisionRegression,
    /// A vision task family has no items.
    VisionTaskMissing {
        /// Family with no items.
        task: VisionTask,
    },
    /// The agentic traces did not beat the champion.
    NoAgenticWin,
    /// Shuffling the image pixels barely changed the score.
    IgnoresTheImage,
    /// The submitted encoder is not permissively licensed.
    EncoderLicense,
    /// Paired test refused (slice mismatch / too thin).
    PairedRefusal,
}

/// Serializable paired-test summary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairedSummary {
    /// Overlapping examples.
    pub n_paired: u64,
    /// Examples outside the dead zone.
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

/// Result of gate 1 on its own.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LmIntact {
    /// Champion text-holdout mean.
    pub champion: f64,
    /// Challenger text-holdout mean.
    pub challenger: f64,
    /// `challenger − champion`.
    pub delta: f64,
    /// Whether the LM cleared the gate.
    pub passes: bool,
}

/// Full promote / reject verdict. Consensus-critical once leaves are signed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromoteVerdict {
    /// Whether this submission may become champion after operator audit.
    pub eligible: bool,
    /// Gate 1 report. `None` when the text holdout could not be compared.
    pub lm_intact: Option<LmIntact>,
    /// Vision holdout paired outcome.
    pub vision: Option<PairedSummary>,
    /// Agentic holdout paired outcome.
    pub agentic: Option<PairedSummary>,
    /// Pixel-shuffle drop on the agentic traces.
    pub shuffle_drop: f64,
    /// Gates that failed (empty ⇒ all clear).
    pub failed: Vec<GateFail>,
    /// Lattice score to emit if this hotkey is the live champion (`0` otherwise).
    pub lattice: u64,
}

/// Gate 1: the submitted LM must not regress the Relearn text holdout.
///
/// `champion_lm_hash` is the champion's LM weights hash. An encoder-only
/// submission must match it exactly; an encoder-plus-LM submission is allowed
/// to differ and is judged on the score alone.
#[must_use]
pub fn check_lm_intact(
    champion: &MmSliceScores,
    challenger: &MmSliceScores,
    champion_lm_hash: &str,
) -> (Option<LmIntact>, Vec<GateFail>) {
    let mut failed = Vec::new();

    if challenger.kind.requires_champion_lm_hash() {
        let same = !champion_lm_hash.trim().is_empty()
            && challenger
                .lm_weights_hash
                .trim()
                .eq_ignore_ascii_case(champion_lm_hash.trim());
        if !same {
            failed.push(GateFail::LmWeightsChanged);
        }
    }

    let (Some(champ_m), Some(chal_m)) = (
        MmSliceScores::mean(&champion.text_holdout),
        MmSliceScores::mean(&challenger.text_holdout),
    ) else {
        failed.push(GateFail::LmEvidenceMissing);
        return (None, failed);
    };

    let delta = chal_m - champ_m;
    let passes = delta >= -(LM_EPSILON + DEADZONE);
    if !passes {
        failed.push(GateFail::LmRegression {
            drop_bps: ((-delta) * 10_000.0).round().max(0.0) as u64,
        });
    }
    (
        Some(LmIntact {
            champion: champ_m,
            challenger: chal_m,
            delta,
            passes,
        }),
        failed,
    )
}

/// Run one paired comparison and push the gates it failed.
///
/// A refusal is always "champion holds": a slice too thin to decide never
/// becomes a displacement.
fn paired(
    metric: &str,
    slice_id: &str,
    champion: &ExampleSeries,
    challenger: &ExampleSeries,
    no_win: GateFail,
    regression: Option<GateFail>,
    failed: &mut Vec<GateFail>,
) -> Option<PairedOutcome> {
    let outcome = paired_test(&PairedInput {
        metric: metric.to_owned(),
        direction: Direction::HigherBetter,
        slice_id: slice_id.to_owned(),
        champion: champion.clone(),
        challenger: challenger.clone(),
    });
    match outcome {
        Ok(o) if o.displaces => Some(o),
        Ok(o) => {
            failed.push(no_win);
            if let Some(r) = regression {
                failed.push(r);
            }
            Some(o)
        }
        Err(
            PairedRefusal::NotEnoughDecided
            | PairedRefusal::NoOverlap
            | PairedRefusal::SliceMismatch,
        ) => {
            failed.push(GateFail::PairedRefusal);
            failed.push(no_win);
            None
        }
    }
}

/// Judge challenger vs champion across both gates.
///
/// Gate 1 is a hard zero: if the LM regressed, the verdict is ineligible and
/// the lattice is `0` even when every vision number improved.
#[must_use]
pub fn judge_challenger(
    champion: &MmSliceScores,
    challenger: &MmSliceScores,
    champion_lm_hash: &str,
    encoder_permissive: bool,
) -> PromoteVerdict {
    let (lm_intact, mut failed) = check_lm_intact(champion, challenger, champion_lm_hash);

    if !encoder_permissive {
        failed.push(GateFail::EncoderLicense);
    }

    let vision = paired(
        "relearn_mm.vision_holdout",
        VISION_SLICE_ID,
        &champion.vision_holdout,
        &challenger.vision_holdout,
        GateFail::NoVisionWin,
        Some(GateFail::VisionRegression),
        &mut failed,
    );

    for task in VisionTask::ALL {
        let missing = challenger
            .vision_by_task
            .get(&task)
            .is_none_or(ExampleSeries::is_empty);
        if missing {
            failed.push(GateFail::VisionTaskMissing { task });
        }
    }

    let agentic = paired(
        "relearn_mm.agentic_holdout",
        AGENTIC_SLICE_ID,
        &champion.agentic_series,
        &challenger.agentic_series,
        GateFail::NoAgenticWin,
        None,
        &mut failed,
    );

    if !challenger.agentic.uses_the_image() {
        failed.push(GateFail::IgnoresTheImage);
    }

    failed.sort_by(|a, b| format!("{a:?}").cmp(&format!("{b:?}")));
    failed.dedup();

    let eligible = failed.is_empty();
    let lattice = if eligible {
        vision
            .as_ref()
            .map_or(0, |o| lattice_from_win_rate(o.win_rate_lcb_bps))
    } else {
        0
    };

    PromoteVerdict {
        eligible,
        lm_intact,
        vision: vision.as_ref().map(PairedSummary::from_outcome),
        agentic: agentic.as_ref().map(PairedSummary::from_outcome),
        shuffle_drop: challenger.agentic.shuffle_drop(),
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

    const CHAMP_HASH: &str = "aaaa1111";

    fn series(prefix: &str, n: usize, val: f64) -> ExampleSeries {
        ExampleSeries::from_pairs((0..n).map(|i| (format!("{prefix}{i}"), val)))
    }

    fn by_task(val: f64) -> BTreeMap<VisionTask, ExampleSeries> {
        VisionTask::ALL
            .into_iter()
            .map(|t| (t, series(t.as_str(), 40, val)))
            .collect()
    }

    fn slice(text: f64, vision: f64, agentic: f64, kind: SubmissionKind) -> MmSliceScores {
        MmSliceScores {
            text_holdout: series("t", 120, text),
            vision_holdout: series("v", 160, vision),
            vision_by_task: by_task(vision),
            agentic: AgenticEvidence {
                traces: 32,
                score: agentic,
                shuffled_score: agentic - 0.30,
            },
            agentic_series: series("a", 120, agentic),
            vision_public: series("vp", 120, vision),
            lm_weights_hash: CHAMP_HASH.into(),
            kind,
        }
    }

    fn champ() -> MmSliceScores {
        slice(0.70, 0.50, 0.50, SubmissionKind::EncoderOnly)
    }

    fn judge(champion: &MmSliceScores, challenger: &MmSliceScores) -> PromoteVerdict {
        judge_challenger(champion, challenger, CHAMP_HASH, true)
    }

    #[test]
    fn vision_win_with_intact_text_is_eligible() {
        let v = judge(
            &champ(),
            &slice(0.70, 0.85, 0.85, SubmissionKind::EncoderOnly),
        );
        assert!(v.eligible, "failed={:?}", v.failed);
        assert!(v.lattice > 0);
        assert!(v.lm_intact.expect("lm report").passes);
        assert!(v.vision.expect("vision").displaces);
        assert!(v.agentic.expect("agentic").displaces);
    }

    #[test]
    fn lm_regression_is_a_hard_zero_even_with_a_huge_vision_win() {
        // Vision and agentic both improve a lot; the text side drops.
        let chal = slice(0.40, 0.95, 0.95, SubmissionKind::EncoderAndLm);
        let v = judge(&champ(), &chal);
        assert!(!v.eligible, "LM regression must never be promotable");
        assert_eq!(v.lattice, 0, "lattice must be zero, not reduced");
        let hit = v
            .failed
            .iter()
            .any(|f| matches!(f, GateFail::LmRegression { .. }));
        assert!(hit, "expected LmRegression, got {:?}", v.failed);
        // The vision side genuinely won; only gate 1 blocked it.
        assert!(v.vision.expect("vision").displaces);
        assert!(!v.lm_intact.expect("lm report").passes);
    }

    #[test]
    fn lm_noise_inside_epsilon_is_tolerated() {
        let chal = slice(0.695, 0.85, 0.85, SubmissionKind::EncoderAndLm);
        let v = judge(&champ(), &chal);
        assert!(v.eligible, "failed={:?}", v.failed);
    }

    #[test]
    fn encoder_only_submission_must_keep_the_champion_lm_weights() {
        let mut chal = slice(0.70, 0.85, 0.85, SubmissionKind::EncoderOnly);
        chal.lm_weights_hash = "bbbb2222".into();
        let v = judge(&champ(), &chal);
        assert!(v.failed.contains(&GateFail::LmWeightsChanged));
        assert!(!v.eligible);

        // The same weights hash with kind EncoderAndLm is judged on score only.
        let mut both = chal.clone();
        both.kind = SubmissionKind::EncoderAndLm;
        assert!(judge(&champ(), &both).eligible);
    }

    #[test]
    fn shuffled_pixels_must_hurt_or_the_model_is_not_looking() {
        let mut chal = slice(0.70, 0.85, 0.85, SubmissionKind::EncoderOnly);
        // Same score with the image destroyed: a text-only heuristic.
        chal.agentic.shuffled_score = chal.agentic.score;
        let v = judge(&champ(), &chal);
        assert!(v.failed.contains(&GateFail::IgnoresTheImage));
        assert!(!v.eligible);
        assert!(v.shuffle_drop.abs() < 1e-9);
    }

    #[test]
    fn a_small_shuffle_drop_still_counts_as_ignoring_the_image() {
        let mut chal = slice(0.70, 0.85, 0.85, SubmissionKind::EncoderOnly);
        chal.agentic.shuffled_score = chal.agentic.score - 0.02;
        assert!(judge(&champ(), &chal)
            .failed
            .contains(&GateFail::IgnoresTheImage));
    }

    #[test]
    fn zero_traces_cannot_satisfy_the_shuffle_control() {
        let mut chal = slice(0.70, 0.85, 0.85, SubmissionKind::EncoderOnly);
        chal.agentic.traces = 0;
        assert!(judge(&champ(), &chal)
            .failed
            .contains(&GateFail::IgnoresTheImage));
    }

    #[test]
    fn vision_regression_blocks() {
        let v = judge(
            &champ(),
            &slice(0.70, 0.30, 0.30, SubmissionKind::EncoderOnly),
        );
        assert!(v.failed.contains(&GateFail::VisionRegression));
        assert!(!v.eligible);
        assert_eq!(v.lattice, 0);
    }

    #[test]
    fn missing_vision_task_family_blocks() {
        let mut chal = slice(0.70, 0.85, 0.85, SubmissionKind::EncoderOnly);
        chal.vision_by_task.remove(&VisionTask::Ocr);
        let v = judge(&champ(), &chal);
        assert!(v.failed.contains(&GateFail::VisionTaskMissing {
            task: VisionTask::Ocr
        }));
        assert!(!v.eligible);
    }

    #[test]
    fn non_permissive_encoder_blocks() {
        let chal = slice(0.70, 0.85, 0.85, SubmissionKind::EncoderOnly);
        let v = judge_challenger(&champ(), &chal, CHAMP_HASH, false);
        assert!(v.failed.contains(&GateFail::EncoderLicense));
        assert!(!v.eligible);
    }

    #[test]
    fn missing_text_evidence_blocks_rather_than_passing() {
        let mut chal = slice(0.70, 0.85, 0.85, SubmissionKind::EncoderOnly);
        chal.text_holdout = ExampleSeries::default();
        let v = judge(&champ(), &chal);
        assert!(v.failed.contains(&GateFail::LmEvidenceMissing));
        assert!(v.lm_intact.is_none());
        assert!(!v.eligible);
    }

    #[test]
    fn thin_slices_refuse_rather_than_promote() {
        let mut chal = slice(0.70, 0.85, 0.85, SubmissionKind::EncoderOnly);
        chal.vision_holdout = series("v", 8, 0.85);
        chal.agentic_series = series("a", 8, 0.85);
        let mut c = champ();
        c.vision_holdout = series("v", 8, 0.50);
        c.agentic_series = series("a", 8, 0.50);
        let v = judge(&c, &chal);
        assert!(v.failed.contains(&GateFail::PairedRefusal));
        assert!(!v.eligible);
    }

    #[test]
    fn weighted_vision_renormalizes_missing_families() {
        let mut s = slice(0.70, 0.80, 0.80, SubmissionKind::EncoderOnly);
        let w = VisionTaskWeights::default();
        assert!((s.weighted_vision(&w).expect("all tasks") - 0.80).abs() < 1e-9);
        s.vision_by_task
            .insert(VisionTask::Ocr, series("ocr", 40, 0.40));
        let got = s.weighted_vision(&w).expect("weighted");
        assert!((got - (0.80 * 3.0 + 0.40) / 4.0).abs() < 1e-9, "{got}");
        s.vision_by_task.clear();
        assert!(s.weighted_vision(&w).is_none());
    }

    #[test]
    fn lattice_endpoints() {
        assert_eq!(lattice_from_win_rate(0), 0);
        assert_eq!(lattice_from_win_rate(10_000), SCORE_MAX);
        assert_eq!(champion_hold_lattice(), SCORE_MAX / 2);
    }
}
