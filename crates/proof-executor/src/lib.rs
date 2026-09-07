//! Concrete, CPU-only execution with durable intents and bounded Docker targets.
//!
//! Scientific collection runs committed Python scripts with the original seed
//! in `PROOF_SEED` and argv[1]. This is not a general training harness. Script
//! stdout, including JSON metrics, remains untrusted. After each collect run the
//! controller captures `/work/artifact` from the stopped sandbox and hands its
//! bytes to the configured `proof_measure::MeasurementObserver`; only that
//! observer's metrics and positive FLOP count become a `Measurement`. With the
//! default `NoObserver`, or any run the observer refuses, collection retains
//! the failure and never manufactures `ScientificEvidence`. Contamination hits
//! come from `proof_task::contamination` over the committed script text against
//! the verified holdout; no declared training manifest exists on this path.
//! No host-simulation or remote-provider fallback exists.

#![forbid(unsafe_code)]

mod chain;
mod docker;
mod execution;
mod store;

pub use chain::*;
pub use docker::{DockerSandbox, RunObservation, ARTIFACT_DIR};
pub use store::Intent;

use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use proof_autonomy::{commitment, CapabilityOperation, ResourceGrant};
use proof_autonomy_pg::ControllerLease;
use proof_measure::{MeasurementObserver, NoObserver};
use proof_research::{ResearchStore, RetainedArtifacts, ScientificEvidence, ScientificRecipe};
use proof_runtime::{ExecutionRequest, ExperimentExecutor, RuntimeError};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgPool;
use tokio::sync::watch;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "snake_case")]
pub enum Failure {
    #[error("execution input is unsupported or unbounded")]
    Input,
    #[error("stored authorization is absent, expired or fenced")]
    Authorization,
    #[error("exact committed script is unavailable")]
    MissingScript,
    #[error("commitment mismatch")]
    Commitment,
    #[error("trusted finalized chain observation unavailable")]
    Chain,
    #[error("isolated execution target unavailable or mismatched")]
    Target,
    #[error("execution was interrupted or its deadline expired")]
    Interrupted,
    #[error("execution or required dependency failed")]
    Execution,
    #[error("bounded log or artifact limit exceeded")]
    LogLimit,
    #[error("target stop is not confirmed")]
    StopUnconfirmed,
    #[error("operation already dispatched; reconcile without rerunning")]
    Reconcile,
    #[error("independent FLOP and scientific metric observations unavailable")]
    UnobservedMeasurements,
    #[error("durable execution journal unavailable")]
    Database,
}

impl From<sqlx::Error> for Failure {
    fn from(_: sqlx::Error) -> Self {
        Self::Database
    }
}
impl From<proof_measure::ObserverError> for Failure {
    fn from(value: proof_measure::ObserverError) -> Self {
        use proof_measure::ObserverError as E;
        match value {
            E::LogLimit => Self::LogLimit,
            E::Deadline => Self::Interrupted,
            _ => Self::UnobservedMeasurements,
        }
    }
}
impl From<proof_autonomy_pg::StoreError> for Failure {
    fn from(_: proof_autonomy_pg::StoreError) -> Self {
        Self::Authorization
    }
}
impl From<Failure> for RuntimeError {
    fn from(value: Failure) -> Self {
        match value {
            Failure::Input | Failure::Authorization | Failure::Commitment => Self::Scope,
            _ => Self::Unavailable,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Invocation {
    Terminal {
        argv: Vec<String>,
        timeout_ms: u64,
    },
    Script {
        script_digest: String,
        seed: Option<u64>,
        timeout_ms: u64,
    },
}
impl Invocation {
    fn timeout_ms(&self) -> u64 {
        match self {
            Self::Terminal { timeout_ms, .. } | Self::Script { timeout_ms, .. } => *timeout_ms,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub schema_version: u32,
    pub recipe_digest: Option<String>,
    /// Bounded kernel bytes are admitted only as part of a quota-checked intent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kernel_source: Option<String>,
    pub runs: Vec<Invocation>,
}
impl Plan {
    fn operation(&self) -> CapabilityOperation {
        if self.recipe_digest.is_some() {
            CapabilityOperation::Collect
        } else {
            CapabilityOperation::Execute
        }
    }
}

#[derive(Clone)]
pub struct DurableExecutor {
    journal: store::Journal,
    research: ResearchStore,
    target: DockerSandbox,
    chain: Arc<dyn TrustedChain>,
    observer: Arc<dyn MeasurementObserver>,
}

/// Result of one durable dispatch; evidence exists only for observed collects.
pub(crate) struct Outcome {
    pub value: Value,
    pub evidence: Option<(ScientificEvidence, RetainedArtifacts)>,
}

impl DurableExecutor {
    #[must_use]
    pub fn new(
        pool: PgPool,
        research: ResearchStore,
        target: DockerSandbox,
        chain: Arc<dyn TrustedChain>,
    ) -> Self {
        Self {
            journal: store::Journal::new(pool),
            research,
            target,
            chain,
            observer: Arc::new(NoObserver),
        }
    }

    /// Install the trusted controller-side observer. Without one every
    /// collect fails with [`Failure::UnobservedMeasurements`].
    #[must_use]
    pub fn with_observer(mut self, observer: Arc<dyn MeasurementObserver>) -> Self {
        self.observer = observer;
        self
    }

    /// Controller-only provisioning step: maps a stored grant to a known daemon
    /// and exact image. Resource strings never become Docker container names.
    ///
    /// # Errors
    /// Wrong quote image, stale grant, changed mapping, or unavailable database.
    pub async fn bind_local_target(
        &self,
        lease: &ControllerLease,
        grant: &ResourceGrant,
    ) -> Result<(), Failure> {
        self.journal.bind(lease, grant, &self.target).await
    }

    /// Retain exact UTF-8 Python source, outside all sandboxes, by content hash.
    ///
    /// # Errors
    /// Unsupported bytes, conflicting content or unavailable database.
    pub async fn retain_script(&self, bytes: &[u8]) -> Result<String, Failure> {
        self.journal.put_script(bytes).await
    }

    /// # Errors
    /// Stale controller or unavailable journal.
    pub async fn intents(&self, lease: &ControllerLease) -> Result<Vec<Intent>, Failure> {
        self.journal.intents(lease).await
    }

    /// Resolve uncertain original operations, never redispatch them. A possibly
    /// delayed start is not declared harmless before its target deadline passes.
    ///
    /// # Errors
    /// Stale owner, foreign target, live original deadline or unconfirmed stop.
    pub async fn reconcile(&self, lease: &ControllerLease) -> Result<(), Failure> {
        for intent in self.intents(lease).await? {
            if !["dispatched", "reconcile"].contains(&intent.state.as_str()) {
                continue;
            }
            if intent.engine_id != self.target.engine_id || intent.image_id != self.target.image_id
            {
                return Err(Failure::Target);
            }
            let mut retained = Ok(());
            for index in 0..intent.plan.runs.len() {
                let _ = self.target.interrupt(&intent, index).await;
                if let Err(error) = self.retain_available(&intent, index).await {
                    retained = Err(error);
                }
            }
            let stopped = self.cleanup(&intent).await;
            // Re-read DB time, not a caller supplied wall clock.
            let expired: bool =
                sqlx::query_scalar("SELECT extract(epoch FROM clock_timestamp()) * 1000 > $1")
                    .bind(intent.deadline_ms)
                    .fetch_one(&self.journal.pool)
                    .await?;
            self.journal
                .finish(
                    &intent,
                    Some(Failure::Interrupted),
                    stopped && expired && retained.is_ok(),
                )
                .await?;
            retained?;
            if !stopped || !expired {
                return Err(Failure::Reconcile);
            }
        }
        Ok(())
    }

    async fn dispatch(
        &self,
        lease: &ControllerLease,
        grant: &ResourceGrant,
        plan: Plan,
    ) -> Result<Outcome, Failure> {
        // Spawn BEFORE any awaited intent/target write. Dropping the caller
        // signals cancellation; the owned worker continues cleanup and journaling.
        let (sender, receiver) = watch::channel(false);
        let _cancel = Cancel(sender);
        let executor = self.clone();
        let lease = *lease;
        let grant = grant.clone();
        tokio::spawn(async move { executor.work(&lease, &grant, plan, receiver).await })
            .await
            .map_err(|_| Failure::Reconcile)?
    }
}

struct Cancel(watch::Sender<bool>);
impl Drop for Cancel {
    fn drop(&mut self) {
        let _ = self.0.send(true);
    }
}

#[async_trait]
impl ExperimentExecutor for DurableExecutor {
    async fn execute(
        &self,
        lease: &ControllerLease,
        grant: &ResourceGrant,
        request: &ExecutionRequest,
    ) -> Result<Value, RuntimeError> {
        let timeout = match request {
            ExecutionRequest::Terminal { timeout_ms, .. }
            | ExecutionRequest::Kernel { timeout_ms, .. } => *timeout_ms,
        };
        if !(1..=25_000).contains(&timeout) {
            return Err(RuntimeError::Scope);
        }
        let mut kernel_source = None;
        let run = match request {
            ExecutionRequest::Terminal { argv, .. } => {
                if argv.is_empty()
                    || argv.len() > 64
                    || argv.iter().map(String::len).sum::<usize>() > 32768
                    || argv.iter().any(|s| s.contains('\0'))
                {
                    return Err(RuntimeError::Scope);
                }
                Invocation::Terminal {
                    argv: argv.clone(),
                    timeout_ms: u64::from(timeout),
                }
            }
            ExecutionRequest::Kernel { code, .. } => {
                if code.is_empty() || code.len() > 32768 || code.contains('\0') {
                    return Err(RuntimeError::Scope);
                }
                kernel_source = Some(code.clone());
                Invocation::Script {
                    script_digest: proof_research::artifact_digest(code.as_bytes()),
                    seed: None,
                    timeout_ms: u64::from(timeout),
                }
            }
        };
        Ok(self
            .dispatch(
                lease,
                grant,
                Plan {
                    schema_version: 1,
                    recipe_digest: None,
                    kernel_source,
                    runs: vec![run],
                },
            )
            .await?
            .value)
    }

    async fn collect(
        &self,
        lease: &ControllerLease,
        grant: &ResourceGrant,
        recipe: &ScientificRecipe,
    ) -> Result<(ScientificEvidence, RetainedArtifacts), RuntimeError> {
        let digest = commitment(recipe).map_err(|_| RuntimeError::Scope)?;
        let recipe = self.research.recipe(&digest).await?;
        let mut runs = Vec::new();
        for seed in &recipe.seeds {
            for script in [
                &recipe.topic.baseline.script_sha256,
                &recipe.candidate_script_digest,
            ] {
                runs.push(Invocation::Script {
                    script_digest: script.clone(),
                    seed: Some(*seed),
                    timeout_ms: recipe.maximum_wall_ms,
                });
            }
        }
        let outcome = self
            .dispatch(
                lease,
                grant,
                Plan {
                    schema_version: 1,
                    recipe_digest: Some(digest),
                    kernel_source: None,
                    runs,
                },
            )
            .await?;
        // Only observer-produced measurements reach here; stdout never does.
        outcome.evidence.ok_or(RuntimeError::Unavailable)
    }
}

const POLL: Duration = Duration::from_millis(100);
