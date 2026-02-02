//! Collaboration tool handlers.

use super::{
    AgentControl, AgentRole, AgentStatus, CollabError, Result, ThreadId,
    guards::exceeds_thread_spawn_depth_limit, source::next_thread_spawn_depth, timeouts,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{Duration, Instant};

// ============================================================================
// spawn_agent
// ============================================================================

/// Arguments for spawn_agent tool.
#[derive(Debug, Clone, Deserialize)]
pub struct SpawnAgentArgs {
    /// Message/prompt for the new agent.
    pub message: String,

    /// Type of agent to spawn.
    #[serde(default)]
    pub agent_type: Option<AgentRole>,
}

/// Result of spawn_agent tool.
#[derive(Debug, Clone, Serialize)]
pub struct SpawnAgentResult {
    /// ID of the spawned agent.
    pub agent_id: String,
}

// ============================================================================
// send_input
// ============================================================================

/// Arguments for send_input tool.
#[derive(Debug, Clone, Deserialize)]
pub struct SendInputArgs {
    /// ID of the target agent.
    pub id: String,

    /// Message to send.
    pub message: String,

    /// Whether to interrupt the agent before sending.
    #[serde(default)]
    pub interrupt: bool,
}

/// Result of send_input tool.
#[derive(Debug, Clone, Serialize)]
pub struct SendInputResult {
    /// Submission ID.
    pub submission_id: String,
}

// ============================================================================
// wait
// ============================================================================

/// Arguments for wait tool.
#[derive(Debug, Clone, Deserialize)]
pub struct WaitArgs {
    /// IDs of agents to wait for.
    pub ids: Vec<String>,

    /// Timeout in milliseconds.
    #[serde(default)]
    pub timeout_ms: Option<i64>,
}

/// Result of wait tool.
#[derive(Debug, Clone, Serialize)]
pub struct WaitResult {
    /// Status of each agent.
    pub status: HashMap<String, AgentStatus>,

    /// Whether the wait timed out.
    pub timed_out: bool,
}

// ============================================================================
// close_agent
// ============================================================================

/// Arguments for close_agent tool.
#[derive(Debug, Clone, Deserialize)]
pub struct CloseAgentArgs {
    /// ID of the agent to close.
    pub id: String,
}

/// Result of close_agent tool.
#[derive(Debug, Clone, Serialize)]
pub struct CloseAgentResult {
    /// Final status of the agent.
    pub status: AgentStatus,
}

// ============================================================================
// CollabHandler
// ============================================================================

/// Handler for collaboration tools.
pub struct CollabHandler {
    control: AgentControl,
}

impl CollabHandler {
    /// Create a new collaboration handler.
    pub fn new(control: AgentControl) -> Self {
        Self { control }
    }

    /// Handle spawn_agent tool call.
    pub async fn handle_spawn_agent(
        &self,
        args: SpawnAgentArgs,
        parent_thread_id: ThreadId,
        current_depth: i32,
    ) -> Result<SpawnAgentResult> {
        // Validate message
        if args.message.trim().is_empty() {
            return Err(CollabError::EmptyMessage);
        }

        // Check depth limit
        let child_depth = next_thread_spawn_depth(&super::SessionSource::SubAgent(
            super::SubAgentSource::ThreadSpawn {
                parent_thread_id,
                depth: current_depth,
            },
        ));

        if exceeds_thread_spawn_depth_limit(child_depth) {
            return Err(CollabError::DepthLimitExceeded);
        }

        // Build config
        let role = args.agent_type.unwrap_or(AgentRole::General);
        let config =
            self.control
                .build_spawn_config(role, &args.message, parent_thread_id, child_depth);

        // Spawn the agent
        let agent_id = self
            .control
            .spawn_agent(config, args.message.clone(), None)
            .await?;

        Ok(SpawnAgentResult {
            agent_id: agent_id.to_string(),
        })
    }

    /// Handle send_input tool call.
    pub async fn handle_send_input(&self, args: SendInputArgs) -> Result<SendInputResult> {
        // Validate message
        if args.message.trim().is_empty() {
            return Err(CollabError::EmptyMessage);
        }

        // Parse agent ID
        let agent_id =
            ThreadId::parse(&args.id).map_err(|_| CollabError::AgentNotFound(ThreadId::new()))?;

        // Interrupt if requested
        if args.interrupt {
            self.control.interrupt_agent(agent_id).await?;
        }

        // Send the prompt
        let submission_id = self.control.send_prompt(agent_id, args.message).await?;

        Ok(SendInputResult { submission_id })
    }

    /// Handle wait tool call.
    pub async fn handle_wait(&self, args: WaitArgs) -> Result<WaitResult> {
        // Validate IDs
        if args.ids.is_empty() {
            return Err(CollabError::Internal("ids must be non-empty".to_string()));
        }

        // Validate and clamp timeout
        let timeout_ms = args.timeout_ms.unwrap_or(timeouts::DEFAULT_WAIT_TIMEOUT_MS);
        if timeout_ms <= 0 {
            return Err(CollabError::InvalidTimeout);
        }
        let timeout_ms =
            timeout_ms.clamp(timeouts::MIN_WAIT_TIMEOUT_MS, timeouts::MAX_WAIT_TIMEOUT_MS);

        // Parse agent IDs
        let mut agent_ids = Vec::with_capacity(args.ids.len());
        for id_str in &args.ids {
            match ThreadId::parse(id_str) {
                Ok(id) => agent_ids.push(id),
                Err(_) => {
                    // Agent not found, will be reported in status
                }
            }
        }

        // Get initial statuses
        let mut statuses = HashMap::new();
        let mut pending = Vec::new();

        for id in &agent_ids {
            let status = self.control.get_status(*id).await;
            if status.is_final() {
                statuses.insert(id.to_string(), status);
            } else {
                pending.push(*id);
            }
        }

        // Wait for pending agents with timeout
        let deadline = Instant::now() + Duration::from_millis(timeout_ms as u64);
        let mut timed_out = false;

        for id in pending {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                timed_out = true;
                statuses.insert(id.to_string(), self.control.get_status(id).await);
                continue;
            }

            match self.control.subscribe_status(id).await {
                Ok(mut rx) => loop {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        timed_out = true;
                        statuses.insert(id.to_string(), rx.borrow().clone());
                        break;
                    }

                    tokio::select! {
                        _ = tokio::time::sleep(remaining) => {
                            timed_out = true;
                            statuses.insert(id.to_string(), rx.borrow().clone());
                            break;
                        }
                        result = rx.changed() => {
                            if result.is_err() {
                                statuses.insert(id.to_string(), AgentStatus::NotFound);
                                break;
                            }
                            let status = rx.borrow().clone();
                            if status.is_final() {
                                statuses.insert(id.to_string(), status);
                                break;
                            }
                        }
                    }
                },
                Err(_) => {
                    statuses.insert(id.to_string(), AgentStatus::NotFound);
                }
            }
        }

        Ok(WaitResult {
            status: statuses,
            timed_out,
        })
    }

    /// Handle close_agent tool call.
    pub async fn handle_close_agent(&self, args: CloseAgentArgs) -> Result<CloseAgentResult> {
        // Parse agent ID
        let agent_id =
            ThreadId::parse(&args.id).map_err(|_| CollabError::AgentNotFound(ThreadId::new()))?;

        // Get current status
        let status = self.control.get_status(agent_id).await;

        // Shutdown if not already
        if !matches!(status, AgentStatus::Shutdown | AgentStatus::NotFound) {
            self.control.shutdown_agent(agent_id).await?;
        }

        Ok(CloseAgentResult {
            status: self.control.get_status(agent_id).await,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ThreadManagerState;
    use std::sync::Arc;

    #[tokio::test]
    async fn test_spawn_agent() {
        let state = Arc::new(ThreadManagerState::new());
        let control = AgentControl::new(Arc::downgrade(&state));
        let handler = CollabHandler::new(control);

        let args = SpawnAgentArgs {
            message: "Test task".to_string(),
            agent_type: Some(AgentRole::General),
        };

        let result = handler
            .handle_spawn_agent(args, ThreadId::new(), 0)
            .await
            .unwrap();

        assert!(!result.agent_id.is_empty());
    }

    #[tokio::test]
    async fn test_spawn_depth_limit() {
        let state = Arc::new(ThreadManagerState::new());
        let control = AgentControl::new(Arc::downgrade(&state));
        let handler = CollabHandler::new(control);

        let args = SpawnAgentArgs {
            message: "Test task".to_string(),
            agent_type: None,
        };

        // Should fail at depth 1 (exceeds limit of 1)
        let result = handler.handle_spawn_agent(args, ThreadId::new(), 1).await;

        assert!(matches!(result, Err(CollabError::DepthLimitExceeded)));
    }
}
