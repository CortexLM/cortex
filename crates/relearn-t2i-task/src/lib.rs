//! Relearn Image challenge identity, verified model pins, and seed derivation.
//!
//! ```text
//! challenge_id     = "relearn-image"
//! scoring_version  = 1
//! task_id domain   = b"base-relearn-t2i-task-id-v1"
//! receipt domain   = b"base-relearn-t2i-receipt-v1"
//! ```
//!
//! Distinct from `relearn` / `relearn-agent` / `bounty` so leaf digests never
//! collide. Master-centralized eval; miners pay Lium.
//!
//! The crate, env prefix, and domain tags keep the pre-launch `t2i` spelling.
//! The domain tags are hashed into the committed holdout commitment and into
//! every leaf digest, so renaming them would invalidate pins rather than
//! rename a product ([`docs/NAMING.md`](../../../docs/NAMING.md)).
//!
//! Two rules in this crate are product rules, not style:
//!
//! 1. The generator seed is the pinned Cosmos3 checkpoint. Flux-family bases
//!    are refused outright ([`base_is_rejected`]).
//! 2. Eval prompts are **frozen** in the pin. Miners never supply their own
//!    prompt upsampler on the scored split, so two submissions are always
//!    compared on identical prompt strings and identical generation seeds
//!    ([`derive_generation_seed`]).

#![forbid(unsafe_code)]
#![allow(clippy::doc_markdown, clippy::module_name_repetitions)]

mod pin;
mod prompts;

pub use pin::{
    is_fixture_holdout_commitment, read_private_holdout_commitment, PinError, PromptPin,
    RelearnT2iPin, SamplerConfig, SeedCell, FIXTURE_HOLDOUT_COMMITMENT,
    LIVE_HOLDOUT_COMMITMENT_ENV, LIVE_HOLDOUT_COMMITMENT_FILE_ENV, MIN_SCORED_CELLS,
};
pub use prompts::{
    frozen_prompt_commitment, verify_holdout_prompts, FrozenPrompt, HoldoutError, PromptSplit,
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Normative challenge id (trust-root / leaf `challenge_id` string).
pub const CHALLENGE_ID: &str = "relearn-image";

/// UTF-8 bytes of [`CHALLENGE_ID`].
pub const CHALLENGE_ID_BYTES: &[u8] = b"relearn-image";

/// Pre-launch spelling kept by the crate names, env prefix, and domain tags.
///
/// Never a challenge id: nothing signs or routes under this string.
pub const INTERNAL_NAME: &str = "relearn-t2i";

/// Live `challenge_scoring_version` (Q-Judger displacement + pillar gates).
pub const SCORING_VERSION: u16 = 1;

/// Domain tag for task id digests.
pub const TASK_ID_DOMAIN: &[u8] = b"base-relearn-t2i-task-id-v1";

/// Domain tag for holdout prompt-set commitments.
pub const HOLDOUT_DOMAIN: &[u8] = b"base-relearn-t2i-holdout-v1";

/// Domain tag for eval-receipt digests.
pub const RECEIPT_DOMAIN: &[u8] = b"base-relearn-t2i-receipt-v1";

/// Domain tag for promotion attestations.
pub const PROMOTE_DOMAIN: &[u8] = b"base-relearn-t2i-promote-v1";

/// Domain tag for per-image generation seeds.
pub const SEED_DOMAIN: &[u8] = b"base-relearn-t2i-seed-v1";

/// Integer score lattice max (same scale as other challenges).
pub const SCORE_MAX: u64 = 1_000_000;

/// Pinned generator seed miners fine-tune.
///
/// Verified 2026-08-30 against <https://huggingface.co/nvidia/Cosmos3-Super-Text2Image>:
/// 65B Cosmos3 Super text-to-image, BF16-only, `Cosmos3OmniPipeline` in
/// Diffusers and `vllm serve … --omni` in vLLM-Omni.
pub const BASE_MODEL_ID: &str = "nvidia/Cosmos3-Super-Text2Image";

/// License miners inherit from the pinned base. Card wording: the model is
/// "ready for commercial and non-commercial use" under OpenMDW 1.1.
pub const BASE_MODEL_LICENSE: &str = "OpenMDW-1.1";

/// Canonical license text for [`BASE_MODEL_LICENSE`].
pub const BASE_MODEL_LICENSE_URL: &str = "https://openmdw.ai/license/1-1/";

/// Q-Judger — the only judge for this challenge.
///
/// Verified 2026-08-30 against <https://huggingface.co/Qwen/Qwen-Image-Bench>:
/// Apache-2.0, fine-tuned from Qwen3.6-27B, JSON scores over five L1 pillars.
pub const JUDGE_MODEL_ID: &str = "Qwen/Qwen-Image-Bench";

/// Base model Q-Judger was fine-tuned from (documentation pin).
pub const JUDGE_BASE_MODEL_ID: &str = "Qwen3.6-27B";

/// Benchmark prompt set (Hugging Face dataset id).
pub const JUDGE_DATASET_ID: &str = "Qwen/Qwen-Image-Bench";

/// Judge / bench harness source.
pub const JUDGE_GIT_URL: &str = "https://github.com/QwenLM/Qwen-Image-Bench";

/// Lowest Qwen-Image-Bench prompt id.
pub const BENCH_PROMPT_ID_MIN: u32 = 1;

/// Highest Qwen-Image-Bench prompt id.
pub const BENCH_PROMPT_ID_MAX: u32 = 1000;

/// Public miner / eval-image repo (shared with the text challenge).
pub const RELEARN_GIT_URL: &str = "https://github.com/CortexLM/relearn";

/// Base families that may never be the miner seed.
///
/// Flux is refused as a product decision: its weights are non-commercial,
/// which is incoherent for a subnet that pays for redistributable artifacts.
pub const REJECTED_BASE_SUBSTRINGS: &[&str] = &[
    "flux",
    "black-forest-labs",
    "blackforestlabs",
    "flux.1",
    "flux1",
];

/// True when `model_id` names a base family this challenge refuses.
#[must_use]
pub fn base_is_rejected(model_id: &str) -> bool {
    let lower = model_id.to_ascii_lowercase();
    REJECTED_BASE_SUBSTRINGS.iter().any(|b| lower.contains(b))
}

/// True when the declared base is exactly the pinned checkpoint.
///
/// Comparison is case-insensitive on the Hugging Face repo id and ignores an
/// optional `@revision` suffix; the revision itself is checked against the pin
/// separately so a stale card cannot pass as the pin.
#[must_use]
pub fn base_matches_pin(declared: &str, pinned: &str) -> bool {
    let strip = |s: &str| -> String {
        s.trim()
            .split('@')
            .next()
            .unwrap_or("")
            .trim_matches('/')
            .to_ascii_lowercase()
    };
    !declared.trim().is_empty() && strip(declared) == strip(pinned)
}

/// Level-1 pillar of the Qwen-Image-Bench hierarchy.
///
/// The five pillars are the paper's top level. Order is the paper's order and
/// is part of the wire format: pillar gates are reported per variant name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum L1Dimension {
    /// Realism, detail, resolution.
    Quality,
    /// Composition, color, lighting, anatomy, emotion, style.
    Aesthetics,
    /// Attributes, actions, layout, relations, scene.
    Alignment,
    /// Fairness, safety and compliance, world knowledge.
    RealWorldFidelity,
    /// Imagination, text rendering, design, visual storytelling.
    CreativeGeneration,
}

impl L1Dimension {
    /// All five pillars in the paper's order.
    pub const ALL: [Self; 5] = [
        Self::Quality,
        Self::Aesthetics,
        Self::Alignment,
        Self::RealWorldFidelity,
        Self::CreativeGeneration,
    ];

    /// Card-spelled pillar name (Q-Judger JSON key).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Quality => "Quality",
            Self::Aesthetics => "Aesthetics",
            Self::Alignment => "Alignment",
            Self::RealWorldFidelity => "Real-world Fidelity",
            Self::CreativeGeneration => "Creative Generation",
        }
    }

    /// Parse a pillar from a Q-Judger JSON key. Tolerant of case, spaces,
    /// hyphens, and underscores; unknown keys return `None` (fail closed).
    #[must_use]
    pub fn parse(key: &str) -> Option<Self> {
        let norm = |s: &str| -> String {
            s.chars()
                .filter(char::is_ascii_alphanumeric)
                .map(|c| c.to_ascii_lowercase())
                .collect()
        };
        let want = norm(key);
        Self::ALL.into_iter().find(|d| norm(d.as_str()) == want)
    }
}

/// Deterministic generation seed for one `(prompt_id, variation_index)` cell.
///
/// Every miner generates the scored split at these seeds, so two artifacts are
/// always compared on the same sampler trajectory. The salt lives in the pin so
/// an operator can rotate the whole seed lattice without changing the formula.
///
/// The result is masked into positive `i64` range because the Diffusers and
/// vLLM-Omni paths both accept a signed seed.
#[must_use]
pub fn derive_generation_seed(prompt_id: u32, variation_index: u32, pin_salt: &str) -> u64 {
    let mut h = Sha256::new();
    h.update(SEED_DOMAIN);
    h.update([0xff]);
    h.update(pin_salt.as_bytes());
    h.update([0xff]);
    h.update(prompt_id.to_le_bytes());
    h.update([0xff]);
    h.update(variation_index.to_le_bytes());
    let d = h.finalize();
    let mut eight = [0u8; 8];
    eight.copy_from_slice(&d[..8]);
    u64::from_le_bytes(eight) >> 1
}

/// Stable cell key for a scored image: `p{prompt_id}#v{variation_index}`.
///
/// Both sides of a paired comparison key on this string, so the paired test
/// only ever lines up images generated from the same prompt and same seed.
#[must_use]
pub fn cell_key(prompt_id: u32, variation_index: u32) -> String {
    format!("p{prompt_id}#v{variation_index}")
}

/// True when `prompt_id` is inside the published Qwen-Image-Bench range.
#[must_use]
pub const fn is_bench_prompt_id(prompt_id: u32) -> bool {
    prompt_id >= BENCH_PROMPT_ID_MIN && prompt_id <= BENCH_PROMPT_ID_MAX
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn challenge_id_is_distinct() {
        assert_eq!(CHALLENGE_ID, "relearn-image");
        assert_eq!(CHALLENGE_ID_BYTES, b"relearn-image");
        for other in [
            "relearn",
            "relearn-agent",
            "relearn-mm",
            "bounty",
            "prism",
            "design",
        ] {
            assert_ne!(CHALLENGE_ID, other);
        }
        // The legacy spelling must never route or sign.
        assert_ne!(CHALLENGE_ID, INTERNAL_NAME);
    }

    #[test]
    fn domain_tags_are_t2i_prefixed_and_unique() {
        let tags = [
            TASK_ID_DOMAIN,
            HOLDOUT_DOMAIN,
            RECEIPT_DOMAIN,
            PROMOTE_DOMAIN,
            SEED_DOMAIN,
        ];
        for t in tags {
            let s = std::str::from_utf8(t).unwrap_or("");
            assert!(s.contains("relearn-t2i"), "{s}");
        }
        for i in 0..tags.len() {
            for j in (i + 1)..tags.len() {
                assert_ne!(tags[i], tags[j]);
            }
        }
    }

    #[test]
    fn base_pin_is_cosmos3_not_flux() {
        assert_eq!(BASE_MODEL_ID, "nvidia/Cosmos3-Super-Text2Image");
        assert_eq!(BASE_MODEL_LICENSE, "OpenMDW-1.1");
        assert!(!base_is_rejected(BASE_MODEL_ID));
    }

    #[test]
    fn flux_family_is_rejected() {
        for bad in [
            "black-forest-labs/FLUX.1-dev",
            "black-forest-labs/FLUX.1-schnell",
            "BLACK-FOREST-LABS/flux.1-pro",
            "someone/flux1-merged-lora",
            "mirror/Flux",
        ] {
            assert!(base_is_rejected(bad), "{bad} must be rejected");
        }
    }

    #[test]
    fn base_match_ignores_case_and_revision() {
        assert!(base_matches_pin(
            "NVIDIA/cosmos3-super-text2image",
            BASE_MODEL_ID
        ));
        assert!(base_matches_pin(
            "nvidia/Cosmos3-Super-Text2Image@da579b9",
            BASE_MODEL_ID
        ));
        assert!(!base_matches_pin(
            "nvidia/Cosmos3-Super-Image2Video",
            BASE_MODEL_ID
        ));
        assert!(!base_matches_pin("", BASE_MODEL_ID));
    }

    #[test]
    fn judge_pin_is_q_judger() {
        assert_eq!(JUDGE_MODEL_ID, "Qwen/Qwen-Image-Bench");
        assert_eq!(JUDGE_DATASET_ID, "Qwen/Qwen-Image-Bench");
        assert_eq!(JUDGE_BASE_MODEL_ID, "Qwen3.6-27B");
    }

    #[test]
    fn seed_derivation_is_stable_and_separated() {
        let a = derive_generation_seed(7, 0, "salt-a");
        assert_eq!(a, derive_generation_seed(7, 0, "salt-a"));
        assert_ne!(a, derive_generation_seed(7, 1, "salt-a"));
        assert_ne!(a, derive_generation_seed(8, 0, "salt-a"));
        assert_ne!(a, derive_generation_seed(7, 0, "salt-b"));
        // Positive i64 range so signed-seed backends round-trip it.
        assert!(i64::try_from(a).is_ok());
    }

    #[test]
    fn seed_derivation_has_known_vector() {
        // Frozen so a refactor that changes the preimage fails loudly:
        // every miner's images would otherwise silently stop being comparable.
        assert_eq!(
            derive_generation_seed(1, 0, "cortex-t2i-v0"),
            5_534_307_901_387_864_795
        );
    }

    #[test]
    fn cell_keys_are_unique_per_cell() {
        assert_eq!(cell_key(12, 3), "p12#v3");
        assert_ne!(cell_key(12, 3), cell_key(123, 3));
    }

    #[test]
    fn pillars_round_trip_through_json_keys() {
        for d in L1Dimension::ALL {
            assert_eq!(L1Dimension::parse(d.as_str()), Some(d));
        }
        assert_eq!(
            L1Dimension::parse("real_world_fidelity"),
            Some(L1Dimension::RealWorldFidelity)
        );
        assert_eq!(
            L1Dimension::parse("Creative-Generation"),
            Some(L1Dimension::CreativeGeneration)
        );
        assert_eq!(L1Dimension::parse("Vibes"), None);
    }

    #[test]
    fn bench_prompt_range_is_one_to_thousand() {
        assert!(is_bench_prompt_id(1));
        assert!(is_bench_prompt_id(1000));
        assert!(!is_bench_prompt_id(0));
        assert!(!is_bench_prompt_id(1001));
    }
}
