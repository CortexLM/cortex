//! Relearn Multimodal challenge identity and verified encoder pins.
//!
//! ```text
//! challenge_id     = "relearn-mm"
//! scoring_version  = 1
//! task_id domain   = b"base-relearn-mm-task-id-v1"
//! receipt domain   = b"base-relearn-mm-receipt-v1"
//! ```
//!
//! Miners attach a permissively licensed vision encoder plus a projector to the
//! champion Relearn LLM. They are paid to make the model see, and they are not
//! paid to break it: the text side of the champion is a hard gate, so a vision
//! win that costs language ability scores zero rather than partial credit.
//!
//! Encoder licenses are restricted to OSI-permissive terms
//! ([`license_is_permissive`]) because the artifact has to be redistributable.

#![forbid(unsafe_code)]
#![allow(clippy::doc_markdown, clippy::module_name_repetitions)]

mod pin;

pub use pin::{
    PinError, RelearnMmPin, SubmissionKind, VisionTaskWeights, MIN_AGENTIC_TRACES, MIN_TEXT_ITEMS,
    MIN_VISION_ITEMS_PER_TASK,
};

/// Normative challenge id (trust-root / leaf `challenge_id` string).
pub const CHALLENGE_ID: &str = "relearn-mm";

/// UTF-8 bytes of [`CHALLENGE_ID`].
pub const CHALLENGE_ID_BYTES: &[u8] = b"relearn-mm";

/// Live `challenge_scoring_version`.
pub const SCORING_VERSION: u16 = 1;

/// Domain tag for task id digests.
pub const TASK_ID_DOMAIN: &[u8] = b"base-relearn-mm-task-id-v1";

/// Domain tag for holdout slice ids.
pub const HOLDOUT_DOMAIN: &[u8] = b"base-relearn-mm-holdout-v1";

/// Domain tag for eval-receipt digests.
pub const RECEIPT_DOMAIN: &[u8] = b"base-relearn-mm-receipt-v1";

/// Domain tag for promotion attestations.
pub const PROMOTE_DOMAIN: &[u8] = b"base-relearn-mm-promote-v1";

/// Integer score lattice max (same scale as other challenges).
pub const SCORE_MAX: u64 = 1_000_000;

/// Language model the encoder attaches to: the Relearn champion's base.
pub const LM_BASE_MODEL_ID: &str = relearn_challenge_task::BASE_MODEL_ID;

/// Pinned vision encoder.
///
/// Verified 2026-08-30 against <https://huggingface.co/google/siglip2-so400m-patch14-384>:
/// Apache-2.0, SigLIP 2 So400m at 384px, intended for use as a VLM vision tower.
pub const ENCODER_MODEL_ID: &str = "google/siglip2-so400m-patch14-384";

/// License of [`ENCODER_MODEL_ID`].
pub const ENCODER_LICENSE: &str = "apache-2.0";

/// Alternate encoders whose cards were verified Apache-2.0 on 2026-08-30.
///
/// An operator may repin to one of these without a code change. Every entry was
/// checked on the model card, not inferred from the family name.
pub const VERIFIED_ENCODER_ALTERNATES: &[(&str, &str)] = &[
    ("google/siglip2-so400m-patch14-384", "apache-2.0"),
    ("google/siglip-so400m-patch14-384", "apache-2.0"),
    // The vision tower inside Idefics2 is SigLIP; the card is Apache-2.0.
    ("HuggingFaceM4/idefics2-8b", "apache-2.0"),
];

/// Licenses a submitted encoder may carry.
///
/// OSI-permissive only. OpenRAIL and other use-restricted terms are refused
/// here even though the T2I challenge accepts OpenMDW for its generator: that
/// base is a single operator-chosen pin, whereas the encoder is miner-supplied
/// and has to stay redistributable without per-use conditions.
pub const PERMISSIVE_LICENSES: &[&str] =
    &["apache-2.0", "mit", "bsd-2-clause", "bsd-3-clause", "isc"];

/// Normalize a license string for comparison (`Apache 2.0` → `apache-2.0`).
#[must_use]
pub fn normalize_license(raw: &str) -> String {
    let lower = raw.trim().to_ascii_lowercase();
    let collapsed: String = lower
        .chars()
        .map(|c| if c == ' ' || c == '_' { '-' } else { c })
        .collect();
    match collapsed.as_str() {
        "apache-2" | "apache2" | "apache-license-2.0" => "apache-2.0".to_owned(),
        "bsd-3" => "bsd-3-clause".to_owned(),
        "bsd-2" => "bsd-2-clause".to_owned(),
        other => other.to_owned(),
    }
}

/// True when `license` is OSI-permissive enough for a submitted encoder.
#[must_use]
pub fn license_is_permissive(license: &str) -> bool {
    let norm = normalize_license(license);
    PERMISSIVE_LICENSES.contains(&norm.as_str())
}

/// Frozen vision holdout task families.
///
/// Deliberately not ImageNet or COCO test splits: both are in the pretraining
/// mix of every candidate encoder, so a score on them measures memorization.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum VisionTask {
    /// Free-form captioning scored against reference captions.
    Captioning,
    /// Visual question answering.
    Vqa,
    /// Reading text rendered inside the image.
    Ocr,
    /// Spatial relations between objects.
    SpatialRelations,
}

impl VisionTask {
    /// All frozen task families.
    pub const ALL: [Self; 4] = [
        Self::Captioning,
        Self::Vqa,
        Self::Ocr,
        Self::SpatialRelations,
    ];

    /// Wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Captioning => "captioning",
            Self::Vqa => "vqa",
            Self::Ocr => "ocr",
            Self::SpatialRelations => "spatial_relations",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn challenge_id_is_distinct() {
        assert_eq!(CHALLENGE_ID, "relearn-mm");
        for other in ["relearn", "relearn-t2i", "bounty", "prism", "design"] {
            assert_ne!(CHALLENGE_ID, other);
        }
    }

    #[test]
    fn domain_tags_are_mm_prefixed_and_unique() {
        let tags = [
            TASK_ID_DOMAIN,
            HOLDOUT_DOMAIN,
            RECEIPT_DOMAIN,
            PROMOTE_DOMAIN,
        ];
        for t in tags {
            assert!(std::str::from_utf8(t).unwrap_or("").contains("relearn-mm"));
        }
        for i in 0..tags.len() {
            for j in (i + 1)..tags.len() {
                assert_ne!(tags[i], tags[j]);
            }
        }
        assert_ne!(TASK_ID_DOMAIN, relearn_challenge_task::TASK_ID_DOMAIN);
    }

    #[test]
    fn encoder_pin_is_verified_apache_siglip2() {
        assert_eq!(ENCODER_MODEL_ID, "google/siglip2-so400m-patch14-384");
        assert_eq!(ENCODER_LICENSE, "apache-2.0");
        assert!(license_is_permissive(ENCODER_LICENSE));
    }

    #[test]
    fn lm_side_tracks_the_relearn_champion_base() {
        assert_eq!(LM_BASE_MODEL_ID, "Qwen/Qwen3.8-Flash-Next");
    }

    #[test]
    fn every_verified_alternate_is_permissive() {
        assert!(VERIFIED_ENCODER_ALTERNATES
            .iter()
            .any(|(id, _)| *id == ENCODER_MODEL_ID));
        for (id, license) in VERIFIED_ENCODER_ALTERNATES {
            assert!(license_is_permissive(license), "{id} carries {license}");
        }
    }

    #[test]
    fn license_normalization_accepts_common_spellings() {
        for ok in ["Apache-2.0", "apache 2.0", "APACHE_2.0", "MIT", " mit "] {
            assert!(license_is_permissive(ok), "{ok} should be permissive");
        }
    }

    #[test]
    fn use_restricted_licenses_are_refused() {
        for bad in [
            "openrail",
            "openrail-m",
            "creativeml-openrail-m",
            "cc-by-nc-4.0",
            "cc-by-nc-sa-4.0",
            "llama3.1",
            "gemma",
            "other",
            "",
        ] {
            assert!(!license_is_permissive(bad), "{bad} must be refused");
        }
    }

    #[test]
    fn openmdw_is_not_permissive_enough_for_a_miner_encoder() {
        // The T2I generator base is OpenMDW by operator choice; a miner-supplied
        // encoder must be OSI-permissive.
        assert!(!license_is_permissive("OpenMDW-1.1"));
    }

    #[test]
    fn vision_tasks_have_stable_wire_names() {
        assert_eq!(VisionTask::ALL.len(), 4);
        assert_eq!(VisionTask::Ocr.as_str(), "ocr");
        assert_eq!(VisionTask::SpatialRelations.as_str(), "spatial_relations");
    }
}
