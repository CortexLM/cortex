#![allow(clippy::expect_used, clippy::unwrap_used)]
mod common;
use common::{pg::Fixture, *};
use proof_autonomy::ExperimentState;
use proof_worker::WorkerError;
use std::sync::{atomic::Ordering, Arc};
use tokio::sync::watch;

#[tokio::test]
async fn competing_workers_rent_once_and_missing_evidence_is_rejected() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let e = approved(&f).await;
    let provider = Arc::new(Provider::default());
    let agent = Agent::new(Mode::Finish);
    let first = worker(&f, provider.clone(), agent.clone()).await;
    let second = worker(&f, provider.clone(), agent.clone()).await;
    let (_signal, shutdown) = watch::channel(false);
    let (a, b) = tokio::join!(
        first.tick(e.id, shutdown.clone()),
        second.tick(e.id, shutdown)
    );
    assert!(a.is_ok() || b.is_ok());
    assert_eq!(provider.rents.load(Ordering::SeqCst), 1);
    assert_eq!(agent.calls.lock().await.len(), 1);
    assert_eq!(
        f.store
            .experiment(e.id, &e.miner_hotkey)
            .await
            .unwrap()
            .state,
        ExperimentState::Rejected
    );
    f.close().await;
}

#[tokio::test]
async fn uncertain_rental_is_reconciled_not_repeated() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let e = approved(&f).await;
    let provider = Arc::new(Provider::default());
    provider.ambiguous.store(true, Ordering::SeqCst);
    let agent = Agent::new(Mode::Finish);
    let worker = worker(&f, provider.clone(), agent).await;
    let (_signal, shutdown) = watch::channel(false);
    worker.tick(e.id, shutdown.clone()).await.unwrap();
    assert_eq!(
        f.store
            .experiment(e.id, &e.miner_hotkey)
            .await
            .unwrap()
            .state,
        ExperimentState::Provisioning
    );
    worker.tick(e.id, shutdown).await.unwrap();
    assert_eq!(provider.rents.load(Ordering::SeqCst), 1);
    assert_eq!(provider.reconciles.load(Ordering::SeqCst), 1);
    assert_eq!(provider.deletes.load(Ordering::SeqCst), 1);
    f.close().await;
}

#[tokio::test]
async fn returned_interruption_is_not_repolled_and_resume_keeps_budget_identity() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let e = approved(&f).await;
    let provider = Arc::new(Provider::default());
    let agent = Agent::new(Mode::Interrupt);
    let worker = worker(&f, provider.clone(), agent.clone()).await;
    let (_signal, shutdown) = watch::channel(false);
    assert!(matches!(
        bounded(worker.tick(e.id, shutdown.clone())).await,
        Err(WorkerError::Interrupted)
    ));
    assert_eq!(provider.deletes.load(Ordering::SeqCst), 0);
    *agent.mode.lock().await = Mode::Finish;
    worker.tick(e.id, shutdown).await.unwrap();
    let calls = agent.calls.lock().await;
    assert_eq!(calls.len(), 2);
    assert!(!calls[0].resume);
    assert!(calls[1].resume);
    assert_eq!(calls[0].run.id, calls[1].run.id);
    assert_eq!(calls[0].run.deadline_ms, calls[1].run.deadline_ms);
    assert!(calls[1].lease.fence > calls[0].lease.fence);
    drop(calls);
    assert_eq!(provider.rents.load(Ordering::SeqCst), 1);
    f.close().await;
}

#[tokio::test]
async fn shutdown_stops_agent_without_marking_run_failed_or_deleting_resource() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let e = approved(&f).await;
    let provider = Arc::new(Provider::default());
    let agent = Agent::new(Mode::Wait);
    let worker = worker(&f, provider.clone(), agent.clone()).await;
    let (signal, shutdown) = watch::channel(false);
    let (result, ()) = bounded(async {
        tokio::join!(worker.tick(e.id, shutdown), async {
            agent.started.notified().await;
            signal.send(true).unwrap();
        })
    })
    .await;
    assert!(matches!(result, Err(WorkerError::Interrupted)));
    let phase: String =
        sqlx::query_scalar("SELECT phase FROM proof_runtime_run WHERE experiment_id=$1")
            .bind(e.id)
            .fetch_one(f.database.pool())
            .await
            .unwrap();
    assert_eq!(phase, "started");
    assert_eq!(provider.deletes.load(Ordering::SeqCst), 0);
    f.close().await;
}

#[tokio::test]
async fn cancellation_fences_runtime_then_independent_worker_verifies_cleanup() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let e = approved(&f).await;
    let provider = Arc::new(Provider::default());
    let agent = Agent::new(Mode::Wait);
    let worker = worker(&f, provider.clone(), agent.clone()).await;
    let (_signal, shutdown) = watch::channel(false);
    let (result, ()) = bounded(async {
        tokio::join!(worker.tick(e.id, shutdown.clone()), async {
            agent.started.notified().await;
            cancel(&f, e.id).await;
        })
    })
    .await;
    assert!(matches!(result, Err(WorkerError::Interrupted)));
    worker.tick(e.id, shutdown).await.unwrap();
    assert_eq!(
        f.store
            .experiment(e.id, &e.miner_hotkey)
            .await
            .unwrap()
            .state,
        ExperimentState::Cancelled
    );
    assert_eq!(provider.rents.load(Ordering::SeqCst), 1);
    assert_eq!(agent.calls.lock().await.len(), 1);
    f.close().await;
}

#[tokio::test]
async fn failed_agent_is_cleaned_but_delete_ack_does_not_finish_experiment() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let e = approved(&f).await;
    let provider = Arc::new(Provider::default());
    provider.deletion_pending.store(true, Ordering::SeqCst);
    let agent = Agent::new(Mode::Fail);
    let worker = worker(&f, provider.clone(), agent.clone()).await;
    let (_signal, shutdown) = watch::channel(false);
    assert!(worker.tick(e.id, shutdown.clone()).await.is_err());
    assert_eq!(
        f.store
            .experiment(e.id, &e.miner_hotkey)
            .await
            .unwrap()
            .state,
        ExperimentState::Deleting
    );
    provider.deletion_pending.store(false, Ordering::SeqCst);
    worker.tick(e.id, shutdown).await.unwrap();
    assert_eq!(
        f.store
            .experiment(e.id, &e.miner_hotkey)
            .await
            .unwrap()
            .state,
        ExperimentState::Rejected
    );
    assert_eq!(agent.calls.lock().await.len(), 1);
    f.close().await;
}

#[tokio::test]
async fn long_running_experiment_cannot_starve_other_cleanup() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let e = approved(&f).await;
    let other = approved(&f).await;
    let provider = Arc::new(Provider::default());
    let agent = Agent::new(Mode::Wait);
    let worker = worker(&f, provider, agent.clone()).await;
    let (signal, shutdown) = watch::channel(false);
    let (result, ()) = bounded(async {
        tokio::join!(worker.run(shutdown), async {
            agent.started.notified().await;
            let running_id = agent.calls.lock().await[0].lease.experiment_id;
            let cleanup_id = if running_id == e.id { other.id } else { e.id };
            cancel(&f, cleanup_id).await;
            loop {
                if f.store
                    .experiment(cleanup_id, &e.miner_hotkey)
                    .await
                    .unwrap()
                    .state
                    == ExperimentState::Cancelled
                {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            assert_eq!(agent.calls.lock().await.len(), 1);
            signal.send(true).unwrap();
        })
    })
    .await;
    result.unwrap();
    f.close().await;
}
