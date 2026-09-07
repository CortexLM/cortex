use proof_autonomy::ExperimentState;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Opaque reference resolved by the trusted broker, never by an agent.
#[derive(Debug, Clone)]
pub struct MinerAccount {
    pub id: Uuid,
    pub miner_hotkey: String,
    pub credential_ref: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateExperiment {
    pub id: Uuid,
    pub account_id: Uuid,
    pub recipe_digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CancelExperiment {
    pub experiment_id: Uuid,
    pub revision: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ViewExperiment {
    pub experiment_id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExperimentView {
    pub experiment: Experiment,
    pub quote: Option<proof_autonomy::MachineQuote>,
    pub resources: Vec<Resource>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Experiment {
    pub id: Uuid,
    pub miner_hotkey: String,
    pub account_id: Uuid,
    pub recipe_digest: String,
    pub state: ExperimentState,
    pub revision: i64,
    pub current_quote: Option<Uuid>,
}

/// Retain the fence even after release. An owner UUID alone is not sufficient.
#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::FromRow)]
pub struct ControllerLease {
    pub experiment_id: Uuid,
    pub owner_id: Uuid,
    pub fence: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntentKind {
    Provision,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntentStatus {
    Pending,
    Dispatched,
    Reconcile,
    Completed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceIntent {
    pub id: Uuid,
    pub experiment_id: Uuid,
    pub kind: IntentKind,
    pub quote_id: Option<Uuid>,
    pub status: IntentStatus,
    pub controller_fence: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExperimentEvent {
    pub revision: i64,
    pub kind: String,
    pub state: ExperimentState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
pub struct Resource {
    pub account_id: Uuid,
    pub resource_id: String,
    pub experiment_id: Uuid,
    pub intent_id: Uuid,
    pub quote_id: Uuid,
    pub status: String,
    pub authorized_until: i64,
    pub deletion_id: Option<Uuid>,
}

/// Broker-only account binding. Never serialize it to an agent or miner.
pub struct ProvisionContext {
    pub account: MinerAccount,
    pub quote: proof_autonomy::MachineQuote,
}

/// A persisted exact target, not a resource name supplied by an agent.
pub struct DeletionTarget {
    pub account: MinerAccount,
    pub resource: Resource,
}
