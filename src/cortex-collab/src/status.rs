//! Agent status tracking.

use serde::{Deserialize, Serialize};

/// Status of an agent in the collaboration system.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AgentStatus {
    /// Agent is being initialized.
    #[default]
    PendingInit,

    /// Agent is running.
    Running,

    /// Agent completed successfully with optional message.
    Completed(Option<String>),

    /// Agent encountered an error.
    Errored(String),

    /// Agent was shutdown.
    Shutdown,

    /// Agent was not found.
    NotFound,
}

impl AgentStatus {
    /// Check if the status is final (no more changes expected).
    pub fn is_final(&self) -> bool {
        matches!(
            self,
            AgentStatus::Completed(_)
                | AgentStatus::Errored(_)
                | AgentStatus::Shutdown
                | AgentStatus::NotFound
        )
    }

    /// Check if the agent is still active.
    pub fn is_active(&self) -> bool {
        matches!(self, AgentStatus::PendingInit | AgentStatus::Running)
    }

    /// Get a human-readable description.
    pub fn description(&self) -> &str {
        match self {
            AgentStatus::PendingInit => "pending initialization",
            AgentStatus::Running => "running",
            AgentStatus::Completed(_) => "completed",
            AgentStatus::Errored(_) => "errored",
            AgentStatus::Shutdown => "shutdown",
            AgentStatus::NotFound => "not found",
        }
    }
}

impl std::fmt::Display for AgentStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AgentStatus::PendingInit => write!(f, "PendingInit"),
            AgentStatus::Running => write!(f, "Running"),
            AgentStatus::Completed(msg) => {
                if let Some(m) = msg {
                    write!(f, "Completed: {}", m)
                } else {
                    write!(f, "Completed")
                }
            }
            AgentStatus::Errored(err) => write!(f, "Errored: {}", err),
            AgentStatus::Shutdown => write!(f, "Shutdown"),
            AgentStatus::NotFound => write!(f, "NotFound"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_final() {
        assert!(!AgentStatus::PendingInit.is_final());
        assert!(!AgentStatus::Running.is_final());
        assert!(AgentStatus::Completed(None).is_final());
        assert!(AgentStatus::Completed(Some("done".to_string())).is_final());
        assert!(AgentStatus::Errored("error".to_string()).is_final());
        assert!(AgentStatus::Shutdown.is_final());
        assert!(AgentStatus::NotFound.is_final());
    }

    #[test]
    fn test_is_active() {
        assert!(AgentStatus::PendingInit.is_active());
        assert!(AgentStatus::Running.is_active());
        assert!(!AgentStatus::Completed(None).is_active());
        assert!(!AgentStatus::Errored("error".to_string()).is_active());
        assert!(!AgentStatus::Shutdown.is_active());
        assert!(!AgentStatus::NotFound.is_active());
    }
}
