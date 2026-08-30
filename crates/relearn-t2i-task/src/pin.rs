//! `config/relearn-t2i-pin.toml`: base checkpoint, judge, sampler, splits.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::prompts::FrozenPrompt;
use crate::{
    base_is_rejected, base_matches_pin, derive_generation_seed, is_bench_prompt_id, BASE_MODEL_ID,
    BASE_MODEL_LICENSE, BASE_MODEL_LICENSE_URL, CHALLENGE_ID, JUDGE_DATASET_ID, JUDGE_GIT_URL,
    JUDGE_MODEL_ID, RELEARN_GIT_URL, SCORING_VERSION,
};

/// Minimum scored image cells per split.
///
/// The paired displacement test refuses a verdict below 100 decided examples,
/// so a pin that cannot reach 100 cells can never promote anything. Rejecting
/// it here turns a silent permanent champion-hold into a config error.
pub const MIN_SCORED_CELLS: usize = 100;

/// Frozen sampler recipe. Defaults are the Cosmos3 card's text-to-image recipe.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SamplerConfig {
    /// Output width in pixels.
    pub width: u32,
    /// Output height in pixels.
    pub height: u32,
    /// Denoising steps.
    pub num_inference_steps: u32,
    /// Classifier-free guidance scale.
    pub guidance_scale: f64,
    /// Flow-matching shift passed to the scheduler.
    pub flow_shift: f64,
    /// Negative prompt (empty in the card recipe).
    pub negative_prompt: String,
    /// Frames per generation; 1 for single-image output.
    pub num_frames: u32,
    /// Compute dtype. Cosmos3 is tested at BF16 only.
    pub dtype: String,
    /// Scheduler class name.
    pub scheduler: String,
}

impl Default for SamplerConfig {
    fn default() -> Self {
        Self {
            width: 1024,
            height: 1024,
            num_inference_steps: 50,
            guidance_scale: 4.0,
            flow_shift: 3.0,
            negative_prompt: String::new(),
            num_frames: 1,
            dtype: "bfloat16".into(),
            scheduler: "UniPCMultistepScheduler".into(),
        }
    }
}

/// Split and seed-lattice pins.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PromptPin {
    /// Salt mixed into every generation seed. Rotating it reshuffles the whole
    /// lattice without touching the derivation formula.
    pub pin_salt: String,
    /// Images generated per prompt.
    pub variations_per_prompt: u32,
    /// Published prompt ids. Miners may train on these.
    pub public_ids: Vec<u32>,
    /// Commitment over the holdout records. The records themselves stay off git.
    pub holdout_commitment: String,
    /// Expected holdout record count.
    pub holdout_size: usize,
}

impl Default for PromptPin {
    fn default() -> Self {
        Self {
            pin_salt: String::new(),
            variations_per_prompt: 4,
            public_ids: Vec::new(),
            holdout_commitment: String::new(),
            holdout_size: 0,
        }
    }
}

/// Everything Cortex needs to reproduce a Relearn T2I eval.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RelearnT2iPin {
    /// Must be [`CHALLENGE_ID`].
    pub challenge_id: String,
    /// Must be [`SCORING_VERSION`].
    pub scoring_version: u16,
    /// Pinned generator checkpoint miners fine-tune.
    pub base: String,
    /// License miners inherit from `base`.
    pub base_license: String,
    /// Canonical license text URL.
    pub base_license_url: String,
    /// Pinned Hugging Face revision of `base` (empty until recorded).
    pub base_revision: String,
    /// Judge model. Must be Q-Judger.
    pub judge_model: String,
    /// Bench prompt dataset.
    pub judge_dataset: String,
    /// Judge / bench harness source.
    pub judge_git: String,
    /// Eval image reference (no floating tag in prod).
    pub eval_image: String,
    /// `sha256:…` digest. Empty until the first digest-pinned eval image ships.
    pub eval_image_digest: String,
    /// Miner-facing repo.
    pub relearn_git: String,
    /// Frozen sampler recipe.
    pub sampler: SamplerConfig,
    /// Split and seed pins.
    pub prompts: PromptPin,
    /// Frozen public-split prompt records.
    #[serde(rename = "frozen_prompt")]
    pub frozen_prompts: Vec<FrozenPrompt>,
}

impl Default for RelearnT2iPin {
    fn default() -> Self {
        Self {
            challenge_id: CHALLENGE_ID.into(),
            scoring_version: SCORING_VERSION,
            base: BASE_MODEL_ID.into(),
            base_license: BASE_MODEL_LICENSE.into(),
            base_license_url: BASE_MODEL_LICENSE_URL.into(),
            base_revision: String::new(),
            judge_model: JUDGE_MODEL_ID.into(),
            judge_dataset: JUDGE_DATASET_ID.into(),
            judge_git: JUDGE_GIT_URL.into(),
            eval_image: "ghcr.io/cortexlm/relearn-t2i-eval".into(),
            eval_image_digest: String::new(),
            relearn_git: RELEARN_GIT_URL.into(),
            sampler: SamplerConfig::default(),
            prompts: PromptPin::default(),
            frozen_prompts: Vec::new(),
        }
    }
}

/// Why a pin was refused.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PinError {
    /// TOML did not parse.
    #[error("parse relearn-t2i pin: {0}")]
    Parse(String),
    /// Pin declares the wrong challenge id or scoring version.
    #[error("pin identity mismatch: {0}")]
    Identity(String),
    /// The pinned base is a refused family (Flux).
    #[error("base {0:?} is a refused family for this challenge")]
    RejectedBase(String),
    /// The judge is not Q-Judger.
    #[error("judge must be {expected:?}, pin says {got:?}")]
    JudgeNotQJudger {
        /// Required judge id.
        expected: String,
        /// What the pin declared.
        got: String,
    },
    /// `public_ids` and the frozen public records disagree.
    #[error("public_ids and frozen_prompt records disagree")]
    PublicSplitMismatch,
    /// A pinned prompt id is outside the bench range.
    #[error("prompt id {0} outside Qwen-Image-Bench range 1..=1000")]
    PromptIdOutOfRange(u32),
    /// The seed salt is empty, so the lattice is not pinned.
    #[error("prompts.pin_salt must not be empty")]
    EmptySalt,
    /// A split cannot reach the paired test's evidence floor.
    #[error("{split} yields {cells} scored cells, below the {min} floor")]
    TooFewCells {
        /// `public` or `holdout`.
        split: String,
        /// Cells the pin would produce.
        cells: usize,
        /// Required floor.
        min: usize,
    },
    /// Holdout commitment is not a 64-hex digest.
    #[error("prompts.holdout_commitment must be 64 hex chars")]
    BadHoldoutCommitment,
}

/// One image cell to generate: prompt, variation index, and frozen seed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeedCell {
    /// Bench prompt id.
    pub prompt_id: u32,
    /// Variation index within the prompt.
    pub variation_index: u32,
    /// Frozen generation seed.
    pub seed: u64,
}

impl RelearnT2iPin {
    /// Parse and validate `config/relearn-t2i-pin.toml`.
    ///
    /// # Errors
    ///
    /// [`PinError::Parse`] on malformed TOML, otherwise any [`validate`]
    /// failure.
    ///
    /// [`validate`]: Self::validate
    pub fn from_toml(body: &str) -> Result<Self, PinError> {
        let pin: Self = toml::from_str(body).map_err(|e| PinError::Parse(e.to_string()))?;
        pin.validate()?;
        Ok(pin)
    }

    /// Enforce every product rule the pin is responsible for.
    ///
    /// # Errors
    ///
    /// See [`PinError`]. Notably a Flux-family base and a non-Q-Judger judge
    /// are both hard refusals, not warnings.
    pub fn validate(&self) -> Result<(), PinError> {
        if self.challenge_id != CHALLENGE_ID {
            return Err(PinError::Identity(format!(
                "challenge_id must be {CHALLENGE_ID:?}, got {:?}",
                self.challenge_id
            )));
        }
        if self.scoring_version != SCORING_VERSION {
            return Err(PinError::Identity(format!(
                "scoring_version must be {SCORING_VERSION}, got {}",
                self.scoring_version
            )));
        }
        if base_is_rejected(&self.base) {
            return Err(PinError::RejectedBase(self.base.clone()));
        }
        if !base_matches_pin(&self.base, BASE_MODEL_ID) {
            return Err(PinError::Identity(format!(
                "base must be {BASE_MODEL_ID:?}, got {:?}",
                self.base
            )));
        }
        if !base_matches_pin(&self.judge_model, JUDGE_MODEL_ID) {
            return Err(PinError::JudgeNotQJudger {
                expected: JUDGE_MODEL_ID.into(),
                got: self.judge_model.clone(),
            });
        }
        if self.prompts.pin_salt.trim().is_empty() {
            return Err(PinError::EmptySalt);
        }
        for id in &self.prompts.public_ids {
            if !is_bench_prompt_id(*id) {
                return Err(PinError::PromptIdOutOfRange(*id));
            }
        }
        let mut frozen: Vec<u32> = self.frozen_prompts.iter().map(|p| p.id).collect();
        frozen.sort_unstable();
        let mut declared = self.prompts.public_ids.clone();
        declared.sort_unstable();
        if frozen != declared {
            return Err(PinError::PublicSplitMismatch);
        }
        let commitment = self.prompts.holdout_commitment.trim();
        if commitment.len() != 64 || !commitment.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(PinError::BadHoldoutCommitment);
        }
        self.check_cells("public", declared.len())?;
        self.check_cells("holdout", self.prompts.holdout_size)?;
        Ok(())
    }

    fn check_cells(&self, split: &str, prompt_count: usize) -> Result<(), PinError> {
        let cells = prompt_count.saturating_mul(self.prompts.variations_per_prompt as usize);
        if cells < MIN_SCORED_CELLS {
            return Err(PinError::TooFewCells {
                split: split.to_owned(),
                cells,
                min: MIN_SCORED_CELLS,
            });
        }
        Ok(())
    }

    /// True when a live rent is allowed (real digest pin present).
    #[must_use]
    pub fn can_rent(&self) -> bool {
        self.eval_image_digest.starts_with("sha256:") && self.eval_image_digest.len() >= 71
    }

    /// Frozen seed for one cell.
    #[must_use]
    pub fn seed_for(&self, prompt_id: u32, variation_index: u32) -> u64 {
        derive_generation_seed(prompt_id, variation_index, &self.prompts.pin_salt)
    }

    /// Every cell to generate for `prompt_ids`, in deterministic order.
    #[must_use]
    pub fn seed_cells(&self, prompt_ids: &[u32]) -> Vec<SeedCell> {
        let mut ids = prompt_ids.to_vec();
        ids.sort_unstable();
        ids.dedup();
        let mut out = Vec::with_capacity(ids.len() * self.prompts.variations_per_prompt as usize);
        for id in ids {
            for v in 0..self.prompts.variations_per_prompt {
                out.push(SeedCell {
                    prompt_id: id,
                    variation_index: v,
                    seed: self.seed_for(id, v),
                });
            }
        }
        out
    }

    /// Check a miner's declared base and license against the pin.
    ///
    /// This is the artifact-side license attestation: a submission must say it
    /// fine-tuned the pinned Cosmos3 checkpoint under OpenMDW 1.1.
    ///
    /// # Errors
    ///
    /// [`PinError::RejectedBase`] for a Flux-family declaration,
    /// [`PinError::Identity`] for any other mismatch.
    pub fn attest_artifact_base(
        &self,
        declared_base: &str,
        declared_license: &str,
    ) -> Result<(), PinError> {
        if base_is_rejected(declared_base) {
            return Err(PinError::RejectedBase(declared_base.to_owned()));
        }
        if !base_matches_pin(declared_base, &self.base) {
            return Err(PinError::Identity(format!(
                "artifact base must be {:?}, got {declared_base:?}",
                self.base
            )));
        }
        let norm = |s: &str| s.trim().to_ascii_lowercase().replace([' ', '_'], "-");
        if norm(declared_license) != norm(&self.base_license) {
            return Err(PinError::Identity(format!(
                "artifact license must be {:?}, got {declared_license:?}",
                self.base_license
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use super::*;

    fn pin_body(base: &str, judge: &str) -> String {
        let mut body = format!(
            r#"
challenge_id = "relearn-t2i"
scoring_version = 1
base = "{base}"
base_license = "OpenMDW-1.1"
judge_model = "{judge}"

[prompts]
pin_salt = "cortex-t2i-v0"
variations_per_prompt = 4
public_ids = [{ids}]
holdout_commitment = "{c}"
holdout_size = 25
"#,
            ids = (1..=25)
                .map(|i| i.to_string())
                .collect::<Vec<_>>()
                .join(", "),
            c = "ab".repeat(32),
        );
        for i in 1..=25 {
            let _ = write!(
                body,
                "\n[[frozen_prompt]]\nid = {i}\ntext = \"frozen prompt {i}\"\n"
            );
        }
        body
    }

    #[test]
    fn default_pin_is_cosmos3_and_q_judger() {
        let p = RelearnT2iPin::default();
        assert_eq!(p.base, "nvidia/Cosmos3-Super-Text2Image");
        assert_eq!(p.judge_model, "Qwen/Qwen-Image-Bench");
        assert_eq!(p.base_license, "OpenMDW-1.1");
        assert!(!p.can_rent());
    }

    #[test]
    fn sampler_defaults_follow_the_card_recipe() {
        let s = SamplerConfig::default();
        assert_eq!((s.width, s.height), (1024, 1024));
        assert_eq!(s.num_inference_steps, 50);
        assert!((s.guidance_scale - 4.0).abs() < f64::EPSILON);
        assert!((s.flow_shift - 3.0).abs() < f64::EPSILON);
        assert_eq!(s.num_frames, 1);
        assert_eq!(s.dtype, "bfloat16");
    }

    #[test]
    fn valid_pin_parses() {
        let p = RelearnT2iPin::from_toml(&pin_body(
            "nvidia/Cosmos3-Super-Text2Image",
            "Qwen/Qwen-Image-Bench",
        ))
        .expect("pin parses");
        assert_eq!(p.frozen_prompts.len(), 25);
        assert_eq!(p.seed_cells(&p.prompts.public_ids).len(), 100);
    }

    #[test]
    fn flux_base_is_refused_by_the_pin() {
        let err = RelearnT2iPin::from_toml(&pin_body(
            "black-forest-labs/FLUX.1-dev",
            "Qwen/Qwen-Image-Bench",
        ))
        .expect_err("flux must not pin");
        assert!(matches!(err, PinError::RejectedBase(_)), "{err:?}");
    }

    #[test]
    fn swapping_the_judge_is_refused() {
        let err = RelearnT2iPin::from_toml(&pin_body(
            "nvidia/Cosmos3-Super-Text2Image",
            "openai/some-vlm",
        ))
        .expect_err("only Q-Judger judges");
        assert!(matches!(err, PinError::JudgeNotQJudger { .. }), "{err:?}");
    }

    #[test]
    fn thin_split_is_refused_instead_of_holding_forever() {
        let mut p = RelearnT2iPin::from_toml(&pin_body(
            "nvidia/Cosmos3-Super-Text2Image",
            "Qwen/Qwen-Image-Bench",
        ))
        .expect("pin");
        p.prompts.variations_per_prompt = 1;
        let err = p.validate().expect_err("too few cells");
        assert!(matches!(err, PinError::TooFewCells { .. }), "{err:?}");
    }

    #[test]
    fn public_ids_must_match_frozen_records() {
        let mut p = RelearnT2iPin::from_toml(&pin_body(
            "nvidia/Cosmos3-Super-Text2Image",
            "Qwen/Qwen-Image-Bench",
        ))
        .expect("pin");
        p.prompts.public_ids.push(999);
        assert_eq!(p.validate(), Err(PinError::PublicSplitMismatch));
    }

    #[test]
    fn empty_salt_is_refused() {
        let mut p = RelearnT2iPin::from_toml(&pin_body(
            "nvidia/Cosmos3-Super-Text2Image",
            "Qwen/Qwen-Image-Bench",
        ))
        .expect("pin");
        p.prompts.pin_salt = "  ".into();
        assert_eq!(p.validate(), Err(PinError::EmptySalt));
    }

    #[test]
    fn seed_cells_are_deterministic_and_deduped() {
        let p = RelearnT2iPin::from_toml(&pin_body(
            "nvidia/Cosmos3-Super-Text2Image",
            "Qwen/Qwen-Image-Bench",
        ))
        .expect("pin");
        let a = p.seed_cells(&[3, 1, 3]);
        let b = p.seed_cells(&[1, 3]);
        assert_eq!(a, b);
        assert_eq!(a.len(), 8);
        assert_eq!(a[0].seed, p.seed_for(1, 0));
    }

    #[test]
    fn artifact_attestation_rejects_flux_and_wrong_license() {
        let p = RelearnT2iPin::default();
        p.attest_artifact_base("nvidia/Cosmos3-Super-Text2Image", "OpenMDW 1.1")
            .expect("pinned base + license");
        assert!(matches!(
            p.attest_artifact_base("black-forest-labs/FLUX.1-dev", "OpenMDW-1.1"),
            Err(PinError::RejectedBase(_))
        ));
        assert!(matches!(
            p.attest_artifact_base("nvidia/Cosmos3-Super-Text2Image", "cc-by-nc-4.0"),
            Err(PinError::Identity(_))
        ));
        assert!(matches!(
            p.attest_artifact_base("stabilityai/sd-3.5", "OpenMDW-1.1"),
            Err(PinError::Identity(_))
        ));
    }

    #[test]
    fn digest_pin_gates_live_rent() {
        let mut p = RelearnT2iPin::default();
        assert!(!p.can_rent());
        p.eval_image_digest = format!("sha256:{}", "00".repeat(32));
        assert!(p.can_rent());
    }
}
