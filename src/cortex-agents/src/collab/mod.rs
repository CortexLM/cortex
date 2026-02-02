//! Collaboration tools for multi-agent communication.
//!
//! This module provides tools for spawning, communicating with, and managing
//! subagents in a multi-agent system.
//!
//! # Tools
//!
//! - **spawn_agent**: Create a new subagent for parallel task execution
//! - **send_input**: Send messages to running subagents
//! - **wait**: Wait for subagents to complete with timeout
//! - **close_agent**: Gracefully shutdown subagents
//!
//! # Example
//!
//! ```rust,ignore
//! use cortex_agents::collab::{spawn, wait, close};
//! use cortex_agents::AgentControl;
//!
//! let control = AgentControl::new();
//!
//! // Spawn a subagent
//! let result = spawn::handle(&control, spawn::SpawnAgentArgs {
//!     message: "Search for all error handling patterns".to_string(),
//!     agent_type: None,
//!     config: None,
//! }, None).await?;
//!
//! // Wait for completion
//! let wait_result = wait::handle(&control, wait::WaitArgs {
//!     ids: vec![result.agent_id.clone()],
//!     timeout_ms: Some(60_000),
//! }).await?;
//!
//! // Close if needed
//! close::handle(&control, close::CloseAgentArgs {
//!     id: result.agent_id,
//!     force: false,
//! }).await?;
//! ```
//!
//! # Security
//!
//! The collaboration system enforces several security limits:
//!
//! - **Depth limit**: Maximum spawn depth prevents infinite recursion
//! - **Concurrency limit**: Maximum concurrent agents per session
//! - **Timeout enforcement**: All wait operations have enforced timeouts
//! - **RAII cleanup**: AgentGuard ensures agents are cleaned up
//!
//! # Constants
//!
//! Key configuration constants:
//!
//! ```rust
//! use cortex_agents::collab::spawn::MAX_THREAD_SPAWN_DEPTH;
//! use cortex_agents::collab::wait::{MIN_WAIT_TIMEOUT_MS, MAX_WAIT_TIMEOUT_MS};
//!
//! assert_eq!(MAX_THREAD_SPAWN_DEPTH, 1); // No nested subagents
//! assert_eq!(MIN_WAIT_TIMEOUT_MS, 10_000); // 10 second minimum
//! assert_eq!(MAX_WAIT_TIMEOUT_MS, 300_000); // 5 minute maximum
//! ```

pub mod close;
pub mod error;
pub mod send_input;
pub mod spawn;
pub mod wait;

// Re-export commonly used types
pub use close::{AgentGuard, CloseAgentArgs, CloseAgentResult};
pub use error::{CollabError, CollabResult};
pub use send_input::{PendingInput, SendInputArgs, SendInputResult};
pub use spawn::{SpawnAgentArgs, SpawnAgentResult, SpawnConfig};
pub use wait::{AgentStatusInfo, WaitArgs, WaitResult};

// Re-export constants
pub use spawn::{MAX_CONCURRENT_AGENTS, MAX_THREAD_SPAWN_DEPTH};
pub use wait::{DEFAULT_WAIT_TIMEOUT_MS, MAX_WAIT_TIMEOUT_MS, MIN_WAIT_TIMEOUT_MS};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::AgentControl;

    #[tokio::test]
    async fn test_full_workflow() {
        let control = AgentControl::new();

        // 1. Spawn an agent
        let spawn_args = SpawnAgentArgs {
            message: "Test task".to_string(),
            agent_type: Some("general".to_string()),
            config: None,
        };
        let spawn_result = spawn::handle(&control, spawn_args, None).await.unwrap();
        assert!(!spawn_result.agent_id.is_empty());

        // 2. Send input to the agent
        let send_args = SendInputArgs {
            id: spawn_result.agent_id.clone(),
            message: "Additional instruction".to_string(),
            interrupt: false,
        };
        let send_result = send_input::handle(&control, send_args).await.unwrap();
        assert!(send_result.submission_id > 0);

        // 3. Close the agent
        let close_args = CloseAgentArgs {
            id: spawn_result.agent_id.clone(),
            force: false,
        };
        let close_result = close::handle(&control, close_args).await.unwrap();
        assert_eq!(close_result.final_status, "shutdown");

        // 4. Wait should show shutdown status
        let wait_args = WaitArgs {
            ids: vec![spawn_result.agent_id.clone()],
            timeout_ms: Some(10_000),
        };
        let wait_result = wait::handle(&control, wait_args).await.unwrap();
        assert!(!wait_result.timed_out);

        let status = wait_result.status.get(&spawn_result.agent_id).unwrap();
        assert!(status.completed);
    }

    #[tokio::test]
    async fn test_parallel_agents() {
        let control = AgentControl::new();

        // Spawn multiple agents
        let mut agent_ids = Vec::new();
        for i in 0..3 {
            let args = SpawnAgentArgs {
                message: format!("Task {}", i),
                agent_type: None,
                config: None,
            };
            let result = spawn::handle(&control, args, None).await.unwrap();
            agent_ids.push(result.agent_id);
        }

        // All should be active
        assert_eq!(control.active_count(), 3);

        // Close all
        let results = close::close_multiple(&control, &agent_ids, false).await;
        assert!(results.iter().all(|r| r.is_ok()));
    }
}
