//! Close agent tool for graceful shutdown of subagents.
//!
//! This module provides the `close_agent` tool that allows agents to
//! shut down running subagents cleanly.
//!
//! # Example
//!
//! ```rust,ignore
//! use cortex_agents::collab::close::{CloseAgentArgs, handle};
//!
//! let args = CloseAgentArgs {
//!     id: "agent-uuid".to_string(),
//!     force: false,
//! };
//!
//! let result = handle(&control, args).await?;
//! println!("Agent closed with status: {}", result.final_status);
//! ```

use super::error::{CollabError, CollabResult};
use crate::control::{AgentControl, AgentThreadId, AgentThreadStatus};
use serde::{Deserialize, Serialize};

/// Arguments for close_agent tool.
#[derive(Debug, Clone, Deserialize)]
pub struct CloseAgentArgs {
    /// The agent ID to close.
    pub id: String,

    /// Whether to force close even if agent is in the middle of a task.
    #[serde(default)]
    pub force: bool,
}

/// Result of close_agent operation.
#[derive(Debug, Clone, Serialize)]
pub struct CloseAgentResult {
    /// The agent ID that was closed.
    pub agent_id: String,

    /// Final status of the agent.
    pub final_status: String,

    /// Whether the agent was already in a final state.
    pub was_already_final: bool,

    /// Optional result/output from the agent before closure.
    pub result: Option<String>,
}

/// Handle the close_agent tool call.
///
/// Gracefully shuts down a subagent, optionally forcing closure.
///
/// # Arguments
///
/// * `control` - The agent control system.
/// * `args` - Close arguments including agent ID and force flag.
///
/// # Returns
///
/// Returns the final status of the closed agent.
///
/// # Errors
///
/// Returns an error if:
/// - Agent ID is invalid
/// - Agent is not found
pub async fn handle(
    control: &AgentControl,
    args: CloseAgentArgs,
) -> CollabResult<CloseAgentResult> {
    // 1. Parse and validate the agent ID
    let agent_id =
        AgentThreadId::parse(&args.id).map_err(|_| CollabError::InvalidAgentId(args.id.clone()))?;

    // 2. Get current status
    let status = control.get_status(agent_id).await;

    // 3. Handle based on current status
    match status {
        AgentThreadStatus::NotFound => {
            return Err(CollabError::AgentNotFound(agent_id));
        }
        AgentThreadStatus::Completed(msg) => {
            return Ok(CloseAgentResult {
                agent_id: agent_id.to_string(),
                final_status: "completed".to_string(),
                was_already_final: true,
                result: msg,
            });
        }
        AgentThreadStatus::Errored(err) => {
            return Ok(CloseAgentResult {
                agent_id: agent_id.to_string(),
                final_status: "errored".to_string(),
                was_already_final: true,
                result: Some(err),
            });
        }
        AgentThreadStatus::Shutdown => {
            return Ok(CloseAgentResult {
                agent_id: agent_id.to_string(),
                final_status: "shutdown".to_string(),
                was_already_final: true,
                result: None,
            });
        }
        AgentThreadStatus::PendingInit | AgentThreadStatus::Running => {
            // Agent is active, proceed with shutdown
        }
    }

    // 4. Shutdown the agent
    control
        .shutdown_agent(agent_id)
        .await
        .map_err(CollabError::ControlError)?;

    // Log the closure
    tracing::info!(
        agent_id = %agent_id,
        force = args.force,
        "Agent closed"
    );

    Ok(CloseAgentResult {
        agent_id: agent_id.to_string(),
        final_status: "shutdown".to_string(),
        was_already_final: false,
        result: None,
    })
}

/// Close multiple agents at once.
///
/// Useful for cleanup operations that need to close all subagents.
///
/// # Arguments
///
/// * `control` - The agent control system.
/// * `ids` - Agent IDs to close.
/// * `force` - Whether to force close.
///
/// # Returns
///
/// Returns results for each agent closure attempt.
pub async fn close_multiple(
    control: &AgentControl,
    ids: &[String],
    force: bool,
) -> Vec<CollabResult<CloseAgentResult>> {
    let mut results = Vec::with_capacity(ids.len());

    for id in ids {
        let args = CloseAgentArgs {
            id: id.clone(),
            force,
        };
        results.push(handle(control, args).await);
    }

    results
}

/// RAII guard for automatic agent cleanup.
///
/// When dropped, automatically closes the agent if it hasn't completed.
pub struct AgentGuard {
    control: AgentControl,
    agent_id: AgentThreadId,
    closed: bool,
}

impl AgentGuard {
    /// Create a new guard for an agent.
    pub fn new(control: AgentControl, agent_id: AgentThreadId) -> Self {
        Self {
            control,
            agent_id,
            closed: false,
        }
    }

    /// Get the agent ID.
    pub fn agent_id(&self) -> AgentThreadId {
        self.agent_id
    }

    /// Manually close the agent and consume the guard.
    pub async fn close(mut self) -> CollabResult<CloseAgentResult> {
        self.closed = true;
        let args = CloseAgentArgs {
            id: self.agent_id.to_string(),
            force: false,
        };
        handle(&self.control, args).await
    }

    /// Mark the guard as no longer needing cleanup.
    /// Use this if the agent completed naturally.
    pub fn release(mut self) {
        self.closed = true;
    }
}

impl Drop for AgentGuard {
    fn drop(&mut self) {
        if !self.closed {
            // Spawn a task to close the agent asynchronously
            let control = self.control.clone();
            let agent_id = self.agent_id;

            tokio::spawn(async move {
                if let Err(e) = control.shutdown_agent(agent_id).await {
                    tracing::warn!(
                        agent_id = %agent_id,
                        error = %e,
                        "Failed to close agent during guard drop"
                    );
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AgentInfo;

    #[tokio::test]
    async fn test_close_running_agent() {
        let control = AgentControl::new();

        // Spawn agent
        let agent_id = control
            .spawn_agent(AgentInfo::new("test"), None)
            .await
            .unwrap();
        control
            .set_status(agent_id, AgentThreadStatus::Running)
            .await
            .unwrap();

        let args = CloseAgentArgs {
            id: agent_id.to_string(),
            force: false,
        };

        let result = handle(&control, args).await.unwrap();

        assert_eq!(result.agent_id, agent_id.to_string());
        assert_eq!(result.final_status, "shutdown");
        assert!(!result.was_already_final);
    }

    #[tokio::test]
    async fn test_close_completed_agent() {
        let control = AgentControl::new();

        // Spawn and complete agent
        let agent_id = control
            .spawn_agent(AgentInfo::new("test"), None)
            .await
            .unwrap();
        control
            .set_status(
                agent_id,
                AgentThreadStatus::Completed(Some("result".to_string())),
            )
            .await
            .unwrap();

        let args = CloseAgentArgs {
            id: agent_id.to_string(),
            force: false,
        };

        let result = handle(&control, args).await.unwrap();

        assert_eq!(result.final_status, "completed");
        assert!(result.was_already_final);
        assert_eq!(result.result, Some("result".to_string()));
    }

    #[tokio::test]
    async fn test_close_invalid_id() {
        let control = AgentControl::new();

        let args = CloseAgentArgs {
            id: "not-a-uuid".to_string(),
            force: false,
        };

        let result = handle(&control, args).await;
        assert!(matches!(result, Err(CollabError::InvalidAgentId(_))));
    }

    #[tokio::test]
    async fn test_close_nonexistent_agent() {
        let control = AgentControl::new();

        let fake_id = AgentThreadId::new();
        let args = CloseAgentArgs {
            id: fake_id.to_string(),
            force: false,
        };

        let result = handle(&control, args).await;
        assert!(matches!(result, Err(CollabError::AgentNotFound(_))));
    }

    #[tokio::test]
    async fn test_close_multiple() {
        let control = AgentControl::new();

        // Spawn multiple agents
        let id1 = control
            .spawn_agent(AgentInfo::new("test1"), None)
            .await
            .unwrap();
        let id2 = control
            .spawn_agent(AgentInfo::new("test2"), None)
            .await
            .unwrap();

        control
            .set_status(id1, AgentThreadStatus::Running)
            .await
            .unwrap();
        control
            .set_status(id2, AgentThreadStatus::Running)
            .await
            .unwrap();

        let ids = vec![id1.to_string(), id2.to_string()];
        let results = close_multiple(&control, &ids, false).await;

        assert_eq!(results.len(), 2);
        assert!(results[0].is_ok());
        assert!(results[1].is_ok());
    }

    #[tokio::test]
    async fn test_agent_guard() {
        let control = AgentControl::new();

        let agent_id = control
            .spawn_agent(AgentInfo::new("test"), None)
            .await
            .unwrap();
        control
            .set_status(agent_id, AgentThreadStatus::Running)
            .await
            .unwrap();

        {
            let guard = AgentGuard::new(control.clone(), agent_id);
            assert_eq!(guard.agent_id(), agent_id);

            // Guard dropped here - should trigger shutdown
        }

        // Give time for the async cleanup
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Agent should be shutdown
        let status = control.get_status(agent_id).await;
        assert!(matches!(status, AgentThreadStatus::Shutdown));
    }

    #[tokio::test]
    async fn test_agent_guard_release() {
        let control = AgentControl::new();

        let agent_id = control
            .spawn_agent(AgentInfo::new("test"), None)
            .await
            .unwrap();
        control
            .set_status(agent_id, AgentThreadStatus::Running)
            .await
            .unwrap();

        {
            let guard = AgentGuard::new(control.clone(), agent_id);
            guard.release(); // Mark as released - no cleanup needed
        }

        // Give time for any async operations
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Agent should still be running (not shutdown)
        let status = control.get_status(agent_id).await;
        assert!(matches!(status, AgentThreadStatus::Running));
    }
}
