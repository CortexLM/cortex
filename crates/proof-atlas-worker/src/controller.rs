use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

use async_trait::async_trait;
use chain_live::FinalizedSnapshot;
use proof_autonomy::is_digest;
use proof_rounds::{AtlasOperations, FinalizedRoundSource, RoundError, RoundLease, RoundPublisher};
use proof_runtime::{RuntimeCall, RuntimeError, RuntimeOperations};
use serde_json::Value;
use tokio::{sync::watch, time::Instant};
use uuid::Uuid;

use crate::{AtlasAgent, AtlasChain, AtlasConfig, AtlasError, AtlasJob, AtlasProgress, AtlasStore};

pub struct AtlasWorker {
    store: AtlasStore,
    chain: Arc<dyn AtlasChain>,
    agent: Arc<dyn AtlasAgent>,
    publisher: Arc<dyn RoundPublisher>,
    secret: [u8; 32],
    binding: String,
    maximum_seconds: u32,
    config: AtlasConfig,
    owner: Uuid,
}

impl AtlasWorker {
    /// # Errors
    /// Invalid runtime/policy/key binding, overflow or unsafe lease intervals.
    pub fn new(
        store: AtlasStore,
        chain: Arc<dyn AtlasChain>,
        agent: Arc<dyn AtlasAgent>,
        publisher: Arc<dyn RoundPublisher>,
        secret: [u8; 32],
        config: AtlasConfig,
    ) -> Result<Self, AtlasError> {
        let binding = agent.binding()?;
        let maximum_seconds = agent.maximum_seconds();
        let public =
            challenge_common::public_key_from_secret(&secret).map_err(|_| AtlasError::Invalid)?;
        if !is_digest(&binding)
            || binding != store.config.runtime_digest
            || !is_digest(&store.config.policy_digest)
            || hex::encode(public) != store.config.proof_public_key
            || !(1..=86_400).contains(&maximum_seconds)
        {
            return Err(AtlasError::Invalid);
        }
        store.boundary(0)?;
        Ok(Self {
            store,
            chain,
            agent,
            publisher,
            secret,
            binding,
            maximum_seconds,
            config: config.validate()?,
            owner: Uuid::new_v4(),
        })
    }

    /// Drive one durable round at a time. Publication is awaited, never
    /// repeatedly cancelled by the polling interval; unrelated chain outages
    /// cannot starve already-decided outbox work.
    ///
    /// # Errors
    /// Startup schema/privilege failure. Per-round failures remain retryable.
    pub async fn run(&self, mut shutdown: watch::Receiver<bool>) -> Result<(), AtlasError> {
        self.store.ready().await?;
        loop {
            if stopped(&shutdown) {
                return Ok(());
            }
            let _ = self.tick(shutdown.clone()).await;
            tokio::select! {
                () = cancelled(&mut shutdown) => return Ok(()),
                () = tokio::time::sleep(Duration::from_secs(u64::from(self.config.retry_seconds))) => {}
            }
        }
    }

    /// One scheduler pass; models cannot select its round, chain or publisher.
    ///
    /// # Errors
    /// Invalid frozen data, finality unavailable, fencing, shutdown or failed publication.
    pub async fn tick(
        &self,
        mut shutdown: watch::Receiver<bool>,
    ) -> Result<AtlasProgress, AtlasError> {
        let (round, frozen, decided) = tokio::select! {
            biased;
            () = cancelled(&mut shutdown) => return Err(AtlasError::Interrupted),
            head = self.store.head() => head?,
        };
        if decided {
            return self.publish(round, shutdown).await;
        }
        if !frozen {
            let block = self.store.boundary(round)?;
            let mut stop = shutdown.clone();
            let freeze = async {
                let chain = self.chain.clone();
                let rounds = self.store.rounds.clone();
                tokio::task::spawn_blocking(move || {
                    if stopped(&stop) {
                        return Err(AtlasError::Interrupted);
                    }
                    if chain.finalized_height()? < block {
                        return Ok(false);
                    }
                    if stopped(&stop) {
                        return Err(AtlasError::Interrupted);
                    }
                    let boundary = Boundary(chain.boundary(block)?);
                    // RoundStore's existing source trait is not Sync, so keep
                    // its non-Send freeze future entirely on this blocking thread.
                    tokio::runtime::Handle::current().block_on(async {
                        tokio::select! {
                            biased;
                            () = cancelled(&mut stop) => Err(AtlasError::Interrupted),
                            result = rounds.freeze(&boundary, round) => result.map_err(AtlasError::from),
                        }
                    })?;
                    Ok::<_, AtlasError>(true)
                })
                .await
                .map_err(|_| AtlasError::Unavailable)?
            };
            let frozen = tokio::select! {
                biased;
                () = cancelled(&mut shutdown) => return Err(AtlasError::Interrupted),
                frozen = freeze => frozen?,
            };
            if !frozen {
                return Ok(AtlasProgress::Waiting { block });
            }
        }
        let lease = tokio::select! {
            biased;
            () = cancelled(&mut shutdown) => return Err(AtlasError::Interrupted),
            lease = self.store.rounds.acquire(round, self.owner, self.config.lease_seconds) => match lease {
                Ok(lease) => lease,
                Err(RoundError::Fenced) => return Ok(AtlasProgress::Busy { round }),
                Err(error) => return Err(error.into()),
            },
        };
        let result = self.invoke(&lease, shutdown.clone()).await;
        let _ = tokio::time::timeout(Duration::from_secs(1), self.store.release(&lease)).await;
        match result {
            Ok(true) => self.publish(round, shutdown).await,
            Ok(false) | Err(AtlasError::Exhausted) => Ok(AtlasProgress::Blocked { round }),
            Err(error) => Err(error),
        }
    }

    async fn publish(
        &self,
        round: u64,
        mut shutdown: watch::Receiver<bool>,
    ) -> Result<AtlasProgress, AtlasError> {
        tokio::select! {
            biased;
            () = cancelled(&mut shutdown) => Err(AtlasError::Interrupted),
            result = self.store.rounds.publish(round, self.publisher.as_ref()) => {
                result?;
                Ok(AtlasProgress::Published { round })
            }
        }
    }

    async fn invoke(
        &self,
        lease: &RoundLease,
        mut shutdown: watch::Receiver<bool>,
    ) -> Result<bool, AtlasError> {
        let prepare = async {
            let (run, resume) = self
                .store
                .begin_run(lease, &self.binding, self.maximum_seconds)
                .await?;
            let operations =
                AtlasOperations::bind(self.store.rounds.clone(), *lease, self.secret).await?;
            let frozen = self.store.rounds.authorize(lease).await?;
            // Observe before the DB query: connection latency cannot extend the budget.
            let observed = Instant::now();
            let deadline = observed + self.store.remaining(lease).await?;
            let scope = operations.scope().clone();
            Ok::<_, AtlasError>((
                AtlasJob {
                    frozen,
                    lease: *lease,
                    run,
                    scope,
                    resume,
                },
                operations,
                deadline,
            ))
        };
        let (job, operations, deadline) = tokio::select! {
            biased;
            () = cancelled(&mut shutdown) => return Err(AtlasError::Interrupted),
            prepared = prepare => prepared?,
        };
        let (stop, signal) = watch::channel(false);
        let operations = Arc::new(BoundedOperations {
            inner: operations,
            stop: signal.clone(),
            deadline,
        });
        let started = AtomicBool::new(false);
        let execution = async {
            started.store(true, Ordering::Relaxed);
            self.agent.run(&job, operations, signal).await
        };
        tokio::pin!(execution);
        let mut heartbeat = Box::pin(self.heartbeat(lease));
        let (result, pending) = tokio::select! {
            biased;
            () = cancelled(&mut shutdown) => (Err(AtlasError::Interrupted), true),
            () = &mut heartbeat => (Err(AtlasError::Interrupted), true),
            () = tokio::time::sleep_until(deadline) => (Err(AtlasError::Exhausted), true),
            result = &mut execution => (result, false),
        };
        drop(heartbeat);
        let _ = stop.send(true);
        if pending {
            // Revoke even an in-flight private call before waiting for process cleanup.
            let _ = tokio::time::timeout(Duration::from_secs(1), self.store.release(lease)).await;
            // Never first-poll an unstarted invocation merely to drain it.
            if started.load(Ordering::Relaxed) {
                let _ = tokio::time::timeout(
                    Duration::from_secs(u64::from(self.config.stop_grace_seconds)),
                    &mut execution,
                )
                .await;
            }
            return Err(result.err().unwrap_or(AtlasError::Interrupted));
        }
        if matches!(result, Err(AtlasError::Interrupted)) {
            return Err(AtlasError::Interrupted);
        }
        // Revalidate ownership AFTER the process, even if it claims success.
        tokio::select! {
            biased;
            () = cancelled(&mut shutdown) => Err(AtlasError::Interrupted),
            result = self.store.finish(lease) => result,
        }
    }

    async fn heartbeat(&self, lease: &RoundLease) {
        let interval = Duration::from_secs(u64::from(self.config.heartbeat_seconds));
        loop {
            tokio::time::sleep(interval).await;
            if !matches!(
                tokio::time::timeout(
                    interval,
                    self.store.rounds.renew(lease, self.config.lease_seconds)
                )
                .await,
                Ok(Ok(()))
            ) {
                return;
            }
        }
    }
}

struct Boundary(FinalizedSnapshot);
impl FinalizedRoundSource for Boundary {
    fn boundary(&self, block: u64) -> Result<FinalizedSnapshot, RoundError> {
        if self.0.block != block {
            return Err(RoundError::Invalid);
        }
        Ok(self.0.clone())
    }
}
struct BoundedOperations {
    inner: AtlasOperations,
    stop: watch::Receiver<bool>,
    deadline: Instant,
}
#[async_trait]
impl RuntimeOperations for BoundedOperations {
    async fn call(&self, request: RuntimeCall) -> Result<Value, RuntimeError> {
        if stopped(&self.stop) || Instant::now() >= self.deadline {
            return Err(RuntimeError::Scope);
        }
        let mut stop = self.stop.clone();
        tokio::select! {
            biased;
            () = cancelled(&mut stop) => Err(RuntimeError::Scope),
            result = tokio::time::timeout_at(self.deadline, self.inner.call(request)) => {
                let value = result.map_err(|_| RuntimeError::Scope)??;
                if stopped(&self.stop) || Instant::now() >= self.deadline { return Err(RuntimeError::Scope); }
                Ok(value)
            }
        }
    }
}
fn stopped(signal: &watch::Receiver<bool>) -> bool {
    *signal.borrow() || signal.has_changed().is_err()
}
async fn cancelled(signal: &mut watch::Receiver<bool>) {
    while !stopped(signal) {
        if signal.changed().await.is_err() {
            break;
        }
    }
}
