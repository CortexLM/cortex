#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use common::{consent, miner, sign_action, signed, Fixture, SEED};
use proof_autonomy::{ContractError, ExperimentState};
use proof_autonomy_pg::{CancelExperiment, IntentKind, IntentStatus, PgStore, StoreError};
use uuid::Uuid;

#[tokio::test]
async fn consent_and_dispatch_survive_new_connections() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let (experiment, lease, quote) = f.quoted().await;
    let approved = f
        .store
        .consent(experiment.id, 1, &consent(&quote))
        .await
        .unwrap();
    assert_eq!(approved.state, ExperimentState::Approved);
    assert_eq!(approved.revision, 2);

    let restarted = PgStore::new(f.database.app_pool().await.unwrap());
    assert_eq!(
        restarted
            .experiment(experiment.id, &quote.miner_hotkey)
            .await
            .unwrap(),
        approved
    );
    let intents = restarted
        .intents(experiment.id, &quote.miner_hotkey)
        .await
        .unwrap();
    assert_eq!(intents.len(), 1);
    assert_eq!(intents[0].status, IntentStatus::Pending);
    let dispatched = restarted
        .begin_provision(&lease, 2, intents[0].id)
        .await
        .unwrap();
    assert_eq!(dispatched, quote);
    let intents = restarted
        .intents(experiment.id, &quote.miner_hotkey)
        .await
        .unwrap();
    assert_eq!(intents[0].status, IntentStatus::Dispatched);
    assert_eq!(intents[0].controller_fence, Some(lease.fence));
    assert!(restarted
        .begin_provision(&lease, 3, intents[0].id)
        .await
        .is_err());
    let events = restarted
        .events(experiment.id, &quote.miner_hotkey)
        .await
        .unwrap();
    assert_eq!(
        events.iter().map(|e| e.revision).collect::<Vec<_>>(),
        vec![0, 1, 2, 3]
    );
    assert_eq!(events[3].kind, "provision_dispatched");
    f.close().await;
}

#[tokio::test]
async fn simultaneous_creates_consume_one_nonce() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let request = f.request();
    let action = signed(&request, "/v2/experiments", f.now().await, &SEED);
    let (a, b) = tokio::join!(
        f.store.create_experiment(&request, &action),
        f.store.create_experiment(&request, &action),
    );
    assert!(matches!(
        (a, b),
        (Ok(_), Err(StoreError::Replay)) | (Err(StoreError::Replay), Ok(_))
    ));
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM proof_action_nonce")
        .fetch_one(f.database.pool())
        .await
        .unwrap();
    assert_eq!(count, 1);
    assert_eq!(
        f.store
            .events(request.id, &action.miner_hotkey)
            .await
            .unwrap()
            .len(),
        1
    );
    f.close().await;
}

#[tokio::test]
async fn failed_create_rolls_back_its_nonce() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let mut request = f.request();
    request.account_id = Uuid::new_v4();
    let action = signed(&request, "/v2/experiments", f.now().await, &SEED);
    assert_eq!(
        f.store
            .create_experiment(&request, &action)
            .await
            .unwrap_err(),
        StoreError::Scope
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM proof_action_nonce")
        .fetch_one(f.database.pool())
        .await
        .unwrap();
    assert_eq!(count, 0);
    let mut account = f.account.clone();
    account.id = request.account_id;
    account.credential_ref = Uuid::new_v4();
    f.store.register_account(&account).await.unwrap();
    f.store.create_experiment(&request, &action).await.unwrap();
    f.close().await;
}

#[tokio::test]
async fn duplicate_id_conflict_does_not_burn_a_new_nonce() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let experiment = f.create().await;
    let mut request = f.request();
    request.id = experiment.id;
    let action = signed(&request, "/v2/experiments", f.now().await, &SEED);
    assert_eq!(
        f.store
            .create_experiment(&request, &action)
            .await
            .unwrap_err(),
        StoreError::Conflict
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM proof_action_nonce WHERE nonce = $1")
        .bind(action.nonce)
        .fetch_one(f.database.pool())
        .await
        .unwrap();
    assert_eq!(count, 0);
    f.close().await;
}

#[tokio::test]
async fn owner_body_path_and_expiry_are_authenticated() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let request = f.request();
    let now = f.now().await;
    let wrong_owner = signed(&request, "/v2/experiments", now, &[9; 32]);
    assert_eq!(
        f.store
            .create_experiment(&request, &wrong_owner)
            .await
            .unwrap_err(),
        StoreError::Scope
    );
    let mut bad = signed(&request, "/v2/experiments", now, &SEED);
    bad.path = "/other".into();
    assert!(f.store.create_experiment(&request, &bad).await.is_err());
    bad = signed(&request, "/v2/experiments", now, &SEED);
    bad.expires_at = now;
    sign_action(&mut bad, &SEED);
    assert_eq!(
        f.store.create_experiment(&request, &bad).await.unwrap_err(),
        StoreError::Contract(ContractError::Expired),
    );
    let good = signed(&request, "/v2/experiments", now, &SEED);
    let mut altered = request.clone();
    altered.recipe_digest = "b".repeat(64);
    assert!(f.store.create_experiment(&altered, &good).await.is_err());
    f.store.create_experiment(&request, &good).await.unwrap();
    assert_eq!(
        f.store
            .experiment(request.id, &miner(&[9; 32]))
            .await
            .unwrap_err(),
        StoreError::Scope
    );
    assert!(f.store.intents(request.id, &miner(&[9; 32])).await.is_err());
    assert!(f.store.events(request.id, &miner(&[9; 32])).await.is_err());
    f.close().await;
}

#[tokio::test]
async fn concurrent_consent_creates_exactly_one_rent_intent() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let (experiment, _, quote) = f.quoted().await;
    let signed = consent(&quote);
    let (a, b) = tokio::join!(
        f.store.consent(experiment.id, 1, &signed),
        f.store.consent(experiment.id, 1, &signed),
    );
    assert!(matches!(
        (a, b),
        (Ok(_), Err(StoreError::Conflict)) | (Err(StoreError::Conflict), Ok(_))
    ));
    assert_eq!(
        f.store
            .intents(experiment.id, &quote.miner_hotkey)
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(f.store.consent(experiment.id, 2, &signed).await.is_err());
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM proof_quote_consent")
        .fetch_one(f.database.pool())
        .await
        .unwrap();
    assert_eq!(count, 1);
    f.close().await;
}

#[tokio::test]
async fn stale_or_altered_consent_has_no_effect() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let (experiment, _, quote) = f.quoted().await;
    assert_eq!(
        f.store
            .consent(experiment.id, 0, &consent(&quote))
            .await
            .unwrap_err(),
        StoreError::Conflict,
    );
    let mut changed = quote.clone();
    changed.offer_id = "substituted-offer".into();
    assert!(f
        .store
        .consent(experiment.id, 1, &consent(&changed))
        .await
        .is_err());
    assert_eq!(
        f.store
            .experiment(experiment.id, &quote.miner_hotkey)
            .await
            .unwrap(),
        experiment
    );
    assert!(f
        .store
        .intents(experiment.id, &quote.miner_hotkey)
        .await
        .unwrap()
        .is_empty());
    f.store
        .consent(experiment.id, 1, &consent(&quote))
        .await
        .unwrap();
    f.close().await;
}

#[tokio::test]
async fn cancellation_revokes_ownership_without_claiming_deletion() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let (experiment, lease, quote) = f.quoted().await;
    f.store
        .consent(experiment.id, 1, &consent(&quote))
        .await
        .unwrap();
    let request = CancelExperiment {
        experiment_id: experiment.id,
        revision: 2,
    };
    let path = format!("/v2/experiments/{}/cancel", experiment.id);
    let outsider = signed(&request, &path, f.now().await, &[9; 32]);
    assert_eq!(
        f.store.cancel(&request, &outsider).await.unwrap_err(),
        StoreError::Scope
    );
    let action = signed(&request, &path, f.now().await, &SEED);
    let cancelled = f.store.cancel(&request, &action).await.unwrap();
    assert_eq!(cancelled.state, ExperimentState::Cancelling);
    assert!(!cancelled.state.terminal());
    assert_eq!(
        f.store.renew(&lease, 60).await.unwrap_err(),
        StoreError::Fenced
    );
    let intents = f
        .store
        .intents(experiment.id, &quote.miner_hotkey)
        .await
        .unwrap();
    assert_eq!(intents.len(), 2);
    assert!(intents
        .iter()
        .any(|i| i.kind == IntentKind::Cancel && i.status == IntentStatus::Pending));
    let rent = intents
        .iter()
        .find(|i| i.kind == IntentKind::Provision)
        .unwrap();
    assert_eq!(rent.status, IntentStatus::Cancelled);
    assert!(f
        .store
        .begin_provision(&lease, cancelled.revision, rent.id)
        .await
        .is_err());
    assert!(f.store.cancel(&request, &action).await.is_err());
    let events = f
        .store
        .events(experiment.id, &quote.miner_hotkey)
        .await
        .unwrap();
    assert_eq!(events.last().unwrap().kind, "cancel_requested");
    f.close().await;
}

#[tokio::test]
async fn expired_while_waiting_for_a_lock_rolls_back_the_entire_action() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let mut lock = f.database.pool().begin().await.unwrap();
    sqlx::query("SELECT id FROM proof_miner_account WHERE id = $1 FOR UPDATE")
        .bind(f.account.id)
        .execute(&mut *lock)
        .await
        .unwrap();
    let request = f.request();
    let now = f.now().await;
    let mut action = signed(&request, "/v2/experiments", now, &SEED);
    action.expires_at = now + 1;
    sign_action(&mut action, &SEED);
    let release = async move {
        sqlx::query("SELECT pg_sleep(1.1)")
            .execute(&mut *lock)
            .await
            .unwrap();
        lock.commit().await.unwrap();
    };
    let (result, ()) = tokio::join!(f.store.create_experiment(&request, &action), release);
    assert_eq!(
        result.unwrap_err(),
        StoreError::Contract(ContractError::Expired)
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM proof_action_nonce")
        .fetch_one(f.database.pool())
        .await
        .unwrap();
    assert_eq!(count, 0);
    assert!(f
        .store
        .experiment(request.id, &action.miner_hotkey)
        .await
        .is_err());
    f.close().await;
}

#[tokio::test]
async fn cancellation_and_consent_are_serialized_by_revision() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let (experiment, _, quote) = f.quoted().await;
    let mut request = CancelExperiment {
        experiment_id: experiment.id,
        revision: 1,
    };
    let path = format!("/v2/experiments/{}/cancel", experiment.id);
    let action = signed(&request, &path, f.now().await, &SEED);
    let authorization = consent(&quote);
    let (approved, cancelled) = tokio::join!(
        f.store.consent(experiment.id, 1, &authorization),
        f.store.cancel(&request, &action),
    );
    match (approved, cancelled) {
        (Ok(approved), Err(StoreError::Conflict)) => {
            request.revision = approved.revision;
            let action = signed(&request, &path, f.now().await, &SEED);
            f.store.cancel(&request, &action).await.unwrap();
        }
        (Err(StoreError::Conflict), Ok(_)) => {}
        other => panic!("only one revision can commit: {other:?}"),
    }
    let intents = f
        .store
        .intents(experiment.id, &quote.miner_hotkey)
        .await
        .unwrap();
    assert!(intents
        .iter()
        .all(|i| i.kind != IntentKind::Provision || i.status == IntentStatus::Cancelled));
    assert_eq!(
        f.store
            .experiment(experiment.id, &quote.miner_hotkey)
            .await
            .unwrap()
            .state,
        ExperimentState::Cancelling
    );
    f.close().await;
}

#[tokio::test]
async fn replacement_quote_cannot_dispatch_an_old_consent() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let (experiment, lease, old) = f.quoted().await;
    f.store
        .consent(experiment.id, 1, &consent(&old))
        .await
        .unwrap();
    let old_intent = f
        .store
        .intents(experiment.id, &old.miner_hotkey)
        .await
        .unwrap()[0]
        .clone();
    let mut quote = old.clone();
    quote.id = Uuid::new_v4();
    quote.offer_id = "replacement-offer".into();
    f.store.publish_quote(&lease, 2, &quote).await.unwrap();
    assert!(f
        .store
        .consent(experiment.id, 3, &consent(&old))
        .await
        .is_err());
    assert!(f
        .store
        .begin_provision(&lease, 3, old_intent.id)
        .await
        .is_err());
    f.store
        .consent(experiment.id, 3, &consent(&quote))
        .await
        .unwrap();
    let intents = f
        .store
        .intents(experiment.id, &quote.miner_hotkey)
        .await
        .unwrap();
    assert_eq!(
        intents
            .iter()
            .find(|i| i.id == old_intent.id)
            .unwrap()
            .status,
        IntentStatus::Cancelled
    );
    let new_intent = intents
        .iter()
        .find(|i| i.quote_id == Some(quote.id))
        .unwrap();
    assert_eq!(
        f.store
            .begin_provision(&lease, 4, new_intent.id)
            .await
            .unwrap(),
        quote
    );
    f.close().await;
}
