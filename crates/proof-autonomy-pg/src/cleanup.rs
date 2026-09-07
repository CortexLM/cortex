use proof_autonomy::{DeletionResult, ExperimentState};
use sqlx::types::Json;
use uuid::Uuid;

use crate::{
    resources::account,
    transaction::{advance, lock_experiment, require_lease},
    ControllerLease, DeletionTarget, Experiment, PgStore, Resource, StoreError,
};

impl PgStore {
    /// Persist deletion intent before contacting the provider. Revocation and
    /// grant expiry block execution, not cleanup. Repeated calls keep the same
    /// deletion id. Unknown rentals still need provider reconciliation.
    ///
    /// # Errors
    /// Stale lease, wrong resource or illegal lifecycle.
    pub async fn begin_cleanup(
        &self,
        lease: &ControllerLease,
        resource_id: &str,
    ) -> Result<DeletionTarget, StoreError> {
        let mut tx = self.pool.begin().await?;
        let mut experiment = lock_experiment(&mut tx, lease.experiment_id, None).await?;
        require_lease(&mut tx, lease).await?;
        if experiment.state != ExperimentState::Deleting && !experiment.state.terminal() {
            advance(
                &mut tx,
                &mut experiment,
                ExperimentState::Deleting,
                "cleanup_started",
            )
            .await?;
        }
        let resource: Resource = sqlx::query_as(
            "UPDATE proof_resource SET status = 'deleting', deletion_id = COALESCE(deletion_id, $3) \
             WHERE experiment_id = $1 AND resource_id = $2 AND status <> 'deleted' RETURNING *",
        )
        .bind(experiment.id).bind(resource_id).bind(Uuid::new_v4())
        .fetch_optional(&mut *tx).await?.ok_or(StoreError::Scope)?;
        let account = account(&mut tx, &experiment).await?;
        require_lease(&mut tx, lease).await?;
        tx.commit().await?;
        Ok(DeletionTarget { account, resource })
    }

    /// Record a trusted provider verification, not a bare DELETE acknowledgement.
    /// Failure to verify billing termination keeps the resource unresolved.
    ///
    /// # Errors
    /// Stale lease, wrong target/deletion id or database failure.
    pub async fn record_deletion(
        &self,
        lease: &ControllerLease,
        resource_id: &str,
        deletion_id: Uuid,
        result: &DeletionResult,
    ) -> Result<(), StoreError> {
        let mut tx = self.pool.begin().await?;
        let experiment = lock_experiment(&mut tx, lease.experiment_id, None).await?;
        require_lease(&mut tx, lease).await?;
        let bound: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM proof_resource WHERE experiment_id = $1 \
             AND resource_id = $2 AND deletion_id = $3 AND status IN ('deleting', 'deleted'))",
        )
        .bind(experiment.id)
        .bind(resource_id)
        .bind(deletion_id)
        .fetch_one(&mut *tx)
        .await?;
        if !bound {
            return Err(StoreError::Scope);
        }
        sqlx::query(
            "INSERT INTO proof_deletion_observation \
             (id, account_id, resource_id, deletion_id, controller_fence, result) \
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(Uuid::new_v4())
        .bind(experiment.account_id)
        .bind(resource_id)
        .bind(deletion_id)
        .bind(lease.fence)
        .bind(Json(result))
        .execute(&mut *tx)
        .await?;
        if result == &DeletionResult::Confirmed {
            sqlx::query(
                "UPDATE proof_resource SET status = 'deleted' WHERE experiment_id = $1 AND resource_id = $2",
            )
            .bind(experiment.id).bind(resource_id).execute(&mut *tx).await?;
        }
        require_lease(&mut tx, lease).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Finish an unsuccessful or cancelled experiment only when no uncertain
    /// rental or undeleted resource remains. Scientific completion is separate.
    ///
    /// # Errors
    /// Incomplete cleanup, stale lease or illegal transition.
    pub async fn finish_cleanup(&self, lease: &ControllerLease) -> Result<Experiment, StoreError> {
        let mut tx = self.pool.begin().await?;
        let mut experiment = lock_experiment(&mut tx, lease.experiment_id, None).await?;
        require_lease(&mut tx, lease).await?;
        let unresolved: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM proof_service_intent WHERE experiment_id = $1 \
             AND kind = 'provision' AND status IN ('pending', 'dispatched', 'reconcile')) \
             OR EXISTS(SELECT 1 FROM proof_resource WHERE experiment_id = $1 AND status <> 'deleted')",
        )
        .bind(experiment.id).fetch_one(&mut *tx).await?;
        if unresolved {
            return Err(StoreError::Conflict);
        }
        if experiment.state.terminal() {
            require_lease(&mut tx, lease).await?;
            tx.commit().await?;
            return Ok(experiment);
        }
        if experiment.state != ExperimentState::Deleting {
            advance(
                &mut tx,
                &mut experiment,
                ExperimentState::Deleting,
                "cleanup_started",
            )
            .await?;
        }
        let cancelled: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM proof_service_intent WHERE experiment_id = $1 AND kind = 'cancel')",
        )
        .bind(experiment.id).fetch_one(&mut *tx).await?;
        let next = if cancelled {
            ExperimentState::Cancelled
        } else {
            ExperimentState::Rejected
        };
        advance(&mut tx, &mut experiment, next, "cleanup_verified").await?;
        sqlx::query(
            "UPDATE proof_service_intent SET status = 'completed' WHERE experiment_id = $1 AND kind = 'cancel'",
        )
        .bind(experiment.id).execute(&mut *tx).await?;
        require_lease(&mut tx, lease).await?;
        tx.commit().await?;
        Ok(experiment)
    }
}
