//! Agent role definitions.

use serde::{Deserialize, Serialize};

/// Role of an agent, affecting its capabilities and behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AgentRole {
    /// Default agent with standard capabilities.
    #[default]
    Default,

    /// Planner agent - focuses on analysis and planning.
    /// Read-only access, no code modifications.
    Planner,

    /// Coder agent - focuses on code implementation.
    /// Full access to code editing tools.
    Coder,

    /// Research agent - focuses on investigation.
    /// Read-only, optimized for exploration.
    Research,

    /// Explorer agent - fast codebase exploration.
    /// Lightweight, read-only access.
    Explorer,

    /// General purpose sub-agent.
    /// Used for parallel task delegation.
    General,
}

impl AgentRole {
    /// Get a human-readable name for the role.
    pub fn name(&self) -> &str {
        match self {
            AgentRole::Default => "Default",
            AgentRole::Planner => "Planner",
            AgentRole::Coder => "Coder",
            AgentRole::Research => "Research",
            AgentRole::Explorer => "Explorer",
            AgentRole::General => "General",
        }
    }

    /// Get a description of the role.
    pub fn description(&self) -> &str {
        match self {
            AgentRole::Default => "Standard agent with all capabilities",
            AgentRole::Planner => "Analysis and planning agent (read-only)",
            AgentRole::Coder => "Code implementation agent (full access)",
            AgentRole::Research => "Investigation agent (read-only)",
            AgentRole::Explorer => "Fast codebase exploration (lightweight)",
            AgentRole::General => "General purpose sub-agent",
        }
    }

    /// Check if the role has write access.
    pub fn has_write_access(&self) -> bool {
        matches!(
            self,
            AgentRole::Default | AgentRole::Coder | AgentRole::General
        )
    }

    /// Check if the role is read-only.
    pub fn is_read_only(&self) -> bool {
        matches!(
            self,
            AgentRole::Planner | AgentRole::Research | AgentRole::Explorer
        )
    }

    /// Get the system prompt modifier for this role.
    pub fn system_prompt_modifier(&self) -> &str {
        match self {
            AgentRole::Default => "",
            AgentRole::Planner => {
                "\n\nYou are in PLANNER mode. Focus on analysis and planning. \
                 Do not make code changes - only analyze, explore, and plan."
            }
            AgentRole::Coder => {
                "\n\nYou are in CODER mode. Focus on implementing code changes. \
                 Be efficient and write clean, well-documented code."
            }
            AgentRole::Research => {
                "\n\nYou are in RESEARCH mode. Focus on investigation and analysis. \
                 Explore the codebase thoroughly to answer questions."
            }
            AgentRole::Explorer => {
                "\n\nYou are in EXPLORER mode. Quickly explore and understand the codebase. \
                 Be fast and efficient in your search."
            }
            AgentRole::General => {
                "\n\nYou are a sub-agent handling a delegated task. \
                 Complete your assigned task efficiently and report back."
            }
        }
    }
}

impl std::fmt::Display for AgentRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

impl std::str::FromStr for AgentRole {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "default" => Ok(AgentRole::Default),
            "planner" | "plan" => Ok(AgentRole::Planner),
            "coder" | "code" => Ok(AgentRole::Coder),
            "research" => Ok(AgentRole::Research),
            "explorer" | "explore" => Ok(AgentRole::Explorer),
            "general" => Ok(AgentRole::General),
            _ => Err(format!("Unknown agent role: {}", s)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_role_access() {
        assert!(AgentRole::Default.has_write_access());
        assert!(AgentRole::Coder.has_write_access());
        assert!(AgentRole::General.has_write_access());

        assert!(AgentRole::Planner.is_read_only());
        assert!(AgentRole::Research.is_read_only());
        assert!(AgentRole::Explorer.is_read_only());
    }

    #[test]
    fn test_role_from_str() {
        assert_eq!("planner".parse::<AgentRole>().unwrap(), AgentRole::Planner);
        assert_eq!("plan".parse::<AgentRole>().unwrap(), AgentRole::Planner);
        assert_eq!("coder".parse::<AgentRole>().unwrap(), AgentRole::Coder);
        assert_eq!("code".parse::<AgentRole>().unwrap(), AgentRole::Coder);
    }
}
