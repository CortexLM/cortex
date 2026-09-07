use std::sync::Arc;

use async_trait::async_trait;
use chain_live::LiveChainClient;
use proof_rounds::{FinalizedRoundSource, FrozenRound, RoundLease};
use proof_runtime::{RuntimeOperations, RuntimeScope};
use tokio::sync::watch;
use uuid::Uuid;

use crate::AtlasError;

/// Trusted read-only chain input, deliberately without a tip fallback. Calls
/// run on the blocking pool; implementations must bound their RPC transport.
pub trait AtlasChain: FinalizedRoundSource + Send + Sync {
    /// # Errors
    /// Missing finality or unavailable RPC.
    fn finalized_height(&self) -> Result<u64, AtlasError>;
}
impl AtlasChain for LiveChainClient {
    fn finalized_height(&self) -> Result<u64, AtlasError> {
        LiveChainClient::finalized_height(self).map_err(|_| AtlasError::Unavailable)
    }
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AtlasRun {
    pub id: Uuid,
    pub binding: String,
    pub frozen_digest: String,
    pub deadline_ms: i64,
    pub controller_fence: i64,
    pub phase: String,
}

#[derive(Debug, Clone)]
pub struct AtlasJob {
    pub frozen: FrozenRound,
    pub lease: RoundLease,
    pub run: AtlasRun,
    pub scope: RuntimeScope,
    pub resume: bool,
}

/// Controller-owned adapter to the shared headless driver, not model output.
/// Mount operations only on private IPC. Resume the SAME id/checkpoint within
/// the original deadline; missing/uncertain recovery must fail closed, not
/// launch a fresh session. Stop and reap descendants before returning and on
/// future drop, even if cancellation arrives while spawning/resuming.
#[async_trait]
pub trait AtlasAgent: Send + Sync {
    /// # Errors
    /// Invalid immutable runtime configuration.
    fn binding(&self) -> Result<String, AtlasError>;
    fn maximum_seconds(&self) -> u32;
    /// Success is only process completion, NOT a decision or scheduling input.
    ///
    /// # Errors
    /// Interrupted ownership, unsafe recovery or runtime failure.
    async fn run(
        &self,
        job: &AtlasJob,
        operations: Arc<dyn RuntimeOperations>,
        stop: watch::Receiver<bool>,
    ) -> Result<(), AtlasError>;
}

#[derive(Debug, Clone, Copy)]
pub struct AtlasConfig {
    pub lease_seconds: u32,
    pub heartbeat_seconds: u32,
    pub retry_seconds: u32,
    pub stop_grace_seconds: u32,
}
impl AtlasConfig {
    /// # Errors
    /// Unbounded intervals or insufficient lease renewal margin.
    pub fn validate(self) -> Result<Self, AtlasError> {
        if !(10..=300).contains(&self.lease_seconds)
            || self.heartbeat_seconds == 0
            || self.heartbeat_seconds > self.lease_seconds / 3
            || !(1..=300).contains(&self.retry_seconds)
            || !(1..=30).contains(&self.stop_grace_seconds)
        {
            return Err(AtlasError::Invalid);
        }
        Ok(self)
    }
}
impl Default for AtlasConfig {
    fn default() -> Self {
        Self {
            lease_seconds: 30,
            heartbeat_seconds: 5,
            retry_seconds: 1,
            stop_grace_seconds: 10,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum AtlasProgress {
    Waiting { block: u64 },
    Busy { round: u64 },
    Blocked { round: u64 },
    Published { round: u64 },
}
