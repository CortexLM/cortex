//! Fail-closed leaf emission from the CortexLM/backend public feed.
//!
//! Validators never read the bounty feed or evaluate reports. They verify
//! sealed bundles, so the only way a backend adjudication becomes weight is
//! this loop: fetch the public snapshot, derive `E` at a pinned block, sign an
//! exact-`E` leaf set, and `POST /v1/weights/raw`.
//!
//! A tick that cannot read the feed pays **nobody**. It does not invent a
//! score, and it does not fall back to a local scorer — but it still covers
//! every participant in `E` with `NoScore(ChallengeInternal)`, the code
//! `BUNDLE_SPEC` §3.3.1 defines as "challenge-side fault; still must cover the
//! participant". That distinction is load-bearing in both directions:
//!
//! - **It must not pay.** An all-`NoScore` set burns the challenge share to
//!   uid 0, which is the honest outcome when nothing was adjudicated.
//! - **It must still cover `E`.** A paid challenge with no leaves fails D24
//!   completeness, so `POST /v1/admin/seal` answers 409 and the epoch seals
//!   for *no* challenge. Silence here would make an unconfigured bounty host
//!   take down relearn's weights too.
//!
//! A failed tick also tries not to overwrite good leaves: once *this process*
//! has scored an epoch, a later feed outage inside that same epoch holds
//! rather than superseding a champion's score with a burn. That watermark is
//! in-process, so a restart inside an outage can still burn an epoch that had
//! scores (the gateway exposes no read side for raw leaves to consult). The
//! next successful tick supersedes the burn with the published scores, and the
//! bias is deliberate: erring toward a burn pays nobody who was not already
//! paid, while erring toward silence would 409 the seal for every challenge.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use bundle::{NoScoreReasonCode, ScoreOrAbsence};
use chain::{gather_schedule_state, ChainClient};
use challenge_common::{
    emit_signed_leaf_set, expected_set_at_chain, submit_signed_leaf_set, GatewayClient, Hotkey,
    PinnedBlockHash,
};
use thiserror::Error;

use crate::backend::{fetch_public_snapshot, BackendError};
use crate::{emission_from_public_snapshot, CHALLENGE_ID_BYTES};

/// Default seconds between emitter ticks.
pub const DEFAULT_EMIT_POLL_SECS: u64 = 120;

/// Why a tick could not emit anything at all.
#[derive(Debug, Error)]
pub enum EmitError {
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

/// What one tick put on the gateway.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmitOutcome {
    /// The feed was read and published rows became scores.
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
    /// The feed could not be read, so `E` was covered with
    /// `NoScore(ChallengeInternal)`: nobody is paid, the share burns to uid 0,
    /// and the bundle can still seal.
    Burned {
        /// Subnet epoch the burn set was signed for.
        epoch: u64,
        /// Size of `E`.
        participants: usize,
        /// Why the feed was unreadable.
        reason: String,
    },
    /// The feed could not be read, but a scored set already stands for this
    /// epoch. Overwriting it with a burn would take back a score the backend
    /// really did publish.
    Held {
        /// Epoch whose scored leaves were left in place.
        epoch: u64,
        /// Why the feed was unreadable.
        reason: String,
    },
}

/// Emitter for one host: backend feed in, signed leaf set out.
pub struct BountyEmitter<C> {
    chain: C,
    gateway: Arc<GatewayClient>,
    sk: [u8; 32],
    netuid: u16,
    backend_base: Option<String>,
    scored_epoch: AtomicU64,
}

impl<C: ChainClient + Send + Sync> BountyEmitter<C> {
    /// Build an emitter.
    ///
    /// `backend_base` is the operator-configured base URL; `None` falls back
    /// to `BOUNTY_BACKEND_PUBLIC_URL`, and an absent value there pays nobody
    /// rather than selecting some other scorer.
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
            scored_epoch: AtomicU64::new(0),
        }
    }

    /// Highest epoch this process scored from the feed (0 = none yet).
    pub fn scored_epoch(&self) -> u64 {
        self.scored_epoch.load(Ordering::Relaxed)
    }

    /// Read the feed and submit one exact-`E` leaf set.
    ///
    /// # Errors
    /// See [`EmitError`] — those are the failures that leave `E` uncovered.
    /// A missing or broken feed is not among them; it is an
    /// [`EmitOutcome::Burned`] (or [`EmitOutcome::Held`]) instead.
    pub async fn tick(&self) -> Result<EmitOutcome, EmitError> {
        let feed = fetch_public_snapshot(self.backend_base.as_deref()).await;
        let pinned = self.expected_set_at_last_epoch()?;
        let (epoch, pin_block, hotkeys) = pinned;
        let snapshot = match feed {
            Ok(s) => s,
            Err(e) => return self.cover_without_a_feed(epoch, &hotkeys, &e).await,
        };
        let (_plan, leaf_scores) = emission_from_public_snapshot(&hotkeys, &snapshot);
        let paid = leaf_scores
            .values()
            .filter(|s| matches!(s, ScoreOrAbsence::Score { value } if *value > 0))
            .count();
        self.submit(epoch, &hotkeys, &leaf_scores).await?;
        self.scored_epoch.fetch_max(epoch, Ordering::Relaxed);
        Ok(EmitOutcome::Scored {
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
                    "bounty leaf set submitted from the backend public feed"
                ),
                Ok(EmitOutcome::Burned {
                    epoch,
                    participants,
                    reason,
                }) => tracing::warn!(
                    epoch,
                    participants,
                    %reason,
                    "bounty could not read the feed: covered E with ChallengeInternal, \
                     so the challenge share burns to uid 0"
                ),
                Ok(EmitOutcome::Held { epoch, reason }) => tracing::warn!(
                    epoch,
                    %reason,
                    "bounty could not read the feed; keeping this epoch's scored leaves"
                ),
                Err(e) => tracing::warn!(
                    error = %e,
                    "bounty emitted nothing this tick; E is uncovered and seal will 409 until \
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

    /// Cover `E` when the feed is unreadable: burn, or hold an already-scored
    /// epoch.
    async fn cover_without_a_feed(
        &self,
        epoch: u64,
        hotkeys: &BTreeSet<Hotkey>,
        cause: &BackendError,
    ) -> Result<EmitOutcome, EmitError> {
        let reason = cause.to_string();
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
