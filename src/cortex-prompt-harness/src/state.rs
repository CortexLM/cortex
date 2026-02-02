//! State types for tracking prompt context over time.
//!
//! This module provides types for capturing and comparing the state
//! of the prompt context between messages.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::context::{AgentConfig, PromptContext, TaskConfig, TaskStatus};

/// Snapshot of an agent's state.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct AgentState {
    /// Agent name.
    pub name: String,
    /// Whether the agent is enabled.
    pub enabled: bool,
    /// Agent mode.
    pub mode: Option<String>,
    /// Allowed tools.
    pub allowed_tools: Option<Vec<String>>,
    /// Denied tools.
    pub denied_tools: Vec<String>,
    /// Model override.
    pub model: Option<String>,
    /// Temperature override.
    pub temperature: Option<f32>,
    /// Hash of the prompt (for detecting changes).
    pub prompt_hash: Option<u64>,
}

impl AgentState {
    /// Create from an AgentConfig.
    pub fn from_config(config: &AgentConfig) -> Self {
        Self {
            name: config.name.clone(),
            enabled: config.enabled,
            mode: config.mode.clone(),
            allowed_tools: config.allowed_tools.clone(),
            denied_tools: config.denied_tools.clone(),
            model: config.model.clone(),
            temperature: config.temperature,
            prompt_hash: config.prompt.as_ref().map(|p| hash_string(p)),
        }
    }

    /// Check if this state differs from another.
    pub fn differs_from(&self, other: &AgentState) -> bool {
        self != other
    }

    /// Get a list of differences from another state.
    pub fn diff(&self, other: &AgentState) -> Vec<String> {
        let mut diffs = Vec::new();

        if self.enabled != other.enabled {
            diffs.push(format!("enabled: {} -> {}", other.enabled, self.enabled));
        }

        if self.mode != other.mode {
            diffs.push(format!("mode: {:?} -> {:?}", other.mode, self.mode));
        }

        if self.model != other.model {
            diffs.push(format!("model: {:?} -> {:?}", other.model, self.model));
        }

        if self.temperature != other.temperature {
            diffs.push(format!(
                "temperature: {:?} -> {:?}",
                other.temperature, self.temperature
            ));
        }

        if self.allowed_tools != other.allowed_tools {
            diffs.push("allowed_tools changed".to_string());
        }

        if self.denied_tools != other.denied_tools {
            diffs.push("denied_tools changed".to_string());
        }

        if self.prompt_hash != other.prompt_hash {
            diffs.push("prompt changed".to_string());
        }

        diffs
    }
}

/// Snapshot of a task's state.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskState {
    /// Task ID.
    pub id: String,
    /// Task status.
    pub status: TaskStatus,
    /// Task priority.
    pub priority: i32,
}

impl TaskState {
    /// Create from a TaskConfig.
    pub fn from_config(config: &TaskConfig) -> Self {
        Self {
            id: config.id.clone(),
            status: config.status,
            priority: config.priority,
        }
    }
}

/// Complete snapshot of the prompt state.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PromptState {
    /// Current working directory.
    pub cwd: Option<String>,
    /// Current model.
    pub model: Option<String>,
    /// Current provider.
    pub provider: Option<String>,
    /// Active agent state.
    pub agent: Option<AgentState>,
    /// Subagent states (by name).
    pub subagents: HashMap<String, AgentState>,
    /// Task states (by ID).
    pub tasks: HashMap<String, TaskState>,
    /// Timestamp of this snapshot.
    pub timestamp: Option<chrono::DateTime<chrono::Utc>>,
}

impl PromptState {
    /// Create a new empty state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create from a PromptContext.
    pub fn from_context(context: &PromptContext) -> Self {
        Self {
            cwd: context.cwd.clone(),
            model: context.model.clone(),
            provider: context.provider.clone(),
            agent: context.agent.as_ref().map(AgentState::from_config),
            subagents: context
                .subagents
                .iter()
                .map(|a| (a.name.clone(), AgentState::from_config(a)))
                .collect(),
            tasks: context
                .tasks
                .iter()
                .map(|t| (t.id.clone(), TaskState::from_config(t)))
                .collect(),
            timestamp: Some(chrono::Utc::now()),
        }
    }

    /// Get all agent names (main + subagents).
    pub fn agent_names(&self) -> HashSet<String> {
        let mut names: HashSet<String> = self.subagents.keys().cloned().collect();
        if let Some(ref agent) = self.agent {
            names.insert(agent.name.clone());
        }
        names
    }

    /// Get all task IDs.
    pub fn task_ids(&self) -> HashSet<String> {
        self.tasks.keys().cloned().collect()
    }

    /// Check if an agent is enabled.
    pub fn is_agent_enabled(&self, name: &str) -> bool {
        if let Some(ref agent) = self.agent {
            if agent.name == name {
                return agent.enabled;
            }
        }
        self.subagents.get(name).map(|a| a.enabled).unwrap_or(false)
    }

    /// Get task status.
    pub fn task_status(&self, id: &str) -> Option<TaskStatus> {
        self.tasks.get(id).map(|t| t.status)
    }
}

/// Simple hash function for strings.
fn hash_string(s: &str) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    s.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_state_from_config() {
        let config = AgentConfig::new("test")
            .with_prompt("Test prompt")
            .with_mode("build")
            .enabled(true);

        let state = AgentState::from_config(&config);

        assert_eq!(state.name, "test");
        assert!(state.enabled);
        assert_eq!(state.mode, Some("build".to_string()));
        assert!(state.prompt_hash.is_some());
    }

    #[test]
    fn test_agent_state_diff() {
        let state1 = AgentState {
            name: "test".to_string(),
            enabled: true,
            mode: Some("build".to_string()),
            ..Default::default()
        };

        let state2 = AgentState {
            name: "test".to_string(),
            enabled: false,
            mode: Some("plan".to_string()),
            ..Default::default()
        };

        let diffs = state1.diff(&state2);
        assert!(!diffs.is_empty());
        assert!(diffs.iter().any(|d| d.contains("enabled")));
        assert!(diffs.iter().any(|d| d.contains("mode")));
    }

    #[test]
    fn test_task_state_from_config() {
        let config = TaskConfig::new("task-1", "Do something")
            .with_priority(10)
            .in_progress();

        let state = TaskState::from_config(&config);

        assert_eq!(state.id, "task-1");
        assert_eq!(state.status, TaskStatus::InProgress);
        assert_eq!(state.priority, 10);
    }

    #[test]
    fn test_prompt_state_from_context() {
        let context = PromptContext::new()
            .with_cwd("/project")
            .with_model("claude")
            .with_agent(AgentConfig::new("build"))
            .add_subagent(AgentConfig::new("research"))
            .add_task(TaskConfig::new("t1", "Task 1"));

        let state = PromptState::from_context(&context);

        assert_eq!(state.cwd, Some("/project".to_string()));
        assert!(state.agent.is_some());
        assert_eq!(state.subagents.len(), 1);
        assert_eq!(state.tasks.len(), 1);
    }

    #[test]
    fn test_prompt_state_agent_names() {
        let context = PromptContext::new()
            .with_agent(AgentConfig::new("main"))
            .add_subagent(AgentConfig::new("sub1"))
            .add_subagent(AgentConfig::new("sub2"));

        let state = PromptState::from_context(&context);
        let names = state.agent_names();

        assert!(names.contains("main"));
        assert!(names.contains("sub1"));
        assert!(names.contains("sub2"));
    }
}
