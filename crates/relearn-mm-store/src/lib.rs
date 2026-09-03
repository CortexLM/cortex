//! In-memory Relearn Multimodal store: submissions, encoder manifests, champion.
//!
//! The champion carries two things a challenger is measured against: its text
//! holdout series (gate 1) and its LM weights hash. An encoder-only challenger
//! has to match that hash, so the store is where "same LM, new eyes" is
//! actually enforced.

#![forbid(unsafe_code)]
#![allow(
    clippy::doc_markdown,
    clippy::module_name_repetitions,
    clippy::must_use_candidate
)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use relearn_mm_score::{MmSliceScores, PromoteVerdict};
use relearn_mm_task::SubmissionKind;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Submission lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubmissionState {
    /// Digest accepted; holdout still sealed.
    Accepted,
    /// Digest frozen; eval running.
    Evaluating,
    /// Eval finished; waiting operator audit.
    AwaitingAdmin,
    /// Rejected (LM regression / vision gates / license / integrity).
    Rejected,
    /// Operator-promoted champion.
    Champion,
}

/// What a miner declares about the multimodal artifact they submitted.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EncoderManifest {
    /// Encoder repo id (miner may bring a different permissive encoder).
    pub encoder_model: String,
    /// Encoder license. Must be OSI-permissive.
    pub encoder_license: String,
    /// Projector architecture description (documentation only).
    pub projector: String,
    /// Encoder-only or encoder plus an LM adapter.
    pub kind: SubmissionKind,
    /// SHA-256 hex of the submitted LM weights.
    ///
    /// For [`SubmissionKind::EncoderOnly`] this must equal the champion's hash.
    pub lm_weights_hash: String,
}

/// One miner submission.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Submission {
    /// Stable id (`mm_` + 16 hex).
    pub id: String,
    /// 64-hex miner hotkey.
    pub miner_hotkey: String,
    /// SHA-256 hex of the miner artifact. Frozen at accept.
    pub artifact_digest: String,
    /// Optional locator (HF repo, object URL).
    pub artifact_uri: Option<String>,
    /// Declared encoder, license, kind, and LM hash.
    pub manifest: EncoderManifest,
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

/// In-memory store (v0).
#[derive(Clone, Default)]
pub struct MemoryStore {
    inner: Arc<Mutex<Inner>>,
}

#[derive(Default)]
struct Inner {
    next: u64,
    submissions: BTreeMap<String, Submission>,
    champion_id: Option<String>,
    scores: BTreeMap<String, MmSliceScores>,
    champion_scores: Option<MmSliceScores>,
    base_champion: Option<MmSliceScores>,
    champion_lm_hash: String,
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

    /// Record the champion's LM weights hash (gate 1's reference).
    ///
    /// # Errors
    ///
    /// [`StoreError::Poison`].
    pub fn set_champion_lm_hash(&self, hash: &str) -> Result<(), StoreError> {
        self.lock()?.champion_lm_hash = hash.trim().to_ascii_lowercase();
        Ok(())
    }

    /// Champion LM weights hash, or empty when unset.
    ///
    /// # Errors
    ///
    /// [`StoreError::Poison`].
    pub fn champion_lm_hash(&self) -> Result<String, StoreError> {
        Ok(self.lock()?.champion_lm_hash.clone())
    }

    /// Insert a newly accepted submission.
    ///
    /// # Errors
    ///
    /// [`StoreError::Poison`].
    pub fn insert(&self, mut row: Submission) -> Result<Submission, StoreError> {
        let mut g = self.lock()?;
        if row.id.is_empty() {
            let n = g.next;
            g.next = g.next.saturating_add(1);
            row.id = format!("mm_{n:016x}");
        }
        g.submissions.insert(row.id.clone(), row.clone());
        Ok(row)
    }

    /// Fetch one row.
    ///
    /// # Errors
    ///
    /// [`StoreError::NotFound`].
    pub fn get(&self, id: &str) -> Result<Submission, StoreError> {
        let g = self.lock()?;
        g.submissions
            .get(id)
            .cloned()
            .ok_or_else(|| StoreError::NotFound(id.to_owned()))
    }

    /// List newest-first.
    ///
    /// # Errors
    ///
    /// [`StoreError::Poison`].
    pub fn list(&self) -> Result<Vec<Submission>, StoreError> {
        let g = self.lock()?;
        let mut rows: Vec<_> = g.submissions.values().cloned().collect();
        rows.sort_by(|a, b| b.id.cmp(&a.id));
        Ok(rows)
    }

    /// Patch state / verdict / receipt.
    ///
    /// # Errors
    ///
    /// [`StoreError::NotFound`].
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
    ///
    /// # Errors
    ///
    /// [`StoreError::Poison`].
    pub fn champion_id(&self) -> Result<Option<String>, StoreError> {
        Ok(self.lock()?.champion_id.clone())
    }

    /// Promote `id`, demote the previous champion, and adopt its LM hash.
    ///
    /// # Errors
    ///
    /// [`StoreError::Illegal`] unless the row is `awaiting_admin` with an
    /// eligible verdict; [`StoreError::NotFound`] for an unknown id.
    pub fn promote(&self, id: &str) -> Result<Submission, StoreError> {
        let mut g = self.lock()?;
        let prev = g.champion_id.clone();
        let new_lm_hash = {
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
                    "promote refused: verdict not eligible (LM regression or vision gates)".into(),
                ));
            }
            row.manifest.lm_weights_hash.trim().to_ascii_lowercase()
        };
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
        // An encoder-and-LM promote moves the reference the next challenger's
        // gate 1 is measured against; leaving the old hash would let the next
        // encoder-only submission ship a stale language model.
        if !new_lm_hash.is_empty() {
            g.champion_lm_hash = new_lm_hash;
        }
        g.champion_id = Some(id.to_owned());
        g.submissions
            .get(id)
            .cloned()
            .ok_or_else(|| StoreError::NotFound(id.to_owned()))
    }

    /// Persist challenger slices so a later promote displaces vs this run.
    ///
    /// # Errors
    ///
    /// [`StoreError::Poison`].
    pub fn record_scores(&self, id: &str, scores: MmSliceScores) -> Result<(), StoreError> {
        self.lock()?.scores.insert(id.to_owned(), scores);
        Ok(())
    }

    /// Seed / replace the implicit baseline champion scores.
    ///
    /// # Errors
    ///
    /// [`StoreError::Poison`].
    pub fn set_base_champion(&self, scores: MmSliceScores) -> Result<(), StoreError> {
        self.lock()?.base_champion = Some(scores);
        Ok(())
    }

    /// Champion slice scores (promoted miner, else the pinned baseline).
    ///
    /// # Errors
    ///
    /// [`StoreError::Poison`].
    pub fn champion_scores(&self) -> Result<Option<MmSliceScores>, StoreError> {
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

#[cfg(test)]
mod tests {
    use prism_competition::ExampleSeries;

    use super::*;

    const CHAMP_HASH: &str = "aaaa1111";

    fn scores(text: f64) -> MmSliceScores {
        MmSliceScores {
            text_holdout: ExampleSeries::from_pairs((0..8).map(|i| (format!("t{i}"), text))),
            lm_weights_hash: CHAMP_HASH.into(),
            ..MmSliceScores::default()
        }
    }

    fn verdict(eligible: bool) -> PromoteVerdict {
        PromoteVerdict {
            eligible,
            lm_intact: None,
            vision: None,
            agentic: None,
            shuffle_drop: 0.3,
            failed: Vec::new(),
            lattice: if eligible { 42 } else { 0 },
        }
    }

    fn row(kind: SubmissionKind, lm_hash: &str, v: Option<PromoteVerdict>) -> Submission {
        Submission {
            id: String::new(),
            miner_hotkey: "00".repeat(32),
            artifact_digest: "11".repeat(32),
            artifact_uri: None,
            manifest: EncoderManifest {
                encoder_model: "google/siglip2-so400m-patch14-384".into(),
                encoder_license: "apache-2.0".into(),
                projector: "2-layer MLP".into(),
                kind,
                lm_weights_hash: lm_hash.into(),
            },
            nonce: "aa".into(),
            submission_digest: "bb".repeat(32),
            state: SubmissionState::AwaitingAdmin,
            receipt_json: None,
            verdict: v,
            detail: None,
        }
    }

    #[test]
    fn digest_stable_and_distinct() {
        let a = freeze_submission_digest("aa", "bb", "n1");
        assert_eq!(a, freeze_submission_digest("aa", "bb", "n1"));
        assert_ne!(a, freeze_submission_digest("aa", "bb", "n2"));
    }

    #[test]
    fn manifest_defaults_to_the_strict_kind() {
        assert_eq!(
            EncoderManifest::default().kind,
            SubmissionKind::EncoderOnly,
            "an unstated kind must not skip the LM hash check"
        );
    }

    #[test]
    fn promote_refuses_ineligible() {
        let st = MemoryStore::new();
        let r = st
            .insert(row(SubmissionKind::EncoderOnly, CHAMP_HASH, None))
            .expect("insert");
        assert!(st.promote(&r.id).is_err());
        let r2 = st
            .insert(row(
                SubmissionKind::EncoderOnly,
                CHAMP_HASH,
                Some(verdict(false)),
            ))
            .expect("insert");
        assert!(st.promote(&r2.id).is_err());
    }

    #[test]
    fn promote_moves_the_champion_lm_hash_reference() {
        let st = MemoryStore::new();
        st.set_champion_lm_hash(CHAMP_HASH).expect("seed hash");
        assert_eq!(st.champion_lm_hash().expect("read"), CHAMP_HASH);

        let r = st
            .insert(row(
                SubmissionKind::EncoderAndLm,
                "CCCC3333",
                Some(verdict(true)),
            ))
            .expect("insert");
        st.record_scores(&r.id, scores(0.9)).expect("scores");
        st.promote(&r.id).expect("promote");
        assert_eq!(
            st.champion_lm_hash().expect("read"),
            "cccc3333",
            "the next encoder-only submission must be measured against the new LM"
        );
    }

    #[test]
    fn champion_scores_follow_promote_not_base() {
        let st = MemoryStore::new();
        st.set_base_champion(scores(0.4)).expect("base");
        let r = st
            .insert(row(
                SubmissionKind::EncoderOnly,
                CHAMP_HASH,
                Some(verdict(true)),
            ))
            .expect("insert");
        st.record_scores(&r.id, scores(0.8)).expect("scores");
        st.promote(&r.id).expect("promote");
        let got = st.champion_scores().expect("read").expect("some");
        assert!((MmSliceScores::mean(&got.text_holdout).unwrap_or(0.0) - 0.8).abs() < 1e-9);
    }

    #[test]
    fn promoting_a_second_champion_demotes_the_first() {
        let st = MemoryStore::new();
        let a = st
            .insert(row(
                SubmissionKind::EncoderOnly,
                CHAMP_HASH,
                Some(verdict(true)),
            ))
            .expect("insert a");
        st.promote(&a.id).expect("promote a");
        let b = st
            .insert(row(
                SubmissionKind::EncoderOnly,
                CHAMP_HASH,
                Some(verdict(true)),
            ))
            .expect("insert b");
        st.promote(&b.id).expect("promote b");
        assert_eq!(st.get(&a.id).expect("a").state, SubmissionState::Rejected);
        assert_eq!(st.champion_id().expect("id"), Some(b.id));
    }
}
