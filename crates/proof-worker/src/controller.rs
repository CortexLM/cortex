use std::{collections::BTreeMap, sync::Arc, time::Duration};

use futures::{stream::FuturesUnordered, StreamExt};
use proof_autonomy::{CapabilityOperation, ExperimentState, ProvisionResult};
use proof_autonomy_pg::{
    ControllerLease, Experiment, IntentKind, IntentStatus, PgStore, StoreError,
};
use proof_broker::{Broker, MinerProvider};
use proof_research::{EvidencePublisher, ResearchStore};
use tokio::sync::watch;
use uuid::Uuid;

use crate::{ExperimentAgent, QuoteSource, RuntimeJob, WorkStore, WorkerConfig, WorkerError};

pub struct ExperimentWorker<P> {
    work: WorkStore,
    broker: Broker<P>,
    research: ResearchStore,
    quotes: Arc<dyn QuoteSource>,
    agent: Arc<dyn ExperimentAgent>,
    publisher: Arc<dyn EvidencePublisher>,
    config: WorkerConfig,
    owner: Uuid,
}

impl<P: MinerProvider> ExperimentWorker<P> {
    /// # Errors
    /// Invalid intervals or unbounded provider requests.
    pub fn new(
        work: WorkStore,
        provider: P,
        research: ResearchStore,
        quotes: Arc<dyn QuoteSource>,
        agent: Arc<dyn ExperimentAgent>,
        publisher: Arc<dyn EvidencePublisher>,
        config: WorkerConfig,
    ) -> Result<Self, WorkerError> {
        Ok(Self {
            broker: Broker::new(
                work.orchestration.clone(),
                provider,
                Duration::from_secs(20),
            )?,
            work,
            research,
            quotes,
            agent,
            publisher,
            config: config.validate()?,
            owner: Uuid::new_v4(),
        })
    }

    /// Run available jobs until explicit service shutdown. Per-job errors never
    /// terminate cleanup for unrelated miners; retries retain all durable intent.
    ///
    /// # Errors
    /// Startup privilege/schema failure or unavailable scheduling database.
    pub async fn run(&self, mut shutdown: watch::Receiver<bool>) -> Result<(), WorkerError> {
        self.work.ready().await?;
        let mut jobs = FuturesUnordered::new();
        let mut active = BTreeMap::new();
        let poll = tokio::time::interval(Duration::from_secs(1));
        let mut scans = Box::pin(futures::stream::unfold(poll, |mut poll| async {
            poll.tick().await;
            let batch = async {
                let mut candidates = self.work.candidates(64, true).await?;
                candidates.extend(self.work.candidates(64, false).await?);
                Ok::<_, WorkerError>(candidates)
            }
            .await;
            Some((batch, poll))
        }));
        let (stop, signal) = watch::channel(false);
        let result = 'schedule: loop {
            if *shutdown.borrow() || shutdown.has_changed().is_err() {
                break Ok(());
            }
            tokio::select! {
                Some(batch) = scans.next() => {
                    let candidates = match batch {
                        Ok(candidates) => candidates,
                        Err(error) => break 'schedule Err(error),
                    };
                    for candidate in candidates {
                        let cleanup = candidate.cleanup;
                        let maximum = if cleanup { self.config.concurrent_cleanup } else { self.config.concurrent_experiments };
                            if active.values().filter(|lane| **lane == cleanup).count() >= maximum { continue; }
                            if active.contains_key(&candidate.id) { continue; }
                            let signal = signal.clone();
                            active.insert(candidate.id, cleanup);
                            jobs.push(async move {
                                let _ = self.tick(candidate.id, signal).await;
                                candidate.id
                            });
                    }
                },
                Some(id) = jobs.next(), if !jobs.is_empty() => { active.remove(&id); },
                _ = shutdown.changed() => {},
            }
        };
        let _ = stop.send(true);
        while jobs.next().await.is_some() {}
        result
    }

    /// One owned lifecycle pass with lease heartbeats and bounded shutdown.
    ///
    /// # Errors
    /// Another controller, invalid durable state, interrupted work or unavailable adapter.
    pub async fn tick(
        &self,
        id: Uuid,
        mut shutdown: watch::Receiver<bool>,
    ) -> Result<(), WorkerError> {
        if *shutdown.borrow() || shutdown.has_changed().is_err() {
            return Err(WorkerError::Interrupted);
        }
        let grace = Duration::from_secs(u64::from(self.config.stop_grace_seconds));
        let lease = tokio::select! {
            result = tokio::time::timeout(grace, self.work.orchestration.acquire(id, self.owner, self.config.lease_seconds)) =>
                result.map_err(|_| WorkerError::Unavailable)??,
            _ = shutdown.changed() => return Err(WorkerError::Interrupted),
        };
        let (stop, receiver) = watch::channel(false);
        let mut operation = Box::pin(self.drive(&lease, receiver));
        let mut renewal = Box::pin(self.heartbeat(&lease));
        let (result, pending) = loop {
            tokio::select! {
                result = &mut operation => break (result, false),
                () = &mut renewal => break (Err(WorkerError::Interrupted), true),
                _ = shutdown.changed() => {
                    if *shutdown.borrow() || shutdown.has_changed().is_err() { break (Err(WorkerError::Interrupted), true); }
                }
            }
        };
        drop(renewal);
        if matches!(result, Err(WorkerError::Interrupted)) {
            let _ = stop.send(true);
            if pending {
                let _ = tokio::time::timeout(grace, &mut operation).await;
            }
        }
        // A suspended operation/heartbeat may hold the rows needed below.
        drop(operation);
        let _ = tokio::time::timeout(grace, async {
            if matches!(result, Err(WorkerError::Interrupted)) {
                let _ = self.work.interrupt_run(&lease).await;
            }
            let _ = self.work.defer(&lease, self.config.retry_seconds).await;
            let _ = self.work.orchestration.release(&lease).await;
        })
        .await;
        result
    }

    async fn heartbeat(&self, lease: &ControllerLease) {
        let interval = Duration::from_secs(u64::from(self.config.heartbeat_seconds));
        loop {
            tokio::time::sleep(interval).await;
            let renewed = tokio::time::timeout(
                interval,
                self.work
                    .orchestration
                    .renew(lease, self.config.lease_seconds),
            )
            .await;
            if !matches!(renewed, Ok(Ok(()))) {
                return;
            }
        }
    }

    async fn drive(
        &self,
        lease: &ControllerLease,
        stop: watch::Receiver<bool>,
    ) -> Result<(), WorkerError> {
        let store = &self.work.orchestration;
        self.work.refresh_expired_quote(lease).await?;
        let tx = store.controller_transaction(lease).await?;
        let mut experiment = tx.experiment().clone();
        tx.commit().await?;
        if experiment.state == ExperimentState::Discussion {
            if let Some(quote) =
                tokio::time::timeout(Duration::from_secs(30), self.quotes.quote(&experiment))
                    .await
                    .map_err(|_| WorkerError::Unavailable)??
            {
                store
                    .publish_quote(lease, experiment.revision, &quote)
                    .await?;
            }
            return Ok(());
        }
        if experiment.state == ExperimentState::AwaitingConsent {
            return Ok(());
        }
        let intents = store
            .intents(experiment.id, &experiment.miner_hotkey)
            .await?;
        for intent in intents.iter().filter(|i| i.kind == IntentKind::Provision) {
            if matches!(
                intent.status,
                IntentStatus::Dispatched | IntentStatus::Reconcile
            ) {
                self.broker.reconcile(lease, intent.id).await?;
            } else if intent.status == IntentStatus::Pending
                && experiment.state == ExperimentState::Approved
            {
                let result = self
                    .broker
                    .provision(lease, experiment.revision, intent.id)
                    .await?;
                if matches!(result, ProvisionResult::Uncertain) {
                    return Ok(());
                }
            }
        }
        experiment = store
            .experiment(experiment.id, &experiment.miner_hotkey)
            .await?;
        let resources = store
            .resources(experiment.id, &experiment.miner_hotkey)
            .await?;
        if experiment.state == ExperimentState::Provisioning
            && resources.len() == 1
            && resources[0].status == "quarantined"
        {
            match store.adopt_resource(lease, experiment.revision).await {
                Ok(adopted) => experiment = adopted,
                Err(StoreError::Scope | StoreError::Conflict | StoreError::Contract(_)) => {}
                Err(error) => return Err(error.into()),
            }
        }
        if matches!(
            experiment.state,
            ExperimentState::Running | ExperimentState::Collecting
        ) {
            self.invoke(lease, &experiment, stop).await?;
        }
        self.cleanup(lease, store).await
    }

    async fn invoke(
        &self,
        lease: &ControllerLease,
        experiment: &Experiment,
        stop: watch::Receiver<bool>,
    ) -> Result<(), WorkerError> {
        let resources = self
            .work
            .orchestration
            .resources(experiment.id, &experiment.miner_hotkey)
            .await?;
        let existing: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM proof_scientific_evidence WHERE experiment_id = $1)",
        )
        .bind(experiment.id)
        .fetch_one(&self.work.pool)
        .await?;
        let [resource] = resources.as_slice() else {
            return Ok(());
        };
        if existing {
            return Ok(());
        }
        match self
            .work
            .orchestration
            .authorize_resource(lease, &resource.resource_id, CapabilityOperation::Inspect)
            .await
        {
            Ok(_) => {}
            Err(StoreError::Scope | StoreError::Contract(_)) => return Ok(()),
            Err(error) => return Err(error.into()),
        }
        let (run, resume) = match self
            .work
            .begin_run(lease, &self.agent.binding()?, self.agent.maximum_seconds())
            .await
        {
            Ok(run) => run,
            Err(WorkerError::Invalid) => return Ok(()),
            Err(error) => return Err(error),
        };
        let job = RuntimeJob {
            run,
            lease: *lease,
            resource: resource.clone(),
            resume,
        };
        let result = self.run_agent(&job, stop).await;
        if matches!(result, Err(WorkerError::Interrupted)) {
            return result;
        }
        self.work.finish_run(lease, result.is_ok()).await?;
        Ok(())
    }

    async fn run_agent(
        &self,
        job: &RuntimeJob,
        mut shutdown: watch::Receiver<bool>,
    ) -> Result<(), WorkerError> {
        let remaining = self.work.remaining_run(&job.lease).await?;
        if *shutdown.borrow() || shutdown.has_changed().is_err() {
            return Err(WorkerError::Interrupted);
        }
        if remaining.is_zero() {
            return Err(WorkerError::Unavailable);
        }
        let (stop, signal) = watch::channel(false);
        let operation = self.agent.run(job, signal);
        tokio::pin!(operation);
        let result = tokio::select! {
            result = &mut operation => return result,
            () = tokio::time::sleep(remaining) => Err(WorkerError::Unavailable),
            _ = shutdown.changed() => Err(WorkerError::Interrupted),
        };
        let _ = stop.send(true);
        let _ = tokio::time::timeout(
            Duration::from_secs(u64::from(self.config.stop_grace_seconds)),
            &mut operation,
        )
        .await;
        result
    }

    async fn cleanup(&self, lease: &ControllerLease, store: &PgStore) -> Result<(), WorkerError> {
        let tx = store.controller_transaction(lease).await?;
        let experiment = tx.experiment().clone();
        tx.commit().await?;
        for resource in store
            .resources(experiment.id, &experiment.miner_hotkey)
            .await?
        {
            if resource.status != "deleted" {
                self.broker.cleanup(lease, &resource.resource_id).await?;
            }
        }
        let digest: Option<String> = sqlx::query_scalar(
            "SELECT digest FROM proof_scientific_evidence WHERE experiment_id = $1",
        )
        .bind(experiment.id)
        .fetch_optional(&self.work.pool)
        .await?;
        let cancelled = store
            .intents(experiment.id, &experiment.miner_hotkey)
            .await?
            .iter()
            .any(|i| i.kind == IntentKind::Cancel);
        if experiment.state.terminal() {
            return Ok(());
        }
        if let Some(digest) = digest.filter(|_| !cancelled) {
            self.research
                .publish(&digest, self.publisher.as_ref())
                .await?;
            self.research.complete(lease, &digest).await?;
        } else {
            store.finish_cleanup(lease).await?;
        }
        Ok(())
    }
}
