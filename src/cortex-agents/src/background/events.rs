//! Event types for background agent operations.
//!
//! Provides event types that are broadcast when background agents
//! change state, complete, or encounter errors.

use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

/// Status of a background agent.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentStatus {
    /// Agent is being initialized.
    #[default]
    Initializing,
    /// Agent is currently running.
    Running,
    /// Agent completed successfully.
    Completed,
    /// Agent failed with an error.
    Failed,
    /// Agent was cancelled by user.
    Cancelled,
    /// Agent timed out.
    TimedOut,
}

impl std::fmt::Display for AgentStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AgentStatus::Initializing => write!(f, "initializing"),
            AgentStatus::Running => write!(f, "running"),
            AgentStatus::Completed => write!(f, "completed"),
            AgentStatus::Failed => write!(f, "failed"),
            AgentStatus::Cancelled => write!(f, "cancelled"),
            AgentStatus::TimedOut => write!(f, "timed out"),
        }
    }
}

/// Result of a background agent execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentResult {
    /// Summary of what the agent accomplished.
    pub summary: String,
    /// Detailed output from the agent.
    pub output: String,
    /// Whether the agent completed successfully.
    pub success: bool,
    /// Number of tokens used.
    pub tokens_used: Option<u64>,
    /// Duration of execution.
    #[serde(with = "duration_serde")]
    pub duration: Duration,
    /// Files that were modified (if any).
    pub files_modified: Vec<String>,
    /// Any errors encountered.
    pub errors: Vec<String>,
}

impl AgentResult {
    /// Creates a new successful result.
    pub fn success(
        summary: impl Into<String>,
        output: impl Into<String>,
        duration: Duration,
    ) -> Self {
        Self {
            summary: summary.into(),
            output: output.into(),
            success: true,
            tokens_used: None,
            duration,
            files_modified: Vec::new(),
            errors: Vec::new(),
        }
    }

    /// Creates a new failed result.
    pub fn failure(error: impl Into<String>, duration: Duration) -> Self {
        let error_str = error.into();
        Self {
            summary: format!("Failed: {}", error_str),
            output: String::new(),
            success: false,
            tokens_used: None,
            duration,
            files_modified: Vec::new(),
            errors: vec![error_str],
        }
    }

    /// Creates a cancelled result.
    pub fn cancelled(duration: Duration) -> Self {
        Self {
            summary: "Cancelled by user".to_string(),
            output: String::new(),
            success: false,
            tokens_used: None,
            duration,
            files_modified: Vec::new(),
            errors: vec!["Cancelled by user".to_string()],
        }
    }

    /// Sets the tokens used.
    pub fn with_tokens(mut self, tokens: u64) -> Self {
        self.tokens_used = Some(tokens);
        self
    }

    /// Adds modified files.
    pub fn with_files(mut self, files: Vec<String>) -> Self {
        self.files_modified = files;
        self
    }
}

impl Default for AgentResult {
    fn default() -> Self {
        Self {
            summary: String::new(),
            output: String::new(),
            success: false,
            tokens_used: None,
            duration: Duration::ZERO,
            files_modified: Vec::new(),
            errors: Vec::new(),
        }
    }
}

/// Events emitted by background agents.
///
/// These events are broadcast to all subscribers when agent state changes.
#[derive(Debug, Clone)]
pub enum AgentEvent {
    /// Agent execution started.
    Started {
        /// Unique agent ID.
        id: String,
        /// Task description.
        task: String,
        /// When the agent started.
        started_at: Instant,
    },

    /// Agent made progress (e.g., tool call, step completion).
    Progress {
        /// Agent ID.
        id: String,
        /// Progress message.
        message: String,
        /// Optional percentage (0-100).
        percentage: Option<u8>,
    },

    /// Agent is executing a tool.
    ToolCall {
        /// Agent ID.
        id: String,
        /// Tool name.
        tool_name: String,
        /// Tool arguments (may be truncated).
        arguments: String,
    },

    /// Agent completed successfully.
    Completed {
        /// Agent ID.
        id: String,
        /// Execution result.
        result: AgentResult,
    },

    /// Agent failed with an error.
    Failed {
        /// Agent ID.
        id: String,
        /// Error message.
        error: String,
        /// Duration before failure.
        duration: Duration,
    },

    /// Agent was cancelled.
    Cancelled {
        /// Agent ID.
        id: String,
        /// Duration before cancellation.
        duration: Duration,
    },

    /// Agent timed out.
    TimedOut {
        /// Agent ID.
        id: String,
        /// Timeout duration.
        timeout: Duration,
    },
}

impl AgentEvent {
    /// Returns the agent ID for this event.
    pub fn agent_id(&self) -> &str {
        match self {
            AgentEvent::Started { id, .. } => id,
            AgentEvent::Progress { id, .. } => id,
            AgentEvent::ToolCall { id, .. } => id,
            AgentEvent::Completed { id, .. } => id,
            AgentEvent::Failed { id, .. } => id,
            AgentEvent::Cancelled { id, .. } => id,
            AgentEvent::TimedOut { id, .. } => id,
        }
    }

    /// Returns true if this is a terminal event (completed, failed, cancelled, timed out).
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            AgentEvent::Completed { .. }
                | AgentEvent::Failed { .. }
                | AgentEvent::Cancelled { .. }
                | AgentEvent::TimedOut { .. }
        )
    }

    /// Returns the status implied by this event.
    pub fn status(&self) -> AgentStatus {
        match self {
            AgentEvent::Started { .. } => AgentStatus::Running,
            AgentEvent::Progress { .. } => AgentStatus::Running,
            AgentEvent::ToolCall { .. } => AgentStatus::Running,
            AgentEvent::Completed { .. } => AgentStatus::Completed,
            AgentEvent::Failed { .. } => AgentStatus::Failed,
            AgentEvent::Cancelled { .. } => AgentStatus::Cancelled,
            AgentEvent::TimedOut { .. } => AgentStatus::TimedOut,
        }
    }
}

/// Serde support for Duration.
mod duration_serde {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::time::Duration;

    pub fn serialize<S>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        duration.as_millis().serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Duration, D::Error>
    where
        D: Deserializer<'de>,
    {
        let millis = u64::deserialize(deserializer)?;
        Ok(Duration::from_millis(millis))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_status_display() {
        assert_eq!(AgentStatus::Running.to_string(), "running");
        assert_eq!(AgentStatus::Completed.to_string(), "completed");
        assert_eq!(AgentStatus::Failed.to_string(), "failed");
        assert_eq!(AgentStatus::Cancelled.to_string(), "cancelled");
    }

    #[test]
    fn test_agent_result_success() {
        let result = AgentResult::success("Done", "Output", Duration::from_secs(10));
        assert!(result.success);
        assert_eq!(result.summary, "Done");
        assert!(result.errors.is_empty());
    }

    #[test]
    fn test_agent_result_failure() {
        let result = AgentResult::failure("Something went wrong", Duration::from_secs(5));
        assert!(!result.success);
        assert!(result.summary.contains("Failed"));
        assert_eq!(result.errors.len(), 1);
    }

    #[test]
    fn test_agent_event_is_terminal() {
        let started = AgentEvent::Started {
            id: "1".to_string(),
            task: "test".to_string(),
            started_at: Instant::now(),
        };
        assert!(!started.is_terminal());

        let completed = AgentEvent::Completed {
            id: "1".to_string(),
            result: AgentResult::default(),
        };
        assert!(completed.is_terminal());

        let failed = AgentEvent::Failed {
            id: "1".to_string(),
            error: "error".to_string(),
            duration: Duration::ZERO,
        };
        assert!(failed.is_terminal());
    }

    #[test]
    fn test_agent_event_agent_id() {
        let event = AgentEvent::Progress {
            id: "test-agent".to_string(),
            message: "working".to_string(),
            percentage: Some(50),
        };
        assert_eq!(event.agent_id(), "test-agent");
    }
}
