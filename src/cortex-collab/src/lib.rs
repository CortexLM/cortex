//! Multi-agent collaboration system for Cortex CLI.
//!
//! This crate implements multi-agent collaboration features:
//! - Agent spawning and lifecycle management
//! - Inter-agent communication
//! - Wait/sync mechanisms with timeout
//! - Agent status tracking
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                     AgentControl                             │
//! │  ┌─────────────────┐  ┌─────────────────┐                   │
//! │  │  spawn_agent    │  │  send_input     │                   │
//! │  └─────────────────┘  └─────────────────┘                   │
//! │  ┌─────────────────┐  ┌─────────────────┐                   │
//! │  │  wait           │  │  close_agent    │                   │
//! │  └─────────────────┘  └─────────────────┘                   │
//! └───────────────────────────┬─────────────────────────────────┘
//!                             │
//!                             ▼
//! ┌─────────────────────────────────────────────────────────────┐
//! │                    ThreadManagerState                        │
//! │  ┌─────────────────────────────────────────────────────────┐│
//! │  │  threads: HashMap<ThreadId, AgentThread>                 ││
//! │  │  status_watchers: HashMap<ThreadId, watch::Sender>       ││
//! │  └─────────────────────────────────────────────────────────┘│
//! └─────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Example
//!
//! ```rust,ignore
//! use cortex_collab::{AgentControl, SpawnAgentArgs};
//!
//! let control = AgentControl::new();
//!
//! // Spawn a new agent
//! let agent_id = control.spawn_agent(SpawnAgentArgs {
//!     message: "Analyze the codebase".to_string(),
//!     agent_type: Some(AgentRole::Planner),
//! }).await?;
//!
//! // Wait for completion
//! let status = control.wait(vec![agent_id], Some(30_000)).await?;
//! ```

pub mod control;
pub mod guards;
pub mod handler;
pub mod role;
pub mod source;
pub mod status;
pub mod thread_manager;

pub use control::AgentControl;
pub use guards::{Guards, MAX_THREAD_SPAWN_DEPTH, SpawnReservation};
pub use handler::{
    CloseAgentArgs, CloseAgentResult, CollabHandler, SendInputArgs, SendInputResult,
    SpawnAgentArgs, SpawnAgentResult, WaitArgs, WaitResult,
};
pub use role::AgentRole;
pub use source::{SessionSource, SubAgentSource};
pub use status::AgentStatus;
pub use thread_manager::{AgentThread, ThreadId, ThreadManager, ThreadManagerState};

use thiserror::Error;

/// Errors for the collaboration system.
#[derive(Debug, Error)]
pub enum CollabError {
    /// Agent not found.
    #[error("Agent not found: {0}")]
    AgentNotFound(ThreadId),

    /// Agent depth limit exceeded.
    #[error("Agent depth limit reached. Solve the task yourself.")]
    DepthLimitExceeded,

    /// Empty message provided.
    #[error("Empty message can't be sent to an agent")]
    EmptyMessage,

    /// Spawn limit exceeded.
    #[error("Spawn limit exceeded")]
    SpawnLimitExceeded,

    /// Invalid timeout.
    #[error("timeout_ms must be greater than zero")]
    InvalidTimeout,

    /// Internal error.
    #[error("Internal error: {0}")]
    Internal(String),
}

pub type Result<T> = std::result::Result<T, CollabError>;

/// Timeout constants for wait operations.
pub mod timeouts {
    /// Minimum wait timeout (10 seconds) - anti-polling measure.
    pub const MIN_WAIT_TIMEOUT_MS: i64 = 10_000;

    /// Default wait timeout (30 seconds).
    pub const DEFAULT_WAIT_TIMEOUT_MS: i64 = 30_000;

    /// Maximum wait timeout (5 minutes).
    pub const MAX_WAIT_TIMEOUT_MS: i64 = 300_000;
}
