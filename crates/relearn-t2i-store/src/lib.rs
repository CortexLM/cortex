//! In-memory Relearn T2I store: submissions, artifact manifests, champion,
//! and the sealed holdout prompt set.
//!
//! The holdout records are the secret here, not a random seed: once a miner
//! knows which Qwen-Image-Bench ids are scored, the holdout stops measuring
//! generalization. So the records are loaded once from an operator file,
//! verified against the commitment in `config/relearn-t2i-pin.toml`, and are
//! only readable after a submission digest has been frozen. The public view
//! carries the commitment and the size, never the ids or the prompt text.

#![forbid(unsafe_code)]
#![allow(
    clippy::doc_markdown,
    clippy::module_name_repetitions,
    clippy::must_use_candidate
)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use relearn_t2i_score::{PromoteVerdict, T2iSliceScores};
use relearn_t2i_task::{
    verify_holdout_prompts, FrozenPrompt, HoldoutError, SamplerConfig, HOLDOUT_DOMAIN,
};
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
    /// Rejected (regression / gates / integrity / license).
    Rejected,
    /// Operator-promoted champion.
    Champion,
}

/// What a miner declares about the artifact they submitted.
///
/// `base` and `base_license` are the license attestation: the artifact must be
/// a fine-tune of the pinned Cosmos3 checkpoint, inheriting OpenMDW 1.1.
/// `train_prompt_ids` is what the contamination gate reads.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ArtifactManifest {
    /// Declared base checkpoint.
    pub base: String,
    /// Declared license inherited from the base.
    pub base_license: String,
    /// Declared sampler / dtype used to produce the claimed outputs.
    #[serde(default)]
    pub sampler: SamplerConfig,
    /// Bench prompt ids present in the submitted training metadata.
    #[serde(default)]
    pub train_prompt_ids: Vec<u32>,
    /// `cell_key` → sha256 hex of the image the miner claims that cell produced.
    /// The seed-replay gate regenerates a few of these.
    #[serde(default)]
    pub claimed_output_hashes: BTreeMap<String, String>,
}

/// One miner submission.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Submission {
    /// Stable id (`t2i_` + 16 hex).
    pub id: String,
    /// 64-hex miner hotkey.
    pub miner_hotkey: String,
    /// SHA-256 hex of the miner artifact (weights / adapter). Frozen at accept.
    pub artifact_digest: String,
    /// Optional locator (HF repo, object URL).
    pub artifact_uri: Option<String>,
    /// Declared base, license, sampler, and training metadata.
    pub manifest: ArtifactManifest,
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

/// Public description of the sealed holdout. Carries no prompt ids or text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HoldoutSeal {
    /// Slice id bound into the paired test.
    pub slice_id: String,
    /// Commitment pinned in `config/relearn-t2i-pin.toml`.
    pub commitment: String,
    /// Number of holdout prompts.
    pub size: usize,
    /// Whether the operator has loaded matching records on this host.
    pub loaded: bool,
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
    /// Operator holdout file did not match the committed digest.
    #[error("holdout: {0}")]
    Holdout(#[from] HoldoutError),
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
    scores: BTreeMap<String, T2iSliceScores>,
    champion_scores: Option<T2iSliceScores>,
    base_champion: Option<T2iSliceScores>,
    holdout: Option<Vec<FrozenPrompt>>,
    holdout_commitment: String,
    holdout_size: usize,
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

    /// Record the pin's holdout commitment before any records are loaded.
    ///
    /// # Errors
    ///
    /// [`StoreError::Poison`].
    pub fn set_holdout_commitment(&self, commitment: &str, size: usize) -> Result<(), StoreError> {
        let mut g = self.lock()?;
        g.holdout_commitment = commitment.trim().to_ascii_lowercase();
        g.holdout_size = size;
        Ok(())
    }

    /// Load operator-supplied holdout records, verified against the pin.
    ///
    /// # Errors
    ///
    /// [`StoreError::Holdout`] when the records do not match the commitment,
    /// overlap the public split, or are structurally invalid. Nothing is stored
    /// on failure: a host with a bad holdout file scores nothing rather than
    /// silently falling back to the public split.
    pub fn load_holdout(
        &self,
        records: Vec<FrozenPrompt>,
        public_ids: &[u32],
    ) -> Result<(), StoreError> {
        let (commitment, size) = {
            let g = self.lock()?;
            (g.holdout_commitment.clone(), g.holdout_size)
        };
        verify_holdout_prompts(&records, public_ids, &commitment, size)?;
        self.lock()?.holdout = Some(records);
        Ok(())
    }

    /// Public seal description (no ids, no text).
    ///
    /// # Errors
    ///
    /// [`StoreError::Poison`].
    pub fn holdout_seal(&self) -> Result<HoldoutSeal, StoreError> {
        let g = self.lock()?;
        Ok(HoldoutSeal {
            slice_id: relearn_t2i_score::HOLDOUT_SLICE_ID.to_owned(),
            commitment: g.holdout_commitment.clone(),
            size: g.holdout_size,
            loaded: g.holdout.is_some(),
        })
    }

    /// Holdout records, readable only after a submission digest is frozen.
    ///
    /// # Errors
    ///
    /// [`StoreError::Illegal`] when the digest is empty or no verified records
    /// are loaded.
    pub fn unseal_holdout(&self, frozen_digest: &str) -> Result<Vec<FrozenPrompt>, StoreError> {
        if frozen_digest.trim().is_empty() {
            return Err(StoreError::Illegal(
                "holdout stays sealed until the submission digest is frozen".into(),
            ));
        }
        let g = self.lock()?;
        g.holdout
            .clone()
            .ok_or_else(|| StoreError::Illegal("no verified holdout loaded".into()))
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
            row.id = format!("t2i_{n:016x}");
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

    /// Promote `id` and demote the previous champion.
    ///
    /// # Errors
    ///
    /// [`StoreError::Illegal`] unless the row is `awaiting_admin` with an
    /// eligible verdict; [`StoreError::NotFound`] for an unknown id.
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
    ///
    /// # Errors
    ///
    /// [`StoreError::Poison`].
    pub fn record_scores(&self, id: &str, scores: T2iSliceScores) -> Result<(), StoreError> {
        self.lock()?.scores.insert(id.to_owned(), scores);
        Ok(())
    }

    /// Seed / replace the implicit base-model champion scores.
    ///
    /// # Errors
    ///
    /// [`StoreError::Poison`].
    pub fn set_base_champion(&self, scores: T2iSliceScores) -> Result<(), StoreError> {
        self.lock()?.base_champion = Some(scores);
        Ok(())
    }

    /// Champion slice scores (promoted miner, else the pinned base).
    ///
    /// # Errors
    ///
    /// [`StoreError::Poison`].
    pub fn champion_scores(&self) -> Result<Option<T2iSliceScores>, StoreError> {
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

/// Per-epoch slice id derived from the holdout commitment.
#[must_use]
pub fn holdout_slice_id(epoch: u64, commitment: &str) -> String {
    let mut h = Sha256::new();
    h.update(HOLDOUT_DOMAIN);
    h.update(epoch.to_le_bytes());
    h.update(commitment.as_bytes());
    format!(
        "relearn-t2i-holdout-{epoch}-{}",
        hex::encode(&h.finalize()[..4])
    )
}

#[cfg(test)]
mod tests {
    use prism_competition::ExampleSeries;
    use relearn_t2i_task::frozen_prompt_commitment;

    use super::*;

    fn prompts() -> Vec<FrozenPrompt> {
        (900..=903)
            .map(|id| FrozenPrompt {
                id,
                text: format!("holdout prompt {id}"),
                upsampled_json: None,
            })
            .collect()
    }

    fn seeded_store() -> MemoryStore {
        let st = MemoryStore::new();
        let recs = prompts();
        st.set_holdout_commitment(&frozen_prompt_commitment(&recs), recs.len())
            .expect("commit");
        st
    }

    fn slice(v: f64) -> T2iSliceScores {
        T2iSliceScores {
            holdout: ExampleSeries::from_pairs((0..8).map(|i| (format!("p1#v{i}"), v))),
            ..T2iSliceScores::default()
        }
    }

    fn row(state: SubmissionState, verdict: Option<PromoteVerdict>) -> Submission {
        Submission {
            id: String::new(),
            miner_hotkey: "00".repeat(32),
            artifact_digest: "11".repeat(32),
            artifact_uri: None,
            manifest: ArtifactManifest::default(),
            nonce: "aa".into(),
            submission_digest: "bb".repeat(32),
            state,
            receipt_json: None,
            verdict,
            detail: None,
        }
    }

    #[test]
    fn digest_stable_and_distinct() {
        let a = freeze_submission_digest("aa", "bb", "n1");
        assert_eq!(a, freeze_submission_digest("aa", "bb", "n1"));
        assert_ne!(a, freeze_submission_digest("aa", "bb", "n2"));
        assert_eq!(a.len(), 64);
    }

    #[test]
    fn holdout_loads_only_when_it_matches_the_commitment() {
        let st = seeded_store();
        let seal = st.holdout_seal().expect("seal");
        assert_eq!(seal.size, 4);
        assert!(!seal.loaded);

        let mut tampered = prompts();
        tampered[0].text = "leaked".into();
        assert!(st.load_holdout(tampered, &[1, 2]).is_err());
        assert!(!st.holdout_seal().expect("seal").loaded);

        st.load_holdout(prompts(), &[1, 2]).expect("verified load");
        assert!(st.holdout_seal().expect("seal").loaded);
    }

    #[test]
    fn seal_never_exposes_prompt_ids() {
        let st = seeded_store();
        st.load_holdout(prompts(), &[]).expect("load");
        let seal = st.holdout_seal().expect("seal");
        let json = serde_json::to_string(&seal).expect("json");
        for id in 900..=903 {
            assert!(
                !json.contains(&format!("{id}")),
                "seal leaked id {id}: {json}"
            );
        }
        assert!(!json.contains("holdout prompt"));
    }

    #[test]
    fn unseal_requires_a_frozen_digest() {
        let st = seeded_store();
        st.load_holdout(prompts(), &[]).expect("load");
        assert!(st.unseal_holdout("").is_err());
        assert!(st.unseal_holdout("   ").is_err());
        assert_eq!(st.unseal_holdout("deadbeef").expect("unseal").len(), 4);
    }

    #[test]
    fn unseal_without_loaded_records_fails_closed() {
        let st = seeded_store();
        assert!(st.unseal_holdout("deadbeef").is_err());
    }

    #[test]
    fn promote_refuses_ineligible() {
        let st = MemoryStore::new();
        let r = st
            .insert(row(SubmissionState::AwaitingAdmin, None))
            .expect("insert");
        assert!(st.promote(&r.id).is_err());
    }

    #[test]
    fn champion_scores_follow_promote_not_base() {
        let st = MemoryStore::new();
        st.set_base_champion(slice(0.4)).expect("base");
        let r = st
            .insert(row(
                SubmissionState::AwaitingAdmin,
                Some(PromoteVerdict {
                    eligible: true,
                    paired: None,
                    ab: relearn_t2i_score::PairedAb::default(),
                    pillars: BTreeMap::new(),
                    failed: Vec::new(),
                    lattice: 12,
                }),
            ))
            .expect("insert");
        st.record_scores(&r.id, slice(0.8)).expect("scores");
        st.promote(&r.id).expect("promote");
        let got = st.champion_scores().expect("read").expect("some");
        assert!((T2iSliceScores::mean(&got.holdout).unwrap_or(0.0) - 0.8).abs() < 1e-9);
    }

    #[test]
    fn slice_ids_track_epoch_and_commitment() {
        let a = holdout_slice_id(3, "aa");
        assert_eq!(a, holdout_slice_id(3, "aa"));
        assert_ne!(a, holdout_slice_id(4, "aa"));
        assert_ne!(a, holdout_slice_id(3, "bb"));
    }
}
