//! Frozen eval prompts and the holdout commitment.
//!
//! NVIDIA recommends upsampling a short prompt into a JSON structure before
//! handing it to Cosmos3. That is fine for a miner's own training, and fatal
//! for a benchmark: two miners with two upsamplers are no longer being scored
//! on the same prompt. So the scored prompts are frozen — the exact string (or
//! the exact upsampled JSON document) is stored once and replayed verbatim.
//!
//! The public split ships in `config/relearn-t2i-pin.toml`. The holdout split
//! must not, because that file is public: git carries only a commitment, and
//! the operator supplies the records out of band. A mismatch fails closed.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{is_bench_prompt_id, HOLDOUT_DOMAIN};

/// One frozen eval prompt, addressed by Qwen-Image-Bench id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrozenPrompt {
    /// Qwen-Image-Bench prompt id (1..=1000).
    pub id: u32,
    /// Original bench prompt text.
    pub text: String,
    /// Frozen upsampled JSON document, when the pin uses one. Serialized
    /// exactly as it will be sent to the generator.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upsampled_json: Option<String>,
}

impl FrozenPrompt {
    /// The exact string handed to the generator for this cell.
    ///
    /// Callers must not re-run an upsampler on this value.
    #[must_use]
    pub fn generator_input(&self) -> &str {
        self.upsampled_json.as_deref().unwrap_or(&self.text)
    }
}

/// Why a frozen prompt set was refused.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum HoldoutError {
    /// The record set is empty; there is nothing to score.
    #[error("frozen prompt set is empty")]
    Empty,
    /// Two records claim the same bench id.
    #[error("duplicate prompt id {0}")]
    DuplicateId(u32),
    /// A record is outside the published bench id range.
    #[error("prompt id {0} outside Qwen-Image-Bench range 1..=1000")]
    OutOfRange(u32),
    /// A record has no prompt body at all.
    #[error("prompt id {0} has empty text")]
    EmptyText(u32),
    /// The supplied holdout does not match the committed digest.
    #[error("holdout commitment mismatch (expected {expected}, got {got})")]
    CommitmentMismatch {
        /// Digest pinned in git.
        expected: String,
        /// Digest of what the operator supplied.
        got: String,
    },
    /// Holdout size disagrees with the pin.
    #[error("holdout size mismatch (expected {expected}, got {got})")]
    SizeMismatch {
        /// Count pinned in git.
        expected: usize,
        /// Count the operator supplied.
        got: usize,
    },
    /// A holdout id is also in the published public split.
    #[error("holdout prompt id {0} is also in the public split")]
    OverlapsPublic(u32),
}

/// Commitment over a frozen prompt set.
///
/// Domain-separated, id-sorted, and length-prefixed so neither reordering nor
/// splicing two prompt bodies together can collide.
#[must_use]
pub fn frozen_prompt_commitment(records: &[FrozenPrompt]) -> String {
    let mut sorted: Vec<&FrozenPrompt> = records.iter().collect();
    sorted.sort_by_key(|r| r.id);
    let mut h = Sha256::new();
    h.update(HOLDOUT_DOMAIN);
    h.update([0xff]);
    h.update(
        u64::try_from(sorted.len())
            .unwrap_or(u64::MAX)
            .to_le_bytes(),
    );
    for r in sorted {
        h.update(r.id.to_le_bytes());
        for field in [r.text.as_str(), r.upsampled_json.as_deref().unwrap_or("")] {
            h.update(u64::try_from(field.len()).unwrap_or(u64::MAX).to_le_bytes());
            h.update(field.as_bytes());
        }
    }
    hex::encode(h.finalize())
}

fn validate_records(records: &[FrozenPrompt]) -> Result<(), HoldoutError> {
    if records.is_empty() {
        return Err(HoldoutError::Empty);
    }
    let mut seen = std::collections::BTreeSet::new();
    for r in records {
        if !is_bench_prompt_id(r.id) {
            return Err(HoldoutError::OutOfRange(r.id));
        }
        if r.generator_input().trim().is_empty() {
            return Err(HoldoutError::EmptyText(r.id));
        }
        if !seen.insert(r.id) {
            return Err(HoldoutError::DuplicateId(r.id));
        }
    }
    Ok(())
}

/// Verify an operator-supplied holdout against the committed digest.
///
/// # Errors
///
/// Any structural problem, a size disagreement, a public-split overlap, or a
/// commitment mismatch. Every one of those is fail-closed: the caller must
/// refuse to score rather than fall back to the public split.
pub fn verify_holdout_prompts(
    records: &[FrozenPrompt],
    public_ids: &[u32],
    expected_commitment: &str,
    expected_size: usize,
) -> Result<(), HoldoutError> {
    validate_records(records)?;
    if records.len() != expected_size {
        return Err(HoldoutError::SizeMismatch {
            expected: expected_size,
            got: records.len(),
        });
    }
    for r in records {
        if public_ids.contains(&r.id) {
            return Err(HoldoutError::OverlapsPublic(r.id));
        }
    }
    let got = frozen_prompt_commitment(records);
    if !got.eq_ignore_ascii_case(expected_commitment.trim()) {
        return Err(HoldoutError::CommitmentMismatch {
            expected: expected_commitment.trim().to_ascii_lowercase(),
            got,
        });
    }
    Ok(())
}

/// A resolved eval split: the published prompts plus the unsealed holdout.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PromptSplit {
    /// Published prompts. Miners may train on these.
    pub public: Vec<FrozenPrompt>,
    /// Holdout prompts. Empty until the operator unseals them.
    pub holdout: Vec<FrozenPrompt>,
}

impl PromptSplit {
    /// Public split only (holdout still sealed).
    #[must_use]
    pub fn public_only(public: Vec<FrozenPrompt>) -> Self {
        Self {
            public,
            holdout: Vec::new(),
        }
    }

    /// Whether a holdout has been unsealed for this run.
    #[must_use]
    pub fn holdout_unsealed(&self) -> bool {
        !self.holdout.is_empty()
    }

    /// Public prompt ids in ascending order.
    #[must_use]
    pub fn public_ids(&self) -> Vec<u32> {
        let mut v: Vec<u32> = self.public.iter().map(|p| p.id).collect();
        v.sort_unstable();
        v
    }

    /// Holdout prompt ids in ascending order. Never logged or served.
    #[must_use]
    pub fn holdout_ids(&self) -> Vec<u32> {
        let mut v: Vec<u32> = self.holdout.iter().map(|p| p.id).collect();
        v.sort_unstable();
        v
    }

    /// Validate the public split structurally.
    ///
    /// # Errors
    ///
    /// See [`HoldoutError`].
    pub fn validate_public(&self) -> Result<(), HoldoutError> {
        validate_records(&self.public)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(id: u32, text: &str) -> FrozenPrompt {
        FrozenPrompt {
            id,
            text: text.into(),
            upsampled_json: None,
        }
    }

    fn holdout() -> Vec<FrozenPrompt> {
        vec![p(900, "a red cube on a wooden table"), p(901, "two cats")]
    }

    #[test]
    fn commitment_is_order_independent() {
        let a = frozen_prompt_commitment(&holdout());
        let mut rev = holdout();
        rev.reverse();
        assert_eq!(a, frozen_prompt_commitment(&rev));
        assert_eq!(a.len(), 64);
    }

    #[test]
    fn commitment_changes_with_body() {
        let a = frozen_prompt_commitment(&holdout());
        let mut edited = holdout();
        edited[0].text.push('.');
        assert_ne!(a, frozen_prompt_commitment(&edited));
    }

    #[test]
    fn commitment_is_length_prefixed_against_splicing() {
        let ab = frozen_prompt_commitment(&[p(1, "ab"), p(2, "c")]);
        let a_bc = frozen_prompt_commitment(&[p(1, "a"), p(2, "bc")]);
        assert_ne!(ab, a_bc);
    }

    #[test]
    fn upsampled_json_is_the_generator_input() {
        let mut r = p(5, "a bird");
        assert_eq!(r.generator_input(), "a bird");
        r.upsampled_json = Some("{\"subject\":\"a bird\"}".into());
        assert_eq!(r.generator_input(), "{\"subject\":\"a bird\"}");
    }

    #[test]
    fn verify_accepts_the_committed_set() {
        let recs = holdout();
        let c = frozen_prompt_commitment(&recs);
        verify_holdout_prompts(&recs, &[1, 2, 3], &c, 2).expect("committed holdout verifies");
    }

    #[test]
    fn verify_rejects_edited_set() {
        let recs = holdout();
        let c = frozen_prompt_commitment(&recs);
        let mut edited = recs;
        edited[1].text = "three cats".into();
        let err = verify_holdout_prompts(&edited, &[], &c, 2).expect_err("must reject");
        assert!(matches!(err, HoldoutError::CommitmentMismatch { .. }));
    }

    #[test]
    fn verify_rejects_public_overlap() {
        let recs = holdout();
        let c = frozen_prompt_commitment(&recs);
        let err = verify_holdout_prompts(&recs, &[901], &c, 2).expect_err("must reject");
        assert_eq!(err, HoldoutError::OverlapsPublic(901));
    }

    #[test]
    fn verify_rejects_size_drift() {
        let recs = holdout();
        let c = frozen_prompt_commitment(&recs);
        let err = verify_holdout_prompts(&recs, &[], &c, 3).expect_err("must reject");
        assert!(matches!(err, HoldoutError::SizeMismatch { .. }));
    }

    #[test]
    fn verify_rejects_out_of_range_and_duplicates() {
        let bad = vec![p(0, "x")];
        assert_eq!(
            verify_holdout_prompts(&bad, &[], "00", 1).expect_err("range"),
            HoldoutError::OutOfRange(0)
        );
        let dup = vec![p(5, "x"), p(5, "y")];
        assert_eq!(
            verify_holdout_prompts(&dup, &[], "00", 2).expect_err("dup"),
            HoldoutError::DuplicateId(5)
        );
        let empty: Vec<FrozenPrompt> = Vec::new();
        assert_eq!(
            verify_holdout_prompts(&empty, &[], "00", 0).expect_err("empty"),
            HoldoutError::Empty
        );
    }

    #[test]
    fn split_reports_seal_state_and_ids() {
        let mut split = PromptSplit::public_only(vec![p(2, "b"), p(1, "a")]);
        assert!(!split.holdout_unsealed());
        assert_eq!(split.public_ids(), vec![1, 2]);
        assert!(split.holdout_ids().is_empty());
        split.validate_public().expect("public ok");
        split.holdout = holdout();
        assert!(split.holdout_unsealed());
        assert_eq!(split.holdout_ids(), vec![900, 901]);
    }
}
