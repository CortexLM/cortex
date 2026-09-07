#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use common::{consent, signed, Fixture, SEED};
use proof_autonomy::ExperimentState;
use proof_autonomy_pg::{CancelExperiment, IntentKind, IntentStatus, StoreError};
use uuid::Uuid;

#[tokio::test]
async fn simultaneous_controllers_have_one_owner() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let experiment = f.create().await;
    let (a, b) = tokio::join!(
        f.store.acquire(experiment.id, Uuid::new_v4(), 60),
        f.store.acquire(experiment.id, Uuid::new_v4(), 60),
    );
    let lease = match (a, b) {
        (Ok(lease), Err(StoreError::Fenced)) | (Err(StoreError::Fenced), Ok(lease)) => lease,
        other => panic!("must have one live controller: {other:?}"),
    };
    assert_eq!(lease.fence, 1);
    f.store.renew(&lease, 60).await.unwrap();
    assert_eq!(
        f.store
            .acquire(experiment.id, lease.owner_id, 60)
            .await
            .unwrap_err(),
        StoreError::Fenced
    );
    f.close().await;
}

#[tokio::test]
async fn takeover_and_release_never_reuse_a_fence() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let (experiment, lease, _) = f.quoted().await;
    f.expire(&lease).await;
    assert_eq!(
        f.store.renew(&lease, 60).await.unwrap_err(),
        StoreError::Fenced
    );
    let new = f
        .store
        .acquire(experiment.id, Uuid::new_v4(), 60)
        .await
        .unwrap();
    assert_eq!(new.fence, lease.fence + 1);
    assert_eq!(
        f.store.release(&lease).await.unwrap_err(),
        StoreError::Fenced
    );
    f.store.release(&new).await.unwrap();
    let third = f
        .store
        .acquire(experiment.id, new.owner_id, 60)
        .await
        .unwrap();
    assert_eq!(third.fence, new.fence + 1);
    assert_eq!(
        f.store.renew(&new, 60).await.unwrap_err(),
        StoreError::Fenced
    );
    f.close().await;
}

#[tokio::test]
async fn obsolete_controller_cannot_write_or_cross_experiments() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let (experiment, lease, quote) = f.quoted().await;
    f.store
        .consent(experiment.id, 1, &consent(&quote))
        .await
        .unwrap();
    let intent = f
        .store
        .intents(experiment.id, &quote.miner_hotkey)
        .await
        .unwrap()[0]
        .clone();
    f.expire(&lease).await;
    let new = f
        .store
        .acquire(experiment.id, Uuid::new_v4(), 60)
        .await
        .unwrap();
    assert_eq!(
        f.store
            .begin_provision(&lease, 2, intent.id)
            .await
            .unwrap_err(),
        StoreError::Fenced
    );
    let other = f.create().await;
    let other_lease = f.store.acquire(other.id, Uuid::new_v4(), 60).await.unwrap();
    assert!(f
        .store
        .begin_provision(&other_lease, 0, intent.id)
        .await
        .is_err());
    assert_eq!(
        f.store
            .publish_quote(&other_lease, 0, &quote)
            .await
            .unwrap_err(),
        StoreError::Scope
    );
    f.store.begin_provision(&new, 2, intent.id).await.unwrap();
    f.close().await;
}

#[tokio::test]
async fn controller_crash_requires_reconciliation_not_another_rent() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let (experiment, lease, quote) = f.quoted().await;
    f.store
        .consent(experiment.id, 1, &consent(&quote))
        .await
        .unwrap();
    let intent = f
        .store
        .intents(experiment.id, &quote.miner_hotkey)
        .await
        .unwrap()[0]
        .clone();
    f.store.begin_provision(&lease, 2, intent.id).await.unwrap();
    // The process can die before learning whether its request created a pod.
    f.expire(&lease).await;
    let takeover = f
        .store
        .acquire(experiment.id, Uuid::new_v4(), 60)
        .await
        .unwrap();
    let pending = f
        .store
        .intents(experiment.id, &quote.miner_hotkey)
        .await
        .unwrap();
    assert_eq!(pending[0].status, IntentStatus::Reconcile);
    assert_eq!(pending[0].controller_fence, Some(lease.fence));
    assert!(f
        .store
        .begin_provision(&takeover, 3, intent.id)
        .await
        .is_err());
    assert_eq!(
        f.store
            .experiment(experiment.id, &quote.miner_hotkey)
            .await
            .unwrap()
            .state,
        ExperimentState::Provisioning
    );
    f.close().await;
}

#[tokio::test]
async fn cancellation_of_dispatched_rent_preserves_uncertainty() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let (experiment, lease, quote) = f.quoted().await;
    f.store
        .consent(experiment.id, 1, &consent(&quote))
        .await
        .unwrap();
    let intent = f
        .store
        .intents(experiment.id, &quote.miner_hotkey)
        .await
        .unwrap()[0]
        .clone();
    f.store.begin_provision(&lease, 2, intent.id).await.unwrap();
    let request = CancelExperiment {
        experiment_id: experiment.id,
        revision: 3,
    };
    let path = format!("/v2/experiments/{}/cancel", experiment.id);
    let action = signed(&request, &path, f.now().await, &SEED);
    let cancelled = f.store.cancel(&request, &action).await.unwrap();
    assert_eq!(cancelled.state, ExperimentState::Cancelling);
    let intents = f
        .store
        .intents(experiment.id, &quote.miner_hotkey)
        .await
        .unwrap();
    assert_eq!(
        intents
            .iter()
            .find(|i| i.kind == IntentKind::Provision)
            .unwrap()
            .status,
        IntentStatus::Reconcile
    );
    assert_eq!(
        f.store.renew(&lease, 60).await.unwrap_err(),
        StoreError::Fenced
    );
    f.store
        .acquire(experiment.id, Uuid::new_v4(), 60)
        .await
        .unwrap();
    f.close().await;
}

#[tokio::test]
async fn revoked_accounts_cannot_approve_but_can_cancel() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let (experiment, _, quote) = f.quoted().await;
    sqlx::query("UPDATE proof_miner_account SET revoked = true WHERE id = $1")
        .bind(f.account.id)
        .execute(f.database.pool())
        .await
        .unwrap();
    assert_eq!(
        f.store
            .consent(experiment.id, 1, &consent(&quote))
            .await
            .unwrap_err(),
        StoreError::Scope
    );
    let request = CancelExperiment {
        experiment_id: experiment.id,
        revision: 1,
    };
    let path = format!("/v2/experiments/{}/cancel", experiment.id);
    let action = signed(&request, &path, f.now().await, &SEED);
    f.store.cancel(&request, &action).await.unwrap();
    f.close().await;
}

#[tokio::test]
async fn invalid_lease_duration_does_not_create_ownership() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let experiment = f.create().await;
    for duration in [0, 301, u32::MAX] {
        assert_eq!(
            f.store
                .acquire(experiment.id, Uuid::new_v4(), duration)
                .await
                .unwrap_err(),
            StoreError::Fenced
        );
    }
    assert_eq!(
        f.store
            .acquire(experiment.id, Uuid::nil(), 60)
            .await
            .unwrap_err(),
        StoreError::Fenced
    );
    let lease = f
        .store
        .acquire(experiment.id, Uuid::new_v4(), 60)
        .await
        .unwrap();
    assert_eq!(lease.fence, 1);
    f.close().await;
}
