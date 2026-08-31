//! Frozen Relearn holdout records and the pin commitment.
//!
//! Git carries only `holdout_commitment` + `holdout_size`. The records live in
//! an operator file (`RELEARN_HOLDOUT_FILE`) and are checked at boot. A
//! mismatch is fail-closed: submissions answer 503 rather than scoring a
//! reconstructable seed or falling back to the public split.
//!
//! The previous seed (`sha256(HOLDOUT_DOMAIN ‖ epoch ‖ digest)`) was
//! regenerable from git. That path is gone.

#![allow(clippy::cast_precision_loss)]

use std::collections::{BTreeSet, HashSet};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::HOLDOUT_DOMAIN;

/// Minimum holdout items. Matches the paired test's evidence floor.
pub const MIN_HOLDOUT_ITEMS: usize = 100;

/// Largest Jaccard similarity on prompt 3-grams allowed between public and holdout.
pub const NGRAM_JACCARD_MAX: f64 = 0.80;

/// Task family on a holdout (or public) item.
///
/// Vision families exist because the live base is a native VLM. A text-only
/// record leaves `image_hash` empty and does not take the pixel-shuffle gate.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HoldoutTask {
    /// Language-only item (no image).
    #[default]
    Text,
    /// Free-form captioning.
    Captioning,
    /// Visual question answering.
    Vqa,
    /// Reading text rendered inside the image.
    Ocr,
    /// Spatial relations between objects.
    Spatial,
}

impl HoldoutTask {
    /// Families that carry an image and must take the pixel-shuffle control.
    pub const VISION: [Self; 4] = [Self::Captioning, Self::Vqa, Self::Ocr, Self::Spatial];

    /// Wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Captioning => "captioning",
            Self::Vqa => "vqa",
            Self::Ocr => "ocr",
            Self::Spatial => "spatial",
        }
    }

    /// Whether this family is scored with an image.
    #[must_use]
    pub const fn is_vision(self) -> bool {
        !matches!(self, Self::Text)
    }
}

/// One frozen eval item. Images stay out of git; only the hash is committed
/// into the operator file (and then into the pin commitment).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HoldoutItem {
    /// Stable item id. Never published for the holdout split.
    pub id: u32,
    /// Prompt / question text.
    pub prompt: String,
    /// Source dataset id (fingerprint only).
    #[serde(default)]
    pub dataset_id: String,
    /// Task family.
    #[serde(default)]
    pub task: HoldoutTask,
    /// SHA-256 hex of the image bytes. Empty for text-only items.
    #[serde(default)]
    pub image_hash: String,
}

impl HoldoutItem {
    /// Canonical fingerprint used by the contamination gate.
    #[must_use]
    pub fn fingerprint(&self) -> String {
        format!("id:{}", self.id)
    }

    /// Image fingerprint, when present.
    #[must_use]
    pub fn image_fingerprint(&self) -> Option<String> {
        let h = self.image_hash.trim().to_ascii_lowercase();
        if h.len() == 64 && h.chars().all(|c| c.is_ascii_hexdigit()) {
            Some(format!("image:{h}"))
        } else {
            None
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
    #[error("duplicate holdout id {0}")]
    DuplicateId(u32),
    /// A record has no prompt body.
    #[error("holdout id {0} has empty prompt")]
    EmptyPrompt(u32),
    /// A vision item is missing a real image hash.
    #[error("holdout id {0} is a vision task without an image hash")]
    MissingImageHash(u32),
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
    #[error("holdout id {0} is also in the public split")]
    OverlapsPublic(u32),
    /// Public and holdout share a near-duplicate prompt or image.
    #[error("holdout near-duplicate of public item ({0})")]
    NearDuplicate(String),
}

/// Commitment over a holdout set.
///
/// Domain-separated, id-sorted, and length-prefixed so neither reordering nor
/// splicing two prompt bodies together can collide.
#[must_use]
pub fn holdout_commitment(records: &[HoldoutItem]) -> String {
    let mut sorted: Vec<&HoldoutItem> = records.iter().collect();
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
        h.update(r.task.as_str().as_bytes());
        h.update([0xff]);
        for field in [
            r.prompt.as_str(),
            r.dataset_id.as_str(),
            r.image_hash.trim(),
        ] {
            h.update(u64::try_from(field.len()).unwrap_or(u64::MAX).to_le_bytes());
            h.update(field.as_bytes());
        }
    }
    hex::encode(h.finalize())
}

fn validate_records(records: &[HoldoutItem]) -> Result<(), HoldoutError> {
    if records.is_empty() {
        return Err(HoldoutError::Empty);
    }
    let mut seen = BTreeSet::new();
    for r in records {
        if r.prompt.trim().is_empty() {
            return Err(HoldoutError::EmptyPrompt(r.id));
        }
        if r.task.is_vision() && r.image_fingerprint().is_none() {
            return Err(HoldoutError::MissingImageHash(r.id));
        }
        if !seen.insert(r.id) {
            return Err(HoldoutError::DuplicateId(r.id));
        }
    }
    Ok(())
}

/// Word 3-grams of a prompt, lowercased.
fn trigrams(text: &str) -> HashSet<String> {
    let words: Vec<String> = text
        .split_whitespace()
        .map(|w| {
            w.chars()
                .filter(|c| c.is_alphanumeric())
                .collect::<String>()
                .to_ascii_lowercase()
        })
        .filter(|w| !w.is_empty())
        .collect();
    if words.len() < 3 {
        return HashSet::new();
    }
    words
        .windows(3)
        .map(|w| format!("{} {} {}", w[0], w[1], w[2]))
        .collect()
}

fn jaccard(a: &HashSet<String>, b: &HashSet<String>) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let inter = a.intersection(b).count() as f64;
    let union = a.union(b).count() as f64;
    if union <= 0.0 {
        0.0
    } else {
        inter / union
    }
}

/// Public↔holdout near-duplicates (id, image-hash, dataset+id, prompt n-gram).
#[must_use]
pub fn near_duplicates(public: &[HoldoutItem], holdout: &[HoldoutItem]) -> Vec<String> {
    let mut hits = Vec::new();
    let published_ids: BTreeSet<u32> = public.iter().map(|p| p.id).collect();
    let published_images: BTreeSet<String> = public
        .iter()
        .filter_map(HoldoutItem::image_fingerprint)
        .collect();
    let dataset_keys: BTreeSet<(String, u32)> = public
        .iter()
        .filter(|p| !p.dataset_id.trim().is_empty())
        .map(|p| (p.dataset_id.trim().to_owned(), p.id))
        .collect();
    let prompt_grams: Vec<(u32, HashSet<String>)> = public
        .iter()
        .map(|p| (p.id, trigrams(&p.prompt)))
        .filter(|(_, g)| g.len() >= 4)
        .collect();

    for h in holdout {
        if published_ids.contains(&h.id) {
            hits.push(format!("id:{}", h.id));
        }
        if let Some(img) = h.image_fingerprint() {
            if published_images.contains(&img) {
                hits.push(img);
            }
        }
        let ds = h.dataset_id.trim();
        if !ds.is_empty() && dataset_keys.contains(&(ds.to_owned(), h.id)) {
            hits.push(format!("dataset:{ds}#{}", h.id));
        }
        let hg = trigrams(&h.prompt);
        if hg.len() >= 4 {
            for (pid, pg) in &prompt_grams {
                if jaccard(&hg, pg) > NGRAM_JACCARD_MAX {
                    hits.push(format!("ngram:{}~{pid}", h.id));
                }
            }
        }
    }
    hits.sort();
    hits.dedup();
    hits
}

/// Verify an operator-supplied holdout against the committed digest.
///
/// # Errors
///
/// Structural problems, size disagreement, public-split overlap, near-duplicates,
/// or a commitment mismatch. Every one is fail-closed.
pub fn verify_holdout_items(
    records: &[HoldoutItem],
    public: &[HoldoutItem],
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
    if let Some(hit) = near_duplicates(public, records).into_iter().next() {
        return Err(HoldoutError::NearDuplicate(hit));
    }
    let got = holdout_commitment(records);
    if !got.eq_ignore_ascii_case(expected_commitment.trim()) {
        return Err(HoldoutError::CommitmentMismatch {
            expected: expected_commitment.trim().to_ascii_lowercase(),
            got,
        });
    }
    Ok(())
}

/// Holdout fingerprints that leaked into a submission's training metadata.
#[must_use]
pub fn contamination(
    train_ids: &BTreeSet<u32>,
    train_image_hashes: &BTreeSet<String>,
    train_dataset_ids: &BTreeSet<String>,
    holdout: &[HoldoutItem],
) -> Vec<String> {
    let mut hits = Vec::new();
    for item in holdout {
        if train_ids.contains(&item.id) {
            hits.push(item.fingerprint());
        }
        if let Some(img) = item.image_fingerprint() {
            let raw = img.trim_start_matches("image:");
            if train_image_hashes
                .iter()
                .any(|t| t.trim().eq_ignore_ascii_case(raw))
            {
                hits.push(img);
            }
        }
        let ds = item.dataset_id.trim();
        if !ds.is_empty()
            && train_dataset_ids
                .iter()
                .any(|t| t.trim().eq_ignore_ascii_case(ds))
            && train_ids.contains(&item.id)
        {
            hits.push(format!("dataset:{ds}#{}", item.id));
        }
    }
    hits.sort();
    hits.dedup();
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: u32, prompt: &str) -> HoldoutItem {
        HoldoutItem {
            id,
            prompt: prompt.into(),
            dataset_id: "dev".into(),
            task: HoldoutTask::Text,
            image_hash: String::new(),
        }
    }

    fn vision(id: u32, prompt: &str, hash: &str) -> HoldoutItem {
        HoldoutItem {
            id,
            prompt: prompt.into(),
            dataset_id: "dev-vis".into(),
            task: HoldoutTask::Captioning,
            image_hash: hash.into(),
        }
    }

    fn holdout() -> Vec<HoldoutItem> {
        vec![
            item(900, "a red cube sits on a wooden table near a lamp"),
            item(901, "two cats sleep on a sunlit windowsill together"),
        ]
    }

    #[test]
    fn commitment_is_order_independent_and_length_prefixed() {
        let a = holdout_commitment(&holdout());
        let mut rev = holdout();
        rev.reverse();
        assert_eq!(a, holdout_commitment(&rev));
        assert_eq!(a.len(), 64);
        let spliced_ab = holdout_commitment(&[item(1, "ab"), item(2, "c")]);
        let spliced_bc = holdout_commitment(&[item(1, "a"), item(2, "bc")]);
        assert_ne!(spliced_ab, spliced_bc);
    }

    #[test]
    fn commitment_changes_with_body() {
        let a = holdout_commitment(&holdout());
        let mut edited = holdout();
        edited[0].prompt.push('.');
        assert_ne!(a, holdout_commitment(&edited));
    }

    #[test]
    fn verify_accepts_the_committed_set() {
        let recs = holdout();
        let c = holdout_commitment(&recs);
        verify_holdout_items(&recs, &[], &[1, 2, 3], &c, 2).expect("ok");
    }

    #[test]
    fn verify_rejects_public_id_and_near_dupe() {
        let recs = holdout();
        let c = holdout_commitment(&recs);
        assert!(matches!(
            verify_holdout_items(&recs, &[], &[901], &c, 2),
            Err(HoldoutError::OverlapsPublic(901))
        ));
        let public = vec![item(12, "a red cube sits on a wooden table near a lamp")];
        assert!(matches!(
            verify_holdout_items(&recs, &public, &[], &c, 2),
            Err(HoldoutError::NearDuplicate(_))
        ));
    }

    #[test]
    fn verify_rejects_edited_set_and_size_drift() {
        let recs = holdout();
        let c = holdout_commitment(&recs);
        let mut edited = recs.clone();
        edited[1].prompt = "three dogs".into();
        assert!(matches!(
            verify_holdout_items(&edited, &[], &[], &c, 2),
            Err(HoldoutError::CommitmentMismatch { .. })
        ));
        assert!(matches!(
            verify_holdout_items(&recs, &[], &[], &c, 3),
            Err(HoldoutError::SizeMismatch { .. })
        ));
    }

    #[test]
    fn vision_item_requires_an_image_hash() {
        let bad = vec![vision(5, "caption this", "")];
        assert!(matches!(
            verify_holdout_items(&bad, &[], &[], "00", 1),
            Err(HoldoutError::MissingImageHash(5))
        ));
    }

    #[test]
    fn contamination_detects_id_and_image_overlap() {
        let img = "ab".repeat(32);
        let hold = vec![vision(900, "a scene with a red cube on a table", &img)];
        let ids: BTreeSet<u32> = [900].into_iter().collect();
        let hashes: BTreeSet<String> = [img.clone()].into_iter().collect();
        let no_ids = BTreeSet::new();
        let no_str = BTreeSet::new();
        assert!(contamination(&ids, &no_str, &no_str, &hold)
            .iter()
            .any(|h| h == "id:900"));
        assert!(contamination(&no_ids, &hashes, &no_str, &hold)
            .iter()
            .any(|h| h.starts_with("image:")));
        assert!(contamination(&no_ids, &no_str, &no_str, &hold).is_empty());
    }

    #[test]
    fn image_hash_overlap_is_a_near_duplicate() {
        let img = "cd".repeat(32);
        let public = vec![vision(1, "public caption of a busy street market", &img)];
        let hold = vec![vision(
            900,
            "holdout caption of a quiet harbour at dusk",
            &img,
        )];
        let hits = near_duplicates(&public, &hold);
        assert!(hits.iter().any(|h| h.starts_with("image:")), "{hits:?}");
    }
}
