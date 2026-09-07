use proof_autonomy::{commitment, ExperimentState, MachineQuote, SignedConsent};
use sqlx::{types::Json, Row};
use uuid::Uuid;

use crate::{
    transaction::{active_account, advance, lock_experiment, now, require_lease, Tx},
    ControllerLease, Experiment, PgStore, StoreError,
};

impl PgStore {
    /// Freeze the exact recipe, account and price under a live controller.
    ///
    /// # Errors
    /// Stale fence/revision, invalid quote, wrong account/recipe or duplicate quote.
    pub async fn publish_quote(
        &self,
        lease: &ControllerLease,
        revision: i64,
        quote: &MachineQuote,
    ) -> Result<Experiment, StoreError> {
        if quote.experiment_id != lease.experiment_id {
            return Err(StoreError::Scope);
        }
        let mut tx = self.pool.begin().await?;
        let mut experiment = lock_experiment(&mut tx, lease.experiment_id, Some(revision)).await?;
        require_lease(&mut tx, lease).await?;
        active_account(&mut tx, experiment.account_id, &experiment.miner_hotkey).await?;
        quote.validate(now(&mut tx).await?)?;
        if quote.account_id != experiment.account_id
            || quote.miner_hotkey != experiment.miner_hotkey
            || quote.recipe_digest != experiment.recipe_digest
        {
            return Err(StoreError::Scope);
        }
        experiment
            .state
            .transition(ExperimentState::AwaitingConsent)?;
        let next_revision = revision.checked_add(1).ok_or(StoreError::Conflict)?;
        sqlx::query(
            "INSERT INTO proof_machine_quote (id, experiment_id, experiment_revision, digest, quote) \
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(quote.id)
        .bind(experiment.id)
        .bind(next_revision)
        .bind(commitment(quote)?)
        .bind(Json(quote))
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE proof_service_intent SET status = 'cancelled' \
             WHERE experiment_id = $1 AND kind = 'provision' AND status = 'pending'",
        )
        .bind(experiment.id)
        .execute(&mut *tx)
        .await?;
        experiment.current_quote = Some(quote.id);
        advance(
            &mut tx,
            &mut experiment,
            ExperimentState::AwaitingConsent,
            "quoted",
        )
        .await?;
        require_lease(&mut tx, lease).await?;
        quote.validate(now(&mut tx).await?)?;
        tx.commit().await?;
        Ok(experiment)
    }

    /// Verify stored quote bytes, consume consent once and enqueue the rent
    /// atomically. A caller-supplied quote is never the source of authorization.
    ///
    /// # Errors
    /// Stale revision, replayed/invalid/expired consent or scope mismatch.
    pub async fn consent(
        &self,
        experiment_id: Uuid,
        revision: i64,
        consent: &SignedConsent,
    ) -> Result<Experiment, StoreError> {
        let mut tx = self.pool.begin().await?;
        let mut experiment = lock_experiment(&mut tx, experiment_id, Some(revision)).await?;
        active_account(&mut tx, experiment.account_id, &experiment.miner_hotkey).await?;
        let quote_id = experiment.current_quote.ok_or(StoreError::Conflict)?;
        let (quote, quote_revision, digest) = stored_quote(&mut tx, &experiment).await?;
        if quote_revision != revision {
            return Err(StoreError::Conflict);
        }
        consent.verify(&quote, now(&mut tx).await?)?;
        let changed = sqlx::query(
            "INSERT INTO proof_quote_consent (quote_id, quote_digest, signature) VALUES ($1, $2, $3) \
             ON CONFLICT DO NOTHING",
        )
        .bind(quote_id)
        .bind(&digest)
        .bind(&consent.signature)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if changed != 1 {
            return Err(StoreError::Replay);
        }
        advance(
            &mut tx,
            &mut experiment,
            ExperimentState::Approved,
            "approved",
        )
        .await?;
        sqlx::query(
            "INSERT INTO proof_service_intent (id, experiment_id, kind, quote_id) \
             VALUES ($1, $2, 'provision', $3)",
        )
        .bind(Uuid::new_v4())
        .bind(experiment_id)
        .bind(quote_id)
        .execute(&mut *tx)
        .await?;
        consent.verify(&quote, now(&mut tx).await?)?;
        tx.commit().await?;
        Ok(experiment)
    }

    /// Commit one dispatch before contacting Lium. Repeating this command,
    /// even after takeover, cannot rent again using the same intent.
    ///
    /// # Errors
    /// Stale owner/revision, revoked account, expired consent or consumed intent.
    pub async fn begin_provision(
        &self,
        lease: &ControllerLease,
        revision: i64,
        intent_id: Uuid,
    ) -> Result<MachineQuote, StoreError> {
        let mut tx = self.pool.begin().await?;
        let mut experiment = lock_experiment(&mut tx, lease.experiment_id, Some(revision)).await?;
        require_lease(&mut tx, lease).await?;
        active_account(&mut tx, experiment.account_id, &experiment.miner_hotkey).await?;
        let consent = sqlx::query(
            "SELECT c.quote_digest, c.signature FROM proof_service_intent i \
             JOIN proof_quote_consent c ON c.quote_id = i.quote_id \
             WHERE i.id = $1 AND i.experiment_id = $2 AND i.kind = 'provision' \
               AND i.status = 'pending' AND i.quote_id = $3",
        )
        .bind(intent_id)
        .bind(experiment.id)
        .bind(experiment.current_quote)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(StoreError::Conflict)?;
        let (quote, _, _) = stored_quote(&mut tx, &experiment).await?;
        let consent = SignedConsent {
            quote_digest: consent.try_get("quote_digest")?,
            signature: consent.try_get("signature")?,
        };
        consent.verify(&quote, now(&mut tx).await?)?;
        sqlx::query(
            "UPDATE proof_service_intent SET status = 'dispatched', controller_fence = $2, \
             dispatched_at = clock_timestamp() WHERE id = $1",
        )
        .bind(intent_id)
        .bind(lease.fence)
        .execute(&mut *tx)
        .await?;
        advance(
            &mut tx,
            &mut experiment,
            ExperimentState::Provisioning,
            "provision_dispatched",
        )
        .await?;
        require_lease(&mut tx, lease).await?;
        consent.verify(&quote, now(&mut tx).await?)?;
        tx.commit().await?;
        Ok(quote)
    }
}

pub(crate) async fn stored_quote(
    tx: &mut Tx,
    experiment: &Experiment,
) -> Result<(MachineQuote, i64, String), StoreError> {
    let id = experiment.current_quote.ok_or(StoreError::Conflict)?;
    let row = sqlx::query(
        "SELECT experiment_revision, digest, quote FROM proof_machine_quote \
         WHERE id = $1 AND experiment_id = $2",
    )
    .bind(id)
    .bind(experiment.id)
    .fetch_one(&mut **tx)
    .await?;
    let quote: Json<MachineQuote> = row.try_get("quote")?;
    let digest: String = row.try_get("digest")?;
    if commitment(&quote.0)? != digest
        || quote.id != id
        || quote.experiment_id != experiment.id
        || quote.account_id != experiment.account_id
        || quote.miner_hotkey != experiment.miner_hotkey
        || quote.recipe_digest != experiment.recipe_digest
    {
        return Err(StoreError::Corrupt);
    }
    Ok((quote.0, row.try_get("experiment_revision")?, digest))
}
