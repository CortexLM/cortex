use proof_autonomy::SignedAction;

use crate::{
    quotes::stored_quote,
    transaction::{consume_action, lock_experiment, verify_action},
    ExperimentView, PgStore, StoreError, ViewExperiment,
};

impl PgStore {
    /// Fail closed before mounting public v2 routes. Migrations use a separate
    /// owner connection; the service must use the restricted application role.
    ///
    /// # Errors
    /// Missing schema, owner/superuser connection or incorrect table privileges.
    pub async fn ready(&self) -> Result<(), StoreError> {
        let allowed: bool = sqlx::query_scalar(
            "SELECT current_user = 'base_app' \
             AND NOT has_table_privilege(current_user, 'proof_resource', 'UPDATE') \
             AND NOT has_table_privilege(current_user, 'proof_quote_consent', 'UPDATE')",
        )
        .fetch_one(&self.pool)
        .await?;
        if !allowed {
            return Err(StoreError::Scope);
        }
        sqlx::query("SELECT dispatched_at FROM proof_service_intent LIMIT 0")
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Authenticated, revision-consistent view. Identity is never taken from an
    /// unsigned hotkey header. Revoked accounts can still inspect their cleanup.
    ///
    /// # Errors
    /// Invalid/replayed signature, wrong owner or database failure.
    pub async fn view(
        &self,
        request: &ViewExperiment,
        action: &SignedAction,
    ) -> Result<ExperimentView, StoreError> {
        let mut tx = self.pool.begin().await?;
        let experiment = lock_experiment(&mut tx, request.experiment_id, None).await?;
        if experiment.miner_hotkey != action.miner_hotkey {
            return Err(StoreError::Scope);
        }
        let path = format!("/v2/experiments/{}/view", request.experiment_id);
        consume_action(&mut tx, action, &path, request).await?;
        let quote = if experiment.current_quote.is_some() {
            Some(stored_quote(&mut tx, &experiment).await?.0)
        } else {
            None
        };
        let resources = sqlx::query_as(
            "SELECT * FROM proof_resource WHERE experiment_id = $1 ORDER BY resource_id",
        )
        .bind(experiment.id)
        .fetch_all(&mut *tx)
        .await?;
        verify_action(&mut tx, action, &path, request).await?;
        tx.commit().await?;
        Ok(ExperimentView {
            experiment,
            quote,
            resources,
        })
    }
}
