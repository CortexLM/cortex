//! In-memory Relearn Agent store: submissions, sealed episodes, champion.
//!
//! The episodes are the secret here, not a seed: once a miner knows which
//! goals and environments are scored, the holdout stops measuring whether the
//! agent can act in an environment it has not seen. So the set is loaded once
//! from an operator file, verified against the commitment in
//! `config/relearn-agent-pin.toml`, and is only readable after a submission
//! digest has been frozen. The public view carries the commitment and the
//! size, never ids, goals, or observation hashes.

#![forbid(unsafe_code)]
#![allow(
    clippy::missing_errors_doc,
    clippy::doc_markdown,
    clippy::must_use_candidate
)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use relearn_agent_score::{AgentSliceScores, PromoteVerdict};
use relearn_agent_task::{
    verify_episodes, AgentEpisode, EpisodeError, HOLDOUT_DOMAIN, HOLDOUT_SLICE_ID,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Submission lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubmissionState {
    /// Eval finished; waiting operator audit.
    AwaitingAdmin,
    /// Rejected (regression / gates / integrity).
    Rejected,
    /// Operator-promoted champion.
    Champion,
}

/// Training metadata the contamination gate reads.
///
/// Declaring something is what lets the gate run at all: a manifest that
/// declares nothing fails closed rather than passing.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ArtifactManifest {
    /// Episode ids present in the submitted training metadata.
    pub train_episode_ids: Vec<u32>,
    /// Observation hashes present in the submitted training metadata.
    pub train_observation_hashes: Vec<String>,
    /// Environment ids present in the submitted training metadata.
    pub train_environment_ids: Vec<String>,
}

/// One miner submission.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Submission {
    /// Stable id (`ag_` + 16 hex).
    pub id: String,
    /// 64-hex miner hotkey.
    pub miner_hotkey: String,
    /// SHA-256 hex of the miner artifact. Frozen at accept.
    pub artifact_digest: String,
    /// Optional locator (HF repo, object URL).
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

/// Public description of the sealed episode set. No ids, goals, or hashes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HoldoutSeal {
    /// Slice id bound into the paired test.
    pub slice_id: String,
    /// Commitment pinned in `config/relearn-agent-pin.toml`.
    pub commitment: String,
    /// Number of holdout episodes.
    pub size: usize,
    /// Whether the operator has loaded matching episodes on this host.
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
    /// Operator episode file did not match the committed digest.
    #[error("episodes: {0}")]
    Episodes(#[from] EpisodeError),
}

/// In-memory store (v0). Postgres can replace this without changing the HTTP
/// surface.
#[derive(Clone, Default)]
pub struct MemoryStore {
    inner: Arc<Mutex<Inner>>,
}

#[derive(Default)]
struct Inner {
    next: u64,
    submissions: BTreeMap<String, Submission>,
    champion_id: Option<String>,
    scores: BTreeMap<String, AgentSliceScores>,
    champion_scores: Option<AgentSliceScores>,
    base_champion: Option<AgentSliceScores>,
    episodes: Option<Vec<AgentEpisode>>,
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

    /// Record the pin's commitment before any episodes are loaded.
    pub fn set_holdout_commitment(&self, commitment: &str, size: usize) -> Result<(), StoreError> {
        let mut g = self.lock()?;
        g.holdout_commitment = commitment.trim().to_ascii_lowercase();
        g.holdout_size = size;
        Ok(())
    }

    /// Load operator-supplied episodes, verified against the pin.
    ///
    /// Nothing is stored on failure: a host with a bad episode file scores
    /// nothing rather than silently falling back to the published split.
    pub fn load_episodes(
        &self,
        episodes: Vec<AgentEpisode>,
        public: &[AgentEpisode],
        public_ids: &[u32],
    ) -> Result<(), StoreError> {
        let (commitment, size) = {
            let g = self.lock()?;
            (g.holdout_commitment.clone(), g.holdout_size)
        };
        verify_episodes(&episodes, public, public_ids, &commitment, size)?;
        self.lock()?.episodes = Some(episodes);
        Ok(())
    }

    /// Public seal description (no ids, no goals).
    pub fn holdout_seal(&self) -> Result<HoldoutSeal, StoreError> {
        let g = self.lock()?;
        Ok(HoldoutSeal {
            slice_id: HOLDOUT_SLICE_ID.to_owned(),
            commitment: g.holdout_commitment.clone(),
            size: g.holdout_size,
            loaded: g.episodes.is_some(),
        })
    }

    /// Episodes, readable only after a submission digest is frozen.
    pub fn unseal_episodes(&self, frozen_digest: &str) -> Result<Vec<AgentEpisode>, StoreError> {
        if frozen_digest.trim().is_empty() {
            return Err(StoreError::Illegal(
                "episodes stay sealed until the submission digest is frozen".into(),
            ));
        }
        let g = self.lock()?;
        g.episodes
            .clone()
            .ok_or_else(|| StoreError::Illegal("no verified episodes loaded".into()))
    }

    /// Insert a scored submission in its final state.
    pub fn insert(&self, mut row: Submission) -> Result<Submission, StoreError> {
        let mut g = self.lock()?;
        if row.id.is_empty() {
            let n = g.next;
            g.next = g.next.saturating_add(1);
            row.id = format!("ag_{n:016x}");
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
    pub fn record_scores(&self, id: &str, scores: AgentSliceScores) -> Result<(), StoreError> {
        self.lock()?.scores.insert(id.to_owned(), scores);
        Ok(())
    }

    /// Seed / replace the implicit base-model champion scores.
    pub fn set_base_champion(&self, scores: AgentSliceScores) -> Result<(), StoreError> {
        self.lock()?.base_champion = Some(scores);
        Ok(())
    }

    /// Champion slice scores (promoted miner, else the measured base).
    pub fn champion_scores(&self) -> Result<Option<AgentSliceScores>, StoreError> {
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
        "relearn-agent-holdout-{epoch}-{}",
        hex::encode(&h.finalize()[..4])
    )
}

#[cfg(test)]
mod tests {
    use prism_competition::ExampleSeries;
    use relearn_agent_task::{episode_commitment, ToolKind};

    use super::*;

    fn episode(id: u32) -> AgentEpisode {
        AgentEpisode {
            id,
            goal: format!("holdout episode {id} about a warehouse audit trail"),
            environment_id: "dev-env".into(),
            tools: vec![ToolKind::Inspect, ToolKind::Search],
            observation_hash: format!("{id:064x}"),
            answer_hash: format!("{:064x}", id + 500_000),
            min_tool_calls: 2,
        }
    }

    fn episodes() -> Vec<AgentEpisode> {
        (900..=903).map(episode).collect()
    }

    fn seeded() -> MemoryStore {
        let st = MemoryStore::new();
        let recs = episodes();
        st.set_holdout_commitment(&episode_commitment(&recs), recs.len())
            .expect("commit");
        st
    }

    fn slice(v: f64) -> AgentSliceScores {
        AgentSliceScores {
            holdout: ExampleSeries::from_pairs((0..8).map(|i| (format!("e{i}"), v))),
            ..AgentSliceScores::default()
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

    fn eligible_verdict() -> PromoteVerdict {
        PromoteVerdict {
            eligible: true,
            paired: None,
            tool_ablation: relearn_agent_score::AblationEvidence::default(),
            observation_shuffle: relearn_agent_score::AblationEvidence::default(),
            failed: Vec::new(),
            lattice: 12,
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
    fn episodes_load_only_when_they_match_the_commitment() {
        let st = seeded();
        assert!(!st.holdout_seal().expect("seal").loaded);

        let mut tampered = episodes();
        tampered[0].goal = "leaked".into();
        assert!(st.load_episodes(tampered, &[], &[1, 2]).is_err());
        assert!(!st.holdout_seal().expect("seal").loaded);

        st.load_episodes(episodes(), &[], &[1, 2])
            .expect("verified load");
        assert!(st.holdout_seal().expect("seal").loaded);
    }

    #[test]
    fn the_seal_never_exposes_episode_ids_or_goals() {
        let st = seeded();
        st.load_episodes(episodes(), &[], &[]).expect("load");
        let json = serde_json::to_string(&st.holdout_seal().expect("seal")).expect("json");
        for id in 900..=903 {
            assert!(!json.contains(&format!("{id}")), "seal leaked id {id}: {json}");
        }
        assert!(!json.contains("warehouse"));
    }

    #[test]
    fn unseal_requires_a_frozen_digest_and_loaded_episodes() {
        let st = seeded();
        assert!(st.unseal_episodes("deadbeef").is_err());
        st.load_episodes(episodes(), &[], &[]).expect("load");
        assert!(st.unseal_episodes("").is_err());
        assert!(st.unseal_episodes("   ").is_err());
        assert_eq!(st.unseal_episodes("deadbeef").expect("unseal").len(), 4);
    }

    #[test]
    fn promote_refuses_an_ineligible_verdict() {
        let st = MemoryStore::new();
        let r = st
            .insert(row(SubmissionState::AwaitingAdmin, None))
            .expect("insert");
        assert!(st.promote(&r.id).is_err());
    }

    #[test]
    fn champion_scores_follow_promote_not_the_base() {
        let st = MemoryStore::new();
        st.set_base_champion(slice(0.4)).expect("base");
        let r = st
            .insert(row(SubmissionState::AwaitingAdmin, Some(eligible_verdict())))
            .expect("insert");
        st.record_scores(&r.id, slice(0.8)).expect("scores");
        st.promote(&r.id).expect("promote");
        let got = st.champion_scores().expect("read").expect("some");
        assert!((AgentSliceScores::mean(&got.holdout).unwrap_or(0.0) - 0.8).abs() < 1e-9);
    }

    #[test]
    fn slice_ids_track_epoch_and_commitment() {
        let a = holdout_slice_id(3, "aa");
        assert_eq!(a, holdout_slice_id(3, "aa"));
        assert_ne!(a, holdout_slice_id(4, "aa"));
        assert_ne!(a, holdout_slice_id(3, "bb"));
    }
}
