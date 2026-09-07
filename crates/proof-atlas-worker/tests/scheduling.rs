#![allow(clippy::expect_used, clippy::unwrap_used)]
mod common;
use common::*;
use proof_atlas_worker::*;
use std::{sync::atomic::Ordering, time::Duration};
use tokio::sync::watch;

#[tokio::test]
async fn finalized_boundaries_are_contiguous_and_wait_without_tip_fallback() {
    let Some(s) = Setup::new(false).await else {
        return;
    };
    let agent = Agent::new(Mode::Decide, 60);
    let worker = s.worker(agent.clone());
    let (_keep, signal) = watch::channel(false);
    s.chain.height.store(359, Ordering::SeqCst);
    assert_eq!(
        worker.tick(signal.clone()).await.unwrap(),
        AtlasProgress::Waiting { block: 360 }
    );
    assert_eq!(s.count("proof_atlas_round").await, 0);
    assert!(s.chain.blocks.lock().unwrap().is_empty());
    s.chain.unavailable.store(true, Ordering::SeqCst);
    assert!(worker.tick(signal.clone()).await.is_err());
    assert!(agent.jobs.lock().await.is_empty());
    s.chain.unavailable.store(false, Ordering::SeqCst);
    s.chain.height.store(1080, Ordering::SeqCst);
    for round in 0..3 {
        assert_eq!(
            worker.tick(signal.clone()).await.unwrap(),
            AtlasProgress::Published { round }
        );
    }
    assert_eq!(*s.chain.blocks.lock().unwrap(), [360, 720, 1080]);
    assert_eq!(
        worker.tick(signal).await.unwrap(),
        AtlasProgress::Waiting { block: 1440 }
    );
    assert_eq!(agent.jobs.lock().await.len(), 3);
    assert_eq!(s.count("proof_atlas_runtime").await, 3);
    s.f.close().await;
}

#[tokio::test]
async fn retry_replays_exact_signed_decision_despite_chain_outage() {
    let Some(s) = Setup::new(true).await else {
        return;
    };
    let agent = Agent::new(Mode::Decide, 60);
    let worker = s.worker(agent.clone());
    let (_keep, signal) = watch::channel(false);
    s.receiver.fail.store(true, Ordering::SeqCst);
    assert!(worker.tick(signal.clone()).await.is_err());
    assert_eq!(s.count("proof_atlas_decision").await, 1);
    assert_eq!(s.count("proof_atlas_round").await, 1);
    let stored = s.store.rounds().decision(0).await.unwrap();
    assert_eq!(stored.decision.awards.len(), 1);
    let first = serde_json::to_vec(&s.receiver.documents.lock().await[0]).unwrap();
    s.chain.unavailable.store(true, Ordering::SeqCst);
    s.receiver.fail.store(false, Ordering::SeqCst);
    assert_eq!(
        s.worker(agent.clone()).tick(signal.clone()).await.unwrap(),
        AtlasProgress::Published { round: 0 }
    );
    assert_eq!(
        first,
        serde_json::to_vec(&s.receiver.documents.lock().await[1]).unwrap()
    );
    assert_eq!(
        stored.leaves,
        s.store.rounds().decision(0).await.unwrap().leaves
    );
    assert_eq!(agent.jobs.lock().await.len(), 1);
    assert!(worker.tick(signal).await.is_err());
    assert_eq!(s.count("proof_atlas_round").await, 1);
    s.f.close().await;
}

#[tokio::test]
async fn publication_rechecks_rewardable_artifacts_before_retry() {
    let Some(s) = Setup::new(true).await else {
        return;
    };
    let worker = s.worker(Agent::new(Mode::Decide, 60));
    let (_keep, signal) = watch::channel(false);
    s.receiver.fail.store(true, Ordering::SeqCst);
    assert!(worker.tick(signal.clone()).await.is_err());
    sqlx::query("UPDATE proof_evidence_artifact SET bytes='\\x00'")
        .execute(s.f.database.pool())
        .await
        .unwrap();
    s.receiver.fail.store(false, Ordering::SeqCst);
    assert!(worker.tick(signal).await.is_err());
    assert_eq!(s.receiver.documents.lock().await.len(), 1);
    assert_eq!(s.count("proof_atlas_round").await, 1);
    s.f.close().await;
}

#[tokio::test]
async fn publication_slower_than_polling_completes_without_starvation() {
    let Some(s) = Setup::new(false).await else {
        return;
    };
    let agent = Agent::new(Mode::Decide, 60);
    let worker = s.worker(agent.clone());
    s.receiver.delay_ms.store(2200, Ordering::SeqCst);
    let (stop, signal) = watch::channel(false);
    let running = tokio::spawn(async move { worker.run(signal).await });
    bounded(s.receiver.delivered.notified()).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    stop.send(true).unwrap();
    bounded(running).await.unwrap().unwrap();
    let delivered: bool =
        sqlx::query_scalar("SELECT delivered FROM proof_atlas_publication WHERE round=0")
            .fetch_one(s.f.database.pool())
            .await
            .unwrap();
    assert!(delivered);
    assert_eq!(agent.jobs.lock().await.len(), 1);
    assert_eq!(s.receiver.documents.lock().await.len(), 1);
    s.f.close().await;
}

#[tokio::test]
async fn process_success_without_private_decision_never_advances_or_restarts() {
    let Some(s) = Setup::new(false).await else {
        return;
    };
    let agent = Agent::new(Mode::Finish, 60);
    let worker = s.worker(agent.clone());
    let (_keep, signal) = watch::channel(false);
    for _ in 0..2 {
        assert_eq!(
            worker.tick(signal.clone()).await.unwrap(),
            AtlasProgress::Blocked { round: 0 }
        );
    }
    assert_eq!(agent.jobs.lock().await.len(), 1);
    assert_eq!(s.count("proof_atlas_decision").await, 0);
    assert!(s.receiver.documents.lock().await.is_empty());
    s.f.close().await;
}

#[tokio::test]
async fn shutdown_during_publication_retains_outbox_and_replays_without_model() {
    let Some(s) = Setup::new(false).await else {
        return;
    };
    let agent = Agent::new(Mode::Decide, 60);
    let worker = s.worker(agent.clone());
    s.receiver.delay_ms.store(60_000, Ordering::SeqCst);
    let (stop, signal) = watch::channel(false);
    let running = tokio::spawn(async move { worker.tick(signal).await });
    bounded(s.receiver.started.notified()).await;
    stop.send(true).unwrap();
    assert!(matches!(
        bounded(running).await.unwrap(),
        Err(AtlasError::Interrupted)
    ));
    sqlx::query("UPDATE proof_atlas_publication SET expires_at='-infinity'")
        .execute(s.f.database.pool())
        .await
        .unwrap();
    s.receiver.delay_ms.store(0, Ordering::SeqCst);
    let (_keep, signal) = watch::channel(false);
    s.worker(agent.clone()).tick(signal).await.unwrap();
    let documents = s.receiver.documents.lock().await;
    assert_eq!(
        serde_json::to_vec(&documents[0]).unwrap(),
        serde_json::to_vec(&documents[1]).unwrap()
    );
    assert_eq!(agent.jobs.lock().await.len(), 1);
    s.f.close().await;
}
