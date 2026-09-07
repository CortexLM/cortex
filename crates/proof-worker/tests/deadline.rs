#![allow(clippy::expect_used, clippy::unwrap_used)]
mod common;
use common::{pg::Fixture, *};
use proof_autonomy::ExperimentState;
use std::{sync::Arc, time::Duration};
use tokio::sync::{watch, Mutex, Notify};

#[tokio::test]
async fn controller_bounds_uncooperative_runtime_then_verifies_cleanup() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let e = approved(&f).await;
    let provider = Arc::new(Provider::default());
    let agent = Arc::new(Agent {
        mode: Mutex::new(Mode::IgnoreStop),
        started: Notify::new(),
        calls: Mutex::new(vec![]),
        seconds: 1,
    });
    let worker = worker(&f, provider.clone(), agent.clone()).await;
    let (_stop, signal) = watch::channel(false);
    tokio::time::timeout(Duration::from_secs(8), worker.tick(e.id, signal))
        .await
        .expect("original one-second budget plus bounded stop grace")
        .unwrap();
    assert_eq!(agent.calls.lock().await.len(), 1);
    assert_eq!(
        f.store
            .experiment(e.id, &e.miner_hotkey)
            .await
            .unwrap()
            .state,
        ExperimentState::Rejected
    );
    let phase: String =
        sqlx::query_scalar("SELECT phase FROM proof_runtime_run WHERE experiment_id=$1")
            .bind(e.id)
            .fetch_one(f.database.pool())
            .await
            .unwrap();
    assert_eq!(phase, "failed");
    assert_eq!(
        provider.deletes.load(std::sync::atomic::Ordering::SeqCst),
        1
    );
    f.close().await;
}

#[tokio::test]
async fn shutdown_interrupts_lease_acquisition_waiting_on_database_rows() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let e = approved(&f).await;
    let provider = Arc::new(Provider::default());
    let agent = Agent::new(Mode::Finish);
    let worker = worker(&f, provider, agent.clone()).await;
    let mut locked = f.database.pool().begin().await.unwrap();
    sqlx::query("SELECT id FROM proof_experiment WHERE id=$1 FOR UPDATE")
        .bind(e.id)
        .fetch_one(&mut *locked)
        .await
        .unwrap();
    let (stop, signal) = watch::channel(false);
    let (result, ()) = tokio::time::timeout(Duration::from_secs(2), async {
        tokio::join!(worker.tick(e.id, signal), async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            stop.send(true).unwrap();
        })
    })
    .await
    .expect("shutdown must not wait for the locked row");
    assert!(matches!(
        result,
        Err(proof_worker::WorkerError::Interrupted)
    ));
    assert!(agent.calls.lock().await.is_empty());
    locked.rollback().await.unwrap();
    f.close().await;
}
