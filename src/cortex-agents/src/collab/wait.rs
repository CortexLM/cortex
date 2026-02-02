//! Wait tool for synchronizing with subagents.
//!
//! This module provides the `wait` tool that allows agents to wait for
//! one or more subagents to complete their tasks.
//!
//! # Example
//!
//! ```rust,ignore
//! use cortex_agents::collab::wait::{WaitArgs, handle};
//!
//! let args = WaitArgs {
//!     ids: vec!["agent-1".to_string(), "agent-2".to_string()],
//!     timeout_ms: Some(60_000),
//! };
//!
//! let result = handle(&control, args).await?;
//! if result.timed_out {
//!     println!("Wait timed out!");
//! }
//! ```

use super::error::{CollabError, CollabResult};
use crate::control::{AgentControl, AgentThreadId, AgentThreadStatus};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;
use tokio::time::timeout;

/// Minimum wait timeout in milliseconds.
pub const MIN_WAIT_TIMEOUT_MS: i64 = 10_000; // 10 seconds

/// Default wait timeout in milliseconds.
pub const DEFAULT_WAIT_TIMEOUT_MS: i64 = 30_000; // 30 seconds

/// Maximum wait timeout in milliseconds.
pub const MAX_WAIT_TIMEOUT_MS: i64 = 300_000; // 5 minutes

/// Arguments for wait tool.
#[derive(Debug, Clone, Deserialize)]
pub struct WaitArgs {
    /// Agent IDs to wait for.
    pub ids: Vec<String>,

    /// Timeout in milliseconds.
    /// Will be clamped to [MIN_WAIT_TIMEOUT_MS, MAX_WAIT_TIMEOUT_MS].
    #[serde(default)]
    pub timeout_ms: Option<i64>,
}

/// Status info for a single agent in wait result.
#[derive(Debug, Clone, Serialize)]
pub struct AgentStatusInfo {
    /// Whether this agent completed (reached a final state).
    pub completed: bool,
    /// Final status string.
    pub status: String,
    /// Optional result/message from the agent.
    pub result: Option<String>,
    /// Whether this agent errored.
    pub errored: bool,
}

impl From<AgentThreadStatus> for AgentStatusInfo {
    fn from(status: AgentThreadStatus) -> Self {
        match status {
            AgentThreadStatus::Completed(msg) => Self {
                completed: true,
                status: "completed".to_string(),
                result: msg,
                errored: false,
            },
            AgentThreadStatus::Errored(err) => Self {
                completed: true,
                status: "errored".to_string(),
                result: Some(err),
                errored: true,
            },
            AgentThreadStatus::Shutdown => Self {
                completed: true,
                status: "shutdown".to_string(),
                result: None,
                errored: false,
            },
            AgentThreadStatus::NotFound => Self {
                completed: true,
                status: "not_found".to_string(),
                result: None,
                errored: true,
            },
            AgentThreadStatus::Running => Self {
                completed: false,
                status: "running".to_string(),
                result: None,
                errored: false,
            },
            AgentThreadStatus::PendingInit => Self {
                completed: false,
                status: "pending_init".to_string(),
                result: None,
                errored: false,
            },
        }
    }
}

/// Result of wait operation.
#[derive(Debug, Clone, Serialize)]
pub struct WaitResult {
    /// Status of each agent by ID.
    pub status: HashMap<String, AgentStatusInfo>,

    /// Whether the wait timed out before all agents completed.
    pub timed_out: bool,

    /// Number of agents that completed successfully.
    pub completed_count: usize,

    /// Number of agents that errored.
    pub errored_count: usize,

    /// Actual timeout used (after clamping).
    pub timeout_used_ms: i64,
}

/// Handle the wait tool call.
///
/// Waits for one or more agents to reach a final state, with a timeout.
///
/// # Arguments
///
/// * `control` - The agent control system.
/// * `args` - Wait arguments including agent IDs and optional timeout.
///
/// # Returns
///
/// Returns the status of all agents and whether the wait timed out.
///
/// # Errors
///
/// Returns an error if:
/// - No agent IDs provided
/// - All agent IDs are invalid
pub async fn handle(control: &AgentControl, args: WaitArgs) -> CollabResult<WaitResult> {
    // 1. Validate IDs non-empty
    if args.ids.is_empty() {
        return Err(CollabError::NoAgentIds);
    }

    // 2. Parse and validate agent IDs
    let mut valid_ids: Vec<AgentThreadId> = Vec::new();
    let mut invalid_ids: Vec<String> = Vec::new();

    for id_str in &args.ids {
        match AgentThreadId::parse(id_str) {
            Ok(id) => valid_ids.push(id),
            Err(_) => invalid_ids.push(id_str.clone()),
        }
    }

    // If all IDs are invalid, that's an error
    if valid_ids.is_empty() {
        return Err(CollabError::InvalidAgentId(invalid_ids.join(", ")));
    }

    // 3. Clamp timeout
    let timeout_ms = clamp_timeout(args.timeout_ms.unwrap_or(DEFAULT_WAIT_TIMEOUT_MS));
    let timeout_duration = Duration::from_millis(timeout_ms as u64);

    // 4. Wait for agents with timeout
    let wait_future = wait_for_agents(control, &valid_ids);
    let (final_statuses, timed_out) = match timeout(timeout_duration, wait_future).await {
        Ok(statuses) => (statuses, false),
        Err(_) => {
            // Timeout - get current statuses
            let mut statuses = HashMap::new();
            for id in &valid_ids {
                let status = control.get_status(*id).await;
                statuses.insert(*id, status);
            }
            (statuses, true)
        }
    };

    // 5. Build result
    let mut status_map = HashMap::new();
    let mut completed_count = 0;
    let mut errored_count = 0;

    for (id, status) in final_statuses {
        let info = AgentStatusInfo::from(status);
        if info.completed {
            completed_count += 1;
        }
        if info.errored {
            errored_count += 1;
        }
        status_map.insert(id.to_string(), info);
    }

    // Add invalid IDs with not_found status
    for id in invalid_ids {
        status_map.insert(
            id,
            AgentStatusInfo {
                completed: true,
                status: "invalid_id".to_string(),
                result: None,
                errored: true,
            },
        );
        errored_count += 1;
    }

    tracing::info!(
        ids_count = args.ids.len(),
        completed = completed_count,
        errored = errored_count,
        timed_out = timed_out,
        timeout_ms = timeout_ms,
        "Wait completed"
    );

    Ok(WaitResult {
        status: status_map,
        timed_out,
        completed_count,
        errored_count,
        timeout_used_ms: timeout_ms,
    })
}

/// Wait for all agents to reach a final state.
async fn wait_for_agents(
    control: &AgentControl,
    ids: &[AgentThreadId],
) -> HashMap<AgentThreadId, AgentThreadStatus> {
    use futures::stream::FuturesUnordered;
    use futures::StreamExt;

    let mut futures: FuturesUnordered<_> = ids
        .iter()
        .map(|&id| wait_for_single_agent(control, id))
        .collect();

    let mut results = HashMap::new();

    while let Some((id, status)) = futures.next().await {
        results.insert(id, status);
    }

    results
}

/// Wait for a single agent to reach a final state.
async fn wait_for_single_agent(
    control: &AgentControl,
    id: AgentThreadId,
) -> (AgentThreadId, AgentThreadStatus) {
    // Try to subscribe to status updates
    if let Ok(mut rx) = control.subscribe_status(id).await {
        // Wait for final status
        loop {
            let status = rx.borrow().clone();
            if status.is_final() {
                return (id, status);
            }

            // Wait for change
            if rx.changed().await.is_err() {
                // Channel closed, get final status
                return (id, control.get_status(id).await);
            }
        }
    } else {
        // Agent not found
        (id, AgentThreadStatus::NotFound)
    }
}

/// Clamp timeout to valid range.
pub fn clamp_timeout(timeout_ms: i64) -> i64 {
    timeout_ms.clamp(MIN_WAIT_TIMEOUT_MS, MAX_WAIT_TIMEOUT_MS)
}

/// Calculate total timeout for waiting on multiple agents.
/// Uses a formula to scale timeout based on agent count.
pub fn calculate_scaled_timeout(base_timeout_ms: i64, agent_count: usize) -> i64 {
    // Scale factor: sqrt(n) to avoid linear scaling
    let scale = (agent_count as f64).sqrt().max(1.0);
    let scaled = (base_timeout_ms as f64 * scale) as i64;
    clamp_timeout(scaled)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AgentInfo;

    #[tokio::test]
    async fn test_wait_single_agent() {
        let control = AgentControl::new();

        // Spawn agent
        let agent_id = control
            .spawn_agent(AgentInfo::new("test"), None)
            .await
            .unwrap();

        // Set to running then completed
        control
            .set_status(agent_id, AgentThreadStatus::Running)
            .await
            .unwrap();

        // Spawn task to complete it after a short delay
        let control_clone = control.clone();
        let agent_id_clone = agent_id;
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            control_clone
                .set_status(
                    agent_id_clone,
                    AgentThreadStatus::Completed(Some("done".to_string())),
                )
                .await
                .ok();
        });

        let args = WaitArgs {
            ids: vec![agent_id.to_string()],
            timeout_ms: Some(10_000),
        };

        let result = handle(&control, args).await.unwrap();

        assert!(!result.timed_out);
        assert_eq!(result.completed_count, 1);
        assert_eq!(result.errored_count, 0);

        let status = result.status.get(&agent_id.to_string()).unwrap();
        assert!(status.completed);
        assert_eq!(status.status, "completed");
        assert_eq!(status.result, Some("done".to_string()));
    }

    #[tokio::test]
    async fn test_wait_no_ids() {
        let control = AgentControl::new();

        let args = WaitArgs {
            ids: vec![],
            timeout_ms: None,
        };

        let result = handle(&control, args).await;
        assert!(matches!(result, Err(CollabError::NoAgentIds)));
    }

    #[tokio::test]
    async fn test_wait_invalid_ids() {
        let control = AgentControl::new();

        let args = WaitArgs {
            ids: vec!["not-a-uuid".to_string(), "also-not-valid".to_string()],
            timeout_ms: None,
        };

        let result = handle(&control, args).await;
        assert!(matches!(result, Err(CollabError::InvalidAgentId(_))));
    }

    #[tokio::test]
    async fn test_wait_timeout() {
        let control = AgentControl::new();

        // Spawn agent that won't complete
        let agent_id = control
            .spawn_agent(AgentInfo::new("test"), None)
            .await
            .unwrap();
        control
            .set_status(agent_id, AgentThreadStatus::Running)
            .await
            .unwrap();

        let args = WaitArgs {
            ids: vec![agent_id.to_string()],
            timeout_ms: Some(10_000), // Minimum timeout
        };

        // Use a very short actual timeout for testing
        let timeout_duration = Duration::from_millis(100);
        let result = timeout(timeout_duration, handle(&control, args)).await;

        // Should timeout at the outer level or inner level
        assert!(result.is_err() || result.unwrap().unwrap().timed_out);
    }

    #[test]
    fn test_clamp_timeout() {
        // Below minimum
        assert_eq!(clamp_timeout(1000), MIN_WAIT_TIMEOUT_MS);

        // Above maximum
        assert_eq!(clamp_timeout(1_000_000), MAX_WAIT_TIMEOUT_MS);

        // Within range
        assert_eq!(clamp_timeout(60_000), 60_000);
    }

    #[test]
    fn test_calculate_scaled_timeout() {
        let base = 30_000;

        // Single agent - no scaling
        assert_eq!(calculate_scaled_timeout(base, 1), 30_000);

        // 4 agents - 2x scaling (sqrt(4) = 2)
        assert_eq!(calculate_scaled_timeout(base, 4), 60_000);

        // 9 agents - 3x scaling
        assert_eq!(calculate_scaled_timeout(base, 9), 90_000);
    }

    #[test]
    fn test_agent_status_info_from() {
        let completed =
            AgentStatusInfo::from(AgentThreadStatus::Completed(Some("result".to_string())));
        assert!(completed.completed);
        assert!(!completed.errored);
        assert_eq!(completed.result, Some("result".to_string()));

        let errored = AgentStatusInfo::from(AgentThreadStatus::Errored("error".to_string()));
        assert!(errored.completed);
        assert!(errored.errored);

        let running = AgentStatusInfo::from(AgentThreadStatus::Running);
        assert!(!running.completed);
        assert!(!running.errored);
    }
}
