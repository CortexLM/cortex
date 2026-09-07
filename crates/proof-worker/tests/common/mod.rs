#![allow(clippy::expect_used, clippy::unwrap_used, dead_code)]

#[path = "../../../proof-autonomy-pg/tests/common/mod.rs"]
pub mod pg;

use async_trait::async_trait;
use proof_autonomy::{commitment, DeletionResult, MachineQuote, ProvisionResult};
use proof_autonomy_pg::{ControllerLease, Experiment, MinerAccount, Resource};
use proof_broker::{MinerProvider, ProviderError};
use proof_research::{EvidencePublisher, PublicEvidence, ResearchError, ResearchStore};
use proof_task::ProofPin;
use proof_worker::*;
use std::{
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::sync::{watch, Mutex, Notify};
use uuid::Uuid;

#[derive(Default)]
pub struct Provider {
    pub rents: AtomicUsize,
    pub reconciles: AtomicUsize,
    pub deletes: AtomicUsize,
    pub ambiguous: AtomicBool,
    pub deletion_pending: AtomicBool,
}

pub struct Adapter(pub Arc<Provider>);

#[async_trait]
impl MinerProvider for Adapter {
    async fn preflight(
        &self,
        _: &MinerAccount,
        quote: &MachineQuote,
    ) -> Result<String, ProviderError> {
        Ok(commitment(quote).unwrap())
    }
    async fn rent(&self, _: &MinerAccount, _: &MachineQuote, id: Uuid) -> ProvisionResult {
        self.0.rents.fetch_add(1, Ordering::SeqCst);
        if self.0.ambiguous.load(Ordering::SeqCst) {
            ProvisionResult::Uncertain
        } else {
            ProvisionResult::Confirmed {
                resource_id: format!("pod-{id}"),
            }
        }
    }
    async fn reconcile(&self, _: &MinerAccount, _: &MachineQuote, id: Uuid) -> ProvisionResult {
        self.0.reconciles.fetch_add(1, Ordering::SeqCst);
        ProvisionResult::Confirmed {
            resource_id: format!("pod-{id}"),
        }
    }
    async fn delete(&self, _: &MinerAccount, _: &Resource) -> Result<(), ProviderError> {
        self.0.deletes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    async fn deletion_status(&self, _: &MinerAccount, _: &Resource) -> DeletionResult {
        if self.0.deletion_pending.load(Ordering::SeqCst) {
            DeletionResult::Pending
        } else {
            DeletionResult::Confirmed
        }
    }
}

#[derive(Clone, Copy)]
pub enum Mode {
    Finish,
    Fail,
    Interrupt,
    Wait,
    IgnoreStop,
}

pub struct Agent {
    pub mode: Mutex<Mode>,
    pub started: Notify,
    pub calls: Mutex<Vec<RuntimeJob>>,
    pub seconds: u32,
}
impl Agent {
    pub fn new(mode: Mode) -> Arc<Self> {
        Arc::new(Self {
            mode: Mutex::new(mode),
            started: Notify::new(),
            calls: Mutex::new(vec![]),
            seconds: 120,
        })
    }
}
#[async_trait]
impl ExperimentAgent for Agent {
    fn binding(&self) -> Result<String, WorkerError> {
        Ok("b".repeat(64))
    }
    fn maximum_seconds(&self) -> u32 {
        self.seconds
    }
    async fn run(
        &self,
        job: &RuntimeJob,
        mut stop: watch::Receiver<bool>,
    ) -> Result<(), WorkerError> {
        assert_eq!(
            job.resource.status, "active",
            "fresh post-adoption snapshot"
        );
        self.calls.lock().await.push(job.clone());
        self.started.notify_one();
        let mode = *self.mode.lock().await;
        match mode {
            Mode::Finish => Ok(()),
            Mode::Fail => Err(WorkerError::Unavailable),
            Mode::Interrupt => Err(WorkerError::Interrupted),
            Mode::Wait => {
                while !*stop.borrow() && stop.changed().await.is_ok() {}
                Err(WorkerError::Interrupted)
            }
            Mode::IgnoreStop => std::future::pending().await,
        }
    }
}
pub struct NoQuote;
#[async_trait]
impl QuoteSource for NoQuote {
    async fn quote(&self, _: &Experiment) -> Result<Option<MachineQuote>, WorkerError> {
        Ok(None)
    }
}
pub struct NoPublication;
#[async_trait]
impl EvidencePublisher for NoPublication {
    async fn publish(&self, _: &PublicEvidence) -> Result<String, ResearchError> {
        panic!("synthetic agent did not collect scientific evidence");
    }
}
pub async fn worker(
    f: &pg::Fixture,
    provider: Arc<Provider>,
    agent: Arc<Agent>,
) -> ExperimentWorker<Adapter> {
    let pool = f.database.app_pool().await.unwrap();
    ExperimentWorker::new(
        WorkStore::new(pool.clone()),
        Adapter(provider),
        ResearchStore::new(pool, ProofPin::default()),
        Arc::new(NoQuote),
        agent,
        Arc::new(NoPublication),
        WorkerConfig {
            heartbeat_seconds: 1,
            retry_seconds: 1,
            stop_grace_seconds: 2,
            concurrent_experiments: 1,
            concurrent_cleanup: 1,
            ..WorkerConfig::default()
        },
    )
    .unwrap()
}
pub async fn approved(f: &pg::Fixture) -> Experiment {
    let (e, lease, quote) = f.quoted().await;
    let e = f
        .store
        .consent(e.id, e.revision, &pg::consent(&quote))
        .await
        .unwrap();
    f.store.release(&lease).await.unwrap();
    e
}
pub async fn running(f: &pg::Fixture) -> (Experiment, ControllerLease) {
    let e = approved(f).await;
    let lease = f.store.acquire(e.id, Uuid::new_v4(), 60).await.unwrap();
    let intent = f.store.intents(e.id, &e.miner_hotkey).await.unwrap()[0].id;
    f.store
        .begin_provision(&lease, e.revision, intent)
        .await
        .unwrap();
    f.store
        .record_provision(
            &lease,
            intent,
            &ProvisionResult::Confirmed {
                resource_id: format!("pod-{intent}"),
            },
        )
        .await
        .unwrap();
    let e = f.store.experiment(e.id, &e.miner_hotkey).await.unwrap();
    (
        f.store.adopt_resource(&lease, e.revision).await.unwrap(),
        lease,
    )
}
pub async fn cancel(f: &pg::Fixture, id: Uuid) {
    let e = f
        .store
        .experiment(id, &f.account.miner_hotkey)
        .await
        .unwrap();
    let request = proof_autonomy_pg::CancelExperiment {
        experiment_id: id,
        revision: e.revision,
    };
    f.store
        .cancel(
            &request,
            &pg::signed(
                &request,
                &format!("/v2/experiments/{id}/cancel"),
                f.now().await,
                &pg::SEED,
            ),
        )
        .await
        .unwrap();
}
pub async fn bounded<F: std::future::Future>(future: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(15), future)
        .await
        .expect("bounded test")
}
