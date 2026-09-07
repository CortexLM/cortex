#![allow(clippy::expect_used, clippy::unwrap_used)]
mod common;
use common::*;
use proof_atlas_worker::*;
use proof_rounds::RoundError;
use std::time::Duration;
use tokio::sync::watch;
use uuid::Uuid;

#[tokio::test]
async fn heartbeat_preserves_ownership_and_shutdown_revokes_private_operations() {
    let Some(s) = Setup::new(false).await else {
        return;
    };
    let agent = Agent::new(Mode::Wait, 60);
    let worker = s.worker(agent.clone());
    let (stop, signal) = watch::channel(false);
    let running = tokio::spawn(async move { worker.tick(signal).await });
    bounded(agent.started.notified()).await;
    let job = agent.jobs.lock().await[0].clone();
    let before: f64 =
        sqlx::query_scalar("SELECT extract(epoch FROM expires_at)::float8 FROM proof_atlas_lease")
            .fetch_one(s.f.database.pool())
            .await
            .unwrap();
    tokio::time::sleep(Duration::from_millis(1300)).await;
    let after: f64 =
        sqlx::query_scalar("SELECT extract(epoch FROM expires_at)::float8 FROM proof_atlas_lease")
            .fetch_one(s.f.database.pool())
            .await
            .unwrap();
    assert!(after > before);
    let (_keep, signal) = watch::channel(false);
    assert_eq!(
        s.worker(agent.clone()).tick(signal).await.unwrap(),
        AtlasProgress::Busy { round: 0 }
    );
    assert_eq!(s.count("proof_atlas_runtime").await, 1);
    stop.send(true).unwrap();
    assert!(matches!(
        bounded(running).await.unwrap(),
        Err(AtlasError::Interrupted)
    ));
    bounded(agent.stopped.notified()).await;
    assert!(agent.operations.lock().await[0]
        .call(request(&job, "history", serde_json::json!({"limit":1})))
        .await
        .is_err());
    assert_eq!(s.count("proof_atlas_decision").await, 0);
    s.f.close().await;
}

#[tokio::test]
async fn takeover_resumes_original_run_without_duplicate_invocation_or_reset() {
    let Some(s) = Setup::new(false).await else {
        return;
    };
    let agent = Agent::new(Mode::Wait, 60);
    let worker = s.worker(agent.clone());
    let (_keep, signal) = watch::channel(false);
    let running = tokio::spawn(async move { worker.tick(signal).await });
    bounded(agent.started.notified()).await;
    let old = agent.jobs.lock().await[0].clone();
    assert!(matches!(
        s.store.begin_run(&old.lease, &old.run.binding, 999).await,
        Err(AtlasError::Exhausted)
    ));
    s.expire().await;
    let replacement = Agent::new(Mode::Decide, 3600);
    let (_keep2, signal) = watch::channel(false);
    assert_eq!(
        s.worker(replacement.clone()).tick(signal).await.unwrap(),
        AtlasProgress::Published { round: 0 }
    );
    assert!(matches!(
        bounded(running).await.unwrap(),
        Err(AtlasError::Interrupted)
    ));
    let new = replacement.jobs.lock().await[0].clone();
    assert!(new.resume);
    assert!(!old.resume);
    assert_eq!(old.run.id, new.run.id);
    assert_eq!(old.run.deadline_ms, new.run.deadline_ms);
    assert_eq!(old.run.binding, new.run.binding);
    assert_eq!(old.run.frozen_digest, new.run.frozen_digest);
    assert!(new.lease.fence > old.lease.fence);
    assert!(matches!(
        s.store.rounds().authorize(&old.lease).await,
        Err(RoundError::Fenced)
    ));
    assert_eq!(s.count("proof_atlas_runtime").await, 1);
    assert_eq!(s.count("proof_atlas_runtime_event").await, 3);
    s.f.close().await;
}

#[tokio::test]
async fn absolute_deadline_stops_execution_and_takeover_cannot_buy_new_budget() {
    let Some(s) = Setup::new(false).await else {
        return;
    };
    let agent = Agent::new(Mode::Wait, 1);
    let worker = s.worker(agent.clone());
    let (_keep, signal) = watch::channel(false);
    assert_eq!(
        bounded(worker.tick(signal.clone())).await.unwrap(),
        AtlasProgress::Blocked { round: 0 }
    );
    bounded(agent.stopped.notified()).await;
    let original = agent.jobs.lock().await[0].clone();
    let replacement = Agent::new(Mode::Decide, 3600);
    assert_eq!(
        s.worker(replacement.clone()).tick(signal).await.unwrap(),
        AtlasProgress::Blocked { round: 0 }
    );
    assert!(replacement.jobs.lock().await.is_empty());
    let deadline: i64 = sqlx::query_scalar("SELECT deadline_ms FROM proof_atlas_runtime")
        .fetch_one(s.f.database.pool())
        .await
        .unwrap();
    assert_eq!(deadline, original.run.deadline_ms);
    assert!(s.receiver.documents.lock().await.is_empty());
    s.f.close().await;
}

#[tokio::test]
async fn immutable_binding_and_runtime_grants_fail_closed() {
    let Some(s) = Setup::new(false).await else {
        return;
    };
    s.store.ready().await.unwrap();
    s.store.rounds().freeze(s.chain.as_ref(), 0).await.unwrap();
    let lease = s
        .store
        .rounds()
        .acquire(0, Uuid::new_v4(), 30)
        .await
        .unwrap();
    assert!(matches!(
        s.store.begin_run(&lease, &"e".repeat(64), 60).await,
        Err(AtlasError::Invalid)
    ));
    let (run, resume) = s
        .store
        .begin_run(&lease, &"b".repeat(64), 60)
        .await
        .unwrap();
    assert!(!resume);
    let pool = s.f.database.app_pool().await.unwrap();
    for column in [
        "round",
        "id",
        "binding",
        "frozen_digest",
        "deadline_ms",
        "created_at",
    ] {
        assert!(
            sqlx::query(&format!("UPDATE proof_atlas_runtime SET {column}={column}"))
                .execute(&pool)
                .await
                .is_err()
        );
    }
    for table in ["proof_atlas_runtime", "proof_atlas_runtime_event"] {
        assert!(sqlx::query(&format!("DELETE FROM {table}"))
            .execute(&pool)
            .await
            .is_err());
    }
    assert!(
        sqlx::query("UPDATE proof_atlas_runtime_event SET kind=kind")
            .execute(&pool)
            .await
            .is_err()
    );
    assert!(!run.id.is_nil());
    let research = proof_research::ResearchStore::new(pool, common::rounds::scientific().0);
    assert!(AtlasStore::new(
        s.f.database.pool().clone(),
        research,
        common::rounds::config()
    )
    .ready()
    .await
    .is_err());
    s.f.close().await;
}

#[tokio::test]
async fn preexisting_shutdown_and_closed_signal_never_launch() {
    let Some(s) = Setup::new(false).await else {
        return;
    };
    let agent = Agent::new(Mode::Decide, 60);
    let worker = s.worker(agent.clone());
    let (_stop, signal) = watch::channel(true);
    assert!(matches!(
        worker.tick(signal).await,
        Err(AtlasError::Interrupted)
    ));
    let (stop, signal) = watch::channel(false);
    drop(stop);
    worker.run(signal).await.unwrap();
    assert_eq!(s.count("proof_atlas_round").await, 0);
    assert!(agent.jobs.lock().await.is_empty());
    s.f.close().await;
}

#[tokio::test]
async fn ownership_is_rechecked_when_a_stale_process_claims_success() {
    let Some(s) = Setup::new(false).await else {
        return;
    };
    let agent = Agent::new(Mode::GatedSuccess, 60);
    let worker = s.worker(agent.clone());
    let (_keep, signal) = watch::channel(false);
    let running = tokio::spawn(async move { worker.tick(signal).await });
    bounded(agent.started.notified()).await;
    let job = agent.jobs.lock().await[0].clone();
    let retained: Uuid = sqlx::query_scalar("SELECT id FROM proof_atlas_runtime")
        .fetch_one(s.f.database.pool())
        .await
        .unwrap();
    assert_eq!(retained, job.run.id);
    s.expire().await;
    assert!(agent.operations.lock().await[0]
        .call(request(
            &job,
            "submit_decision",
            serde_json::to_value(rounds::decision(&job.frozen)).unwrap()
        ))
        .await
        .is_err());
    agent.complete.notify_one();
    assert!(bounded(running).await.unwrap().is_err());
    let phase: String = sqlx::query_scalar("SELECT phase FROM proof_atlas_runtime")
        .fetch_one(s.f.database.pool())
        .await
        .unwrap();
    assert_eq!(phase, "started");
    assert_eq!(s.count("proof_atlas_decision").await, 0);
    assert!(s.receiver.documents.lock().await.is_empty());
    s.f.close().await;
}

#[tokio::test]
async fn runtime_interruption_is_resumed_instead_of_completed_or_restarted() {
    let Some(s) = Setup::new(false).await else {
        return;
    };
    let agent = Agent::new(Mode::Interrupt, 60);
    let worker = s.worker(agent.clone());
    let (_keep, signal) = watch::channel(false);
    assert!(matches!(
        worker.tick(signal.clone()).await,
        Err(AtlasError::Interrupted)
    ));
    *agent.mode.lock().await = Mode::Decide;
    assert_eq!(
        worker.tick(signal).await.unwrap(),
        AtlasProgress::Published { round: 0 }
    );
    let jobs = agent.jobs.lock().await;
    assert_eq!(jobs[0].run.id, jobs[1].run.id);
    assert_eq!(jobs[0].run.deadline_ms, jobs[1].run.deadline_ms);
    assert!(jobs[1].resume);
    s.f.close().await;
}
