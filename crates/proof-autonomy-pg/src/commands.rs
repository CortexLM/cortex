use proof_autonomy::{is_digest, ExperimentState, SignedAction};
use sqlx::Row;
use uuid::Uuid;

use crate::{
    transaction::{
        active_account, advance, consume_action, decode, event, lock_experiment, verify_action,
        ExperimentRow,
    },
    CancelExperiment, CreateExperiment, Experiment, ExperimentEvent, MinerAccount, PgStore,
    ServiceIntent, StoreError,
};

impl PgStore {
    /// Register a broker-verified miner account with an opaque keystore reference.
    /// This is a trusted provisioning operation, not a public registration API.
    ///
    /// # Errors
    /// Invalid identity, existing binding or database failure.
    pub async fn register_account(&self, account: &MinerAccount) -> Result<(), StoreError> {
        if account.id.is_nil()
            || account.credential_ref.is_nil()
            || !is_digest(&account.miner_hotkey)
        {
            return Err(StoreError::Scope);
        }
        sqlx::query(
            "INSERT INTO proof_miner_account (id, miner_hotkey, credential_ref) VALUES ($1, $2, $3)",
        )
        .bind(account.id)
        .bind(&account.miner_hotkey)
        .bind(account.credential_ref)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Create an experiment and consume its signed request in one transaction.
    ///
    /// # Errors
    /// Invalid/replayed action, account mismatch or conflicting experiment id.
    pub async fn create_experiment(
        &self,
        request: &CreateExperiment,
        action: &SignedAction,
    ) -> Result<Experiment, StoreError> {
        if request.id.is_nil() || !is_digest(&request.recipe_digest) {
            return Err(StoreError::Scope);
        }
        let mut tx = self.pool.begin().await?;
        consume_action(&mut tx, action, "/v2/experiments", request).await?;
        // Serialize intake for this account. Cancellation never needs this
        // quota lock and remains available even when the account is exhausted.
        sqlx::query("SELECT id FROM proof_miner_account WHERE id = $1 FOR UPDATE")
            .bind(request.account_id)
            .execute(&mut *tx)
            .await?;
        active_account(&mut tx, request.account_id, &action.miner_hotkey).await?;
        let available: bool = sqlx::query_scalar(
            "SELECT count(*) FILTER (WHERE state NOT IN ('completed', 'cancelled', 'rejected')) < 8 \
             AND count(*) FILTER (WHERE created_at > clock_timestamp() - interval '1 hour') < 32 \
             FROM proof_experiment WHERE account_id = $1",
        )
        .bind(request.account_id).fetch_one(&mut *tx).await?;
        if !available {
            return Err(StoreError::Quota);
        }
        sqlx::query(
            "INSERT INTO proof_experiment (id, miner_hotkey, account_id, recipe_digest, state) \
             VALUES ($1, $2, $3, $4, 'discussion')",
        )
        .bind(request.id)
        .bind(&action.miner_hotkey)
        .bind(request.account_id)
        .bind(&request.recipe_digest)
        .execute(&mut *tx)
        .await?;
        let experiment = lock_experiment(&mut tx, request.id, Some(0)).await?;
        event(&mut tx, &experiment, "created").await?;
        verify_action(&mut tx, action, "/v2/experiments", request).await?;
        tx.commit().await?;
        Ok(experiment)
    }

    /// Owner-scoped read; account revocation does not hide cancellation status.
    ///
    /// # Errors
    /// Missing record, wrong owner, corrupt data or database failure.
    pub async fn experiment(&self, id: Uuid, miner: &str) -> Result<Experiment, StoreError> {
        let row: ExperimentRow =
            sqlx::query_as("SELECT * FROM proof_experiment WHERE id = $1 AND miner_hotkey = $2")
                .bind(id)
                .bind(miner)
                .fetch_optional(&self.pool)
                .await?
                .ok_or(StoreError::Scope)?;
        row.decode()
    }

    /// Cancel without asking the agent. Pending rents are suppressed; uncertain
    /// dispatches are retained for cleanup, never reported as deleted.
    ///
    /// # Errors
    /// Invalid signature, nonce replay, stale revision, wrong owner or terminal state.
    pub async fn cancel(
        &self,
        request: &CancelExperiment,
        action: &SignedAction,
    ) -> Result<Experiment, StoreError> {
        let mut tx = self.pool.begin().await?;
        let mut experiment =
            lock_experiment(&mut tx, request.experiment_id, Some(request.revision)).await?;
        let path = format!("/v2/experiments/{}/cancel", request.experiment_id);
        consume_action(&mut tx, action, &path, request).await?;
        if action.miner_hotkey != experiment.miner_hotkey {
            return Err(StoreError::Scope);
        }
        advance(
            &mut tx,
            &mut experiment,
            ExperimentState::Cancelling,
            "cancel_requested",
        )
        .await?;
        sqlx::query(
            "UPDATE proof_controller_lease SET expires_at = clock_timestamp() \
             WHERE experiment_id = $1",
        )
        .bind(experiment.id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE proof_service_intent \
             SET status = CASE WHEN status = 'pending' THEN 'cancelled' ELSE 'reconcile' END \
             WHERE experiment_id = $1 AND kind = 'provision' AND status IN ('pending', 'dispatched')",
        )
        .bind(experiment.id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO proof_service_intent (id, experiment_id, kind) VALUES ($1, $2, 'cancel')",
        )
        .bind(Uuid::new_v4())
        .bind(experiment.id)
        .execute(&mut *tx)
        .await?;
        verify_action(&mut tx, action, &path, request).await?;
        tx.commit().await?;
        Ok(experiment)
    }

    /// Read committed intents, including unresolved dispatches after restart.
    ///
    /// # Errors
    /// Wrong owner, malformed records or database failure.
    pub async fn intents(&self, id: Uuid, miner: &str) -> Result<Vec<ServiceIntent>, StoreError> {
        self.experiment(id, miner).await?;
        let rows = sqlx::query(
            "SELECT * FROM proof_service_intent WHERE experiment_id = $1 ORDER BY created_at, id",
        )
        .bind(id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                Ok(ServiceIntent {
                    id: row.try_get("id")?,
                    experiment_id: row.try_get("experiment_id")?,
                    kind: decode(row.try_get("kind")?)?,
                    quote_id: row.try_get("quote_id")?,
                    status: decode(row.try_get("status")?)?,
                    controller_fence: row.try_get("controller_fence")?,
                })
            })
            .collect()
    }

    /// Ordered immutable lifecycle history, scoped to its miner.
    ///
    /// # Errors
    /// Wrong owner, malformed records or database failure.
    pub async fn events(&self, id: Uuid, miner: &str) -> Result<Vec<ExperimentEvent>, StoreError> {
        self.experiment(id, miner).await?;
        let rows = sqlx::query(
            "SELECT revision, kind, state FROM proof_experiment_event \
             WHERE experiment_id = $1 ORDER BY revision",
        )
        .bind(id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                Ok(ExperimentEvent {
                    revision: row.try_get("revision")?,
                    kind: row.try_get("kind")?,
                    state: decode(row.try_get("state")?)?,
                })
            })
            .collect()
    }
}
