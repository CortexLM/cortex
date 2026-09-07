use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::ContractError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExperimentState {
    Discussion,
    AwaitingConsent,
    Approved,
    Provisioning,
    Running,
    Collecting,
    Cancelling,
    Deleting,
    Reconciling,
    Completed,
    Cancelled,
    Rejected,
}

impl ExperimentState {
    /// Pure state-machine check; persistence must also compare the revision.
    ///
    /// # Errors
    /// An illegal transition, including revival of a terminal experiment.
    pub fn transition(self, next: Self) -> Result<Self, ContractError> {
        use ExperimentState::{
            Approved, AwaitingConsent, Cancelled, Cancelling, Collecting, Completed, Deleting,
            Discussion, Provisioning, Reconciling, Rejected, Running,
        };
        let allowed = match self {
            Discussion => matches!(next, AwaitingConsent | Cancelling | Rejected),
            AwaitingConsent => matches!(next, Discussion | Approved | Cancelling | Rejected),
            Approved => matches!(next, Provisioning | AwaitingConsent | Cancelling),
            Provisioning => matches!(next, Running | Reconciling | Cancelling | Deleting),
            Running => matches!(next, Collecting | Cancelling | Deleting),
            Collecting => matches!(next, Deleting | Cancelling),
            Cancelling | Reconciling => next == Deleting,
            Deleting => matches!(next, Completed | Cancelled | Rejected | Reconciling),
            Completed | Cancelled | Rejected => false,
        };
        if allowed {
            Ok(next)
        } else {
            Err(ContractError::Transition)
        }
    }

    #[must_use]
    pub const fn terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Cancelled | Self::Rejected)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityOperation {
    Inspect,
    Execute,
    Collect,
    Delete,
}

/// The broker resolves this from a stored, hashed capability, not agent input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceGrant {
    pub experiment_id: Uuid,
    pub account_id: Uuid,
    pub resource_id: String,
    pub expires_at: u64,
    pub revoked: bool,
    pub operations: Vec<CapabilityOperation>,
}

impl ResourceGrant {
    /// The agent cannot widen this scope by supplying a resource name.
    ///
    /// # Errors
    /// Expired, revoked, or cross-experiment operation.
    pub fn authorize(
        &self,
        experiment: Uuid,
        account: Uuid,
        resource: &str,
        operation: CapabilityOperation,
        now: u64,
    ) -> Result<(), ContractError> {
        if self.revoked
            || self.experiment_id != experiment
            || self.account_id != account
            || self.resource_id != resource
            || !self.operations.contains(&operation)
        {
            return Err(ContractError::Scope);
        }
        if now >= self.expires_at {
            return Err(ContractError::Expired);
        }
        Ok(())
    }
}

/// An uncertain provider response must not trigger another blind rent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ProvisionResult {
    Confirmed { resource_id: String },
    NotCreated,
    Uncertain,
}

/// A DELETE response alone is not confirmation that billing stopped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum DeletionResult {
    Confirmed,
    Pending,
    Unauthorized,
    Unavailable,
}
