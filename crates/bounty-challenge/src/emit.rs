//! Fail-closed leaf emission from the CortexLM/backend public feed.
//!
//! Validators never read the bounty feed or evaluate reports. They verify
//! sealed bundles, so the only way a backend adjudication becomes weight is
//! this loop: fetch the public snapshot, derive `E` at a pinned block, sign an
//! exact-`E` leaf set, and `POST /v1/weights/raw`.
//!
//! Every tick that cannot read the feed emits **nothing**. That is the whole
//! point: an unmatched challenge share burns to uid 0, so silence costs
//! miners the epoch, while a stand-in score would pay them on numbers no
//! validator can check.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use chain::{gather_schedule_state, ChainClient};
use challenge_common::{
    emit_signed_leaf_set, expected_set_at_chain, submit_signed_leaf_set, GatewayClient,
    PinnedBlockHash,
};
use thiserror::Error;

use crate::backend::{fetch_public_snapshot, BackendError};
use crate::{emission_from_public_snapshot, CHALLENGE_ID_BYTES};

/// Default seconds between emitter ticks.
pub const DEFAULT_EMIT_POLL_SECS: u64 = 120;

/// Why a tick emitted nothing.
#[derive(Debug, Error)]
pub enum EmitError {
    /// The public feed could not be read (unset, unreachable, or malformed).
    #[error("backend public feed: {0}")]
    Backend(#[from] BackendError),
    /// Chain read failed (schedule, block hash, or metagraph).
    #[error("chain: {0}")]
    Chain(String),
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

/// One successful tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmitTick {
    /// Subnet epoch the leaves were signed for.
    pub epoch: u64,
    /// Block the expected set was pinned at.
    pub pin_block: u64,
    /// Size of `E` (every participant gets a leaf).
    pub participants: usize,
    /// Hotkeys that received a positive score this tick.
    pub paid: usize,
}

/// Emitter for one host: backend feed in, signed leaf set out.
pub struct BountyEmitter<C> {
    chain: C,
    gateway: Arc<GatewayClient>,
    sk: [u8; 32],
    netuid: u16,
    backend_base: Option<String>,
    emitted_epoch: AtomicU64,
}

impl<C: ChainClient + Send + Sync> BountyEmitter<C> {
    /// Build an emitter.
    ///
    /// `backend_base` is the operator-configured base URL; `None` falls back
    /// to `BOUNTY_BACKEND_PUBLIC_URL`, and an absent value there is a refusal
    /// rather than a skip.
    pub fn new(
        chain: C,
        gateway: Arc<GatewayClient>,
        sk: [u8; 32],
        netuid: u16,
        backend_base: Option<String>,
    ) -> Self {
        Self {
            chain,
            gateway,
            sk,
            netuid,
            backend_base: backend_base
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty()),
            emitted_epoch: AtomicU64::new(0),
        }
    }

    /// Highest epoch this process has emitted (0 = none yet).
    pub fn emitted_epoch(&self) -> u64 {
        self.emitted_epoch.load(Ordering::Relaxed)
    }

    /// Fetch the feed and submit one exact-`E` leaf set.
    ///
    /// The feed is read **before** any chain or gateway work, so a host that
    /// cannot score never signs a leaf — not even an all-`NoScore` set that
    /// would look like a verdict of "nobody found anything".
    ///
    /// # Errors
    /// See [`EmitError`]. Every variant means nothing was submitted.
    pub async fn tick(&self) -> Result<EmitTick, EmitError> {
        let snapshot = fetch_public_snapshot(self.backend_base.as_deref()).await?;
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
        let hotkeys: BTreeSet<[u8; 32]> = expected.hotkeys();
        let (_plan, leaf_scores) = emission_from_public_snapshot(&hotkeys, &snapshot);
        let paid = leaf_scores
            .values()
            .filter(|s| matches!(s, bundle::ScoreOrAbsence::Score { value } if *value > 0))
            .count();
        let signed =
            emit_signed_leaf_set(&self.sk, CHALLENGE_ID_BYTES, epoch, &hotkeys, &leaf_scores)
                .map_err(|e| EmitError::Leaf(e.to_string()))?;
        submit_signed_leaf_set(self.gateway.as_ref(), &signed)
            .await
            .map_err(|e| EmitError::Submit(e.to_string()))?;
        self.emitted_epoch.fetch_max(epoch, Ordering::Relaxed);
        Ok(EmitTick {
            epoch,
            pin_block,
            participants: hotkeys.len(),
            paid,
        })
    }

    /// Tick forever. A failed tick is logged and retried; it never falls back
    /// to a local verdict.
    pub async fn run(self: Arc<Self>, poll: Duration) {
        let poll = if poll.is_zero() {
            Duration::from_secs(DEFAULT_EMIT_POLL_SECS)
        } else {
            poll
        };
        loop {
            match self.tick().await {
                Ok(t) => tracing::info!(
                    epoch = t.epoch,
                    pin_block = t.pin_block,
                    participants = t.participants,
                    paid = t.paid,
                    "bounty leaf set submitted"
                ),
                Err(e) => tracing::warn!(
                    error = %e,
                    "bounty emitted nothing this tick; the challenge share burns to uid 0"
                ),
            }
            tokio::time::sleep(poll).await;
        }
    }
}
