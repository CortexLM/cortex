//! In-memory Relearn store: submissions, sealed holdout, champion.
//!
//! Holdout records are loaded once from an operator file, verified against
//! the commitment in `config/relearn-pin.toml`, and are only readable after a
//! submission digest has been frozen. The public view carries the commitment
//! and the size, never ids, prompts, or image hashes.

#![forbid(unsafe_code)]
#![allow(
    clippy::missing_errors_doc,
    clippy::doc_markdown,
    clippy::must_use_candidate
)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use prism_competition::ExampleSeries;
use relearn_challenge_task::{
    verify_holdout_items, HoldoutError, HoldoutItem, HoldoutTask, HOLDOUT_DOMAIN,
};
use relearn_score::{ContaminationEvidence, PromoteVerdict, ShuffleEvidence, SliceScores};
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

/// Training metadata the contamination gate reads.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ArtifactManifest {
    /// Item ids present in the submitted training metadata.
    pub train_item_ids: Vec<u32>,
    /// Image hashes present in the submitted training metadata.
    pub train_image_hashes: Vec<String>,
    /// Dataset ids present in the submitted training metadata.
    pub train_dataset_ids: Vec<String>,
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
    /// Declared training fingerprints.
    #[serde(default)]
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

/// Public description of the sealed holdout. Carries no item ids or prompts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HoldoutSeal {
    /// Slice id bound into the paired test.
    pub slice_id: String,
    /// Commitment pinned in `config/relearn-pin.toml`.
    pub commitment: String,
    /// Number of holdout items.
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
    /// State file could not be written.
    #[error("persist: {0}")]
    Persist(String),
    /// State file was present but could not be restored.
    #[error("restore: {0}")]
    Restore(String),
}

/// Persisted snapshot version. Bump when the on-disk shape changes.
const PERSIST_VERSION: u32 = 1;

/// In-memory store (v0). Postgres can replace this without changing the HTTP surface.
///
/// When opened with a state file, submissions, evaluation results, and the
/// champion are restored before serve and rewritten after each mutation.
/// Holdout records stay out of the file — they come from the operator holdout
/// path and must not leak through a copied state file.
#[derive(Clone, Default)]
pub struct MemoryStore {
    inner: Arc<Mutex<Inner>>,
    persist_path: Option<PathBuf>,
}

#[derive(Default)]
struct Inner {
    next: u64,
    submissions: BTreeMap<String, Submission>,
    champion_id: Option<String>,
    scores: BTreeMap<String, SliceScores>,
    champion_scores: Option<SliceScores>,
    base_champion: Option<SliceScores>,
    holdout: Option<Vec<HoldoutItem>>,
    holdout_commitment: String,
    holdout_size: usize,
}

#[derive(Serialize, Deserialize)]
struct PersistSnapshot {
    version: u32,
    next: u64,
    submissions: BTreeMap<String, Submission>,
    scores: BTreeMap<String, PersistSlice>,
    champion_id: Option<String>,
    champion_scores: Option<PersistSlice>,
    base_champion: Option<PersistSlice>,
}

#[derive(Serialize, Deserialize)]
struct PersistSlice {
    holdout: BTreeMap<String, f64>,
    public: BTreeMap<String, f64>,
    perturbed: BTreeMap<String, f64>,
    canaries: BTreeMap<String, f64>,
    general_canary: BTreeMap<String, f64>,
    agent_trace: f64,
    vision_shuffle: BTreeMap<HoldoutTask, ShuffleEvidence>,
    contamination: ContaminationEvidence,
}

impl From<&SliceScores> for PersistSlice {
    fn from(s: &SliceScores) -> Self {
        Self {
            holdout: s.holdout.by_cluster.clone(),
            public: s.public.by_cluster.clone(),
            perturbed: s.perturbed.by_cluster.clone(),
            canaries: s.canaries.by_cluster.clone(),
            general_canary: s.general_canary.by_cluster.clone(),
            agent_trace: s.agent_trace,
            vision_shuffle: s.vision_shuffle.clone(),
            contamination: s.contamination.clone(),
        }
    }
}

impl From<PersistSlice> for SliceScores {
    fn from(s: PersistSlice) -> Self {
        Self {
            holdout: ExampleSeries {
                by_cluster: s.holdout,
            },
            public: ExampleSeries {
                by_cluster: s.public,
            },
            perturbed: ExampleSeries {
                by_cluster: s.perturbed,
            },
            canaries: ExampleSeries {
                by_cluster: s.canaries,
            },
            general_canary: ExampleSeries {
                by_cluster: s.general_canary,
            },
            agent_trace: s.agent_trace,
            vision_shuffle: s.vision_shuffle,
            contamination: s.contamination,
        }
    }
}

impl MemoryStore {
    /// Empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Open a store, restoring from `path` when the file exists.
    ///
    /// A missing file is a first boot (empty store that will persist there).
    /// A present but corrupt file is a hard error — never silently empty-score.
    ///
    /// # Errors
    ///
    /// [`StoreError::Restore`] when the file exists but cannot be decoded.
    pub fn open(path: Option<&Path>) -> Result<Self, StoreError> {
        match path {
            None => Ok(Self::new()),
            Some(p) if p.exists() => Self::restore_from(p),
            Some(p) => Ok(Self {
                persist_path: Some(p.to_path_buf()),
                inner: Arc::new(Mutex::new(Inner::default())),
            }),
        }
    }

    /// Restore submissions, evaluation results, and champion from `path`.
    ///
    /// # Errors
    ///
    /// [`StoreError::Restore`] on I/O, parse, or version mismatch.
    pub fn restore_from(path: &Path) -> Result<Self, StoreError> {
        let body = std::fs::read_to_string(path)
            .map_err(|e| StoreError::Restore(format!("read {}: {e}", path.display())))?;
        let snap: PersistSnapshot = serde_json::from_str(&body)
            .map_err(|e| StoreError::Restore(format!("parse {}: {e}", path.display())))?;
        if snap.version != PERSIST_VERSION {
            return Err(StoreError::Restore(format!(
                "{}: unsupported persist version {}",
                path.display(),
                snap.version
            )));
        }
        let inner = Inner {
            next: snap.next,
            submissions: snap.submissions,
            champion_id: snap.champion_id,
            scores: snap
                .scores
                .into_iter()
                .map(|(k, v)| (k, SliceScores::from(v)))
                .collect(),
            champion_scores: snap.champion_scores.map(SliceScores::from),
            base_champion: snap.base_champion.map(SliceScores::from),
            holdout: None,
            holdout_commitment: String::new(),
            holdout_size: 0,
        };
        Ok(Self {
            persist_path: Some(path.to_path_buf()),
            inner: Arc::new(Mutex::new(inner)),
        })
    }

    fn persist(&self) -> Result<(), StoreError> {
        let Some(path) = &self.persist_path else {
            return Ok(());
        };
        let snap = {
            let g = self.lock()?;
            PersistSnapshot {
                version: PERSIST_VERSION,
                next: g.next,
                submissions: g.submissions.clone(),
                scores: g
                    .scores
                    .iter()
                    .map(|(k, v)| (k.clone(), v.into()))
                    .collect(),
                champion_id: g.champion_id.clone(),
                champion_scores: g.champion_scores.as_ref().map(PersistSlice::from),
                base_champion: g.base_champion.as_ref().map(PersistSlice::from),
            }
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| StoreError::Persist(format!("mkdir {}: {e}", parent.display())))?;
        }
        let tmp = path.with_extension("json.tmp");
        let body = serde_json::to_string(&snap)
            .map_err(|e| StoreError::Persist(format!("encode: {e}")))?;
        std::fs::write(&tmp, body)
            .map_err(|e| StoreError::Persist(format!("write {}: {e}", tmp.display())))?;
        std::fs::rename(&tmp, path).map_err(|e| {
            StoreError::Persist(format!(
                "rename {} -> {}: {e}",
                tmp.display(),
                path.display()
            ))
        })?;
        Ok(())
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Inner>, StoreError> {
        self.inner.lock().map_err(|_| StoreError::Poison)
    }

    /// Record the pin's holdout commitment before any records are loaded.
    pub fn set_holdout_commitment(&self, commitment: &str, size: usize) -> Result<(), StoreError> {
        let mut g = self.lock()?;
        g.holdout_commitment = commitment.trim().to_ascii_lowercase();
        g.holdout_size = size;
        Ok(())
    }

    /// Load operator-supplied holdout records, verified against the pin.
    ///
    /// Nothing is stored on failure: a host with a bad holdout file scores
    /// nothing rather than silently falling back to a reconstructable seed.
    pub fn load_holdout(
        &self,
        records: Vec<HoldoutItem>,
        public: &[HoldoutItem],
        public_ids: &[u32],
    ) -> Result<(), StoreError> {
        let (commitment, size) = {
            let g = self.lock()?;
            (g.holdout_commitment.clone(), g.holdout_size)
        };
        verify_holdout_items(&records, public, public_ids, &commitment, size)?;
        self.lock()?.holdout = Some(records);
        Ok(())
    }

    /// Public seal description (no ids, no prompts).
    pub fn holdout_seal(&self) -> Result<HoldoutSeal, StoreError> {
        let g = self.lock()?;
        Ok(HoldoutSeal {
            slice_id: relearn_score::HOLDOUT_SLICE_ID.to_owned(),
            commitment: g.holdout_commitment.clone(),
            size: g.holdout_size,
            loaded: g.holdout.is_some(),
        })
    }

    /// Holdout records, readable only after a submission digest is frozen.
    pub fn unseal_holdout(&self, frozen_digest: &str) -> Result<Vec<HoldoutItem>, StoreError> {
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
    pub fn insert(&self, mut row: Submission) -> Result<Submission, StoreError> {
        let mut g = self.lock()?;
        if row.id.is_empty() {
            let n = g.next;
            g.next = g.next.saturating_add(1);
            row.id = format!("rl_{n:016x}");
        }
        g.submissions.insert(row.id.clone(), row.clone());
        drop(g);
        self.persist()?;
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
        let out = row.clone();
        drop(g);
        self.persist()?;
        Ok(out)
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
        let out = g
            .submissions
            .get(id)
            .cloned()
            .ok_or_else(|| StoreError::NotFound(id.to_owned()))?;
        drop(g);
        self.persist()?;
        Ok(out)
    }

    /// Persist challenger slices so a later promote displaces vs this run.
    pub fn record_scores(&self, id: &str, scores: SliceScores) -> Result<(), StoreError> {
        self.lock()?.scores.insert(id.to_owned(), scores);
        self.persist()
    }

    /// Seed / replace the implicit base-model champion scores.
    pub fn set_base_champion(&self, scores: SliceScores) -> Result<(), StoreError> {
        self.lock()?.base_champion = Some(scores);
        self.persist()
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

/// Per-epoch slice id derived from the holdout commitment (never from a digest).
#[must_use]
pub fn holdout_slice_id(epoch: u64, commitment: &str) -> String {
    let mut h = Sha256::new();
    h.update(HOLDOUT_DOMAIN);
    h.update(epoch.to_le_bytes());
    h.update(commitment.as_bytes());
    format!(
        "relearn-holdout-{epoch}-{}",
        hex::encode(&h.finalize()[..4])
    )
}

#[cfg(test)]
mod tests {
    use prism_competition::ExampleSeries;
    use relearn_challenge_task::holdout_commitment;
    use relearn_score::SliceScores;

    use super::*;

    fn items() -> Vec<HoldoutItem> {
        (900..=903)
            .map(|id| HoldoutItem {
                id,
                prompt: format!("holdout prompt {id} with enough words to trigram"),
                dataset_id: "dev".into(),
                task: relearn_challenge_task::HoldoutTask::Text,
                image_hash: String::new(),
            })
            .collect()
    }

    fn seeded_store() -> MemoryStore {
        let st = MemoryStore::new();
        let recs = items();
        st.set_holdout_commitment(&holdout_commitment(&recs), recs.len())
            .expect("commit");
        st
    }

    fn slice(v: f64) -> SliceScores {
        SliceScores {
            holdout: ExampleSeries::from_pairs((0..8).map(|i| (format!("h{i}"), v))),
            public: ExampleSeries::from_pairs((0..8).map(|i| (format!("p{i}"), v))),
            perturbed: ExampleSeries::from_pairs((0..8).map(|i| (format!("x{i}"), v))),
            canaries: ExampleSeries::from_pairs((0..8).map(|i| (format!("c{i}"), 0.99))),
            general_canary: ExampleSeries::from_pairs((0..8).map(|i| (format!("g{i}"), 0.97))),
            agent_trace: 0.9,
            ..SliceScores::default()
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
        assert!(!st.holdout_seal().expect("seal").loaded);

        let mut tampered = items();
        tampered[0].prompt = "leaked".into();
        assert!(st.load_holdout(tampered, &[], &[]).is_err());
        assert!(!st.holdout_seal().expect("seal").loaded);

        st.load_holdout(items(), &[], &[]).expect("verified load");
        assert!(st.holdout_seal().expect("seal").loaded);
    }

    #[test]
    fn seal_never_exposes_item_ids_or_prompts() {
        let st = seeded_store();
        st.load_holdout(items(), &[], &[]).expect("load");
        let seal = st.holdout_seal().expect("seal");
        let json = serde_json::to_string(&seal).expect("json");
        assert!(!json.contains("holdout prompt"));
        assert!(!json.contains("prompt"));
        assert!(json.contains("commitment"));
        assert_eq!(seal.size, 4);
        assert!(seal.loaded);
    }

    #[test]
    fn unseal_requires_a_frozen_digest_and_loaded_records() {
        let st = seeded_store();
        assert!(st.unseal_holdout("deadbeef").is_err());
        st.load_holdout(items(), &[], &[]).expect("load");
        assert!(st.unseal_holdout("").is_err());
        assert_eq!(st.unseal_holdout("deadbeef").expect("unseal").len(), 4);
    }

    #[test]
    fn promote_refuses_ineligible() {
        let st = MemoryStore::new();
        let row = st
            .insert(row(SubmissionState::AwaitingAdmin, None))
            .expect("insert");
        assert!(st.promote(&row.id).is_err());
    }

    #[test]
    fn champion_scores_follow_promote_not_base() {
        let st = MemoryStore::new();
        st.set_base_champion(slice(0.4)).expect("base");
        let row = st
            .insert(row(
                SubmissionState::AwaitingAdmin,
                Some(PromoteVerdict {
                    eligible: true,
                    paired: None,
                    failed: Vec::new(),
                    lattice: 12,
                }),
            ))
            .expect("insert");
        st.record_scores(&row.id, slice(0.8)).expect("scores");
        st.promote(&row.id).expect("promote");
        let got = st.champion_scores().expect("read").expect("some");
        assert!((SliceScores::mean(&got.holdout).unwrap_or(0.0) - 0.8).abs() < 1e-9);
    }

    #[test]
    fn slice_ids_track_epoch_and_commitment() {
        let a = holdout_slice_id(3, "aa");
        assert_eq!(a, holdout_slice_id(3, "aa"));
        assert_ne!(a, holdout_slice_id(4, "aa"));
        assert_ne!(a, holdout_slice_id(3, "bb"));
    }

    #[test]
    fn restart_restores_submissions_scores_and_champion() {
        let dir =
            std::env::temp_dir().join(format!("relearn-store-restart-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("state.json");

        let first = MemoryStore::open(Some(&path)).expect("first boot");
        first.set_base_champion(slice(0.4)).expect("base");
        let row = first
            .insert(row(
                SubmissionState::AwaitingAdmin,
                Some(PromoteVerdict {
                    eligible: true,
                    paired: None,
                    failed: Vec::new(),
                    lattice: 12,
                }),
            ))
            .expect("insert");
        first.record_scores(&row.id, slice(0.8)).expect("scores");
        first.promote(&row.id).expect("promote");

        let restarted = MemoryStore::open(Some(&path)).expect("restore");
        let got = restarted.get(&row.id).expect("row survived");
        assert_eq!(got.state, SubmissionState::Champion);
        assert_eq!(restarted.champion_id().expect("id"), Some(row.id));
        let champ = restarted.champion_scores().expect("read").expect("some");
        assert!((SliceScores::mean(&champ.holdout).unwrap_or(0.0) - 0.8).abs() < 1e-9);
        assert!(
            !restarted.holdout_seal().expect("seal").loaded,
            "holdout records must not ride along in the state file"
        );

        std::fs::write(&path, "{not-json").expect("corrupt");
        let Err(err) = MemoryStore::open(Some(&path)) else {
            panic!("corrupt restore must fail closed")
        };
        assert!(
            err.to_string().contains("restore"),
            "corrupt restore must fail closed: {err}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
