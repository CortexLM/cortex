#![allow(clippy::expect_used, clippy::unwrap_used)]
mod common;
use common::{pg::Fixture, *};
use proof_autonomy::ExperimentState;
use proof_worker::{WorkStore, WorkerConfig};
use uuid::Uuid;

#[test]
fn worker_limits_require_bounded_independent_cleanup_capacity() {
    WorkerConfig::default().validate().unwrap();
    assert!(WorkerConfig {
        concurrent_cleanup: 0,
        ..WorkerConfig::default()
    }
    .validate()
    .is_err());
    assert!(WorkerConfig {
        heartbeat_seconds: 30,
        ..WorkerConfig::default()
    }
    .validate()
    .is_err());
}

#[tokio::test]
async fn privileges_preserve_immutable_runtime_budget_and_events() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    assert!(WorkStore::new(f.database.pool().clone())
        .ready()
        .await
        .is_err());
    let pool = f.database.app_pool().await.unwrap();
    WorkStore::new(pool.clone()).ready().await.unwrap();
    assert!(sqlx::query("DELETE FROM proof_runtime_event")
        .execute(&pool)
        .await
        .is_err());
    assert!(sqlx::query("UPDATE proof_runtime_run SET deadline_ms=1")
        .execute(&pool)
        .await
        .is_err());
    sqlx::query("GRANT UPDATE (binding) ON proof_runtime_run TO base_app")
        .execute(f.database.pool())
        .await
        .unwrap();
    assert!(WorkStore::new(pool).ready().await.is_err());
    f.close().await;
}

#[tokio::test]
async fn runtime_identity_deadline_and_binding_survive_takeover() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let (e, old) = running(&f).await;
    let store = WorkStore::new(f.database.app_pool().await.unwrap());
    let (initial, resumed) = store.begin_run(&old, &"c".repeat(64), 100).await.unwrap();
    assert!(!resumed);
    assert!(store.begin_run(&old, &"c".repeat(64), 100).await.is_err());
    f.expire(&old).await;
    let lease = f.store.acquire(e.id, Uuid::new_v4(), 60).await.unwrap();
    assert!(store.begin_run(&lease, &"d".repeat(64), 100).await.is_err());
    let (recovered, resumed) = store
        .begin_run(&lease, &"c".repeat(64), 1000)
        .await
        .unwrap();
    assert!(resumed);
    assert_eq!(initial.id, recovered.id);
    assert_eq!(initial.deadline_ms, recovered.deadline_ms);
    assert!(store
        .begin_run(&lease, &"c".repeat(64), 1000)
        .await
        .is_err());
    assert!(store.finish_run(&old, true).await.is_err());
    store.finish_run(&lease, false).await.unwrap();
    assert!(store
        .begin_run(&lease, &"c".repeat(64), 1000)
        .await
        .is_err());
    f.close().await;
}

#[tokio::test]
async fn exhausted_deadline_cannot_create_another_runtime() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let (_, lease) = running(&f).await;
    let store = WorkStore::new(f.database.app_pool().await.unwrap());
    store.begin_run(&lease, &"c".repeat(64), 100).await.unwrap();
    sqlx::query("UPDATE proof_runtime_run SET deadline_ms=1 WHERE experiment_id=$1")
        .bind(lease.experiment_id)
        .execute(f.database.pool())
        .await
        .unwrap();
    assert!(store
        .begin_run(&lease, &"c".repeat(64), 1000)
        .await
        .is_err());
    f.close().await;
}

#[tokio::test]
async fn new_signed_cancellation_bypasses_old_retry_schedule() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let (e, lease, _) = f.quoted().await;
    let store = WorkStore::new(f.database.app_pool().await.unwrap());
    store.defer(&lease, 300).await.unwrap();
    f.store.release(&lease).await.unwrap();
    assert!(store.candidates(64, false).await.unwrap().is_empty());
    cancel(&f, e.id).await;
    assert_eq!(store.candidates(64, true).await.unwrap()[0].id, e.id);
    f.close().await;
}

#[tokio::test]
async fn expired_consent_refresh_suppresses_pending_rent() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let e = approved(&f).await;
    let lease = f.store.acquire(e.id, Uuid::new_v4(), 60).await.unwrap();
    // Clock-expiry fixture. The original signed quote stays unusable.
    sqlx::query("UPDATE proof_machine_quote SET quote=jsonb_set(quote,'{expires_at}','1') WHERE experiment_id=$1")
        .bind(e.id).execute(f.database.pool()).await.unwrap();
    let store = WorkStore::new(f.database.app_pool().await.unwrap());
    store.refresh_expired_quote(&lease).await.unwrap();
    assert_eq!(
        f.store
            .experiment(e.id, &e.miner_hotkey)
            .await
            .unwrap()
            .state,
        ExperimentState::Discussion
    );
    assert!(f
        .store
        .intents(e.id, &e.miner_hotkey)
        .await
        .unwrap()
        .iter()
        .all(|intent| intent.status == proof_autonomy_pg::IntentStatus::Cancelled));
    f.close().await;
}
