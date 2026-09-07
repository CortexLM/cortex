use proof_autonomy::{
    commitment, CapabilityOperation, ExperimentState, ProvisionResult, ResourceGrant,
};
use sqlx::{types::Json, Row};
use uuid::Uuid;

use crate::{
    quotes::stored_quote,
    transaction::{active_account, advance, lock_experiment, now, require_lease, Tx},
    ControllerLease, Experiment, MinerAccount, PgStore, ProvisionContext, Resource, StoreError,
};

pub(crate) async fn account(
    tx: &mut Tx,
    experiment: &Experiment,
) -> Result<MinerAccount, StoreError> {
    let credential_ref = sqlx::query_scalar(
        "SELECT credential_ref FROM proof_miner_account WHERE id = $1 AND miner_hotkey = $2",
    )
    .bind(experiment.account_id)
    .bind(&experiment.miner_hotkey)
    .fetch_one(&mut **tx)
    .await?;
    Ok(MinerAccount {
        id: experiment.account_id,
        miner_hotkey: experiment.miner_hotkey.clone(),
        credential_ref,
    })
}

impl PgStore {
    /// Recheck the committed dispatch immediately before the broker side effect.
    /// The provider must additionally enforce the quote's absolute expiry.
    ///
    /// # Errors
    /// Stale lease, cancelled dispatch, expired consent or revoked account.
    pub async fn check_dispatch(
        &self,
        lease: &ControllerLease,
        intent: Uuid,
    ) -> Result<(), StoreError> {
        let mut tx = self.pool.begin().await?;
        let experiment = lock_experiment(&mut tx, lease.experiment_id, None).await?;
        require_lease(&mut tx, lease).await?;
        active_account(&mut tx, experiment.account_id, &experiment.miner_hotkey).await?;
        let valid: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM proof_service_intent WHERE id = $1 \
             AND experiment_id = $2 AND status = 'dispatched' AND controller_fence = $3)",
        )
        .bind(intent)
        .bind(experiment.id)
        .bind(lease.fence)
        .fetch_one(&mut *tx)
        .await?;
        if !valid || experiment.state != ExperimentState::Provisioning {
            return Err(StoreError::Conflict);
        }
        let (quote, _, _) = stored_quote(&mut tx, &experiment).await?;
        quote.validate(now(&mut tx).await?)?;
        tx.commit().await?;
        Ok(())
    }

    /// Recover provider correlation under a new owner, without authorizing rent.
    /// Revocation blocks new work but must not prevent lookup of uncertain work.
    ///
    /// # Errors
    /// Stale lease, no uncertain dispatch, or corrupted quote.
    pub async fn reconciliation_context(
        &self,
        lease: &ControllerLease,
        intent: Uuid,
    ) -> Result<(ProvisionContext, ControllerLease), StoreError> {
        let mut tx = self.pool.begin().await?;
        let experiment = lock_experiment(&mut tx, lease.experiment_id, None).await?;
        require_lease(&mut tx, lease).await?;
        let fence: i64 = sqlx::query_scalar(
            "SELECT controller_fence FROM proof_service_intent WHERE id = $1 \
             AND experiment_id = $2 AND kind = 'provision' AND status IN ('dispatched', 'reconcile')",
        ).bind(intent).bind(experiment.id).fetch_optional(&mut *tx).await?
        .ok_or(StoreError::Conflict)?;
        let (quote, _, _) = stored_quote(&mut tx, &experiment).await?;
        let account = account(&mut tx, &experiment).await?;
        tx.commit().await?;
        Ok((
            ProvisionContext { account, quote },
            ControllerLease { fence, ..*lease },
        ))
    }

    /// Read the pending exact quote for provider preflight, without renting.
    ///
    /// # Errors
    /// Wrong scope, stale lease, revoked account or non-pending intent.
    pub async fn provision_context(
        &self,
        lease: &ControllerLease,
        intent: Uuid,
    ) -> Result<ProvisionContext, StoreError> {
        let mut tx = self.pool.begin().await?;
        let experiment = lock_experiment(&mut tx, lease.experiment_id, None).await?;
        require_lease(&mut tx, lease).await?;
        active_account(&mut tx, experiment.account_id, &experiment.miner_hotkey).await?;
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM proof_service_intent \
             WHERE id = $1 AND experiment_id = $2 AND quote_id = $3 \
             AND kind = 'provision' AND status = 'pending')",
        )
        .bind(intent)
        .bind(experiment.id)
        .bind(experiment.current_quote)
        .fetch_one(&mut *tx)
        .await?;
        if !exists {
            return Err(StoreError::Conflict);
        }
        let (quote, _, _) = stored_quote(&mut tx, &experiment).await?;
        quote.validate(now(&mut tx).await?)?;
        let account = account(&mut tx, &experiment).await?;
        tx.commit().await?;
        Ok(ProvisionContext { account, quote })
    }

    /// Append a broker response, including a late response from a fenced worker.
    /// Late resources remain quarantined. This cannot run an agent or revive a
    /// cancelled experiment. Never expose this method as an untrusted callback.
    ///
    /// # Errors
    /// No matching dispatched intent/fence, malformed id or corrupt quote.
    pub async fn record_provision(
        &self,
        dispatcher: &ControllerLease,
        intent: Uuid,
        result: &ProvisionResult,
    ) -> Result<(), StoreError> {
        let mut tx = self.pool.begin().await?;
        let experiment = lock_experiment(&mut tx, dispatcher.experiment_id, None).await?;
        let row = sqlx::query(
            "SELECT i.quote_id, q.digest, q.quote, \
             floor(extract(epoch FROM i.dispatched_at))::bigint AS started \
             FROM proof_service_intent i JOIN proof_machine_quote q ON q.id = i.quote_id \
             WHERE i.id = $1 AND i.experiment_id = $2 AND i.kind = 'provision' \
             AND i.controller_fence = $3 AND i.dispatched_at IS NOT NULL",
        )
        .bind(intent)
        .bind(experiment.id)
        .bind(dispatcher.fence)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(StoreError::Scope)?;
        let quote: Json<proof_autonomy::MachineQuote> = row.try_get("quote")?;
        let digest: String = row.try_get("digest")?;
        if commitment(&quote.0)? != digest
            || quote.id != row.try_get::<Uuid, _>("quote_id")?
            || quote.experiment_id != experiment.id
            || quote.account_id != experiment.account_id
            || quote.miner_hotkey != experiment.miner_hotkey
            || quote.recipe_digest != experiment.recipe_digest
        {
            return Err(StoreError::Corrupt);
        }
        sqlx::query(
            "INSERT INTO proof_provider_observation (id, intent_id, controller_fence, result) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(Uuid::new_v4())
        .bind(intent)
        .bind(dispatcher.fence)
        .bind(Json(result))
        .execute(&mut *tx)
        .await?;
        if let ProvisionResult::Confirmed { resource_id } = result {
            validate_resource_id(resource_id)?;
            let started: i64 = row.try_get("started")?;
            let until = started
                .checked_add(
                    i64::try_from(quote.lifetime_seconds).map_err(|_| StoreError::Corrupt)?,
                )
                .ok_or(StoreError::Corrupt)?;
            sqlx::query(
                "INSERT INTO proof_resource \
                 (account_id, resource_id, experiment_id, intent_id, quote_id, authorized_until) \
                 VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT DO NOTHING",
            )
            .bind(experiment.account_id)
            .bind(resource_id)
            .bind(experiment.id)
            .bind(intent)
            .bind(quote.id)
            .bind(until)
            .execute(&mut *tx)
            .await?;
            let bound: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM proof_resource WHERE account_id = $1 \
                 AND resource_id = $2 AND experiment_id = $3 AND intent_id = $4)",
            )
            .bind(experiment.account_id)
            .bind(resource_id)
            .bind(experiment.id)
            .bind(intent)
            .fetch_one(&mut *tx)
            .await?;
            if !bound {
                return Err(StoreError::Scope);
            }
            // An unexpected second resource invalidates agent access to the
            // first one too. Both remain available to controller cleanup.
            sqlx::query(
                "UPDATE proof_resource SET status = 'quarantined' \
                 WHERE experiment_id = $1 AND status = 'active' \
                 AND (SELECT count(*) FROM proof_resource WHERE experiment_id = $1) <> 1",
            )
            .bind(experiment.id)
            .execute(&mut *tx)
            .await?;
        }
        // Do not revive a completed outcome because of a later timeout. All
        // confirmations are retained, including additional resources to delete.
        sqlx::query(
            "UPDATE proof_service_intent SET status = CASE \
             WHEN $2 THEN 'completed' WHEN status <> 'completed' THEN 'reconcile' ELSE status END \
             WHERE id = $1",
        )
        .bind(intent)
        .bind(!matches!(result, ProvisionResult::Uncertain))
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Adopt exactly one confirmed resource under the current fence. Takeover
    /// never extends the original dispatch's spend deadline.
    ///
    /// # Errors
    /// Cancelled/expired/revoked scope, unresolved rent or multiple resources.
    pub async fn adopt_resource(
        &self,
        lease: &ControllerLease,
        revision: i64,
    ) -> Result<Experiment, StoreError> {
        let mut tx = self.pool.begin().await?;
        let mut experiment = lock_experiment(&mut tx, lease.experiment_id, Some(revision)).await?;
        require_lease(&mut tx, lease).await?;
        active_account(&mut tx, experiment.account_id, &experiment.miner_hotkey).await?;
        let resources: Vec<Resource> =
            sqlx::query_as("SELECT * FROM proof_resource WHERE experiment_id = $1")
                .bind(experiment.id)
                .fetch_all(&mut *tx)
                .await?;
        let [resource] = resources.as_slice() else {
            return Err(StoreError::Conflict);
        };
        if resource.status != "quarantined"
            || resource.quote_id != experiment.current_quote.ok_or(StoreError::Conflict)?
            || u64::try_from(resource.authorized_until).map_err(|_| StoreError::Corrupt)?
                <= now(&mut tx).await?
        {
            return Err(StoreError::Conflict);
        }
        sqlx::query("UPDATE proof_resource SET status = 'active' WHERE experiment_id = $1")
            .bind(experiment.id)
            .execute(&mut *tx)
            .await?;
        advance(
            &mut tx,
            &mut experiment,
            ExperimentState::Running,
            "resource_adopted",
        )
        .await?;
        require_lease(&mut tx, lease).await?;
        if u64::try_from(resource.authorized_until).map_err(|_| StoreError::Corrupt)?
            <= now(&mut tx).await?
        {
            return Err(StoreError::Conflict);
        }
        tx.commit().await?;
        Ok(experiment)
    }

    /// Resolve a stored grant on every broker call. Resource names are not ACLs.
    /// Deletion is controller-owned and uses `begin_cleanup`, not this grant.
    ///
    /// # Errors
    /// Wrong account/resource, revoked account, stale lease or expired lifetime.
    pub async fn authorize_resource(
        &self,
        lease: &ControllerLease,
        resource_id: &str,
        operation: CapabilityOperation,
    ) -> Result<ResourceGrant, StoreError> {
        let mut tx = self.pool.begin().await?;
        let experiment = lock_experiment(&mut tx, lease.experiment_id, None).await?;
        require_lease(&mut tx, lease).await?;
        active_account(&mut tx, experiment.account_id, &experiment.miner_hotkey).await?;
        if !matches!(
            experiment.state,
            ExperimentState::Running | ExperimentState::Collecting
        ) {
            return Err(StoreError::Scope);
        }
        let resource: Resource = sqlx::query_as(
            "SELECT * FROM proof_resource WHERE experiment_id = $1 AND resource_id = $2 \
             AND status = 'active'",
        )
        .bind(experiment.id)
        .bind(resource_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(StoreError::Scope)?;
        let grant = ResourceGrant {
            experiment_id: experiment.id,
            account_id: experiment.account_id,
            resource_id: resource.resource_id,
            expires_at: u64::try_from(resource.authorized_until)
                .map_err(|_| StoreError::Corrupt)?,
            revoked: false,
            operations: vec![
                CapabilityOperation::Inspect,
                CapabilityOperation::Execute,
                CapabilityOperation::Collect,
            ],
        };
        grant.authorize(
            experiment.id,
            experiment.account_id,
            resource_id,
            operation,
            now(&mut tx).await?,
        )?;
        require_lease(&mut tx, lease).await?;
        grant.authorize(
            experiment.id,
            experiment.account_id,
            resource_id,
            operation,
            now(&mut tx).await?,
        )?;
        tx.commit().await?;
        Ok(grant)
    }

    /// Owner-scoped durable resources, including unresolved cleanup.
    ///
    /// # Errors
    /// Wrong owner or database failure.
    pub async fn resources(&self, id: Uuid, miner: &str) -> Result<Vec<Resource>, StoreError> {
        self.experiment(id, miner).await?;
        Ok(sqlx::query_as(
            "SELECT * FROM proof_resource WHERE experiment_id = $1 ORDER BY resource_id",
        )
        .bind(id)
        .fetch_all(&self.pool)
        .await?)
    }
}

fn validate_resource_id(id: &str) -> Result<(), StoreError> {
    if id.is_empty()
        || id.len() > 256
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
    {
        return Err(StoreError::Scope);
    }
    Ok(())
}
