use async_trait::async_trait;
use proof_autonomy::MachineQuote;
use proof_autonomy_pg::{ControllerLease, Experiment, Resource};
use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use uuid::Uuid;

use crate::WorkerError;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct RuntimeRun {
    pub experiment_id: Uuid,
    pub id: Uuid,
    pub binding: String,
    pub deadline_ms: i64,
    pub phase: String,
    pub controller_fence: i64,
}

#[derive(Debug, Clone)]
pub struct RuntimeJob {
    pub run: RuntimeRun,
    pub lease: ControllerLease,
    pub resource: Resource,
    pub resume: bool,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct WorkCandidate {
    pub id: Uuid,
    pub cleanup: bool,
}

/// Controller-owned catalogue/preflight, not an unsigned miner hardware choice.
#[async_trait]
pub trait QuoteSource: Send + Sync {
    /// # Errors
    /// Unavailable catalogue or unsupported exact provider guarantees.
    async fn quote(&self, experiment: &Experiment) -> Result<Option<MachineQuote>, WorkerError>;
}

/// Must supervise the complete isolated runtime, stop descendants on signal,
/// preserve the original budget on resume, and never fabricate evidence.
#[async_trait]
pub trait ExperimentAgent: Send + Sync {
    /// # Errors
    /// Invalid immutable runtime configuration.
    fn binding(&self) -> Result<String, WorkerError>;
    fn maximum_seconds(&self) -> u32;
    /// Return only after stopping descendants. Interruption preserves recovery
    /// state and is not recorded as a completed or failed scientific execution.
    ///
    /// # Errors
    /// Interrupted ownership/shutdown, invalid recovery or runtime failure.
    async fn run(&self, job: &RuntimeJob, stop: watch::Receiver<bool>) -> Result<(), WorkerError>;
}

#[derive(Debug, Clone, Copy)]
pub struct WorkerConfig {
    pub lease_seconds: u32,
    pub heartbeat_seconds: u32,
    pub retry_seconds: u32,
    pub stop_grace_seconds: u32,
    pub concurrent_experiments: usize,
    pub concurrent_cleanup: usize,
}

impl WorkerConfig {
    /// # Errors
    /// Unbounded work or heartbeat too slow to preserve ownership.
    pub fn validate(self) -> Result<Self, WorkerError> {
        if !(10..=300).contains(&self.lease_seconds)
            || self.heartbeat_seconds == 0
            || self.heartbeat_seconds > self.lease_seconds / 3
            || !(1..=300).contains(&self.retry_seconds)
            || !(1..=30).contains(&self.stop_grace_seconds)
            || !(1..=16).contains(&self.concurrent_experiments)
            || !(1..=8).contains(&self.concurrent_cleanup)
        {
            return Err(WorkerError::Invalid);
        }
        Ok(self)
    }
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            lease_seconds: 30,
            heartbeat_seconds: 5,
            retry_seconds: 10,
            stop_grace_seconds: 10,
            concurrent_experiments: 4,
            concurrent_cleanup: 2,
        }
    }
}
