#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use common::{consent, Fixture};
use proof_autonomy_pg::StoreError;

#[tokio::test]
async fn app_role_cannot_rewrite_authorizations_or_audit_records() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let (experiment, _, quote) = f.quoted().await;
    f.store
        .consent(experiment.id, 1, &consent(&quote))
        .await
        .unwrap();
    let app = f.database.app_pool().await.unwrap();
    for table in [
        "proof_machine_quote",
        "proof_quote_consent",
        "proof_action_nonce",
        "proof_experiment_event",
    ] {
        let allowed: bool =
            sqlx::query_scalar("SELECT has_table_privilege(current_user, $1, 'UPDATE,DELETE')")
                .bind(table)
                .fetch_one(&app)
                .await
                .unwrap();
        assert!(!allowed, "{table} must be append-only");
        assert!(sqlx::query(&format!("DELETE FROM {table}"))
            .execute(&app)
            .await
            .is_err());
    }
    assert!(
        sqlx::query("UPDATE proof_machine_quote SET quote = '{}'::jsonb")
            .execute(&app)
            .await
            .is_err()
    );
    assert!(sqlx::query("DELETE FROM proof_controller_lease")
        .execute(&app)
        .await
        .is_err());
    f.close().await;
}

#[tokio::test]
async fn dispatch_rechecks_the_stored_quote_and_consent() {
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
    sqlx::query(
        "UPDATE proof_machine_quote SET quote = jsonb_set(quote, '{offer_id}', '\"altered\"') WHERE id = $1",
    )
    .bind(quote.id).execute(f.database.pool()).await.unwrap();
    assert_eq!(
        f.store
            .begin_provision(&lease, 2, intent.id)
            .await
            .unwrap_err(),
        StoreError::Corrupt
    );
    sqlx::query("UPDATE proof_machine_quote SET quote = $2 WHERE id = $1")
        .bind(quote.id)
        .bind(sqlx::types::Json(&quote))
        .execute(f.database.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE proof_quote_consent SET signature = $2 WHERE quote_id = $1")
        .bind(quote.id)
        .bind("0".repeat(128))
        .execute(f.database.pool())
        .await
        .unwrap();
    assert!(f.store.begin_provision(&lease, 2, intent.id).await.is_err());
    assert_eq!(
        f.store
            .intents(experiment.id, &quote.miner_hotkey)
            .await
            .unwrap()[0]
            .status,
        proof_autonomy_pg::IntentStatus::Pending,
    );
    f.close().await;
}

#[tokio::test]
async fn corrupted_stored_quote_cannot_be_approved() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let (experiment, _, quote) = f.quoted().await;
    sqlx::query(
        "UPDATE proof_machine_quote SET quote = jsonb_set(quote, '{offer_id}', '\"altered\"') WHERE id = $1",
    )
    .bind(quote.id).execute(f.database.pool()).await.unwrap();
    assert_eq!(
        f.store
            .consent(experiment.id, 1, &consent(&quote))
            .await
            .unwrap_err(),
        StoreError::Corrupt
    );
    assert!(f
        .store
        .intents(experiment.id, &quote.miner_hotkey)
        .await
        .unwrap()
        .is_empty());
    f.close().await;
}

#[tokio::test]
async fn event_failure_rolls_back_consent_and_intent_together() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let (experiment, _, quote) = f.quoted().await;
    // Force a late write failure after consent insertion and experiment update.
    sqlx::query(
        "INSERT INTO proof_experiment_event (experiment_id, revision, kind, state) \
         VALUES ($1, 2, 'approved', 'approved')",
    )
    .bind(experiment.id)
    .execute(f.database.pool())
    .await
    .unwrap();
    assert_eq!(
        f.store
            .consent(experiment.id, 1, &consent(&quote))
            .await
            .unwrap_err(),
        StoreError::Conflict
    );
    assert_eq!(
        f.store
            .experiment(experiment.id, &quote.miner_hotkey)
            .await
            .unwrap(),
        experiment
    );
    let consumed: i64 = sqlx::query_scalar("SELECT count(*) FROM proof_quote_consent")
        .fetch_one(f.database.pool())
        .await
        .unwrap();
    assert_eq!(consumed, 0);
    assert!(f
        .store
        .intents(experiment.id, &quote.miner_hotkey)
        .await
        .unwrap()
        .is_empty());
    sqlx::query("DELETE FROM proof_experiment_event WHERE experiment_id = $1 AND revision = 2")
        .bind(experiment.id)
        .execute(f.database.pool())
        .await
        .unwrap();
    f.store
        .consent(experiment.id, 1, &consent(&quote))
        .await
        .unwrap();
    f.close().await;
}
