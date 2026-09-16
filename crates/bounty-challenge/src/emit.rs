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
//!   take down proof's weights too.
//!
//! Reading the feed and finding nothing payable is a *different* failure from
//! not reading it, and it is treated as one. A feed that answers with zero
//! adjudicated rows is reachable, but it still produces no weight, and the
//! honest leaf for that epoch is the same `ChallengeInternal` cover. This case
//! has to be separated out because treating it as a "score" emitted
//! `NotAttempted` for every participant, which is a claim that the challenge
//! *chose* not to invoke them. That claim is false, and it is the worse of the
//! two: `NotAttempted` is not the burn cover, so it seals as a
//! legitimate-looking unpaid epoch while `/v1/status` reports a successful
//! score. An operator has to be able to tell "backend is down" from "backend
//! is up and has crowned nobody".
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

use bounty_challenge_task::{EmitterOutcomeKind, EmitterStatus, EmitterTick};
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

/// Why a readable feed still paid nobody this tick.
const NO_PAYABLE_ROWS_REASON: &str =
    "backend public feed published no payable adjudication (nobody crowned)";

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
    /// The feed answered but nothing payable became weight, so `E` was covered
    /// with `NoScore(ChallengeInternal)`: nobody is paid, the share burns to
    /// uid 0, and the bundle can still seal.
    ///
    /// Distinct from [`Self::Burned`] because the cause is not an outage: the
    /// feed is up and still has no crowned hotkey.
    Unpaid {
        /// Subnet epoch the cover was signed for.
        epoch: u64,
        /// Size of `E`.
        participants: usize,
        /// Why nobody was paid.
        reason: String,
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
    /// The feed produced no weight, but a scored set already stands for this
    /// epoch. Overwriting it with a cover would take back a score the backend
    /// really did publish — whether the feed went down or stayed up and
    /// stopped publishing the crowned hotkey.
    Held {
        /// Epoch whose scored leaves were left in place.
        epoch: u64,
        /// Why this tick would have covered the epoch.
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
    status: Arc<EmitterStatus>,
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
            status: Arc::new(EmitterStatus::new(true)),
        }
    }

    /// Share the read-side status published on `GET /v1/status`.
    #[must_use]
    pub fn with_status(mut self, status: Arc<EmitterStatus>) -> Self {
        self.status = status;
        self
    }

    /// Read-side status handle (shared with the HTTP state).
    #[must_use]
    pub fn status(&self) -> Arc<EmitterStatus> {
        Arc::clone(&self.status)
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
    /// [`EmitOutcome::Burned`] (or [`EmitOutcome::Held`]) instead. A feed that
    /// answers without payable rows is [`EmitOutcome::Unpaid`].
    pub async fn tick(&self) -> Result<EmitOutcome, EmitError> {
        // Whether the feed answered is tracked here rather than inferred from
        // the outcome: a tick that read the feed and then failed at the
        // gateway is an error, and reporting `last_feed_read: false` for it
        // would send an operator to the backend for a fault that is not there.
        let mut feed_read = false;
        let outcome = self.tick_inner(&mut feed_read).await;
        self.record(&outcome, feed_read);
        outcome
    }

    /// One tick, before its outcome is published to [`EmitterStatus`].
    async fn tick_inner(&self, feed_read: &mut bool) -> Result<EmitOutcome, EmitError> {
        let feed = fetch_public_snapshot(self.backend_base.as_deref()).await;
        *feed_read = feed.is_ok();
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
        // A readable feed that crowns nobody must not be signed as a scored
        // epoch. `NotAttempted` claims the challenge chose not to invoke the
        // miner; nobody was crowned, so the honest cover is
        // `ChallengeInternal` and the share burns.
        if paid == 0 {
            return self
                .cover_without_a_feed(
                    epoch,
                    &hotkeys,
                    &BackendError::NoPayableRows(NO_PAYABLE_ROWS_REASON.to_owned()),
                )
                .await;
        }
        self.submit(epoch, &hotkeys, &leaf_scores).await?;
        self.scored_epoch.fetch_max(epoch, Ordering::Relaxed);
        Ok(EmitOutcome::Scored {
            epoch,
            pin_block,
            participants: hotkeys.len(),
            paid,
        })
    }

    /// Publish one completed tick to `/v1/status`.
    ///
    /// `feed_read` is the tick's own record of whether the feed answered, not
    /// something inferred from the outcome: an error after a successful read
    /// is a gateway problem, and saying otherwise would point an operator at
    /// the wrong service.
    fn record(&self, outcome: &Result<EmitOutcome, EmitError>, feed_read: bool) {
        let scored_epoch = self.scored_epoch();
        let tick = match outcome {
            Ok(EmitOutcome::Scored {
                epoch,
                pin_block,
                participants,
                paid,
            }) => EmitterTick {
                kind: EmitterOutcomeKind::Scored,
                epoch: *epoch,
                pin_block: *pin_block,
                participants: *participants,
                paid: *paid,
                feed_read,
                scored_epoch,
                reason: None,
                error: None,
            },
            Ok(EmitOutcome::Unpaid {
                epoch,
                participants,
                reason,
            }) => EmitterTick {
                kind: EmitterOutcomeKind::Unpaid,
                epoch: *epoch,
                pin_block: 0,
                participants: *participants,
                paid: 0,
                feed_read,
                scored_epoch,
                reason: Some(reason),
                error: None,
            },
            Ok(EmitOutcome::Burned {
                epoch,
                participants,
                reason,
            }) => EmitterTick {
                kind: EmitterOutcomeKind::Burned,
                epoch: *epoch,
                pin_block: 0,
                participants: *participants,
                paid: 0,
                feed_read,
                scored_epoch,
                reason: Some(reason),
                error: None,
            },
            Ok(EmitOutcome::Held { epoch, reason, .. }) => EmitterTick {
                kind: EmitterOutcomeKind::Held,
                epoch: *epoch,
                pin_block: 0,
                participants: 0,
                paid: 0,
                feed_read,
                scored_epoch,
                reason: Some(reason),
                error: None,
            },
            Err(e) => EmitterTick {
                kind: EmitterOutcomeKind::Error,
                epoch: 0,
                pin_block: 0,
                participants: 0,
                paid: 0,
                feed_read,
                scored_epoch,
                reason: None,
                error: Some(&e.to_string()),
            },
        };
        self.status.record(tick);
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
                Ok(EmitOutcome::Unpaid {
                    epoch,
                    participants,
                    reason,
                }) => tracing::warn!(
                    epoch,
                    participants,
                    %reason,
                    "bounty read the feed and found nothing payable: covered E with \
                     ChallengeInternal, so the challenge share burns to uid 0"
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
                    "bounty produced no weight this tick; keeping this epoch's scored leaves"
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

    /// Cover `E` when no weight could be produced: burn, or hold an
    /// already-scored epoch.
    ///
    /// `cause` is either an unreadable feed or [`BackendError::NoPayableRows`]
    /// (the feed answered and crowned nobody). Both pay nobody, but only the
    /// outage is reported as [`EmitOutcome::Burned`] — an operator reading
    /// `/v1/status` has to be able to tell "the backend is down" from "the
    /// backend is up and there is nothing to pay".
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
        let participants = hotkeys.len();
        if matches!(cause, BackendError::NoPayableRows(_)) {
            return Ok(EmitOutcome::Unpaid {
                epoch,
                participants,
                reason,
            });
        }
        Ok(EmitOutcome::Burned {
            epoch,
            participants,
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
