//! In-memory Relearn store: submissions, sealed holdout, champion.
//!
//! Holdout items stay sealed until the submission digest is frozen.

#![forbid(unsafe_code)]
#![allow(
    clippy::missing_errors_doc,
    clippy::doc_markdown,
    clippy::must_use_candidate
)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use relearn_challenge_task::HOLDOUT_DOMAIN;
use relearn_score::{PromoteVerdict, SliceScores};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Submission lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubmissionState {
    /// Digest accepted; holdout still sealed.
    Accepted,
    /// Digest frozen; holdout unsealed; eval running.
    Evaluating,
    /// Eval finished; waiting operator audit.
    AwaitingAdmin,
    /// Rejected (regression / gates / integrity).
    Rejected,
    /// Operator-promoted champion.
    Champion,
}

/// One miner submission.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Submission {
    /// Stable id (`rl_` + 16 hex).
    pub id: String,
    /// 64-hex miner hotkey.
    pub miner_hotkey: String,
    /// SHA-256 hex of the miner artifact (weights / adapter). Frozen at accept.
    pub artifact_digest: String,
    /// Optional locator (HF repo, object URL). Never the scored teacher payload.
    pub artifact_uri: Option<String>,
    /// Digest freeze nonce (hex).
    pub nonce: String,
    /// `sha256(hotkey || 0xff || artifact || 0xff || nonce)`.
    pub submission_digest: String,
    /// Lifecycle.
    pub state: SubmissionState,
    /// Eval receipt JSON (if any).
    pub receipt_json: Option<String>,
    /// Judge verdict (if any).
    pub verdict: Option<PromoteVerdict>,
    /// Reject / gate reason.
    pub detail: Option<String>,
}

/// Sealed holdout: items hidden until `unseal_after`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Holdout {
    /// Slice id bound into the paired test.
    pub slice_id: String,
    /// Hex seed. Empty in the public view until unsealed.
    pub seed_hex: String,
    /// Whether the seed has been revealed for this submission.
    pub unsealed: bool,
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
    /// Illegal state transition.
    #[error("illegal state {0}")]
    Illegal(String),
}

/// In-memory store (v0). Postgres can replace this without changing the HTTP surface.
#[derive(Clone, Default)]
pub struct MemoryStore {
    inner: Arc<Mutex<Inner>>,
}

#[derive(Default)]
struct Inner {
    next: u64,
    submissions: BTreeMap<String, Submission>,
    champion_id: Option<String>,
    /// Per-submission holdout slices (in-memory; not serialized on the HTTP row).
    scores: BTreeMap<String, SliceScores>,
    /// Live champion slices (promoted miner). Displacement is vs this, not the base.
    champion_scores: Option<SliceScores>,
    /// Baseline champion scores (base model) until a miner is promoted.
    base_champion: Option<SliceScores>,
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

    /// Insert a newly accepted submission.
    pub fn insert(&self, mut row: Submission) -> Result<Submission, StoreError> {
        let mut g = self.lock()?;
        if row.id.is_empty() {
            let n = g.next;
            g.next = g.next.saturating_add(1);
            row.id = format!("rl_{n:016x}");
        }
        g.submissions.insert(row.id.clone(), row.clone());
        Ok(row)
    }

    /// Fetch one row.
    pub fn get(&self, id: &str) -> Result<Submission, StoreError> {
        let g = self.lock()?;
        g.submissions
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

    /// Patch state / verdict / receipt.
    pub fn patch(
        &self,
        id: &str,
        state: Option<SubmissionState>,
        receipt_json: Option<String>,
        verdict: Option<PromoteVerdict>,
        detail: Option<String>,
    ) -> Result<Submission, StoreError> {
        let mut g = self.lock()?;
        let row = g
            .submissions
            .get_mut(id)
            .ok_or_else(|| StoreError::NotFound(id.to_owned()))?;
        if let Some(s) = state {
            row.state = s;
        }
        if let Some(r) = receipt_json {
            row.receipt_json = Some(r);
        }
        if let Some(v) = verdict {
            row.verdict = Some(v);
        }
        if let Some(d) = detail {
            row.detail = Some(d);
        }
        Ok(row.clone())
    }

    /// Current champion submission id.
    pub fn champion_id(&self) -> Result<Option<String>, StoreError> {
        Ok(self.lock()?.champion_id.clone())
    }

    /// Promote `id` and demote the previous champion.
    pub fn promote(&self, id: &str) -> Result<Submission, StoreError> {
        let mut g = self.lock()?;
        let prev = g.champion_id.clone();
        {
            let row = g
                .submissions
                .get(id)
                .ok_or_else(|| StoreError::NotFound(id.to_owned()))?;
            if row.state != SubmissionState::AwaitingAdmin {
                return Err(StoreError::Illegal(format!(
                    "promote requires awaiting_admin, got {:?}",
                    row.state
                )));
            }
            if !row.verdict.as_ref().is_some_and(|v| v.eligible) {
                return Err(StoreError::Illegal(
                    "promote refused: verdict not eligible (regression or gates)".into(),
                ));
            }
        }
        if let Some(p) = prev {
            if let Some(old) = g.submissions.get_mut(&p) {
                if old.state == SubmissionState::Champion {
                    old.state = SubmissionState::Rejected;
                    old.detail = Some("superseded".into());
                }
            }
        }
        {
            let row = g
                .submissions
                .get_mut(id)
                .ok_or_else(|| StoreError::NotFound(id.to_owned()))?;
            row.state = SubmissionState::Champion;
        }
        if let Some(s) = g.scores.get(id).cloned() {
            g.champion_scores = Some(s);
        }
        g.champion_id = Some(id.to_owned());
        g.submissions
            .get(id)
            .cloned()
            .ok_or_else(|| StoreError::NotFound(id.to_owned()))
    }

    /// Persist challenger slices so a later promote displaces vs this run.
    pub fn record_scores(&self, id: &str, scores: SliceScores) -> Result<(), StoreError> {
        self.lock()?.scores.insert(id.to_owned(), scores);
        Ok(())
    }

    /// Seed / replace the implicit base-model champion scores.
    pub fn set_base_champion(&self, scores: SliceScores) -> Result<(), StoreError> {
        self.lock()?.base_champion = Some(scores);
        Ok(())
    }

    /// Champion slice scores (promoted miner, else base model).
    pub fn champion_scores(&self) -> Result<Option<SliceScores>, StoreError> {
        let g = self.lock()?;
        if let Some(s) = &g.champion_scores {
            return Ok(Some(s.clone()));
        }
        Ok(g.base_champion.clone())
    }
}

/// SHA-256 hex of the frozen submission.
#[must_use]
pub fn freeze_submission_digest(hotkey: &str, artifact_digest: &str, nonce: &str) -> String {
    let mut h = Sha256::new();
    h.update(hotkey.as_bytes());
    h.update([0xff]);
    h.update(artifact_digest.as_bytes());
    h.update([0xff]);
    h.update(nonce.as_bytes());
    hex::encode(h.finalize())
}

/// Build a holdout that is sealed until `digest` is recorded.
#[must_use]
pub fn sealed_holdout(epoch: u64, digest: &str) -> Holdout {
    let mut h = Sha256::new();
    h.update(HOLDOUT_DOMAIN);
    h.update(epoch.to_le_bytes());
    h.update(digest.as_bytes());
    let seed = hex::encode(h.finalize());
    Holdout {
        slice_id: format!("relearn-holdout-{epoch}"),
        seed_hex: String::new(),
        unsealed: false,
    }
    .with_pending_seed(seed)
}

trait WithPending {
    fn with_pending_seed(self, seed: String) -> Self;
}

impl WithPending for Holdout {
    fn with_pending_seed(mut self, seed: String) -> Self {
        // Keep seed off the public struct until unseal.
        self.seed_hex = seed;
        self.unsealed = false;
        self
    }
}

/// Reveal holdout seed only after the submission digest is frozen.
#[must_use]
pub fn unseal_holdout(pending: &Holdout, frozen_digest: &str) -> Option<Holdout> {
    if frozen_digest.is_empty() || pending.seed_hex.is_empty() {
        return None;
    }
    Some(Holdout {
        slice_id: pending.slice_id.clone(),
        seed_hex: pending.seed_hex.clone(),
        unsealed: true,
    })
}

/// Public view: seed stripped until unsealed.
#[must_use]
pub fn public_holdout(h: &Holdout) -> Holdout {
    if h.unsealed {
        h.clone()
    } else {
        Holdout {
            slice_id: h.slice_id.clone(),
            seed_hex: String::new(),
            unsealed: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_stable_and_distinct() {
        let a = freeze_submission_digest("aa", "bb", "n1");
        let b = freeze_submission_digest("aa", "bb", "n1");
        let c = freeze_submission_digest("aa", "bb", "n2");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.len(), 64);
    }

    #[test]
    fn holdout_stays_sealed_in_public_view() {
        let h = sealed_holdout(7, "deadbeef");
        assert!(!h.unsealed);
        assert!(!h.seed_hex.is_empty());
        let pub_v = public_holdout(&h);
        assert!(pub_v.seed_hex.is_empty());
        let open = unseal_holdout(&h, "deadbeef").expect("unseal");
        assert!(open.unsealed);
        assert!(!open.seed_hex.is_empty());
    }

    #[test]
    fn promote_refuses_ineligible() {
        let st = MemoryStore::new();
        let row = st
            .insert(Submission {
                id: String::new(),
                miner_hotkey: "00".repeat(32),
                artifact_digest: "11".repeat(32),
                artifact_uri: None,
                nonce: "aa".into(),
                submission_digest: "bb".repeat(32),
                state: SubmissionState::AwaitingAdmin,
                receipt_json: None,
                verdict: None,
                detail: None,
            })
            .expect("insert");
        assert!(st.promote(&row.id).is_err());
    }

    #[test]
    fn champion_scores_follow_promote_not_base() {
        use prism_competition::ExampleSeries;
        use relearn_score::SliceScores;

        fn series(prefix: &str, n: usize, val: f64) -> ExampleSeries {
            ExampleSeries::from_pairs((0..n).map(|i| (format!("{prefix}{i}"), val)))
        }
        fn slice(v: f64) -> SliceScores {
            SliceScores {
                holdout: series("h", 8, v),
                public: series("p", 8, v),
                perturbed: series("x", 8, v),
                canaries: series("c", 8, v),
                agent_trace: 0.9,
            }
        }

        let st = MemoryStore::new();
        st.set_base_champion(slice(0.4)).expect("base");
        let row = st
            .insert(Submission {
                id: String::new(),
                miner_hotkey: "00".repeat(32),
                artifact_digest: "11".repeat(32),
                artifact_uri: None,
                nonce: "aa".into(),
                submission_digest: "bb".repeat(32),
                state: SubmissionState::AwaitingAdmin,
                receipt_json: None,
                verdict: Some(relearn_score::PromoteVerdict {
                    eligible: true,
                    paired: None,
                    failed: Vec::new(),
                    lattice: 12,
                }),
                detail: None,
            })
            .expect("insert");
        st.record_scores(&row.id, slice(0.8)).expect("scores");
        st.promote(&row.id).expect("promote");
        let got = st.champion_scores().expect("read").expect("some");
        assert!((SliceScores::mean(&got.holdout).unwrap_or(0.0) - 0.8).abs() < 1e-9);
    }
}
