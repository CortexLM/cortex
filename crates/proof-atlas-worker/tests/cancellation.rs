#![allow(clippy::expect_used, clippy::unwrap_used)]
mod common;
use common::*;
use proof_atlas_worker::{AtlasChain, AtlasConfig, AtlasError, AtlasWorker};
use proof_rounds::{FinalizedRoundSource, RoundError};
use std::{sync::Arc, time::Duration};
use tokio::sync::{watch, Notify};

struct DelayedChain {
    inner: Arc<Chain>,
    started: Arc<Notify>,
    finished: Arc<Notify>,
}
impl AtlasChain for DelayedChain {
    fn finalized_height(&self) -> Result<u64, AtlasError> {
        self.started.notify_one();
        std::thread::sleep(Duration::from_millis(300));
        self.finished.notify_one();
        Ok(360)
    }
}
impl FinalizedRoundSource for DelayedChain {
    fn boundary(&self, block: u64) -> Result<chain_live::FinalizedSnapshot, RoundError> {
        self.inner.boundary(block)
    }
}

#[tokio::test]
async fn shutdown_during_blocking_finality_does_not_later_freeze_a_round() {
    let Some(s) = Setup::new(false).await else {
        return;
    };
    let started = Arc::new(Notify::new());
    let finished = Arc::new(Notify::new());
    let agent = Agent::new(Mode::Decide, 30);
    let worker = AtlasWorker::new(
        s.store.clone(),
        Arc::new(DelayedChain {
            inner: s.chain.clone(),
            started: started.clone(),
            finished: finished.clone(),
        }),
        agent.clone(),
        s.receiver.clone(),
        SEED,
        AtlasConfig::default(),
    )
    .unwrap();
    let (stop, signal) = watch::channel(false);
    let (result, ()) = bounded(async {
        tokio::join!(worker.tick(signal), async {
            started.notified().await;
            stop.send(true).unwrap();
        })
    })
    .await;
    assert!(matches!(result, Err(AtlasError::Interrupted)));
    bounded(finished.notified()).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(s.count("proof_atlas_round").await, 0);
    assert!(s.chain.blocks.lock().unwrap().is_empty());
    assert!(agent.jobs.lock().await.is_empty());
    s.f.close().await;
}
