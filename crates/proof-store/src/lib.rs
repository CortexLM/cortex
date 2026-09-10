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

use proof_score::{MinerTopicRun, ProofVerdict, SealedBaseline};
use proof_task::{verify_holdout, HoldoutError, HoldoutRecord, TopicDocument, TopicError};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Submission lifecycle. Wire names are the snake_case of the variant
/// (`queued`, `awaiting_admin`, `rejected`, `champion`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubmissionState {
    /// Accepted and persisted on an open topic whose signed document defers
    /// scoring (`constraints.params.defer_scoring = "true"`). Nothing has
    /// run: no harvest rent, no topic VM, no judge call, no verdict, no
    /// topic mass. The only non-terminal state — the row moves on when the
    /// operator lifts the flag and the queue is drained (FIFO by id).
    Queued,
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// Live `EvalExecutorOffer` id the run was rented on (host stamp; empty
    /// on sim, which rents nothing).
    #[serde(default)]
    pub executor_offer_id: String,
    /// Commitment of the executor configuration a live run was actually held
    /// to (template, `1x`, effective deadline, digest) — the offer's
    /// `config_commitment` when nothing tightened or overrode it, and the
    /// offer's when no run happened (pre-eval reject).
    #[serde(default)]
    pub executor_commitment: String,
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
    /// A drain already holds this topic's one in-flight claim.
    #[error(
        "topic {topic_id} already has a row being scored ({id}); a topic's queue drains one row at a time, oldest first — retry when it lands"
    )]
    Busy {
        /// Topic whose queue is being drained.
        topic_id: String,
        /// The row in flight.
        id: String,
    },
    /// Topic document failed verification.
    #[error("topic: {0}")]
    Topic(#[from] TopicError),
    /// Holdout file did not match the topic commitment.
    #[error("holdout: {0}")]
    Holdout(#[from] HoldoutError),
}

/// What [`MemoryStore::enqueue`] did with an intake row.
#[derive(Debug, Clone, PartialEq)]
pub enum Enqueued {
    /// No row on that topic had this frozen digest: the row was inserted
    /// `queued` under a fresh id.
    Inserted(Submission),
    /// A row on that topic already carries this frozen digest — queued,
    /// scored, or rejected — and is returned unchanged. Nothing was inserted.
    Existing(Submission),
}

impl Enqueued {
    /// The row, either way.
    #[must_use]
    pub fn row(&self) -> &Submission {
        match self {
            Self::Inserted(r) | Self::Existing(r) => r,
        }
    }
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
    /// The one `queued` row per topic a drain is scoring right now
    /// (`topic_id → row id`). One entry per topic is the invariant that
    /// keeps a topic's queue oldest-first and one-at-a-time — promotion
    /// compares each run against the best *at that moment*, so two rows of
    /// one topic must never score side by side — and keeps two concurrent
    /// drains from renting twice for one row. Cleared when the scored row
    /// lands ([`MemoryStore::insert`]) or the drain gives the row back
    /// ([`MemoryStore::release_claim`]).
    scoring: BTreeMap<String, String>,
    topics: BTreeMap<String, TopicDocument>,
    holdouts: BTreeMap<String, Vec<HoldoutRecord>>,
    baselines: BTreeMap<String, SealedBaseline>,
    scores: BTreeMap<String, BTreeMap<String, MinerTopicRun>>,
}

impl Inner {
    /// Next `pf_…` id. Monotonic, so id order is intake order.
    fn mint_id(&mut self) -> String {
        let n = self.next;
        self.next = self.next.saturating_add(1);
        format!("pf_{n:016x}")
    }

    /// The oldest `queued` row on `topic_id` (rows iterate in id order).
    fn queue_head(&self, topic_id: &str) -> Option<Submission> {
        self.submissions
            .values()
            .find(|r| r.state == SubmissionState::Queued && r.topic_id == topic_id)
            .cloned()
    }
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

    /// Insert a submission: a scored row in its final state, or a `queued`
    /// row awaiting a drain. An empty id mints the next `pf_…` (ids are
    /// monotonic, so id order is intake order); a row that carries its id
    /// replaces the stored one — that is how a drained `queued` row lands
    /// scored — and settles the topic's claim when it is on this row.
    pub fn insert(&self, mut row: Submission) -> Result<Submission, StoreError> {
        let mut g = self.lock()?;
        if row.id.is_empty() {
            row.id = g.mint_id();
        }
        if g.scoring.get(&row.topic_id) == Some(&row.id) {
            g.scoring.remove(&row.topic_id);
        }
        g.submissions.insert(row.id.clone(), row.clone());
        Ok(row)
    }

    /// Queue an intake row **once per frozen digest per topic**, atomically:
    /// under one lock, a row on `row.topic_id` that already carries
    /// `row.submission_digest` — `queued`, scored, or rejected — is returned
    /// as [`Enqueued::Existing`] and nothing is inserted; otherwise the row
    /// is minted an id and stored `queued` ([`Enqueued::Inserted`]). Two
    /// concurrent identical submits therefore yield one row, and a retry
    /// after the row was drained finds the scored row rather than queueing a
    /// second paid run. The caller must pass an empty id.
    pub fn enqueue(&self, mut row: Submission) -> Result<Enqueued, StoreError> {
        if !row.id.is_empty() {
            return Err(StoreError::Illegal(
                "enqueue mints the id; pass an empty one".into(),
            ));
        }
        if row.state != SubmissionState::Queued {
            return Err(StoreError::Illegal(
                "enqueue takes a queued intake row".into(),
            ));
        }
        let mut g = self.lock()?;
        if let Some(existing) = g
            .submissions
            .values()
            .find(|r| r.topic_id == row.topic_id && r.submission_digest == row.submission_digest)
            .cloned()
        {
            return Ok(Enqueued::Existing(existing));
        }
        row.id = g.mint_id();
        g.submissions.insert(row.id.clone(), row.clone());
        Ok(Enqueued::Inserted(row))
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

    /// `queued` rows, oldest first (the drain order), on one topic or all.
    /// Rows a drain is scoring right now are still `queued` and still listed.
    pub fn queued(&self, topic_id: Option<&str>) -> Result<Vec<Submission>, StoreError> {
        Ok(self
            .lock()?
            .submissions
            .values()
            .filter(|r| r.state == SubmissionState::Queued)
            .filter(|r| topic_id.is_none_or(|t| r.topic_id == t))
            .cloned()
            .collect())
    }

    /// The row a drain is scoring on `topic_id` right now, if any.
    pub fn scoring_row(&self, topic_id: &str) -> Result<Option<String>, StoreError> {
        Ok(self.lock()?.scoring.get(topic_id).cloned())
    }

    /// Claim the head of `topic_id`'s queue — its oldest `queued` row — for
    /// this caller to score. `None` when the queue is empty;
    /// [`StoreError::Busy`] while another drain holds the topic's claim, so
    /// two drains never score two rows of one topic side by side (nor one
    /// row twice). Atomic under the store lock.
    pub fn claim_next_queued(&self, topic_id: &str) -> Result<Option<Submission>, StoreError> {
        let mut g = self.lock()?;
        if let Some(id) = g.scoring.get(topic_id) {
            return Err(StoreError::Busy {
                topic_id: topic_id.to_owned(),
                id: id.clone(),
            });
        }
        let head = g.queue_head(topic_id);
        if let Some(row) = &head {
            g.scoring.insert(topic_id.to_owned(), row.id.clone());
        }
        Ok(head)
    }

    /// Claim one `queued` row by id. It must be the head of its topic's
    /// queue (the queue drains oldest first — [`StoreError::Illegal`] names
    /// the head otherwise), not already scored ([`StoreError::Illegal`]),
    /// and its topic must have no row in flight ([`StoreError::Busy`]).
    pub fn claim_queued(&self, id: &str) -> Result<Submission, StoreError> {
        let mut g = self.lock()?;
        let row = g
            .submissions
            .get(id)
            .cloned()
            .ok_or_else(|| StoreError::NotFound(id.to_owned()))?;
        if row.state != SubmissionState::Queued {
            return Err(StoreError::Illegal(format!(
                "submission {id} is {:?}, not queued",
                row.state
            )));
        }
        if let Some(in_flight) = g.scoring.get(&row.topic_id) {
            return Err(StoreError::Busy {
                topic_id: row.topic_id.clone(),
                id: in_flight.clone(),
            });
        }
        if let Some(head) = g.queue_head(&row.topic_id).filter(|h| h.id != id) {
            return Err(StoreError::Illegal(format!(
                "submission {id} is not the head of topic {}'s queue; it drains oldest first — score {} first",
                row.topic_id, head.id
            )));
        }
        g.scoring.insert(row.topic_id.clone(), id.to_owned());
        Ok(row)
    }

    /// Give a claimed row back unscored (the host refused, or the drain was
    /// interrupted): it stays `queued` and the next drain may claim it
    /// again. A no-op unless `id` is the row holding `topic_id`'s claim, so
    /// a late release can never drop a claim another drain took since.
    pub fn release_claim(&self, topic_id: &str, id: &str) -> Result<(), StoreError> {
        let mut g = self.lock()?;
        if g.scoring.get(topic_id).is_some_and(|held| held == id) {
            g.scoring.remove(topic_id);
        }
        Ok(())
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
        self.lock()?
            .scores
            .entry(hotkey.to_owned())
            .or_default()
            .insert(topic_id.to_owned(), run);
        Ok(())
    }

    /// Per-topic lattices for one miner (binary SCORE_MAX/0 from `pass`).
    pub fn miner_scores(&self, hotkey: &str) -> Result<BTreeMap<String, u64>, StoreError> {
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
        Ok(self.lock()?.scores.get(hotkey).cloned().unwrap_or_default())
    }

    /// Every hotkey that has any recorded topic score.
    pub fn scored_hotkeys(&self) -> Result<BTreeSet<String>, StoreError> {
        Ok(self.lock()?.scores.keys().cloned().collect())
    }

    /// Best accepted champion primary on a topic, if the operator crowned one.
    pub fn champion_primary(&self, topic: &TopicDocument) -> Result<Option<f64>, StoreError> {
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

    fn queued_row(topic_id: &str, label: &str) -> Submission {
        let hotkey = "aa".repeat(32);
        let artifact = format!("{label:0>64}");
        let nonce = format!("nonce-{label}");
        Submission {
            id: String::new(),
            topic_id: topic_id.into(),
            miner_hotkey: hotkey.clone(),
            artifact_digest: artifact.clone(),
            artifact_uri: Some("https://example.invalid/a.tar".into()),
            claim: "claim".into(),
            declared_flops: 1,
            architecture: String::new(),
            inference_offer_id: String::new(),
            config_commitment: String::new(),
            executor_offer_id: String::new(),
            executor_commitment: String::new(),
            manifest: ArtifactManifest::default(),
            submission_digest: freeze_submission_digest(&hotkey, topic_id, &artifact, &nonce),
            nonce,
            state: SubmissionState::Queued,
            receipt_json: None,
            verdict: None,
            detail: Some("scoring deferred".into()),
        }
    }

    /// The queue is FIFO by id and a topic has **one** claim at a time: while
    /// its head is in flight nothing else on that topic can be claimed (not
    /// the next row, not the same row again); other topics are independent.
    /// Landing the scored row (same id) or releasing the claim frees the
    /// topic; a release names its row, so it never drops a newer claim.
    #[test]
    fn a_topic_drains_one_row_at_a_time_oldest_first() {
        let st = MemoryStore::new();
        let first = st.insert(queued_row("t", "1")).expect("first");
        let second = st.insert(queued_row("t", "2")).expect("second");
        let other = st.insert(queued_row("u", "3")).expect("other topic");
        assert!(first.id < second.id, "ids are monotonic");
        let ids = |rows: Vec<Submission>| rows.into_iter().map(|r| r.id).collect::<Vec<_>>();
        assert_eq!(
            ids(st.queued(None).expect("all")),
            [first.id.clone(), second.id.clone(), other.id.clone()]
        );
        assert_eq!(
            ids(st.queued(Some("t")).expect("t")),
            [first.id.clone(), second.id.clone()]
        );
        assert_eq!(st.scoring_row("t").expect("q"), None);

        // The second row is not the head: it cannot be scored out of order.
        let err = st.claim_queued(&second.id).expect_err("not the head");
        assert!(
            matches!(&err, StoreError::Illegal(m) if m.contains(&first.id)),
            "{err}"
        );

        let claimed = st.claim_next_queued("t").expect("claim").expect("row");
        assert_eq!(claimed.id, first.id, "oldest first");
        assert_eq!(st.scoring_row("t").expect("q"), Some(first.id.clone()));
        assert!(
            matches!(st.claim_next_queued("t"), Err(StoreError::Busy { ref id, .. }) if *id == first.id),
            "a second drain on the same topic is busy, not the next row"
        );
        assert!(
            matches!(st.claim_queued(&first.id), Err(StoreError::Busy { .. })),
            "the in-flight row cannot be claimed twice"
        );
        assert!(
            matches!(st.claim_queued(&second.id), Err(StoreError::Busy { .. })),
            "nor the row behind it"
        );
        assert_eq!(
            st.claim_next_queued("u").expect("claim").map(|r| r.id),
            Some(other.id.clone()),
            "another topic's queue is independent"
        );
        assert_eq!(
            st.queued(Some("t")).expect("still listed").len(),
            2,
            "claimed rows are still queued to readers"
        );

        // A release names its row: releasing the wrong id changes nothing.
        st.release_claim("t", &second.id).expect("release");
        assert_eq!(st.scoring_row("t").expect("q"), Some(first.id.clone()));
        st.release_claim("t", &first.id).expect("release");
        assert_eq!(st.scoring_row("t").expect("q"), None);
        assert_eq!(
            st.claim_next_queued("t").expect("claim").map(|r| r.id),
            Some(first.id.clone()),
            "a released head is claimable again, still first"
        );

        let mut scored = claimed;
        scored.state = SubmissionState::AwaitingAdmin;
        let landed = st.insert(scored).expect("land");
        assert_eq!(landed.id, first.id, "the scored row keeps its id");
        assert_eq!(
            st.get(&first.id).expect("get").state,
            SubmissionState::AwaitingAdmin
        );
        assert_eq!(
            st.scoring_row("t").expect("q"),
            None,
            "landing settles the claim"
        );
        assert!(
            matches!(st.claim_queued(&first.id), Err(StoreError::Illegal(_))),
            "a scored row is not queued"
        );
        assert!(matches!(
            st.claim_queued("pf_missing"),
            Err(StoreError::NotFound(_))
        ));
        assert_eq!(
            ids(st.queued(Some("t")).expect("t")),
            std::slice::from_ref(&second.id)
        );
        // Now the second row is the head and can be named directly.
        assert_eq!(st.claim_queued(&second.id).expect("head").id, second.id);
        // A stale release from the first row's drain must not free it.
        st.release_claim("t", &first.id).expect("stale release");
        assert_eq!(st.scoring_row("t").expect("q"), Some(second.id));
    }

    /// `enqueue` is the deferred path's one atomic step: one row per frozen
    /// digest per topic for the row's whole life — a duplicate finds the
    /// queued row, and a retry after the row was scored finds the scored row
    /// (never a second paid run); the same digest on another topic is its
    /// own row.
    #[test]
    fn enqueue_is_idempotent_per_topic_and_digest_for_the_rows_whole_life() {
        let st = MemoryStore::new();
        let Enqueued::Inserted(row) = st.enqueue(queued_row("t", "1")).expect("enqueue") else {
            panic!("first enqueue inserts");
        };
        assert!(row.id.starts_with("pf_"));
        assert_eq!(st.queued(Some("t")).expect("q").len(), 1);

        let again = st.enqueue(queued_row("t", "1")).expect("enqueue");
        assert_eq!(again, Enqueued::Existing(row.clone()));
        assert_eq!(again.row().id, row.id);
        assert_eq!(st.queued(Some("t")).expect("q").len(), 1, "no second row");

        let Enqueued::Inserted(elsewhere) = st.enqueue(queued_row("u", "1")).expect("enqueue")
        else {
            panic!("another topic is another row");
        };
        assert_ne!(elsewhere.id, row.id);

        let mut scored = row.clone();
        scored.state = SubmissionState::Rejected;
        st.insert(scored.clone()).expect("land");
        let after = st.enqueue(queued_row("t", "1")).expect("enqueue");
        assert_eq!(
            after,
            Enqueued::Existing(scored),
            "a retry after scoring finds the scored row, not a new queued one"
        );
        assert!(st.queued(Some("t")).expect("q").is_empty());

        let mut with_id = queued_row("t", "9");
        with_id.id = "pf_given".into();
        assert!(matches!(st.enqueue(with_id), Err(StoreError::Illegal(_))));
        let mut not_queued = queued_row("t", "9");
        not_queued.state = SubmissionState::AwaitingAdmin;
        assert!(matches!(
            st.enqueue(not_queued),
            Err(StoreError::Illegal(_))
        ));
    }

    #[test]
    fn submission_states_have_snake_case_wire_names() {
        for (state, wire) in [
            (SubmissionState::Queued, "\"queued\""),
            (SubmissionState::AwaitingAdmin, "\"awaiting_admin\""),
            (SubmissionState::Rejected, "\"rejected\""),
            (SubmissionState::Champion, "\"champion\""),
        ] {
            assert_eq!(serde_json::to_string(&state).expect("json"), wire);
        }
    }
}
