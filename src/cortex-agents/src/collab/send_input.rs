//! Send input tool for inter-agent communication.
//!
//! This module provides the `send_input` tool that allows agents to send
//! messages to running subagents.
//!
//! # Example
//!
//! ```rust,ignore
//! use cortex_agents::collab::send_input::{SendInputArgs, handle};
//!
//! let args = SendInputArgs {
//!     id: "agent-uuid".to_string(),
//!     message: "Please also check the tests directory".to_string(),
//!     interrupt: false,
//! };
//!
//! let result = handle(&control, args).await?;
//! println!("Submission ID: {:?}", result.submission_id);
//! ```

use super::error::{CollabError, CollabResult};
use crate::control::{AgentControl, AgentThreadId, AgentThreadStatus};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};

/// Global submission ID counter.
static SUBMISSION_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Arguments for send_input tool.
#[derive(Debug, Clone, Deserialize)]
pub struct SendInputArgs {
    /// The agent ID to send input to.
    pub id: String,

    /// The message to send.
    pub message: String,

    /// Whether to interrupt the agent's current operation before sending.
    #[serde(default)]
    pub interrupt: bool,
}

/// Result of send_input operation.
#[derive(Debug, Clone, Serialize)]
pub struct SendInputResult {
    /// Unique ID for this submission.
    pub submission_id: u64,

    /// The agent ID that received the input.
    pub agent_id: String,

    /// Whether the agent was interrupted.
    pub interrupted: bool,

    /// Current status of the agent after sending.
    pub status: String,
}

/// Pending input to be processed by an agent.
#[derive(Debug, Clone)]
pub struct PendingInput {
    /// Submission ID.
    pub submission_id: u64,
    /// The message content.
    pub message: String,
    /// Whether this was an interrupt.
    pub interrupt: bool,
    /// Timestamp when submitted.
    pub submitted_at: std::time::Instant,
}

impl PendingInput {
    /// Create a new pending input.
    pub fn new(message: String, interrupt: bool) -> Self {
        Self {
            submission_id: SUBMISSION_COUNTER.fetch_add(1, Ordering::SeqCst),
            message,
            interrupt,
            submitted_at: std::time::Instant::now(),
        }
    }
}

/// Handle the send_input tool call.
///
/// Sends a message to a running subagent, optionally interrupting
/// its current operation first.
///
/// # Arguments
///
/// * `control` - The agent control system.
/// * `args` - Send input arguments.
///
/// # Returns
///
/// Returns a submission ID and status on success.
///
/// # Errors
///
/// Returns an error if:
/// - Agent ID is invalid
/// - Agent is not found
/// - Agent is already in a final state
/// - Communication failure
pub async fn handle(control: &AgentControl, args: SendInputArgs) -> CollabResult<SendInputResult> {
    // 1. Parse and validate the agent ID
    let agent_id =
        AgentThreadId::parse(&args.id).map_err(|_| CollabError::InvalidAgentId(args.id.clone()))?;

    // 2. Check agent exists and is in a valid state
    let status = control.get_status(agent_id).await;
    match &status {
        AgentThreadStatus::NotFound => {
            return Err(CollabError::AgentNotFound(agent_id));
        }
        AgentThreadStatus::Completed(_)
        | AgentThreadStatus::Errored(_)
        | AgentThreadStatus::Shutdown => {
            return Err(CollabError::AgentAlreadyFinal(agent_id));
        }
        _ => {}
    }

    // 3. Validate message
    let message = args.message.trim();
    if message.is_empty() {
        return Err(CollabError::EmptyMessage);
    }

    // 4. Create pending input
    let pending = PendingInput::new(message.to_string(), args.interrupt);
    let submission_id = pending.submission_id;

    // 5. If interrupt requested, signal interruption first
    // Note: Actual interrupt handling would need to be implemented
    // in the agent execution loop. This just records the intent.
    let interrupted = args.interrupt;
    if interrupted {
        tracing::debug!(
            agent_id = %agent_id,
            submission_id = submission_id,
            "Interrupt requested for agent"
        );
    }

    // 6. Queue the input for the agent
    // In a full implementation, this would go into a channel/queue
    // that the agent's execution loop reads from.
    tracing::info!(
        agent_id = %agent_id,
        submission_id = submission_id,
        interrupt = interrupted,
        "Queued input for agent"
    );

    // 7. Return the submission result
    let status_str = match control.get_status(agent_id).await {
        AgentThreadStatus::PendingInit => "pending_init",
        AgentThreadStatus::Running => "running",
        AgentThreadStatus::Completed(_) => "completed",
        AgentThreadStatus::Errored(_) => "errored",
        AgentThreadStatus::Shutdown => "shutdown",
        AgentThreadStatus::NotFound => "not_found",
    };

    Ok(SendInputResult {
        submission_id,
        agent_id: agent_id.to_string(),
        interrupted,
        status: status_str.to_string(),
    })
}

/// Check if an agent can receive input.
pub async fn can_receive_input(control: &AgentControl, agent_id: AgentThreadId) -> bool {
    let status = control.get_status(agent_id).await;
    !status.is_final()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AgentInfo;

    #[tokio::test]
    async fn test_send_input() {
        let control = AgentControl::new();

        // Spawn an agent first
        let agent_id = control
            .spawn_agent(AgentInfo::new("test"), None)
            .await
            .unwrap();
        control
            .set_status(agent_id, AgentThreadStatus::Running)
            .await
            .unwrap();

        let args = SendInputArgs {
            id: agent_id.to_string(),
            message: "Test message".to_string(),
            interrupt: false,
        };

        let result = handle(&control, args).await.unwrap();

        assert!(result.submission_id > 0);
        assert_eq!(result.agent_id, agent_id.to_string());
        assert!(!result.interrupted);
        assert_eq!(result.status, "running");
    }

    #[tokio::test]
    async fn test_send_input_invalid_id() {
        let control = AgentControl::new();

        let args = SendInputArgs {
            id: "not-a-valid-uuid".to_string(),
            message: "Test".to_string(),
            interrupt: false,
        };

        let result = handle(&control, args).await;
        assert!(matches!(result, Err(CollabError::InvalidAgentId(_))));
    }

    #[tokio::test]
    async fn test_send_input_agent_not_found() {
        let control = AgentControl::new();

        let fake_id = AgentThreadId::new();
        let args = SendInputArgs {
            id: fake_id.to_string(),
            message: "Test".to_string(),
            interrupt: false,
        };

        let result = handle(&control, args).await;
        assert!(matches!(result, Err(CollabError::AgentNotFound(_))));
    }

    #[tokio::test]
    async fn test_send_input_empty_message() {
        let control = AgentControl::new();

        let agent_id = control
            .spawn_agent(AgentInfo::new("test"), None)
            .await
            .unwrap();
        control
            .set_status(agent_id, AgentThreadStatus::Running)
            .await
            .unwrap();

        let args = SendInputArgs {
            id: agent_id.to_string(),
            message: "   ".to_string(),
            interrupt: false,
        };

        let result = handle(&control, args).await;
        assert!(matches!(result, Err(CollabError::EmptyMessage)));
    }

    #[tokio::test]
    async fn test_send_input_to_completed_agent() {
        let control = AgentControl::new();

        let agent_id = control
            .spawn_agent(AgentInfo::new("test"), None)
            .await
            .unwrap();
        control
            .set_status(agent_id, AgentThreadStatus::Completed(None))
            .await
            .unwrap();

        let args = SendInputArgs {
            id: agent_id.to_string(),
            message: "Test".to_string(),
            interrupt: false,
        };

        let result = handle(&control, args).await;
        assert!(matches!(result, Err(CollabError::AgentAlreadyFinal(_))));
    }

    #[tokio::test]
    async fn test_send_input_with_interrupt() {
        let control = AgentControl::new();

        let agent_id = control
            .spawn_agent(AgentInfo::new("test"), None)
            .await
            .unwrap();
        control
            .set_status(agent_id, AgentThreadStatus::Running)
            .await
            .unwrap();

        let args = SendInputArgs {
            id: agent_id.to_string(),
            message: "Interrupt message".to_string(),
            interrupt: true,
        };

        let result = handle(&control, args).await.unwrap();
        assert!(result.interrupted);
    }

    #[tokio::test]
    async fn test_can_receive_input() {
        let control = AgentControl::new();

        let agent_id = control
            .spawn_agent(AgentInfo::new("test"), None)
            .await
            .unwrap();

        // PendingInit can receive
        assert!(can_receive_input(&control, agent_id).await);

        // Running can receive
        control
            .set_status(agent_id, AgentThreadStatus::Running)
            .await
            .unwrap();
        assert!(can_receive_input(&control, agent_id).await);

        // Completed cannot receive
        control
            .set_status(agent_id, AgentThreadStatus::Completed(None))
            .await
            .unwrap();
        assert!(!can_receive_input(&control, agent_id).await);
    }
}
