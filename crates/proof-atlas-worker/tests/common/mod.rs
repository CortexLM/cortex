#![allow(clippy::expect_used, clippy::unwrap_used, dead_code)]
#[path = "../../../proof-rounds/tests/common/mod.rs"]
pub mod rounds;

use async_trait::async_trait;
use chain_live::FinalizedSnapshot;
use proof_atlas_worker::*;
use proof_autonomy::{commitment, ContributionAward, DecayPlan};
use proof_research::ResearchStore;
use proof_rounds::{FinalizedRoundSource, RoundError, RoundPublication, RoundPublisher};
use proof_runtime::{RuntimeCall, RuntimeOperations};
pub use rounds::pg::{Fixture, SEED};
use std::{
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::sync::{watch, Mutex, Notify};

pub struct Chain {
    pub height: AtomicU64,
    pub unavailable: AtomicBool,
    pub blocks: std::sync::Mutex<Vec<u64>>,
    pub source: rounds::Source,
}
impl AtlasChain for Chain {
    fn finalized_height(&self) -> Result<u64, AtlasError> {
        if self.unavailable.load(Ordering::SeqCst) {
            return Err(AtlasError::Unavailable);
        }
        Ok(self.height.load(Ordering::SeqCst))
    }
}
impl FinalizedRoundSource for Chain {
    fn boundary(&self, block: u64) -> Result<FinalizedSnapshot, RoundError> {
        assert!(block <= self.height.load(Ordering::SeqCst));
        self.blocks.lock().unwrap().push(block);
        self.source.boundary(block)
    }
}
#[derive(Clone, Copy)]
pub enum Mode {
    Decide,
    Wait,
    Interrupt,
    Finish,
    GatedSuccess,
}
pub struct Agent {
    pub mode: Mutex<Mode>,
    pub jobs: Mutex<Vec<AtlasJob>>,
    pub operations: Mutex<Vec<Arc<dyn RuntimeOperations>>>,
    pub started: Notify,
    pub stopped: Notify,
    pub complete: Notify,
    pub budget: u32,
}
impl Agent {
    pub fn new(mode: Mode, budget: u32) -> Arc<Self> {
        Arc::new(Self {
            mode: Mutex::new(mode),
            jobs: Mutex::new(vec![]),
            operations: Mutex::new(vec![]),
            started: Notify::new(),
            stopped: Notify::new(),
            complete: Notify::new(),
            budget,
        })
    }
}
pub fn request(job: &AtlasJob, operation: &str, arguments: serde_json::Value) -> RuntimeCall {
    RuntimeCall {
        schema_version: 1,
        scope: job.scope.clone(),
        operation: operation.into(),
        arguments,
    }
}
#[async_trait]
impl AtlasAgent for Agent {
    fn binding(&self) -> Result<String, AtlasError> {
        Ok("b".repeat(64))
    }
    fn maximum_seconds(&self) -> u32 {
        self.budget
    }
    async fn run(
        &self,
        job: &AtlasJob,
        operations: Arc<dyn RuntimeOperations>,
        mut stop: watch::Receiver<bool>,
    ) -> Result<(), AtlasError> {
        self.jobs.lock().await.push(job.clone());
        self.operations.lock().await.push(operations.clone());
        self.started.notify_one();
        let mode = *self.mode.lock().await;
        match mode {
            Mode::Decide => {
                operations
                    .call(request(job, "history", serde_json::json!({"limit":1})))
                    .await
                    .map_err(|_| AtlasError::Unavailable)?;
                let mut decision = rounds::decision(&job.frozen);
                if let Some((digest, contribution)) = job.frozen.contributions.first_key_value() {
                    decision.awards.push(ContributionAward {
                        contribution_digest: digest.clone(),
                        miner_hotkey: hex::encode(contribution.miner_hotkey),
                        units: 500_000,
                        evidence_digests: contribution.evidence_digests.iter().cloned().collect(),
                        rationale: "Synthetic discovery".into(),
                        decay: DecayPlan {
                            first_round: 0,
                            initial_units: 500_000,
                            retention_ppm: 500_000,
                            expires_round: 10,
                        },
                        decay_revision: None,
                    });
                }
                operations
                    .call(request(
                        job,
                        "submit_decision",
                        serde_json::to_value(decision).unwrap(),
                    ))
                    .await
                    .map_err(|_| AtlasError::Unavailable)?;
                Ok(())
            }
            Mode::Finish => Ok(()),
            Mode::GatedSuccess => {
                self.complete.notified().await;
                Ok(())
            }
            Mode::Interrupt => Err(AtlasError::Interrupted),
            Mode::Wait => {
                while !*stop.borrow() && stop.changed().await.is_ok() {}
                self.stopped.notify_one();
                Err(AtlasError::Interrupted)
            }
        }
    }
}
#[derive(Default)]
pub struct Receiver {
    pub documents: Mutex<Vec<RoundPublication>>,
    pub fail: AtomicBool,
    pub delay_ms: AtomicU64,
    pub started: Notify,
    pub delivered: Notify,
}
#[async_trait]
impl RoundPublisher for Receiver {
    async fn publish(&self, document: &RoundPublication) -> Result<String, RoundError> {
        document.verify(&rounds::miner()).unwrap();
        self.documents.lock().await.push(document.clone());
        self.started.notify_one();
        tokio::time::sleep(Duration::from_millis(self.delay_ms.load(Ordering::SeqCst))).await;
        if self.fail.load(Ordering::SeqCst) {
            return Err(RoundError::Publication);
        }
        self.delivered.notify_one();
        Ok(commitment(document)?)
    }
}
pub struct Setup {
    pub f: Fixture,
    pub store: AtlasStore,
    pub chain: Arc<Chain>,
    pub receiver: Arc<Receiver>,
}
impl Setup {
    pub async fn new(science: bool) -> Option<Self> {
        let f = Fixture::new().await?;
        let pool = f.database.app_pool().await.unwrap();
        let research = if science {
            rounds::completed(&f).await.0
        } else {
            ResearchStore::new(pool.clone(), rounds::scientific().0)
        };
        let store = AtlasStore::new(pool, research, rounds::config());
        let chain = Arc::new(Chain {
            height: AtomicU64::new(360),
            unavailable: AtomicBool::new(false),
            blocks: std::sync::Mutex::new(vec![]),
            source: rounds::source(&f).await,
        });
        Some(Self {
            f,
            store,
            chain,
            receiver: Arc::new(Receiver::default()),
        })
    }
    pub fn worker(&self, agent: Arc<Agent>) -> Arc<AtlasWorker> {
        Arc::new(
            AtlasWorker::new(
                self.store.clone(),
                self.chain.clone(),
                agent,
                self.receiver.clone(),
                SEED,
                AtlasConfig {
                    heartbeat_seconds: 1,
                    stop_grace_seconds: 1,
                    lease_seconds: 10,
                    ..AtlasConfig::default()
                },
            )
            .unwrap(),
        )
    }
    pub async fn expire(&self) {
        sqlx::query("UPDATE proof_atlas_lease SET expires_at='-infinity'")
            .execute(self.f.database.pool())
            .await
            .unwrap();
    }
    pub async fn count(&self, table: &str) -> i64 {
        sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
            .fetch_one(self.f.database.pool())
            .await
            .unwrap()
    }
}
pub async fn bounded<F: std::future::Future>(future: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(12), future)
        .await
        .expect("bounded test")
}
