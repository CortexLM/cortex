//! In-memory Proof store: signed topics, sealed holdouts, baselines, rows.
//!
//! Topics are the secret surface here, not a seed: once a miner knows which
//! holdout shards a topic scores, the commitment stops measuring whether the
//! recipe generalises. So each topic's records are loaded from an operator
//! file, verified against that topic's `holdout_commitment`, and are only
//! readable after a submission digest has been frozen. The public topic
//! document keeps the commitment and never the records.

#![forbid(unsafe_code)]
#![allow(
    clippy::missing_errors_doc,
    clippy::doc_markdown,
    clippy::must_use_candidate
)]

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use proof_score::{ProofVerdict, SealedBaseline};
use proof_task::{verify_holdout, HoldoutError, HoldoutRecord, TopicDocument, TopicError};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Submission lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubmissionState {
    /// Eval finished; waiting operator audit (informational).
    AwaitingAdmin,
    /// Rejected (gates / integrity).
    Rejected,
    /// Operator-promoted (optional; Proof pays on pass, not on a crown).
    Champion,
}

/// Training metadata the contamination gate reads.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ArtifactManifest {
    /// Shard content hashes the miner declared training on.
    pub train_content_hashes: Vec<String>,
    /// Corpus ids the miner declared.
    pub train_dataset_ids: Vec<String>,
}

impl ArtifactManifest {
    /// Whether anything was declared for the gate to check.
    #[must_use]
    pub fn is_declared(&self) -> bool {
        self.train_content_hashes
            .iter()
            .any(|s| !s.trim().is_empty())
            || self.train_dataset_ids.iter().any(|s| !s.trim().is_empty())
    }
}

/// One miner submission against one topic.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Submission {
    /// Stable id (`pf_` + 16 hex).
    pub id: String,
    /// Topic this run is about.
    pub topic_id: String,
    /// 64-hex miner hotkey.
    pub miner_hotkey: String,
    /// SHA-256 hex of the miner artifact.
    pub artifact_digest: String,
    /// Optional locator (git url, object URL).
    pub artifact_uri: Option<String>,
    /// Human claim string.
    pub claim: String,
    /// Declared FLOP budget (must be ≤ topic).
    pub declared_flops: u64,
    /// Architecture id; must equal the topic/pin proxy.
    pub architecture: String,
    /// Declared training fingerprints.
    #[serde(default)]
    pub manifest: ArtifactManifest,
    /// Digest freeze nonce (hex).
    pub nonce: String,
    /// `sha256(hotkey || 0xff || topic || 0xff || artifact || 0xff || nonce)`.
    pub submission_digest: String,
    /// Lifecycle.
    pub state: SubmissionState,
    /// Eval receipt JSON (if any).
    pub receipt_json: Option<String>,
    /// Judge verdict (if any).
    pub verdict: Option<ProofVerdict>,
    /// Reject / gate reason.
    pub detail: Option<String>,
}

/// Store errors.
#[derive(Debug, Error)]
pub enum StoreError {
    /// Lock poisoned.
    #[error("store lock poisoned")]
    Poison,
    /// Unknown submission.
    #[error("unknown submission {0}")]
    NotFound(String),
    /// Unknown / unpublished topic.
    #[error("unknown topic {0}")]
    UnknownTopic(String),
    /// Illegal state transition.
    #[error("illegal state {0}")]
    Illegal(String),
    /// Topic document failed verification.
    #[error("topic: {0}")]
    Topic(#[from] TopicError),
    /// Holdout file did not match the topic commitment.
    #[error("holdout: {0}")]
    Holdout(#[from] HoldoutError),
}

/// In-memory store (v0).
#[derive(Clone, Default)]
pub struct MemoryStore {
    inner: Arc<Mutex<Inner>>,
}

#[derive(Default)]
struct Inner {
    next: u64,
    submissions: BTreeMap<String, Submission>,
    topics: BTreeMap<String, TopicDocument>,
    holdouts: BTreeMap<String, Vec<HoldoutRecord>>,
    baselines: BTreeMap<String, SealedBaseline>,
    scores: BTreeMap<String, BTreeMap<String, u64>>,
}

impl MemoryStore {
    /// Empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Inner>, StoreError> {
        self.inner.lock().map_err(|_| StoreError::Poison)
    }

    /// Insert a topic that has already been schema- and signature-checked.
    pub fn put_topic(&self, doc: TopicDocument) -> Result<(), StoreError> {
        if doc.id.trim().is_empty() {
            return Err(StoreError::Illegal("empty topic id".into()));
        }
        self.lock()?.topics.insert(doc.id.clone(), doc);
        Ok(())
    }

    /// Fetch one topic document (public fields; no holdout records).
    pub fn topic(&self, id: &str) -> Result<TopicDocument, StoreError> {
        self.lock()?
            .topics
            .get(id)
            .cloned()
            .ok_or_else(|| StoreError::UnknownTopic(id.to_owned()))
    }

    /// Every topic currently in the store, newest-id last.
    pub fn topics(&self) -> Result<Vec<TopicDocument>, StoreError> {
        Ok(self.lock()?.topics.values().cloned().collect())
    }

    /// Topic ids that are `open` at `epoch`.
    pub fn open_ids(&self, epoch: u64) -> Result<Vec<String>, StoreError> {
        let g = self.lock()?;
        Ok(g.topics
            .values()
            .filter(|t| t.is_open_at(epoch))
            .map(|t| t.id.clone())
            .collect())
    }

    /// Load verified holdout records for one topic.
    pub fn load_holdout(
        &self,
        topic_id: &str,
        records: Vec<HoldoutRecord>,
    ) -> Result<(), StoreError> {
        let doc = self.topic(topic_id)?;
        verify_holdout(&records, &doc.holdout_commitment, doc.holdout_size)?;
        self.lock()?.holdouts.insert(topic_id.to_owned(), records);
        Ok(())
    }

    /// Whether this topic's holdout is loaded.
    pub fn holdout_loaded(&self, topic_id: &str) -> Result<bool, StoreError> {
        Ok(self.lock()?.holdouts.contains_key(topic_id))
    }

    /// Records, readable only after a submission digest is frozen.
    pub fn unseal_holdout(
        &self,
        topic_id: &str,
        frozen_digest: &str,
    ) -> Result<Vec<HoldoutRecord>, StoreError> {
        if frozen_digest.trim().is_empty() {
            return Err(StoreError::Illegal(
                "holdout stays sealed until the submission digest is frozen".into(),
            ));
        }
        self.lock()?.holdouts.get(topic_id).cloned().ok_or_else(|| {
            StoreError::Illegal(format!("no verified holdout loaded for {topic_id}"))
        })
    }

    /// Record the sealed baseline metric vector for one topic.
    pub fn set_baseline(&self, topic_id: &str, metrics: SealedBaseline) -> Result<(), StoreError> {
        let _ = self.topic(topic_id)?;
        self.lock()?.baselines.insert(topic_id.to_owned(), metrics);
        Ok(())
    }

    /// Sealed baseline, if recorded.
    pub fn baseline(&self, topic_id: &str) -> Result<Option<SealedBaseline>, StoreError> {
        Ok(self.lock()?.baselines.get(topic_id).cloned())
    }

    /// True when at least one open topic has holdout + sealed baseline.
    pub fn any_open_scorable(&self, epoch: u64) -> Result<bool, StoreError> {
        let g = self.lock()?;
        Ok(g.topics.values().any(|t| {
            t.is_open_at(epoch)
                && t.baseline.is_sealed()
                && g.holdouts.contains_key(&t.id)
                && g.baselines.contains_key(&t.id)
        }))
    }

    /// Insert a scored submission in its final state.
    pub fn insert(&self, mut row: Submission) -> Result<Submission, StoreError> {
        let mut g = self.lock()?;
        if row.id.is_empty() {
            let n = g.next;
            g.next = g.next.saturating_add(1);
            row.id = format!("pf_{n:016x}");
        }
        g.submissions.insert(row.id.clone(), row.clone());
        Ok(row)
    }

    /// Fetch one row.
    pub fn get(&self, id: &str) -> Result<Submission, StoreError> {
        self.lock()?
            .submissions
            .get(id)
            .cloned()
            .ok_or_else(|| StoreError::NotFound(id.to_owned()))
    }

    /// List newest-first.
    pub fn list(&self) -> Result<Vec<Submission>, StoreError> {
        let g = self.lock()?;
        let mut rows: Vec<_> = g.submissions.values().cloned().collect();
        rows.sort_by(|a, b| b.id.cmp(&a.id));
        Ok(rows)
    }

    /// Persist one topic lattice for a hotkey so emission can mean them.
    pub fn record_topic_score(
        &self,
        hotkey: &str,
        topic_id: &str,
        lattice: u64,
    ) -> Result<(), StoreError> {
        self.lock()?
            .scores
            .entry(hotkey.to_owned())
            .or_default()
            .insert(topic_id.to_owned(), lattice);
        Ok(())
    }

    /// Per-topic lattices for one miner.
    pub fn miner_scores(&self, hotkey: &str) -> Result<BTreeMap<String, u64>, StoreError> {
        Ok(self.lock()?.scores.get(hotkey).cloned().unwrap_or_default())
    }

    /// Every hotkey that has any recorded topic score.
    pub fn scored_hotkeys(&self) -> Result<BTreeSet<String>, StoreError> {
        Ok(self.lock()?.scores.keys().cloned().collect())
    }
}

/// SHA-256 hex of the frozen submission.
#[must_use]
pub fn freeze_submission_digest(
    hotkey: &str,
    topic_id: &str,
    artifact_digest: &str,
    nonce: &str,
) -> String {
    let mut h = Sha256::new();
    h.update(hotkey.as_bytes());
    h.update([0xff]);
    h.update(topic_id.as_bytes());
    h.update([0xff]);
    h.update(artifact_digest.as_bytes());
    h.update([0xff]);
    h.update(nonce.as_bytes());
    hex::encode(h.finalize())
}

#[cfg(test)]
mod tests {
    use proof_task::{
        default_adamw, holdout_commitment, synthetic_holdout, TopicDocument, TopicStatus,
        FLOPS_BUDGET_MAX, HOLDOUT_SIZE, STRATUM_SIZE,
    };

    use super::*;

    fn sealed() -> proof_task::Baseline {
        let mut b = default_adamw(FLOPS_BUDGET_MAX);
        b.script_sha256 = "11".repeat(32);
        b.metrics_commitment = "22".repeat(32);
        b
    }

    fn topic() -> TopicDocument {
        let recs = synthetic_holdout(STRATUM_SIZE, 1);
        TopicDocument {
            id: "dt-no-ib-v0".into(),
            statement: "no IB".into(),
            baseline: sealed(),
            holdout_commitment: holdout_commitment(&recs),
            holdout_size: HOLDOUT_SIZE,
            status: TopicStatus::Open,
            ..TopicDocument::default()
        }
    }

    #[test]
    fn holdout_loads_only_when_it_matches_the_topic_commitment() {
        let st = MemoryStore::new();
        let t = topic();
        st.put_topic(t.clone()).expect("topic");
        assert!(!st.holdout_loaded(&t.id).expect("q"));

        let mut tampered = synthetic_holdout(STRATUM_SIZE, 1);
        tampered[0].dataset_id = "leaked".into();
        assert!(st.load_holdout(&t.id, tampered).is_err());
        assert!(!st.holdout_loaded(&t.id).expect("q"));

        st.load_holdout(&t.id, synthetic_holdout(STRATUM_SIZE, 1))
            .expect("ok");
        assert!(st.holdout_loaded(&t.id).expect("q"));
    }

    #[test]
    fn unseal_requires_a_frozen_digest() {
        let st = MemoryStore::new();
        let t = topic();
        st.put_topic(t.clone()).expect("topic");
        st.load_holdout(&t.id, synthetic_holdout(STRATUM_SIZE, 1))
            .expect("load");
        assert!(st.unseal_holdout(&t.id, "").is_err());
        assert_eq!(
            st.unseal_holdout(&t.id, "deadbeef").expect("unseal").len(),
            HOLDOUT_SIZE
        );
    }

    #[test]
    fn digest_binds_the_topic() {
        let a = freeze_submission_digest("aa", "dt-no-ib-v0", "bb", "n1");
        assert_eq!(a, freeze_submission_digest("aa", "dt-no-ib-v0", "bb", "n1"));
        assert_ne!(a, freeze_submission_digest("aa", "other", "bb", "n1"));
        assert_eq!(a.len(), 64);
    }

    #[test]
    fn open_ids_respect_status() {
        let st = MemoryStore::new();
        let mut t = topic();
        t.status = TopicStatus::Draft;
        st.put_topic(t).expect("topic");
        assert!(st.open_ids(0).expect("ids").is_empty());
    }
}
