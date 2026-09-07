use uuid::Uuid;

use crate::{
    transaction::{lock_experiment, require_lease},
    ControllerLease, PgStore, StoreError,
};

fn validate_duration(seconds: u32) -> Result<(), StoreError> {
    if !(1..=300).contains(&seconds) {
        return Err(StoreError::Fenced);
    }
    Ok(())
}

impl PgStore {
    /// Acquire expired ownership with a strictly increasing database fence.
    /// Dispatched intents from a prior owner become reconciliation work.
    ///
    /// # Errors
    /// Live owner, terminal experiment with no cleanup, invalid duration or DB failure.
    pub async fn acquire(
        &self,
        experiment_id: Uuid,
        owner_id: Uuid,
        seconds: u32,
    ) -> Result<ControllerLease, StoreError> {
        validate_duration(seconds)?;
        if owner_id.is_nil() {
            return Err(StoreError::Fenced);
        }
        let mut tx = self.pool.begin().await?;
        if lock_experiment(&mut tx, experiment_id, None)
            .await?
            .state
            .terminal()
        {
            let cleanup: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM proof_resource WHERE experiment_id = $1 AND status <> 'deleted')",
            ).bind(experiment_id).fetch_one(&mut *tx).await?;
            if !cleanup {
                return Err(StoreError::Conflict);
            }
        }
        let lease: ControllerLease = sqlx::query_as(
            "INSERT INTO proof_controller_lease (experiment_id, owner_id, fence, expires_at) \
             VALUES ($1, $2, 1, clock_timestamp() + $3 * interval '1 second') \
             ON CONFLICT (experiment_id) DO UPDATE \
             SET owner_id = $2, fence = proof_controller_lease.fence + 1, \
                 expires_at = clock_timestamp() + $3 * interval '1 second' \
             WHERE proof_controller_lease.expires_at <= clock_timestamp() \
             RETURNING experiment_id, owner_id, fence",
        )
        .bind(experiment_id)
        .bind(owner_id)
        .bind(f64::from(seconds))
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(StoreError::Fenced)?;
        sqlx::query(
            "UPDATE proof_service_intent SET status = 'reconcile' \
             WHERE experiment_id = $1 AND status = 'dispatched'",
        )
        .bind(experiment_id)
        .execute(&mut *tx)
        .await?;
        require_lease(&mut tx, &lease).await?;
        tx.commit().await?;
        Ok(lease)
    }

    /// Renew only a still-live lease, never resurrect an expired owner.
    ///
    /// # Errors
    /// Expired, replaced or cancelled ownership, invalid duration or DB failure.
    pub async fn renew(&self, lease: &ControllerLease, seconds: u32) -> Result<(), StoreError> {
        validate_duration(seconds)?;
        let mut tx = self.pool.begin().await?;
        lock_experiment(&mut tx, lease.experiment_id, None).await?;
        require_lease(&mut tx, lease).await?;
        sqlx::query(
            "UPDATE proof_controller_lease \
             SET expires_at = clock_timestamp() + $2 * interval '1 second' WHERE experiment_id = $1",
        )
        .bind(lease.experiment_id)
        .bind(f64::from(seconds))
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Release without deleting the monotonic fencing counter.
    ///
    /// # Errors
    /// Expired or replaced owner, or DB failure.
    pub async fn release(&self, lease: &ControllerLease) -> Result<(), StoreError> {
        let mut tx = self.pool.begin().await?;
        lock_experiment(&mut tx, lease.experiment_id, None).await?;
        require_lease(&mut tx, lease).await?;
        sqlx::query(
            "UPDATE proof_controller_lease SET expires_at = clock_timestamp() WHERE experiment_id = $1",
        )
        .bind(lease.experiment_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }
}
