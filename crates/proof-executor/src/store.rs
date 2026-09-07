use proof_autonomy::{commitment, CapabilityOperation, ExperimentState, ResourceGrant};
use proof_autonomy_pg::{ControllerLease, PgStore};
use proof_research::artifact_digest;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{types::Json, PgPool};
use uuid::Uuid;

use crate::{DockerSandbox, Failure, Plan};

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Intent {
    pub id: Uuid,
    pub experiment_id: Uuid,
    pub operation_key: String,
    pub controller_fence: i64,
    pub owner_id: Uuid,
    pub resource_id: String,
    pub engine_id: String,
    pub image_id: String,
    pub plan: Json<Plan>,
    pub deadline_ms: i64,
    pub state: String,
}

#[derive(Clone)]
pub(crate) struct Journal {
    pub pool: PgPool,
    pub orchestration: PgStore,
}

impl Journal {
    pub fn new(pool: PgPool) -> Self {
        Self {
            orchestration: PgStore::new(pool.clone()),
            pool,
        }
    }

    pub async fn authorize(
        &self,
        lease: &ControllerLease,
        grant: &ResourceGrant,
        operation: CapabilityOperation,
    ) -> Result<(), Failure> {
        let actual = self
            .orchestration
            .authorize_resource(lease, &grant.resource_id, operation)
            .await
            .map_err(|_| Failure::Authorization)?;
        if actual != *grant {
            return Err(Failure::Authorization);
        }
        Ok(())
    }

    pub async fn bind(
        &self,
        lease: &ControllerLease,
        grant: &ResourceGrant,
        target: &DockerSandbox,
    ) -> Result<(), Failure> {
        self.authorize(lease, grant, CapabilityOperation::Inspect)
            .await?;
        let mut tx = self.orchestration.controller_transaction(lease).await?;
        // A local image ID is not silently substituted for a quoted eval image.
        let pinned: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM proof_resource r JOIN proof_machine_quote q ON q.id = r.quote_id \
             WHERE r.experiment_id = $1 AND r.resource_id = $2 AND q.quote->>'image_digest' = $3)",
        ).bind(lease.experiment_id).bind(&grant.resource_id).bind(&target.image_id)
            .fetch_one(tx.connection()).await?;
        if !pinned {
            return Err(Failure::Target);
        }
        sqlx::query(
            "INSERT INTO proof_execution_target (experiment_id, account_id, resource_id, engine_id, image_id) \
             VALUES ($1, $2, $3, $4, $5) ON CONFLICT DO NOTHING",
        ).bind(lease.experiment_id).bind(grant.account_id).bind(&grant.resource_id)
            .bind(&target.engine_id).bind(&target.image_id).execute(tx.connection()).await?;
        let exact: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM proof_execution_target WHERE experiment_id = $1 \
             AND account_id = $2 AND resource_id = $3 AND engine_id = $4 AND image_id = $5)",
        )
        .bind(lease.experiment_id)
        .bind(grant.account_id)
        .bind(&grant.resource_id)
        .bind(&target.engine_id)
        .bind(&target.image_id)
        .fetch_one(tx.connection())
        .await?;
        if !exact {
            return Err(Failure::Target);
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn script(&self, digest: &str) -> Result<Vec<u8>, Failure> {
        let bytes: Vec<u8> =
            sqlx::query_scalar("SELECT bytes FROM proof_execution_script WHERE digest = $1")
                .bind(digest)
                .fetch_optional(&self.pool)
                .await?
                .ok_or(Failure::MissingScript)?;
        validate_script(&bytes)?;
        if artifact_digest(&bytes) != digest {
            return Err(Failure::Commitment);
        }
        Ok(bytes)
    }

    pub async fn put_script(&self, bytes: &[u8]) -> Result<String, Failure> {
        validate_script(bytes)?;
        let digest = artifact_digest(bytes);
        sqlx::query("INSERT INTO proof_execution_script (digest, bytes) VALUES ($1, $2) ON CONFLICT DO NOTHING")
            .bind(&digest).bind(bytes).execute(&self.pool).await?;
        if self.script(&digest).await? != bytes {
            return Err(Failure::Commitment);
        }
        Ok(digest)
    }

    pub async fn begin(
        &self,
        lease: &ControllerLease,
        grant: &ResourceGrant,
        plan: Plan,
        target: &DockerSandbox,
    ) -> Result<Intent, Failure> {
        self.authorize(lease, grant, plan.operation()).await?;
        let mut tx = self.orchestration.controller_transaction(lease).await?;
        if !matches!(
            tx.experiment().state,
            ExperimentState::Running | ExperimentState::Collecting
        ) || plan
            .recipe_digest
            .as_ref()
            .is_some_and(|d| *d != tx.experiment().recipe_digest)
        {
            return Err(Failure::Authorization);
        }
        let bound: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM proof_execution_target WHERE experiment_id = $1 \
             AND account_id = $2 AND resource_id = $3 AND engine_id = $4 AND image_id = $5)",
        )
        .bind(lease.experiment_id)
        .bind(grant.account_id)
        .bind(&grant.resource_id)
        .bind(&target.engine_id)
        .bind(&target.image_id)
        .fetch_one(tx.connection())
        .await?;
        if !bound {
            return Err(Failure::Target);
        }
        let now: i64 = sqlx::query_scalar(
            "SELECT floor(extract(epoch FROM clock_timestamp()) * 1000)::bigint",
        )
        .fetch_one(tx.connection())
        .await?;
        let total = plan.runs.iter().try_fold(0_i64, |sum, run| {
            sum.checked_add(i64::try_from(run.timeout_ms()).map_err(|_| Failure::Input)?)
                .and_then(|n| n.checked_add(5000))
                .ok_or(Failure::Input)
        })?;
        let expires = i64::try_from(grant.expires_at)
            .ok()
            .and_then(|n| n.checked_mul(1000))
            .ok_or(Failure::Input)?;
        let deadline_ms = expires.min(
            now.checked_add(total.min(86_400_000))
                .ok_or(Failure::Input)?,
        );
        if deadline_ms <= now {
            return Err(Failure::Authorization);
        }
        let intent = Intent {
            id: Uuid::new_v4(),
            experiment_id: lease.experiment_id,
            operation_key: commitment(&plan).map_err(|_| Failure::Input)?,
            controller_fence: lease.fence,
            owner_id: lease.owner_id,
            resource_id: grant.resource_id.clone(),
            engine_id: target.engine_id.clone(),
            image_id: target.image_id.clone(),
            plan: Json(plan),
            deadline_ms,
            state: "dispatched".into(),
        };
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM proof_execution_intent WHERE experiment_id = $1",
        )
        .bind(lease.experiment_id)
        .fetch_one(tx.connection())
        .await?;
        if count >= 256 {
            return Err(Failure::Input);
        }
        let inserted = sqlx::query(
            "INSERT INTO proof_execution_intent (id, experiment_id, operation_key, controller_fence, owner_id, \
             resource_id, engine_id, image_id, plan, deadline_ms) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) ON CONFLICT DO NOTHING",
        ).bind(intent.id).bind(intent.experiment_id).bind(&intent.operation_key).bind(intent.controller_fence)
            .bind(intent.owner_id).bind(&intent.resource_id).bind(&intent.engine_id).bind(&intent.image_id)
            .bind(&intent.plan).bind(intent.deadline_ms).execute(tx.connection()).await?.rows_affected();
        if inserted != 1 {
            return Err(Failure::Reconcile);
        }
        tx.commit().await?;
        Ok(intent)
    }

    /// Late observations append using the *dispatcher's* fence. Current lease
    /// expiry, cancellation and takeover must not erase failures or captured bytes.
    pub async fn append(
        &self,
        intent: &Intent,
        observation: Value,
        artifacts: &[Vec<u8>],
    ) -> Result<(), Failure> {
        let mut tx = self.pool.begin().await?;
        let matched: Option<Uuid> = sqlx::query_scalar(
            "SELECT id FROM proof_execution_intent WHERE id = $1 AND experiment_id = $2 \
             AND controller_fence = $3 AND owner_id = $4 AND operation_key = $5 FOR UPDATE",
        )
        .bind(intent.id)
        .bind(intent.experiment_id)
        .bind(intent.controller_fence)
        .bind(intent.owner_id)
        .bind(&intent.operation_key)
        .fetch_optional(&mut *tx)
        .await?;
        if matched.is_none() {
            return Err(Failure::Authorization);
        }
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM proof_execution_observation WHERE intent_id = $1",
        )
        .bind(intent.id)
        .fetch_one(&mut *tx)
        .await?;
        if (count >= 255 && observation.get("event").and_then(Value::as_str) != Some("finish"))
            || count >= 256
            || serde_json::to_vec(&observation)
                .map_err(|_| Failure::Input)?
                .len()
                > 32768
        {
            return Err(Failure::Input);
        }
        for bytes in artifacts {
            if bytes.is_empty() || bytes.len() > 1024 * 1024 {
                return Err(Failure::LogLimit);
            }
            sqlx::query("INSERT INTO proof_execution_artifact (intent_id, digest, bytes) VALUES ($1,$2,$3) ON CONFLICT DO NOTHING")
                .bind(intent.id).bind(artifact_digest(bytes)).bind(bytes).execute(&mut *tx).await?;
        }
        let size: i64 = sqlx::query_scalar(
            "SELECT coalesce(sum(octet_length(bytes)), 0)::bigint FROM proof_execution_artifact WHERE intent_id = $1",
        ).bind(intent.id).fetch_one(&mut *tx).await?;
        if size > 16 * 1024 * 1024 {
            return Err(Failure::LogLimit);
        }
        sqlx::query(
            "INSERT INTO proof_execution_observation (id,intent_id,controller_fence,observation) VALUES ($1,$2,$3,$4)",
        ).bind(Uuid::new_v4()).bind(intent.id).bind(intent.controller_fence)
            .bind(Json(observation)).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn finish(
        &self,
        intent: &Intent,
        failure: Option<Failure>,
        stopped: bool,
    ) -> Result<(), Failure> {
        self.append(
            intent,
            json!({"event": "finish", "failure": failure, "stop_confirmed": stopped}),
            &[],
        )
        .await?;
        // Reconciliation never permits late old owners to revive a terminal row.
        sqlx::query(
            "UPDATE proof_execution_intent SET state = $3 WHERE id = $1 AND controller_fence = $2 \
             AND state IN ('dispatched', 'reconcile')",
        )
        .bind(intent.id)
        .bind(intent.controller_fence)
        .bind(if !stopped {
            "reconcile"
        } else if failure.is_some() {
            "failed"
        } else {
            "completed"
        })
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn intents(&self, lease: &ControllerLease) -> Result<Vec<Intent>, Failure> {
        let mut tx = self.orchestration.controller_transaction(lease).await?;
        let intents: Vec<Intent> = sqlx::query_as(
            "SELECT * FROM proof_execution_intent WHERE experiment_id = $1 ORDER BY created_at",
        )
        .bind(lease.experiment_id)
        .fetch_all(tx.connection())
        .await?;
        for intent in &intents {
            if commitment(&intent.plan.0).map_err(|_| Failure::Commitment)? != intent.operation_key
                || intent.plan.schema_version != 1
                || !(1..=40).contains(&intent.plan.runs.len())
                || intent
                    .plan
                    .runs
                    .iter()
                    .any(|r| !(1..=86_400_000).contains(&r.timeout_ms()))
            {
                return Err(Failure::Commitment);
            }
        }
        tx.commit().await?;
        Ok(intents)
    }
}

fn validate_script(bytes: &[u8]) -> Result<(), Failure> {
    if bytes.is_empty()
        || bytes.len() > 32768
        || bytes.contains(&0)
        || std::str::from_utf8(bytes).is_err()
    {
        return Err(Failure::Input);
    }
    Ok(())
}
