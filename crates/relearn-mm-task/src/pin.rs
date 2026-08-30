//! `config/relearn-mm-pin.toml`: encoder pin, holdout sizes, gate tolerances.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    license_is_permissive, normalize_license, VisionTask, CHALLENGE_ID, ENCODER_LICENSE,
    ENCODER_MODEL_ID, LM_BASE_MODEL_ID, SCORING_VERSION,
};

/// What a miner submitted.
///
/// Defaults to the stricter [`SubmissionKind::EncoderOnly`]: an unstated kind
/// must not be the one that skips the LM weights-hash check.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubmissionKind {
    /// Encoder (and projector) only; the LM must be the champion, bit for bit.
    #[default]
    EncoderOnly,
    /// Encoder plus an LM adapter, so both gates apply to new weights.
    EncoderAndLm,
}

impl SubmissionKind {
    /// Whether the LM weights must hash-match the champion.
    ///
    /// An encoder-only submission is measured as `champion LM + new encoder`,
    /// so the LM hash is the proof that nothing on the text side moved.
    #[must_use]
    pub const fn requires_champion_lm_hash(self) -> bool {
        matches!(self, Self::EncoderOnly)
    }
}

/// Per-task weights for the vision holdout score.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VisionTaskWeights {
    /// Captioning weight.
    pub captioning: f64,
    /// VQA weight.
    pub vqa: f64,
    /// OCR / text-in-image weight.
    pub ocr: f64,
    /// Spatial-relations weight.
    pub spatial_relations: f64,
}

impl Default for VisionTaskWeights {
    fn default() -> Self {
        Self {
            captioning: 0.25,
            vqa: 0.25,
            ocr: 0.25,
            spatial_relations: 0.25,
        }
    }
}

impl VisionTaskWeights {
    /// Weight for one task family.
    #[must_use]
    pub const fn weight(&self, task: VisionTask) -> f64 {
        match task {
            VisionTask::Captioning => self.captioning,
            VisionTask::Vqa => self.vqa,
            VisionTask::Ocr => self.ocr,
            VisionTask::SpatialRelations => self.spatial_relations,
        }
    }

    /// Sum of every weight.
    #[must_use]
    pub fn total(&self) -> f64 {
        VisionTask::ALL.into_iter().map(|t| self.weight(t)).sum()
    }
}

/// Everything Cortex needs to reproduce a Relearn Multimodal eval.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RelearnMmPin {
    /// Must be [`CHALLENGE_ID`].
    pub challenge_id: String,
    /// Must be [`SCORING_VERSION`].
    pub scoring_version: u16,
    /// Language model the encoder attaches to.
    pub lm_base_model: String,
    /// Pinned vision encoder.
    pub encoder_model: String,
    /// License of `encoder_model`. Must be OSI-permissive.
    pub encoder_license: String,
    /// Pinned Hugging Face revision of `encoder_model` (empty until recorded).
    pub encoder_revision: String,
    /// Eval image reference (no floating tag in prod).
    pub eval_image: String,
    /// `sha256:…` digest. Empty until the first digest-pinned eval image ships.
    pub eval_image_digest: String,
    /// Miner-facing repo.
    pub relearn_git: String,
    /// Vision holdout items per task family.
    pub vision_items_per_task: usize,
    /// Agentic image-tool traces on the holdout.
    pub agentic_traces: usize,
    /// Text holdout items reused from the Relearn LLM challenge.
    pub text_holdout_items: usize,
    /// Per-task weights.
    pub vision_weights: VisionTaskWeights,
}

impl Default for RelearnMmPin {
    fn default() -> Self {
        Self {
            challenge_id: CHALLENGE_ID.into(),
            scoring_version: SCORING_VERSION,
            lm_base_model: LM_BASE_MODEL_ID.into(),
            encoder_model: ENCODER_MODEL_ID.into(),
            encoder_license: ENCODER_LICENSE.into(),
            encoder_revision: String::new(),
            eval_image: "ghcr.io/cortexlm/relearn-mm-eval".into(),
            eval_image_digest: String::new(),
            relearn_git: relearn_challenge_task::RELEARN_GIT_URL.into(),
            vision_items_per_task: 40,
            agentic_traces: 120,
            text_holdout_items: 120,
            vision_weights: VisionTaskWeights::default(),
        }
    }
}

/// Why a pin was refused.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PinError {
    /// TOML did not parse.
    #[error("parse relearn-mm pin: {0}")]
    Parse(String),
    /// Pin declares the wrong challenge id or scoring version.
    #[error("pin identity mismatch: {0}")]
    Identity(String),
    /// The encoder license is not OSI-permissive.
    #[error("encoder license {0:?} is not OSI-permissive (Apache-2.0 / MIT / BSD / ISC)")]
    EncoderLicense(String),
    /// The pin names no encoder at all.
    #[error("encoder_model must not be empty")]
    EmptyEncoder,
    /// A holdout split is too thin for a verdict.
    #[error("{split} has {items} items, below the {min} floor")]
    TooFewItems {
        /// Split name.
        split: String,
        /// Items the pin would produce.
        items: usize,
        /// Required floor.
        min: usize,
    },
    /// Vision task weights do not sum to 1.
    #[error("vision_weights must sum to 1.0, got {0}")]
    BadWeights(String),
}

/// Minimum text holdout items. Matches the paired test's evidence floor.
pub const MIN_TEXT_ITEMS: usize = 100;

/// Minimum vision holdout items per task family.
///
/// Four families pooled, so this floor puts the vision slice at the paired
/// test's 100-example bar.
pub const MIN_VISION_ITEMS_PER_TASK: usize = 25;

/// Minimum agentic image-tool traces.
///
/// The agentic comparison runs through the same bootstrap paired test as the
/// other slices, and that test refuses a verdict below 100 decided examples. A
/// pin with fewer traces would make the agentic gate permanently unsatisfiable
/// — every submission would hold the champion for lack of evidence — so the
/// floor is the statistical bar, not a taste call.
pub const MIN_AGENTIC_TRACES: usize = 100;

impl RelearnMmPin {
    /// Parse and validate `config/relearn-mm-pin.toml`.
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
    /// See [`PinError`].
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
        if self.lm_base_model.trim() != LM_BASE_MODEL_ID {
            return Err(PinError::Identity(format!(
                "lm_base_model must be {LM_BASE_MODEL_ID:?}, got {:?}",
                self.lm_base_model
            )));
        }
        if self.encoder_model.trim().is_empty() {
            return Err(PinError::EmptyEncoder);
        }
        if !license_is_permissive(&self.encoder_license) {
            return Err(PinError::EncoderLicense(self.encoder_license.clone()));
        }
        check_floor(
            "text_holdout_items",
            self.text_holdout_items,
            MIN_TEXT_ITEMS,
        )?;
        check_floor(
            "vision_items_per_task",
            self.vision_items_per_task,
            MIN_VISION_ITEMS_PER_TASK,
        )?;
        check_floor("agentic_traces", self.agentic_traces, MIN_AGENTIC_TRACES)?;
        let total = self.vision_weights.total();
        if (total - 1.0).abs() > 1e-6 {
            return Err(PinError::BadWeights(format!("{total:.6}")));
        }
        Ok(())
    }

    /// True when a live rent is allowed (real digest pin present).
    #[must_use]
    pub fn can_rent(&self) -> bool {
        self.eval_image_digest.starts_with("sha256:") && self.eval_image_digest.len() >= 71
    }

    /// Check a submitted encoder against the license policy.
    ///
    /// The encoder id itself is not pinned to one repo: a miner may bring a
    /// better permissive encoder. Only the license is non-negotiable.
    ///
    /// # Errors
    ///
    /// [`PinError::EmptyEncoder`] or [`PinError::EncoderLicense`].
    pub fn attest_encoder(&self, encoder: &str, license: &str) -> Result<(), PinError> {
        if encoder.trim().is_empty() {
            return Err(PinError::EmptyEncoder);
        }
        if !license_is_permissive(license) {
            return Err(PinError::EncoderLicense(normalize_license(license)));
        }
        Ok(())
    }
}

fn check_floor(split: &str, items: usize, min: usize) -> Result<(), PinError> {
    if items < min {
        return Err(PinError::TooFewItems {
            split: split.to_owned(),
            items,
            min,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const BODY: &str = r#"
challenge_id = "relearn-mm"
scoring_version = 1
lm_base_model = "Qwen/Qwen3.8-Flash-Next"
encoder_model = "google/siglip2-so400m-patch14-384"
encoder_license = "apache-2.0"
vision_items_per_task = 40
agentic_traces = 120
text_holdout_items = 120
"#;

    #[test]
    fn default_pin_is_siglip2_on_the_relearn_base() {
        let p = RelearnMmPin::default();
        assert_eq!(p.encoder_model, "google/siglip2-so400m-patch14-384");
        assert_eq!(p.lm_base_model, "Qwen/Qwen3.8-Flash-Next");
        assert!(!p.can_rent());
        p.validate().expect("default validates");
    }

    #[test]
    fn valid_pin_parses() {
        let p = RelearnMmPin::from_toml(BODY).expect("parses");
        assert_eq!(p.vision_items_per_task, 40);
        assert!((p.vision_weights.total() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn non_permissive_encoder_license_is_refused() {
        let body = BODY.replace("apache-2.0", "creativeml-openrail-m");
        let err = RelearnMmPin::from_toml(&body).expect_err("must refuse");
        assert!(matches!(err, PinError::EncoderLicense(_)), "{err:?}");
    }

    #[test]
    fn thin_splits_are_refused() {
        let thin = [
            RelearnMmPin {
                text_holdout_items: 10,
                ..RelearnMmPin::default()
            },
            RelearnMmPin {
                vision_items_per_task: 2,
                ..RelearnMmPin::default()
            },
            RelearnMmPin {
                agentic_traces: 1,
                ..RelearnMmPin::default()
            },
        ];
        for p in thin {
            assert!(matches!(p.validate(), Err(PinError::TooFewItems { .. })));
        }
    }

    #[test]
    fn weights_must_sum_to_one() {
        let p = RelearnMmPin {
            vision_weights: VisionTaskWeights {
                ocr: 0.9,
                ..VisionTaskWeights::default()
            },
            ..RelearnMmPin::default()
        };
        assert!(matches!(p.validate(), Err(PinError::BadWeights(_))));
    }

    #[test]
    fn encoder_attestation_enforces_license_not_repo() {
        let p = RelearnMmPin::default();
        p.attest_encoder("google/siglip-so400m-patch14-384", "Apache 2.0")
            .expect("a different permissive encoder is allowed");
        p.attest_encoder("some-lab/my-mit-encoder", "MIT")
            .expect("MIT is allowed");
        assert!(matches!(
            p.attest_encoder("some-lab/nc-encoder", "cc-by-nc-4.0"),
            Err(PinError::EncoderLicense(_))
        ));
        assert!(matches!(
            p.attest_encoder("  ", "apache-2.0"),
            Err(PinError::EmptyEncoder)
        ));
    }

    #[test]
    fn encoder_only_submissions_must_prove_the_lm_hash() {
        assert!(SubmissionKind::EncoderOnly.requires_champion_lm_hash());
        assert!(!SubmissionKind::EncoderAndLm.requires_champion_lm_hash());
    }

    #[test]
    fn weights_are_addressable_per_task() {
        let w = VisionTaskWeights::default();
        for t in VisionTask::ALL {
            assert!((w.weight(t) - 0.25).abs() < 1e-9);
        }
    }
}
