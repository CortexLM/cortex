//! Per-topic holdout records and the commitment git carries in their place.
//!
//! A Proof holdout record names an evaluation shard by content hash, not by
//! its text: the harness needs to know *which* bytes were scored and which
//! stratum they belong to, and nothing here should ever be able to print the
//! shard itself. Records live in an operator file
//! (`PROOF_HOLDOUT_FILE`, keyed by `topic_id`) and are checked against the
//! topic document's `holdout_commitment` at load.
//!
//! The five scored splits are **measurement strata**, not the problem list —
//! the problem is the topic document. They exist so `epsilon_topic_max_regress`
//! is a real gate: a recipe that wins on average by wrecking long context or
//! non-English is not an improvement, and 24 records per split is what makes
//! that visible. [`HoldoutSplit::CanaryOffpath`] never enters the 120 and
//! never enters the score.

#![allow(
    clippy::doc_markdown,
    clippy::module_name_repetitions,
    clippy::must_use_candidate
)]

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Domain tag for per-topic holdout commitments.
pub const HOLDOUT_DOMAIN: &[u8] = b"base-proof-holdout-v1";

/// Holdout records per topic.
pub const HOLDOUT_SIZE: usize = 120;

/// Records per scored split (`HOLDOUT_SIZE / scored splits`).
pub const STRATUM_SIZE: usize = 24;

fn is_hex64(s: &str) -> bool {
    let t = s.trim();
    t.len() == 64 && t.chars().all(|c| c.is_ascii_hexdigit())
}

/// Smallest packed sequence length a `longctx` record may carry.
pub const LONGCTX_MIN_TOKENS: u32 = 8_192;

/// Largest packed sequence length a `longctx` record may carry.
pub const LONGCTX_MAX_TOKENS: u32 = 32_768;

/// Measurement stratum a holdout record belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HoldoutSplit {
    /// Web / natural language, disjoint from any public web slice.
    WebOod,
    /// Held-out repositories.
    CodeOod,
    /// Math text (`OpenWebMath`-class), disjoint from any public math slice.
    MathOod,
    /// Packed sequences 8k–32k: catches short-context-only tricks.
    Longctx,
    /// Non-English slice.
    MultilingualOod,
    /// Synthetic canaries. Recorded, never added to the paid metric, and
    /// never part of the 120.
    CanaryOffpath,
}

impl HoldoutSplit {
    pub const SCORED: [Self; 5] = [
        Self::WebOod,
        Self::CodeOod,
        Self::MathOod,
        Self::Longctx,
        Self::MultilingualOod,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::WebOod => "web_ood",
            Self::CodeOod => "code_ood",
            Self::MathOod => "math_ood",
            Self::Longctx => "longctx",
            Self::MultilingualOod => "multilingual_ood",
            Self::CanaryOffpath => "canary_offpath",
        }
    }

    pub const fn is_scored(self) -> bool {
        !matches!(self, Self::CanaryOffpath)
    }
}

/// One frozen evaluation shard.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HoldoutRecord {
    /// Stable record id. Never published for a holdout split.
    pub id: u32,
    /// Measurement stratum.
    pub split: HoldoutSplit,
    /// Source corpus fingerprint (an id, never a URL or a host).
    pub dataset_id: String,
    /// SHA-256 hex of the packed shard bytes the harness scores.
    pub content_sha256: String,
    /// Packed sequence length in tokens.
    pub token_count: u32,
}

impl HoldoutRecord {
    pub fn fingerprint(&self) -> String {
        format!("shard:{}", self.content_sha256.to_ascii_lowercase())
    }

    pub fn synthetic(id: u32, split: HoldoutSplit) -> Self {
        let mut h = Sha256::new();
        h.update(b"proof-synthetic-shard-v1");
        h.update(split.as_str().as_bytes());
        h.update(id.to_le_bytes());
        Self {
            id,
            split,
            dataset_id: "synthetic-dev".into(),
            content_sha256: hex::encode(h.finalize()),
            token_count: if split == HoldoutSplit::Longctx {
                LONGCTX_MIN_TOKENS + id % 1_024
            } else {
                2_048
            },
        }
    }
}

/// Why a holdout set was refused.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum HoldoutError {
    /// Nothing to score.
    #[error("holdout set is empty")]
    Empty,
    /// Two records claim the same id.
    #[error("duplicate record id {0}")]
    DuplicateId(u32),
    /// Two records name the same shard bytes.
    #[error("duplicate shard content hash on record {0}")]
    DuplicateContent(u32),
    /// A content hash is not 64 hex chars.
    #[error("record {0} has a malformed content_sha256")]
    MalformedHash(u32),
    /// A record has no corpus fingerprint.
    #[error("record {0} has an empty dataset_id")]
    EmptyDataset(u32),
    /// A record has no tokens, so it cannot produce an NLL.
    #[error("record {0} has token_count 0")]
    EmptyShard(u32),
    /// A `longctx` record is not actually long.
    #[error("record {id} is longctx with {tokens} tokens (need {LONGCTX_MIN_TOKENS}..={LONGCTX_MAX_TOKENS})")]
    NotLongContext {
        /// Record id.
        id: u32,
        /// Declared token count.
        tokens: u32,
    },
    /// The canary stratum leaked into the paid set.
    #[error("record {0} is canary_offpath; the canary is off-score and never in the holdout")]
    CanaryInHoldout(u32),
    /// Record count disagrees with the topic document.
    #[error("holdout count mismatch (expected {expected}, got {got})")]
    SizeMismatch {
        /// Size the topic declared.
        expected: usize,
        /// Size the operator supplied.
        got: usize,
    },
    /// A stratum is over- or under-filled.
    #[error("split {split} has {got} records, expected {expected}")]
    Unstratified {
        /// Stratum name.
        split: &'static str,
        /// Observed count.
        got: usize,
        /// Required count.
        expected: usize,
    },
    /// The supplied set does not match the committed digest.
    #[error("holdout commitment mismatch (expected {expected}, got {got})")]
    CommitmentMismatch {
        /// Digest the topic document committed to.
        expected: String,
        /// Digest of what the operator supplied.
        got: String,
    },
}

fn field(h: &mut Sha256, value: &str) {
    let body = value.as_bytes();
    h.update(u64::try_from(body.len()).unwrap_or(u64::MAX).to_le_bytes());
    h.update(body);
}

/// Commitment over a holdout set.
///
/// Domain-separated, id-sorted, length-prefixed, and covering every field the
/// harness reads — stratum and token count included, so a "verified" holdout
/// cannot be re-labelled to move records between strata after the fact.
pub fn holdout_commitment(records: &[HoldoutRecord]) -> String {
    let mut sorted: Vec<&HoldoutRecord> = records.iter().collect();
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
        h.update(r.token_count.to_le_bytes());
        field(&mut h, r.split.as_str());
        field(&mut h, &r.dataset_id);
        field(&mut h, &r.content_sha256.to_ascii_lowercase());
    }
    hex::encode(h.finalize())
}

fn validate(records: &[HoldoutRecord]) -> Result<(), HoldoutError> {
    if records.is_empty() {
        return Err(HoldoutError::Empty);
    }
    let mut ids = BTreeSet::new();
    let mut hashes = BTreeSet::new();
    for r in records {
        if !r.split.is_scored() {
            return Err(HoldoutError::CanaryInHoldout(r.id));
        }
        if r.dataset_id.trim().is_empty() {
            return Err(HoldoutError::EmptyDataset(r.id));
        }
        if !is_hex64(&r.content_sha256) {
            return Err(HoldoutError::MalformedHash(r.id));
        }
        if r.token_count == 0 {
            return Err(HoldoutError::EmptyShard(r.id));
        }
        if r.split == HoldoutSplit::Longctx
            && (r.token_count < LONGCTX_MIN_TOKENS || r.token_count > LONGCTX_MAX_TOKENS)
        {
            return Err(HoldoutError::NotLongContext {
                id: r.id,
                tokens: r.token_count,
            });
        }
        if !ids.insert(r.id) {
            return Err(HoldoutError::DuplicateId(r.id));
        }
        if !hashes.insert(r.content_sha256.to_ascii_lowercase()) {
            return Err(HoldoutError::DuplicateContent(r.id));
        }
    }
    Ok(())
}

fn check_stratification(
    records: &[HoldoutRecord],
    expected_size: usize,
) -> Result<(), HoldoutError> {
    let per_split = expected_size / HoldoutSplit::SCORED.len();
    let mut counts: BTreeMap<HoldoutSplit, usize> = BTreeMap::new();
    for r in records {
        *counts.entry(r.split).or_default() += 1;
    }
    for split in HoldoutSplit::SCORED {
        let got = counts.get(&split).copied().unwrap_or_default();
        if got != per_split {
            return Err(HoldoutError::Unstratified {
                split: split.as_str(),
                got,
                expected: per_split,
            });
        }
    }
    Ok(())
}

/// Verify an operator-supplied holdout set against a topic's commitment.
///
/// # Errors
///
/// Structural problems, size disagreement, an unbalanced stratum, or a
/// commitment mismatch. Every one is fail-closed: nothing is loaded, so the
/// topic cannot score.
pub fn verify_holdout(
    records: &[HoldoutRecord],
    expected_commitment: &str,
    expected_size: usize,
) -> Result<(), HoldoutError> {
    validate(records)?;
    if records.len() != expected_size {
        return Err(HoldoutError::SizeMismatch {
            expected: expected_size,
            got: records.len(),
        });
    }
    check_stratification(records, expected_size)?;
    let got = holdout_commitment(records);
    if !got.eq_ignore_ascii_case(expected_commitment.trim()) {
        return Err(HoldoutError::CommitmentMismatch {
            expected: expected_commitment.trim().to_ascii_lowercase(),
            got,
        });
    }
    Ok(())
}

/// Holdout fingerprints that leaked into a submission's declared training set.
///
/// Matching is on shard content hashes and corpus ids, because those are the
/// two things a miner can honestly declare and the two things that make a
/// holdout NLL meaningless if they overlap.
pub fn contamination(
    declared_content_hashes: &BTreeSet<String>,
    declared_dataset_ids: &BTreeSet<String>,
    holdout: &[HoldoutRecord],
) -> Vec<String> {
    let mut hits = Vec::new();
    for r in holdout {
        let hash = r.content_sha256.to_ascii_lowercase();
        if declared_content_hashes
            .iter()
            .any(|d| d.trim().eq_ignore_ascii_case(&hash))
        {
            hits.push(r.fingerprint());
        }
        if declared_dataset_ids
            .iter()
            .any(|d| d.trim().eq_ignore_ascii_case(r.dataset_id.trim()))
        {
            hits.push(format!("dataset:{}", r.dataset_id.trim()));
        }
    }
    hits.sort();
    hits.dedup();
    hits
}

/// A stratified synthetic holdout of `per_split * 5` records (CI / local only).
pub fn synthetic_holdout(per_split: usize, first_id: u32) -> Vec<HoldoutRecord> {
    let mut out = Vec::new();
    let mut id = first_id;
    for split in HoldoutSplit::SCORED {
        for _ in 0..per_split {
            out.push(HoldoutRecord::synthetic(id, split));
            id = id.saturating_add(1);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn holdout() -> Vec<HoldoutRecord> {
        synthetic_holdout(STRATUM_SIZE, 1_000)
    }

    #[test]
    fn commitment_is_order_independent_and_covers_every_field() {
        let base = holdout_commitment(&holdout());
        let mut rev = holdout();
        rev.reverse();
        assert_eq!(base, holdout_commitment(&rev));
        assert_eq!(base.len(), 64);

        for mutate in 0..4 {
            let mut edited = holdout();
            match mutate {
                0 => edited[0].split = HoldoutSplit::CodeOod,
                1 => edited[0].dataset_id = "other".into(),
                2 => edited[0].content_sha256 = "ab".repeat(32),
                _ => edited[0].token_count += 1,
            }
            assert_ne!(base, holdout_commitment(&edited), "case {mutate}");
        }
    }

    #[test]
    fn verify_accepts_the_committed_stratified_set() {
        let recs = holdout();
        assert_eq!(recs.len(), HOLDOUT_SIZE);
        let c = holdout_commitment(&recs);
        verify_holdout(&recs, &c, HOLDOUT_SIZE).expect("ok");
    }

    /// The per-split gate is only real if the strata are balanced, so an
    /// unbalanced set never loads.
    #[test]
    fn an_unbalanced_holdout_is_refused() {
        let mut recs = holdout();
        recs[0].split = HoldoutSplit::CodeOod;
        let c = holdout_commitment(&recs);
        assert!(matches!(
            verify_holdout(&recs, &c, HOLDOUT_SIZE),
            Err(HoldoutError::Unstratified { .. })
        ));
    }

    /// The canary can zero a run but is never in the paid set, so it must not
    /// be loadable as a holdout record at all.
    #[test]
    fn a_canary_record_cannot_enter_the_holdout() {
        let mut recs = holdout();
        recs[3].split = HoldoutSplit::CanaryOffpath;
        let c = holdout_commitment(&recs);
        assert!(matches!(
            verify_holdout(&recs, &c, HOLDOUT_SIZE),
            Err(HoldoutError::CanaryInHoldout(_))
        ));
    }

    #[test]
    fn a_longctx_record_that_is_not_long_is_refused() {
        let mut recs = holdout();
        let idx = recs
            .iter()
            .position(|r| r.split == HoldoutSplit::Longctx)
            .expect("longctx stratum");
        recs[idx].token_count = 512;
        let c = holdout_commitment(&recs);
        assert!(matches!(
            verify_holdout(&recs, &c, HOLDOUT_SIZE),
            Err(HoldoutError::NotLongContext { .. })
        ));
    }

    #[test]
    fn verify_rejects_edits_size_drift_and_duplicates() {
        let recs = holdout();
        let c = holdout_commitment(&recs);

        let mut edited = recs.clone();
        edited[7].dataset_id = "leaked".into();
        assert!(matches!(
            verify_holdout(&edited, &c, HOLDOUT_SIZE),
            Err(HoldoutError::CommitmentMismatch { .. })
        ));

        assert!(matches!(
            verify_holdout(&recs, &c, HOLDOUT_SIZE + 1),
            Err(HoldoutError::SizeMismatch { .. })
        ));

        let mut dupe = recs.clone();
        dupe[1].content_sha256 = dupe[0].content_sha256.clone();
        assert!(matches!(
            verify_holdout(&dupe, &holdout_commitment(&dupe), HOLDOUT_SIZE),
            Err(HoldoutError::DuplicateContent(_))
        ));

        let mut malformed = recs;
        malformed[2].content_sha256 = "nope".into();
        assert!(matches!(
            verify_holdout(&malformed, &c, HOLDOUT_SIZE),
            Err(HoldoutError::MalformedHash(_))
        ));
    }

    #[test]
    fn contamination_detects_shard_and_corpus_overlap() {
        let recs = holdout();
        let none = BTreeSet::new();
        let shard: BTreeSet<String> = [recs[0].content_sha256.to_ascii_uppercase()]
            .into_iter()
            .collect();
        assert!(contamination(&shard, &none, &recs)
            .iter()
            .any(|h| h == &recs[0].fingerprint()));

        let corpus: BTreeSet<String> = ["synthetic-dev".into()].into_iter().collect();
        assert!(contamination(&none, &corpus, &recs)
            .iter()
            .any(|h| h.starts_with("dataset:")));

        assert!(contamination(&none, &none, &recs).is_empty());
    }
}
