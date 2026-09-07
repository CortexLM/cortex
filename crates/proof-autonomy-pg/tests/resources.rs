#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use common::{consent, signed, Fixture, SEED};
use proof_autonomy::{CapabilityOperation, DeletionResult, ExperimentState, ProvisionResult};
use proof_autonomy_pg::{CancelExperiment, ControllerLease, Experiment, StoreError};
use uuid::Uuid;

async fn dispatched(f: &Fixture) -> (Experiment, ControllerLease, Uuid) {
    let (e, lease, quote) = f.quoted().await;
    let e = f
        .store
        .consent(e.id, e.revision, &consent(&quote))
        .await
        .unwrap();
    let intent = f.store.intents(e.id, &e.miner_hotkey).await.unwrap()[0].id;
    f.store
        .begin_provision(&lease, e.revision, intent)
        .await
        .unwrap();
    (
        f.store.experiment(e.id, &e.miner_hotkey).await.unwrap(),
        lease,
        intent,
    )
}

fn confirmed(id: &str) -> ProvisionResult {
    ProvisionResult::Confirmed {
        resource_id: id.into(),
    }
}

#[tokio::test]
async fn contradictory_late_resource_after_terminal_state_can_only_be_cleaned() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let (e, old, intent) = dispatched(&f).await;
    f.store
        .record_provision(&old, intent, &ProvisionResult::NotCreated)
        .await
        .unwrap();
    assert_eq!(
        f.store.finish_cleanup(&old).await.unwrap().state,
        ExperimentState::Rejected
    );
    f.store.release(&old).await.unwrap();
    assert!(f.store.acquire(e.id, Uuid::new_v4(), 60).await.is_err());
    f.store
        .record_provision(&old, intent, &confirmed("contradictory-late-pod"))
        .await
        .unwrap();
    let lease = f.store.acquire(e.id, Uuid::new_v4(), 60).await.unwrap();
    assert!(f
        .store
        .authorize_resource(
            &lease,
            "contradictory-late-pod",
            CapabilityOperation::Execute
        )
        .await
        .is_err());
    assert!(f.store.adopt_resource(&lease, e.revision).await.is_err());
    assert!(f.store.finish_cleanup(&lease).await.is_err());
    let target = f
        .store
        .begin_cleanup(&lease, "contradictory-late-pod")
        .await
        .unwrap();
    f.store
        .record_deletion(
            &lease,
            &target.resource.resource_id,
            target.resource.deletion_id.unwrap(),
            &DeletionResult::Confirmed,
        )
        .await
        .unwrap();
    assert_eq!(
        f.store.finish_cleanup(&lease).await.unwrap().state,
        ExperimentState::Rejected
    );
    f.store.release(&lease).await.unwrap();
    assert!(f.store.acquire(e.id, Uuid::new_v4(), 60).await.is_err());
    f.close().await;
}

async fn cancel(f: &Fixture, e: &Experiment) -> ControllerLease {
    let request = CancelExperiment {
        experiment_id: e.id,
        revision: e.revision,
    };
    let path = format!("/v2/experiments/{}/cancel", e.id);
    f.store
        .cancel(&request, &signed(&request, &path, f.now().await, &SEED))
        .await
        .unwrap();
    f.store.acquire(e.id, Uuid::new_v4(), 60).await.unwrap()
}

#[tokio::test]
async fn confirmed_resource_is_quarantined_until_fenced_adoption() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let (e, lease, intent) = dispatched(&f).await;
    f.store
        .record_provision(&lease, intent, &confirmed("pod-1"))
        .await
        .unwrap();
    assert!(f
        .store
        .authorize_resource(&lease, "pod-1", CapabilityOperation::Execute)
        .await
        .is_err());
    f.store.adopt_resource(&lease, e.revision).await.unwrap();
    let grant = f
        .store
        .authorize_resource(&lease, "pod-1", CapabilityOperation::Execute)
        .await
        .unwrap();
    assert_eq!(grant.account_id, f.account.id);
    assert_eq!(grant.experiment_id, e.id);
    assert!(f
        .store
        .authorize_resource(&lease, "pod-2", CapabilityOperation::Inspect)
        .await
        .is_err());
    assert!(f
        .store
        .authorize_resource(&lease, "pod-1", CapabilityOperation::Delete)
        .await
        .is_err());
    let resources = f.store.resources(e.id, &e.miner_hotkey).await.unwrap();
    assert_eq!(
        grant.expires_at,
        u64::try_from(resources[0].authorized_until).unwrap()
    );
    f.close().await;
}

#[tokio::test]
async fn cancellation_retains_late_rental_and_requires_verified_deletion() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let (e, old, intent) = dispatched(&f).await;
    let lease = cancel(&f, &e).await;
    assert_eq!(
        f.store.finish_cleanup(&lease).await,
        Err(StoreError::Conflict)
    );
    f.store
        .record_provision(&old, intent, &confirmed("late-pod"))
        .await
        .unwrap();
    assert!(f.store.adopt_resource(&old, e.revision).await.is_err());
    assert!(f
        .store
        .authorize_resource(&lease, "late-pod", CapabilityOperation::Execute)
        .await
        .is_err());
    let target = f.store.begin_cleanup(&lease, "late-pod").await.unwrap();
    let deletion = target.resource.deletion_id.unwrap();
    assert_eq!(target.account.credential_ref, f.account.credential_ref);
    f.store
        .record_deletion(&lease, "late-pod", deletion, &DeletionResult::Pending)
        .await
        .unwrap();
    assert_eq!(
        f.store.finish_cleanup(&lease).await,
        Err(StoreError::Conflict)
    );
    let retry = f.store.begin_cleanup(&lease, "late-pod").await.unwrap();
    assert_eq!(retry.resource.deletion_id, Some(deletion));
    f.store
        .record_deletion(&lease, "late-pod", deletion, &DeletionResult::Confirmed)
        .await
        .unwrap();
    assert_eq!(
        f.store.finish_cleanup(&lease).await.unwrap().state,
        ExperimentState::Cancelled
    );
    f.close().await;
}

#[tokio::test]
async fn uncertain_creation_never_returns_to_pending_after_takeover() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let (e, old, intent) = dispatched(&f).await;
    f.store
        .record_provision(&old, intent, &ProvisionResult::Uncertain)
        .await
        .unwrap();
    f.expire(&old).await;
    let lease = f.store.acquire(e.id, Uuid::new_v4(), 60).await.unwrap();
    assert!(f.store.provision_context(&lease, intent).await.is_err());
    assert!(f
        .store
        .begin_provision(&lease, e.revision, intent)
        .await
        .is_err());
    assert_eq!(
        f.store.finish_cleanup(&lease).await,
        Err(StoreError::Conflict)
    );
    f.store
        .record_provision(&old, intent, &ProvisionResult::NotCreated)
        .await
        .unwrap();
    assert_eq!(
        f.store.finish_cleanup(&lease).await.unwrap().state,
        ExperimentState::Rejected
    );
    f.close().await;
}

#[tokio::test]
async fn resource_binding_and_cleanup_cannot_cross_experiments() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let (a, la, ia) = dispatched(&f).await;
    let (b, lb, ib) = dispatched(&f).await;
    f.store
        .record_provision(&la, ia, &confirmed("same-pod"))
        .await
        .unwrap();
    assert_eq!(
        f.store
            .record_provision(&lb, ib, &confirmed("same-pod"))
            .await,
        Err(StoreError::Scope)
    );
    assert!(f
        .store
        .resources(b.id, &b.miner_hotkey)
        .await
        .unwrap()
        .is_empty());
    assert!(f.store.begin_cleanup(&lb, "same-pod").await.is_err());
    assert!(f
        .store
        .record_provision(&lb, ia, &confirmed("other"))
        .await
        .is_err());
    assert!(f.store.resources(a.id, &"f".repeat(64)).await.is_err());
    f.close().await;
}

#[tokio::test]
async fn expired_or_revoked_account_cannot_execute_but_can_be_cleaned() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let (e, lease, intent) = dispatched(&f).await;
    f.store
        .record_provision(&lease, intent, &confirmed("pod-1"))
        .await
        .unwrap();
    let e = f.store.adopt_resource(&lease, e.revision).await.unwrap();
    sqlx::query("UPDATE proof_resource SET authorized_until = 1 WHERE experiment_id = $1")
        .bind(e.id)
        .execute(f.database.pool())
        .await
        .unwrap();
    assert!(f
        .store
        .authorize_resource(&lease, "pod-1", CapabilityOperation::Execute)
        .await
        .is_err());
    sqlx::query("UPDATE proof_miner_account SET revoked = true WHERE id = $1")
        .bind(f.account.id)
        .execute(f.database.pool())
        .await
        .unwrap();
    let lease = cancel(&f, &e).await;
    assert!(f.store.begin_cleanup(&lease, "pod-1").await.is_ok());
    f.close().await;
}

#[tokio::test]
async fn stale_deletion_confirmation_cannot_commit_after_takeover() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let (_, old, intent) = dispatched(&f).await;
    f.store
        .record_provision(&old, intent, &confirmed("pod"))
        .await
        .unwrap();
    let target = f.store.begin_cleanup(&old, "pod").await.unwrap();
    f.expire(&old).await;
    let lease = f
        .store
        .acquire(old.experiment_id, Uuid::new_v4(), 60)
        .await
        .unwrap();
    assert_eq!(
        f.store
            .record_deletion(
                &old,
                "pod",
                target.resource.deletion_id.unwrap(),
                &DeletionResult::Confirmed
            )
            .await,
        Err(StoreError::Fenced)
    );
    assert_eq!(
        f.store.finish_cleanup(&lease).await,
        Err(StoreError::Conflict)
    );
    let target = f.store.begin_cleanup(&lease, "pod").await.unwrap();
    f.store
        .record_deletion(
            &lease,
            "pod",
            target.resource.deletion_id.unwrap(),
            &DeletionResult::Confirmed,
        )
        .await
        .unwrap();
    assert_eq!(
        f.store.finish_cleanup(&lease).await.unwrap().state,
        ExperimentState::Rejected
    );
    f.close().await;
}

#[tokio::test]
async fn duplicate_receipts_do_not_extend_spending_deadline() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let (e, lease, intent) = dispatched(&f).await;
    f.store
        .record_provision(&lease, intent, &confirmed("pod"))
        .await
        .unwrap();
    let first = f.store.resources(e.id, &e.miner_hotkey).await.unwrap();
    f.store
        .record_provision(&lease, intent, &confirmed("pod"))
        .await
        .unwrap();
    f.store
        .record_provision(&lease, intent, &ProvisionResult::Uncertain)
        .await
        .unwrap();
    let second = f.store.resources(e.id, &e.miner_hotkey).await.unwrap();
    assert_eq!(first, second);
    assert_eq!(
        f.store.intents(e.id, &e.miner_hotkey).await.unwrap()[0].status,
        proof_autonomy_pg::IntentStatus::Completed
    );
    f.close().await;
}

#[tokio::test]
async fn extra_confirmed_resource_blocks_adoption_and_requires_cleanup() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let (e, lease, intent) = dispatched(&f).await;
    f.store
        .record_provision(&lease, intent, &confirmed("pod-1"))
        .await
        .unwrap();
    f.store
        .record_provision(&lease, intent, &confirmed("pod-2"))
        .await
        .unwrap();
    assert_eq!(
        f.store.adopt_resource(&lease, e.revision).await,
        Err(StoreError::Conflict)
    );
    assert_eq!(
        f.store
            .resources(e.id, &e.miner_hotkey)
            .await
            .unwrap()
            .len(),
        2
    );
    f.close().await;
}

#[tokio::test]
async fn app_role_cannot_rebind_resources_or_rewrite_observations() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let (e, lease, intent) = dispatched(&f).await;
    f.store
        .record_provision(&lease, intent, &confirmed("pod"))
        .await
        .unwrap();
    let app = f.database.app_pool().await.unwrap();
    assert!(
        sqlx::query("UPDATE proof_resource SET authorized_until = authorized_until + 3600")
            .execute(&app)
            .await
            .is_err()
    );
    assert!(sqlx::query("DELETE FROM proof_resource")
        .execute(&app)
        .await
        .is_err());
    assert!(
        sqlx::query("UPDATE proof_provider_observation SET controller_fence = 10")
            .execute(&app)
            .await
            .is_err()
    );
    assert_eq!(
        f.store
            .resources(e.id, &e.miner_hotkey)
            .await
            .unwrap()
            .len(),
        1
    );
    app.close().await;
    f.close().await;
}

#[tokio::test]
async fn unexpected_extra_resource_revokes_an_already_active_grant() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let (e, lease, intent) = dispatched(&f).await;
    f.store
        .record_provision(&lease, intent, &confirmed("pod-1"))
        .await
        .unwrap();
    f.store.adopt_resource(&lease, e.revision).await.unwrap();
    f.store
        .record_provision(&lease, intent, &confirmed("pod-2"))
        .await
        .unwrap();
    assert!(f
        .store
        .authorize_resource(&lease, "pod-1", CapabilityOperation::Execute)
        .await
        .is_err());
    assert!(f.store.begin_cleanup(&lease, "pod-1").await.is_ok());
    assert!(f.store.begin_cleanup(&lease, "pod-2").await.is_ok());
    f.close().await;
}
