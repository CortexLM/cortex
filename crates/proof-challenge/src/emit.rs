//! Fail-closed leaf emission from the in-process Proof store.
//!
//! Validators never evaluate topics. They verify sealed bundles, so the only
//! way a scored miner run becomes weight is this loop: pin `E` at the last
//! epoch block, derive lattices from the store, sign an exact-`E` leaf set,
//! and `POST /v1/weights/raw`.
//!
//! A tick that has no positive score pays **nobody**. It does not invent a
//! lattice, and it does not emit `NotAttempted` for the empty case — it
//! covers every participant in `E` with `NoScore(ChallengeInternal)`, the
//! code `BUNDLE_SPEC` §3.3.1 defines as "challenge-side fault; still must
//! cover the participant". That distinction is load-bearing in both
//! directions:
//!
//! - **It must not pay.** An all-`NoScore` set burns the challenge share to
//!   uid 0, which is the honest outcome when nothing was scored.
//! - **It must still cover `E`.** A paid challenge with no leaves fails D24
//!   completeness, so `POST /v1/admin/seal` answers 409 and the epoch seals
//!   for *no* challenge. Silence here would make an empty Proof store take
//!   down bounty's weights too.
//!
//! A failed tick also tries not to overwrite good leaves: once this host
//! has scored an epoch, a later empty tick inside that same epoch holds
//! rather than superseding a champion's score with a burn. The watermark is
//! persisted (`PROOF_SCORED_EPOCH_FILE`) so a restart with an empty store
//! still holds. Independently, the gateway refuses a `ChallengeInternal`
//! cover from replacing a positive leaf for the same key (409, original
//! kept), so a lost watermark cannot reseal a paid allocation into a uid-0
//! burn. The next successful tick still supersedes a *burn* with scores.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use bundle::{NoScoreReasonCode, ScoreOrAbsence};
use chain::{gather_schedule_state, ChainClient};
use challenge_common::{
    emit_signed_leaf_set, expected_set_at_chain, submit_signed_leaf_set, GatewayClient, Hotkey,
    PinnedBlockHash,
};
use proof_score::{MinerTopicRun, SealedBaseline};
use proof_store::MemoryStore;
use proof_task::{TopicDocument, CHALLENGE_ID_BYTES};
use thiserror::Error;

use crate::{emission_scores, parse_hotkey, store_runs};

/// Default seconds between emitter ticks.
pub const DEFAULT_EMIT_POLL_SECS: u64 = 120;

/// Why a tick that pays nobody is covering `E` with `ChallengeInternal`.
const NO_POSITIVE_REASON: &str = "no positive scores this tick";

/// Why a tick could not emit anything at all.
#[derive(Debug, Error)]
pub enum EmitError {
    /// Chain read failed (schedule, block hash, or metagraph).
    #[error("chain: {0}")]
    Chain(String),
    /// The in-process store could not be read.
    #[error("store: {0}")]
    Store(String),
    /// The subnet has not run an epoch yet.
    #[error("subnet epoch 0: nothing to emit against")]
    EpochZero,
    /// Leaf signing failed.
    #[error("leaf emit: {0}")]
    Leaf(String),
    /// Gateway rejected the leaf set.
    #[error("gateway submit: {0}")]
    Submit(String),
}

/// What one tick put on the gateway.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmitOutcome {
    /// The store held at least one positive lattice and it became scores.
    Scored {
        /// Subnet epoch the leaves were signed for.
        epoch: u64,
        /// Block the expected set was pinned at.
        pin_block: u64,
        /// Size of `E` (every participant gets a leaf).
        participants: usize,
        /// Hotkeys that received a positive score.
        paid: usize,
    },
    /// Nobody had a positive lattice, so `E` was covered with
    /// `NoScore(ChallengeInternal)`: nobody is paid, the share burns to uid 0,
    /// and the bundle can still seal.
    Burned {
        /// Subnet epoch the burn set was signed for.
        epoch: u64,
        /// Size of `E`.
        participants: usize,
        /// Why nothing was paid.
        reason: String,
    },
    /// Nobody had a positive lattice, but a scored set already stands for this
    /// epoch. Overwriting it with a burn would take back a score the store
    /// really did publish.
    Held {
        /// Epoch whose scored leaves were left in place.
        epoch: u64,
        /// Why this tick would have burned.
        reason: String,
    },
}

/// Emitter for one host: in-process store in, signed leaf set out.
pub struct ProofEmitter<C> {
    chain: C,
    gateway: Arc<GatewayClient>,
    sk: [u8; 32],
    netuid: u16,
    store: MemoryStore,
    scored_epoch: AtomicU64,
    scored_epoch_path: Option<PathBuf>,
}

impl<C: ChainClient + Send + Sync> ProofEmitter<C> {
    /// Build an emitter over a cloned [`MemoryStore`] (the HTTP state holds
    /// the other handle).
    pub fn new(
        chain: C,
        gateway: Arc<GatewayClient>,
        sk: [u8; 32],
        netuid: u16,
        store: MemoryStore,
    ) -> Self {
        Self {
            chain,
            gateway,
            sk,
            netuid,
            store,
            scored_epoch: AtomicU64::new(0),
            scored_epoch_path: None,
        }
    }

    /// Restore the scored-epoch watermark from `path` (0 if missing).
    ///
    /// Compose points this at the `proof-artifacts` volume so a restart does
    /// not forget that this epoch already has scored leaves on the gateway.
    #[must_use]
    pub fn with_scored_epoch_path(mut self, path: PathBuf) -> Self {
        let loaded = load_scored_epoch(&path);
        if loaded > 0 {
            tracing::info!(
                epoch = loaded,
                path = %path.display(),
                "proof scored-epoch watermark restored"
            );
        }
        self.scored_epoch.fetch_max(loaded, Ordering::Relaxed);
        self.scored_epoch_path = Some(path);
        self
    }

    /// Highest epoch this host scored from the store (0 = none yet).
    pub fn scored_epoch(&self) -> u64 {
        self.scored_epoch.load(Ordering::Relaxed)
    }

    /// Read the store and submit one exact-`E` leaf set.
    ///
    /// # Errors
    /// See [`EmitError`] — those are the failures that leave `E` uncovered.
    /// An empty or all-zero tick is not among them; it is an
    /// [`EmitOutcome::Burned`] (or [`EmitOutcome::Held`]) instead.
    pub async fn tick(&self) -> Result<EmitOutcome, EmitError> {
        let pinned = self.expected_set_at_last_epoch()?;
        let (epoch, pin_block, hotkeys) = pinned;
        let inputs = emission_inputs(&self.store, epoch)?;
        let leaf_scores = emission_scores(
            &hotkeys,
            &inputs.topics,
            &inputs.sealed,
            &inputs.champion_primary,
            &inputs.per_miner,
        );
        let paid = leaf_scores
            .values()
            .filter(|s| matches!(s, ScoreOrAbsence::Score { value } if *value > 0))
            .count();
        if paid == 0 {
            return self.cover_without_scores(epoch, &hotkeys).await;
        }
        self.submit(epoch, &hotkeys, &leaf_scores).await?;
        self.mark_scored(epoch);
        Ok(EmitOutcome::Scored {
            epoch,
            pin_block,
            participants: hotkeys.len(),
            paid,
        })
    }

    /// Tick forever. A failed tick is logged and retried; it never falls back
    /// to inventing a lattice.
    pub async fn run(self: Arc<Self>, poll: Duration) {
        let poll = if poll.is_zero() {
            Duration::from_secs(DEFAULT_EMIT_POLL_SECS)
        } else {
            poll
        };
        loop {
            match self.tick().await {
                Ok(EmitOutcome::Scored {
                    epoch,
                    pin_block,
                    participants,
                    paid,
                }) => tracing::info!(
                    epoch,
                    pin_block,
                    participants,
                    paid,
                    "proof leaf set submitted from the in-process store"
                ),
                Ok(EmitOutcome::Burned {
                    epoch,
                    participants,
                    reason,
                }) => tracing::warn!(
                    epoch,
                    participants,
                    %reason,
                    "proof had no positive scores: covered E with ChallengeInternal, \
                     so the challenge share burns to uid 0"
                ),
                Ok(EmitOutcome::Held { epoch, reason }) => tracing::warn!(
                    epoch,
                    %reason,
                    "proof had no positive scores; keeping this epoch's scored leaves"
                ),
                Err(e) => tracing::warn!(
                    error = %e,
                    "proof emitted nothing this tick; E is uncovered and seal will 409 until \
                     the next tick succeeds"
                ),
            }
            tokio::time::sleep(poll).await;
        }
    }

    /// `(epoch, pin_block, E)` at the last epoch boundary.
    fn expected_set_at_last_epoch(&self) -> Result<(u64, u64, BTreeSet<Hotkey>), EmitError> {
        let state = gather_schedule_state(&self.chain, self.netuid)
            .map_err(|e| EmitError::Chain(e.to_string()))?;
        let epoch = state.subnet_epoch_index;
        if epoch == 0 {
            return Err(EmitError::EpochZero);
        }
        let pin_block = state.last_epoch_block;
        let block_hash = self
            .chain
            .block_hash(pin_block)
            .map_err(|e| EmitError::Chain(format!("block_hash@{pin_block}: {e}")))?;
        let expected = expected_set_at_chain(
            &trustroot::ParticipantPolicy::AllMetagraphHotkeys,
            PinnedBlockHash::new(block_hash),
            &self.chain,
        )
        .map_err(|e| EmitError::Chain(format!("expected set: {e}")))?;
        Ok((epoch, pin_block, expected.hotkeys()))
    }

    fn mark_scored(&self, epoch: u64) {
        self.scored_epoch.fetch_max(epoch, Ordering::Relaxed);
        if let Some(path) = self.scored_epoch_path.as_ref() {
            persist_scored_epoch(path, self.scored_epoch());
        }
    }

    /// Cover `E` when nobody scored: burn, or hold an already-scored epoch.
    async fn cover_without_scores(
        &self,
        epoch: u64,
        hotkeys: &BTreeSet<Hotkey>,
    ) -> Result<EmitOutcome, EmitError> {
        let reason = NO_POSITIVE_REASON.to_owned();
        if self.scored_epoch() >= epoch {
            return Ok(EmitOutcome::Held { epoch, reason });
        }
        let burn: BTreeMap<Hotkey, ScoreOrAbsence> = hotkeys
            .iter()
            .map(|h| {
                (
                    *h,
                    ScoreOrAbsence::NoScore {
                        reason: NoScoreReasonCode::ChallengeInternal,
                    },
                )
            })
            .collect();
        self.submit(epoch, hotkeys, &burn).await?;
        Ok(EmitOutcome::Burned {
            epoch,
            participants: hotkeys.len(),
            reason,
        })
    }

    async fn submit(
        &self,
        epoch: u64,
        hotkeys: &BTreeSet<Hotkey>,
        scores: &BTreeMap<Hotkey, ScoreOrAbsence>,
    ) -> Result<(), EmitError> {
        let signed = emit_signed_leaf_set(&self.sk, CHALLENGE_ID_BYTES, epoch, hotkeys, scores)
            .map_err(|e| EmitError::Leaf(e.to_string()))?;
        submit_signed_leaf_set(self.gateway.as_ref(), &signed)
            .await
            .map_err(|e| EmitError::Submit(e.to_string()))?;
        Ok(())
    }
}

/// Open topics + baselines + champion primaries + miner runs at `epoch`.
struct EmissionInputs {
    topics: Vec<TopicDocument>,
    sealed: BTreeMap<String, SealedBaseline>,
    champion_primary: BTreeMap<String, f64>,
    per_miner: BTreeMap<Hotkey, BTreeMap<String, MinerTopicRun>>,
}

fn emission_inputs(store: &MemoryStore, epoch: u64) -> Result<EmissionInputs, EmitError> {
    let topics = store
        .topics()
        .map_err(|e| EmitError::Store(e.to_string()))?
        .into_iter()
        .filter(|t| t.is_open_at(epoch))
        .collect::<Vec<_>>();
    let mut sealed = BTreeMap::new();
    let mut champion_primary = BTreeMap::new();
    for t in &topics {
        if let Some(b) = store
            .baseline(&t.id)
            .map_err(|e| EmitError::Store(e.to_string()))?
        {
            sealed.insert(t.id.clone(), b);
        }
        if let Some(p) = store
            .champion_primary(t)
            .map_err(|e| EmitError::Store(e.to_string()))?
        {
            champion_primary.insert(t.id.clone(), p);
        }
    }
    let mut per_miner = BTreeMap::new();
    for (hex, runs) in store_runs(store) {
        if let Some(hk) = parse_hotkey(&hex) {
            per_miner.insert(hk, runs);
        }
    }
    Ok(EmissionInputs {
        topics,
        sealed,
        champion_primary,
        per_miner,
    })
}

fn load_scored_epoch(path: &Path) -> u64 {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|body| body.trim().parse().ok())
        .unwrap_or(0)
}

fn persist_scored_epoch(path: &Path, epoch: u64) {
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "proof scored-epoch watermark: parent dir"
            );
            return;
        }
    }
    let tmp = path.with_extension("tmp");
    if let Err(e) = std::fs::write(&tmp, format!("{epoch}\n")) {
        tracing::warn!(
            path = %tmp.display(),
            error = %e,
            "proof scored-epoch watermark: write"
        );
        return;
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        tracing::warn!(
            path = %path.display(),
            error = %e,
            "proof scored-epoch watermark: persist"
        );
    }
}
