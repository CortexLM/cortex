use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use proof_autonomy::{
    commitment, CapabilityOperation, ExperimentState, MachineQuote, ResourceGrant,
};
use proof_autonomy_pg::{ControllerLease, PgStore};
use proof_research::{ResearchStore, RetainedArtifacts, ScientificEvidence, ScientificRecipe};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{types::Json, PgPool};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::{RuntimeCall, RuntimeError, RuntimeOperations, RuntimeScope};

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExecutionRequest {
    Terminal { argv: Vec<String>, timeout_ms: u32 },
    Kernel { code: String, timeout_ms: u32 },
}

impl ExecutionRequest {
    fn valid(&self, operation: &str) -> bool {
        let (bounded, timeout) = match self {
            Self::Terminal { argv, timeout_ms } => (
                operation == "execute"
                    && !argv.is_empty()
                    && argv.len() <= 64
                    && argv.iter().all(|s| !s.contains('\0'))
                    && argv.iter().map(String::len).sum::<usize>() <= 32 * 1024,
                *timeout_ms,
            ),
            Self::Kernel { code, timeout_ms } => (
                operation == "kernel" && !code.is_empty() && code.len() <= 32 * 1024,
                *timeout_ms,
            ),
        };
        bounded && (1..=25_000).contains(&timeout)
    }
}

/// Trusted execution adapter, never the model. Enforce grant expiry and
/// cancellation at the target; dropping a future is not proof remote work ended.
/// Collect must execute/observe the committed paired runs and retain their bytes.
#[async_trait]
pub trait ExperimentExecutor: Send + Sync {
    async fn execute(
        &self,
        lease: &ControllerLease,
        grant: &ResourceGrant,
        request: &ExecutionRequest,
    ) -> Result<Value, RuntimeError>;

    async fn collect(
        &self,
        lease: &ControllerLease,
        grant: &ResourceGrant,
        recipe: &ScientificRecipe,
    ) -> Result<(ScientificEvidence, RetainedArtifacts), RuntimeError>;
}

pub struct ExperimentOperations {
    store: PgStore,
    research: ResearchStore,
    lease: ControllerLease,
    scope: RuntimeScope,
    resource: String,
    executor: Arc<dyn ExperimentExecutor>,
    active: Mutex<()>,
}

impl ExperimentOperations {
    /// Bind before starting the runtime. There is no provider/default-resource
    /// fallback and no agent-authored evidence admission.
    ///
    /// # Errors
    /// Stale controller, wrong resource, missing registered recipe or DB failure.
    pub async fn bind(
        pool: PgPool,
        research: ResearchStore,
        lease: ControllerLease,
        resource: String,
        executor: Arc<dyn ExperimentExecutor>,
    ) -> Result<Self, RuntimeError> {
        let store = PgStore::new(pool.clone());
        let tx = store.controller_transaction(&lease).await?;
        let scope = RuntimeScope {
            role: "experiment".into(),
            id: lease.experiment_id.to_string(),
            commitment: tx.experiment().recipe_digest.clone(),
        };
        tx.commit().await?;
        research.recipe(&scope.commitment).await?;
        store
            .authorize_resource(&lease, &resource, CapabilityOperation::Inspect)
            .await?;
        Ok(Self {
            store,
            research,
            lease,
            scope,
            resource,
            executor,
            active: Mutex::new(()),
        })
    }

    #[must_use]
    pub fn scope(&self) -> &RuntimeScope {
        &self.scope
    }

    async fn authorize(
        &self,
        operation: CapabilityOperation,
    ) -> Result<ResourceGrant, RuntimeError> {
        Ok(self
            .store
            .authorize_resource(&self.lease, &self.resource, operation)
            .await?)
    }

    async fn watch(&self) -> RuntimeError {
        loop {
            tokio::time::sleep(Duration::from_millis(200)).await;
            if self.authorize(CapabilityOperation::Inspect).await.is_err() {
                return RuntimeError::Scope;
            }
        }
    }

    async fn quote(&self) -> Result<Value, RuntimeError> {
        let mut tx = self.store.controller_transaction(&self.lease).await?;
        let (document, digest): (Json<MachineQuote>, String) = sqlx::query_as(
            "SELECT quote, digest FROM proof_machine_quote WHERE experiment_id = $1 AND id = $2",
        )
        .bind(self.lease.experiment_id)
        .bind(tx.experiment().current_quote)
        .fetch_one(tx.connection())
        .await?;
        if commitment(&document.0).map_err(|_| RuntimeError::Scope)? != digest
            || document.recipe_digest != self.scope.commitment
        {
            return Err(RuntimeError::Scope);
        }
        tx.commit().await?;
        Ok(json!({ "quote": document.0 }))
    }

    async fn report(&self, arguments: Value) -> Result<Value, RuntimeError> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Report {
            text: String,
        }
        let report: Report = serde_json::from_value(arguments)?;
        if report.text.trim().is_empty() || report.text.len() > 16_384 {
            return Err(RuntimeError::Scope);
        }
        let mut tx = self.store.controller_transaction(&self.lease).await?;
        if !matches!(
            tx.experiment().state,
            ExperimentState::Running | ExperimentState::Collecting
        ) {
            return Err(RuntimeError::Scope);
        }
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM proof_agent_report WHERE experiment_id = $1")
                .bind(self.lease.experiment_id)
                .fetch_one(tx.connection())
                .await?;
        if count >= 256 {
            return Err(RuntimeError::Scope);
        }
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO proof_agent_report (id, experiment_id, controller_fence, body) VALUES ($1, $2, $3, $4)",
        ).bind(id).bind(self.lease.experiment_id).bind(self.lease.fence).bind(report.text)
            .execute(tx.connection()).await?;
        tx.commit().await?;
        Ok(json!({ "report_id": id, "authoritative": false }))
    }
}

#[async_trait]
impl RuntimeOperations for ExperimentOperations {
    async fn call(&self, request: RuntimeCall) -> Result<Value, RuntimeError> {
        if request.schema_version != 1 || request.scope != self.scope {
            return Err(RuntimeError::Scope);
        }
        // No overlapping terminal/kernel/collection calls through descendants.
        let _active = self
            .active
            .try_lock()
            .map_err(|_| RuntimeError::Unavailable)?;
        self.authorize(CapabilityOperation::Inspect).await?;
        match request.operation.as_str() {
            "execute" | "kernel" => {
                let execution: ExecutionRequest = serde_json::from_value(request.arguments)?;
                if !execution.valid(&request.operation) {
                    return Err(RuntimeError::Scope);
                }
                let grant = self.authorize(CapabilityOperation::Execute).await?;
                let timeout_ms = match &execution {
                    ExecutionRequest::Terminal { timeout_ms, .. }
                    | ExecutionRequest::Kernel { timeout_ms, .. } => *timeout_ms,
                };
                let result = tokio::select! {
                    result = self.executor.execute(&self.lease, &grant, &execution) => result?,
                    error = self.watch() => return Err(error),
                    () = tokio::time::sleep(Duration::from_millis(u64::from(timeout_ms))) => return Err(RuntimeError::Unavailable),
                };
                self.authorize(CapabilityOperation::Inspect).await?;
                Ok(result)
            }
            "report" => self.report(request.arguments).await,
            "quote" | "read_evidence" | "collect" if request.arguments == json!({}) => {
                if request.operation == "quote" {
                    return self.quote().await;
                }
                if request.operation == "read_evidence" {
                    let tx = self.store.controller_transaction(&self.lease).await?;
                    let miner = tx.experiment().miner_hotkey.clone();
                    tx.commit().await?;
                    return Ok(
                        json!({ "evidence": self.research.evidence(self.lease.experiment_id, &miner).await? }),
                    );
                }
                let grant = self.authorize(CapabilityOperation::Collect).await?;
                let recipe = self.research.recipe(&self.scope.commitment).await?;
                let (evidence, artifacts) = tokio::select! {
                    result = self.executor.collect(&self.lease, &grant, &recipe) => result?,
                    error = self.watch() => return Err(error),
                };
                if evidence.resource_id != self.resource {
                    return Err(RuntimeError::Scope);
                }
                Ok(
                    json!({ "evidence": self.research.record(&self.lease, &evidence, &artifacts).await? }),
                )
            }
            _ => Err(RuntimeError::Scope),
        }
    }
}
