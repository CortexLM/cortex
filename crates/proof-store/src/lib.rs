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

pub mod durable;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use proof_score::{MinerTopicRun, ProofVerdict, SealedBaseline};
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
    /// Deprecated HF architecture id. Ignored; not an architecture lock.
    #[serde(default)]
    pub architecture: String,
    /// RLM judge offer id that scored this run (host stamp, not a miner bind).
    #[serde(default)]
    pub inference_offer_id: String,
    /// Judge `config_commitment` stamped from the host offer.
    #[serde(default)]
    pub config_commitment: String,
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
    /// Durable journal unavailable; the caller must not acknowledge the write.
    #[error("durable store backend unavailable")]
    Backend,
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
    // Serializes volatile compound mutations; Postgres serializes durable writes.
    writes: Arc<tokio::sync::Mutex<()>>,
    /// Write-through journal. `None` keeps the historical volatile behaviour
    /// for tests; a configured service sets it so scored runs survive restart.
    journal: Option<durable::DurableJournal>,
}

#[derive(Default)]
struct Inner {
    next: u64,
    submissions: BTreeMap<String, Submission>,
    topics: BTreeMap<String, TopicDocument>,
    holdouts: BTreeMap<String, Vec<HoldoutRecord>>,
    baselines: BTreeMap<String, SealedBaseline>,
    scores: BTreeMap<String, BTreeMap<String, MinerTopicRun>>,
}

impl MemoryStore {
    /// Empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Attach a durable journal and reload everything it retained, so a
    /// restart resumes with the submissions and scores it had already accepted.
    ///
    /// # Errors
    /// Journal unavailable or a retained row no longer decodes.
    pub async fn with_journal(
        mut self,
        journal: durable::DurableJournal,
    ) -> Result<Self, StoreError> {
        // Probe availability; durable reads never use the process cache.
        journal.snapshot().await?;
        self.journal = Some(journal);
        Ok(self)
    }

    /// Independent, read-consistent payout snapshot. Refresh once per emission.
    pub async fn snapshot_durable(&self) -> Result<Self, StoreError> {
        let Some(journal) = &self.journal else {
            return Ok(self.clone());
        };
        let (submissions, runs) = journal.snapshot().await?;
        let g = self.lock()?;
        let mut inner = Inner {
            topics: g.topics.clone(),
            holdouts: g.holdouts.clone(),
            baselines: g.baselines.clone(),
            ..Inner::default()
        };
        for row in submissions {
            inner.submissions.insert(row.id.clone(), row);
        }
        for (hotkey, topic, run) in runs {
            inner.scores.entry(hotkey).or_default().insert(topic, run);
        }
        Ok(Self {
            inner: Arc::new(Mutex::new(inner)),
            ..Self::default()
        })
    }

    fn volatile_only(&self) -> Result<(), StoreError> {
        if self.journal.is_some() {
            return Err(StoreError::Illegal(
                "durable store requires async API or fresh snapshot".into(),
            ));
        }
        Ok(())
    }

    /// Current committed submission, including commits from other instances.
    pub async fn get_durable(&self, id: &str) -> Result<Submission, StoreError> {
        self.snapshot_durable().await?.get(id)
    }

    /// Current committed submissions, newest first.
    pub async fn list_durable(&self) -> Result<Vec<Submission>, StoreError> {
        self.snapshot_durable().await?.list()
    }

    /// Rows the journal must persist before the caller acknowledges a write.
    #[must_use]
    pub fn journal(&self) -> Option<&durable::DurableJournal> {
        self.journal.as_ref()
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
        self.volatile_only()?;
        let mut g = self.lock()?;
        Self::allocate(&mut g, &mut row)?;
        g.submissions.insert(row.id.clone(), row.clone());
        Ok(row)
    }

    fn allocate(g: &mut Inner, row: &mut Submission) -> Result<(), StoreError> {
        if row.id.is_empty() {
            loop {
                let n = g.next;
                g.next = n
                    .checked_add(1)
                    .ok_or_else(|| StoreError::Illegal("submission ids exhausted".into()))?;
                row.id = format!("pf_{n:016x}");
                if !g.submissions.contains_key(&row.id) {
                    break;
                }
            }
        } else if g.submissions.contains_key(&row.id) {
            return Err(StoreError::Illegal("duplicate submission id".into()));
        }
        Ok(())
    }

    /// Legacy unscored insert, refused with a journal; use `finish_durable`.
    ///
    /// # Errors
    /// Lock poisoned or the journal rejected the write.
    pub async fn insert_durable(&self, row: Submission) -> Result<Submission, StoreError> {
        let _write = self.writes.lock().await;
        self.volatile_only()?;
        self.insert(row)
    }

    /// Commit a final submission and its payout run together.
    pub async fn finish_durable(
        &self,
        row: Submission,
        run: MinerTopicRun,
    ) -> Result<Submission, StoreError> {
        self.commit_final(row, run, false).await
    }

    /// Update terminal fields without changing the original frozen identity.
    pub async fn update_durable(
        &self,
        row: Submission,
        run: MinerTopicRun,
    ) -> Result<Submission, StoreError> {
        self.commit_final(row, run, true).await
    }

    async fn commit_final(
        &self,
        row: Submission,
        run: MinerTopicRun,
        update: bool,
    ) -> Result<Submission, StoreError> {
        if let Some(journal) = &self.journal {
            return journal.commit(row, &run, update).await;
        }
        let _write = self.writes.lock().await;
        if run.primary.is_some_and(|v| !v.is_finite())
            || run.artifact_digest != row.artifact_digest
            || row.verdict.as_ref().is_some_and(|v| v.pass != run.pass)
        {
            return Err(StoreError::Illegal("invalid topic run".into()));
        }
        if update {
            let old = self.get(&row.id)?;
            let identity = |value: &Submission| -> Result<serde_json::Value, StoreError> {
                let mut value = serde_json::to_value(value).map_err(|_| StoreError::Backend)?;
                if let Some(object) = value.as_object_mut() {
                    for key in ["state", "receipt_json", "verdict", "detail"] {
                        object.remove(key);
                    }
                }
                Ok(value)
            };
            if identity(&old)? != identity(&row)? {
                return Err(StoreError::Illegal("immutable identity mismatch".into()));
            }
        }
        let mut g = self.lock()?;
        let mut row = row;
        if !update {
            Self::allocate(&mut g, &mut row)?;
        }
        g.scores
            .entry(row.miner_hotkey.clone())
            .or_default()
            .insert(row.topic_id.clone(), run);
        g.submissions.insert(row.id.clone(), row.clone());
        Ok(row)
    }

    /// Standalone score writes are forbidden with a journal: use finish/update.
    pub async fn record_topic_run_durable(
        &self,
        hotkey: &str,
        topic_id: &str,
        run: MinerTopicRun,
    ) -> Result<(), StoreError> {
        let _write = self.writes.lock().await;
        self.record_topic_run(hotkey, topic_id, run)
    }

    /// Fetch one row.
    pub fn get(&self, id: &str) -> Result<Submission, StoreError> {
        self.volatile_only()?;
        self.lock()?
            .submissions
            .get(id)
            .cloned()
            .ok_or_else(|| StoreError::NotFound(id.to_owned()))
    }

    /// List newest-first.
    pub fn list(&self) -> Result<Vec<Submission>, StoreError> {
        self.volatile_only()?;
        let g = self.lock()?;
        let mut rows: Vec<_> = g.submissions.values().cloned().collect();
        rows.sort_by(|a, b| b.id.cmp(&a.id));
        Ok(rows)
    }

    /// Persist one topic attempt for a hotkey so emission can run WTA / discovery.
    pub fn record_topic_score(
        &self,
        hotkey: &str,
        topic_id: &str,
        lattice: u64,
    ) -> Result<(), StoreError> {
        self.record_topic_run(
            hotkey,
            topic_id,
            MinerTopicRun {
                pass: lattice > 0,
                primary: None,
                artifact_digest: String::new(),
                near_duplicate: false,
            },
        )
    }

    /// Persist the full attempt (primary + artifact) used by payout.
    pub fn record_topic_run(
        &self,
        hotkey: &str,
        topic_id: &str,
        run: MinerTopicRun,
    ) -> Result<(), StoreError> {
        self.volatile_only()?;
        if run.primary.is_some_and(|v| !v.is_finite()) {
            return Err(StoreError::Illegal("non-finite primary".into()));
        }
        self.lock()?
            .scores
            .entry(hotkey.to_owned())
            .or_default()
            .insert(topic_id.to_owned(), run);
        Ok(())
    }

    /// Per-topic lattices for one miner (binary SCORE_MAX/0 from `pass`).
    pub fn miner_scores(&self, hotkey: &str) -> Result<BTreeMap<String, u64>, StoreError> {
        self.volatile_only()?;
        Ok(self
            .lock()?
            .scores
            .get(hotkey)
            .map(|m| {
                m.iter()
                    .map(|(k, r)| (k.clone(), if r.pass { proof_task::SCORE_MAX } else { 0 }))
                    .collect()
            })
            .unwrap_or_default())
    }

    /// Per-topic attempts for one miner.
    pub fn miner_runs(&self, hotkey: &str) -> Result<BTreeMap<String, MinerTopicRun>, StoreError> {
        self.volatile_only()?;
        Ok(self.lock()?.scores.get(hotkey).cloned().unwrap_or_default())
    }

    /// Every hotkey that has any recorded topic score.
    pub fn scored_hotkeys(&self) -> Result<BTreeSet<String>, StoreError> {
        self.volatile_only()?;
        Ok(self.lock()?.scores.keys().cloned().collect())
    }

    /// Best accepted champion primary on a topic, if the operator crowned one.
    pub fn champion_primary(&self, topic: &TopicDocument) -> Result<Option<f64>, StoreError> {
        self.volatile_only()?;
        let g = self.lock()?;
        let mut found = None;
        for row in g.submissions.values() {
            if row.topic_id != topic.id || row.state != SubmissionState::Champion {
                continue;
            }
            let Some(v) = row.verdict.as_ref() else {
                continue;
            };
            if v.pass {
                found = proof_score::primary_from_harness(topic, &v.harness);
            }
        }
        Ok(found)
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
