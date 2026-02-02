//! Error types for collaboration operations.

use crate::control::AgentThreadId;
use thiserror::Error;

/// Errors that can occur during collaboration operations.
#[derive(Debug, Error)]
pub enum CollabError {
    /// Message is empty or invalid.
    #[error("Message cannot be empty")]
    EmptyMessage,

    /// Agent ID is invalid or malformed.
    #[error("Invalid agent ID: {0}")]
    InvalidAgentId(String),

    /// Agent was not found.
    #[error("Agent not found: {0}")]
    AgentNotFound(AgentThreadId),

    /// Maximum spawn depth exceeded.
    #[error("Maximum spawn depth exceeded (max: {max_depth})")]
    DepthLimitExceeded { max_depth: u32 },

    /// Maximum concurrent agents exceeded.
    #[error("Maximum concurrent agents exceeded (max: {max_concurrent})")]
    ConcurrencyLimitExceeded { max_concurrent: usize },

    /// Spawn limit exceeded.
    #[error("Total spawn limit exceeded")]
    SpawnLimitExceeded,

    /// Timeout during wait operation.
    #[error("Wait operation timed out after {timeout_ms}ms")]
    WaitTimeout { timeout_ms: i64 },

    /// No agent IDs provided for wait.
    #[error("No agent IDs provided for wait")]
    NoAgentIds,

    /// Agent already in final state.
    #[error("Agent {0} is already in a final state")]
    AgentAlreadyFinal(AgentThreadId),

    /// Communication error with agent.
    #[error("Failed to communicate with agent: {0}")]
    CommunicationError(String),

    /// Internal error.
    #[error("Internal error: {0}")]
    Internal(String),

    /// Control error from underlying system.
    #[error(transparent)]
    ControlError(#[from] crate::control::AgentControlError),
}

impl CollabError {
    /// Check if this error is recoverable.
    pub fn is_recoverable(&self) -> bool {
        matches!(
            self,
            CollabError::WaitTimeout { .. } | CollabError::CommunicationError(_)
        )
    }

    /// Check if this error is due to a limit being exceeded.
    pub fn is_limit_exceeded(&self) -> bool {
        matches!(
            self,
            CollabError::DepthLimitExceeded { .. }
                | CollabError::ConcurrencyLimitExceeded { .. }
                | CollabError::SpawnLimitExceeded
        )
    }
}

/// Result type for collaboration operations.
pub type CollabResult<T> = std::result::Result<T, CollabError>;
