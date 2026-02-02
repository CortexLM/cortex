//! Spawn agent tool for creating new subagents.
//!
//! This module provides the `spawn_agent` tool that allows agents to create
//! new subagents for parallel task execution.
//!
//! # Example
//!
//! ```rust,ignore
//! use cortex_agents::collab::spawn::{SpawnAgentArgs, handle};
//!
//! let args = SpawnAgentArgs {
//!     message: "Search for all error handling patterns".to_string(),
//!     agent_type: None,
//! };
//!
//! let result = handle(&control, args, 0).await?;
//! println!("Spawned agent: {}", result.agent_id);
//! ```

use super::error::{CollabError, CollabResult};
use crate::control::{AgentControl, AgentControlError, AgentThreadId, AgentThreadStatus};
use crate::AgentInfo;
use serde::{Deserialize, Serialize};

/// Maximum allowed spawn depth (1 = no nested subagents).
pub const MAX_THREAD_SPAWN_DEPTH: u32 = 1;

/// Maximum concurrent agents per session.
pub const MAX_CONCURRENT_AGENTS: usize = 10;

/// Arguments for spawn_agent tool.
#[derive(Debug, Clone, Deserialize)]
pub struct SpawnAgentArgs {
    /// The initial message/task for the new agent.
    pub message: String,

    /// Optional agent type/role to use.
    /// If not specified, uses "general" subagent.
    #[serde(default)]
    pub agent_type: Option<String>,

    /// Optional custom configuration.
    #[serde(default)]
    pub config: Option<SpawnConfig>,
}

/// Additional spawn configuration.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SpawnConfig {
    /// Maximum steps for the spawned agent.
    pub max_steps: Option<usize>,

    /// Temperature override.
    pub temperature: Option<f32>,

    /// Custom prompt prefix.
    pub prompt_prefix: Option<String>,
}

/// Result of spawn_agent operation.
#[derive(Debug, Clone, Serialize)]
pub struct SpawnAgentResult {
    /// The unique ID of the spawned agent.
    pub agent_id: String,

    /// The type of agent spawned.
    pub agent_type: String,

    /// Current status of the agent.
    pub status: String,

    /// Depth level of the spawned agent.
    pub depth: u32,
}

/// Handle the spawn_agent tool call.
///
/// Creates a new subagent with the given message as its initial task.
///
/// # Arguments
///
/// * `control` - The agent control system.
/// * `args` - Spawn arguments including message and optional agent type.
/// * `current_depth` - Current depth in the agent hierarchy.
/// * `parent_id` - Optional parent agent ID.
///
/// # Returns
///
/// Returns the spawned agent's ID and metadata on success.
///
/// # Errors
///
/// Returns an error if:
/// - Message is empty
/// - Maximum depth would be exceeded
/// - Maximum concurrent agents reached
/// - Internal spawn failure
pub async fn handle(
    control: &AgentControl,
    args: SpawnAgentArgs,
    parent_id: Option<AgentThreadId>,
) -> CollabResult<SpawnAgentResult> {
    // 1. Validate message is non-empty
    let message = args.message.trim();
    if message.is_empty() {
        return Err(CollabError::EmptyMessage);
    }

    // 2. Determine agent type
    let agent_type = args.agent_type.as_deref().unwrap_or("general");

    // 3. Build agent info
    let mut agent_info = AgentInfo::new(format!("subagent-{}", uuid::Uuid::new_v4()))
        .with_description(format!("Subagent for: {}", truncate_message(message, 100)));

    // Apply optional config
    if let Some(config) = args.config {
        if let Some(max_steps) = config.max_steps {
            agent_info = agent_info.with_max_steps(max_steps);
        }
        if let Some(temp) = config.temperature {
            agent_info = agent_info.with_temperature(temp);
        }
        if let Some(prefix) = config.prompt_prefix {
            let prompt = agent_info.prompt.unwrap_or_default();
            agent_info.prompt = Some(format!("{}\n\n{}", prefix, prompt));
        }
    }

    // 4. Spawn via AgentControl
    let agent_id = control
        .spawn_agent(agent_info, parent_id)
        .await
        .map_err(|e| match e {
            AgentControlError::DepthLimitExceeded => CollabError::DepthLimitExceeded {
                max_depth: MAX_THREAD_SPAWN_DEPTH,
            },
            AgentControlError::ConcurrencyLimitExceeded => CollabError::ConcurrencyLimitExceeded {
                max_concurrent: MAX_CONCURRENT_AGENTS,
            },
            AgentControlError::SpawnLimitExceeded => CollabError::SpawnLimitExceeded,
            other => CollabError::ControlError(other),
        })?;

    // 5. Set initial status to running
    control
        .set_status(agent_id, AgentThreadStatus::Running)
        .await
        .map_err(CollabError::ControlError)?;

    // Log spawn (without sensitive content)
    tracing::info!(
        agent_id = %agent_id,
        agent_type = agent_type,
        "Spawned new subagent"
    );

    Ok(SpawnAgentResult {
        agent_id: agent_id.to_string(),
        agent_type: agent_type.to_string(),
        status: "running".to_string(),
        depth: get_agent_depth(control, agent_id).await,
    })
}

/// Validate that a spawn is allowed at the current depth.
pub fn validate_spawn_depth(current_depth: u32) -> CollabResult<()> {
    if current_depth >= MAX_THREAD_SPAWN_DEPTH {
        return Err(CollabError::DepthLimitExceeded {
            max_depth: MAX_THREAD_SPAWN_DEPTH,
        });
    }
    Ok(())
}

/// Check if spawning is allowed given current state.
pub fn can_spawn(control: &AgentControl, current_depth: u32) -> bool {
    current_depth < MAX_THREAD_SPAWN_DEPTH && control.active_count() < MAX_CONCURRENT_AGENTS
}

/// Get the depth of an agent.
async fn get_agent_depth(control: &AgentControl, id: AgentThreadId) -> u32 {
    control
        .state()
        .get_thread(id)
        .await
        .map(|t| t.depth)
        .unwrap_or(0)
}

/// Truncate a message for logging/display purposes.
fn truncate_message(message: &str, max_len: usize) -> String {
    if message.len() <= max_len {
        message.to_string()
    } else {
        format!("{}...", &message[..max_len])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::AgentLimits;

    #[tokio::test]
    async fn test_spawn_agent() {
        let control = AgentControl::new();

        let args = SpawnAgentArgs {
            message: "Test task".to_string(),
            agent_type: None,
            config: None,
        };

        let result = handle(&control, args, None).await.unwrap();

        assert!(!result.agent_id.is_empty());
        assert_eq!(result.agent_type, "general");
        assert_eq!(result.status, "running");
        assert_eq!(result.depth, 0);
    }

    #[tokio::test]
    async fn test_spawn_empty_message() {
        let control = AgentControl::new();

        let args = SpawnAgentArgs {
            message: "   ".to_string(),
            agent_type: None,
            config: None,
        };

        let result = handle(&control, args, None).await;
        assert!(matches!(result, Err(CollabError::EmptyMessage)));
    }

    #[tokio::test]
    async fn test_spawn_depth_limit() {
        let limits = AgentLimits {
            max_depth: 1,
            ..Default::default()
        };
        let control = AgentControl::with_limits(limits);

        // Spawn root
        let root_args = SpawnAgentArgs {
            message: "Root task".to_string(),
            agent_type: None,
            config: None,
        };
        let root = handle(&control, root_args, None).await.unwrap();
        let root_id = AgentThreadId::parse(&root.agent_id).unwrap();

        // Spawn child
        let child_args = SpawnAgentArgs {
            message: "Child task".to_string(),
            agent_type: None,
            config: None,
        };
        let child = handle(&control, child_args, Some(root_id)).await.unwrap();
        let child_id = AgentThreadId::parse(&child.agent_id).unwrap();

        // Try to spawn grandchild - should fail
        let grandchild_args = SpawnAgentArgs {
            message: "Grandchild task".to_string(),
            agent_type: None,
            config: None,
        };
        let result = handle(&control, grandchild_args, Some(child_id)).await;

        assert!(matches!(
            result,
            Err(CollabError::DepthLimitExceeded { .. })
        ));
    }

    #[test]
    fn test_validate_spawn_depth() {
        assert!(validate_spawn_depth(0).is_ok());
        assert!(validate_spawn_depth(1).is_err());
        assert!(validate_spawn_depth(2).is_err());
    }

    #[test]
    fn test_truncate_message() {
        assert_eq!(truncate_message("short", 10), "short");
        assert_eq!(
            truncate_message("this is a long message", 10),
            "this is a ..."
        );
    }
}
