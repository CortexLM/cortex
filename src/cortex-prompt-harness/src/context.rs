//! Prompt context types for configuring system prompt generation.
//!
//! This module provides the data structures that describe the current context
//! in which a system prompt should be generated, including:
//!
//! - Current working directory and environment
//! - Active agent configuration
//! - Current tasks and their states
//! - Model and provider information

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Configuration for an agent within the prompt context.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentConfig {
    /// Agent name/identifier.
    pub name: String,
    /// Custom system prompt for this agent.
    pub prompt: Option<String>,
    /// Agent description.
    pub description: Option<String>,
    /// Whether the agent is currently enabled.
    pub enabled: bool,
    /// Agent mode (e.g., "build", "plan", "spec").
    pub mode: Option<String>,
    /// Allowed tools for this agent.
    pub allowed_tools: Option<Vec<String>>,
    /// Denied tools for this agent.
    pub denied_tools: Vec<String>,
    /// Custom temperature for this agent.
    pub temperature: Option<f32>,
    /// Custom model for this agent.
    pub model: Option<String>,
    /// Additional metadata.
    pub metadata: HashMap<String, serde_json::Value>,
}

impl AgentConfig {
    /// Create a new agent config with the given name.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            enabled: true,
            ..Default::default()
        }
    }

    /// Set the custom prompt.
    pub fn with_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.prompt = Some(prompt.into());
        self
    }

    /// Set the description.
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Set the agent mode.
    pub fn with_mode(mut self, mode: impl Into<String>) -> Self {
        self.mode = Some(mode.into());
        self
    }

    /// Set allowed tools.
    pub fn with_allowed_tools(mut self, tools: Vec<String>) -> Self {
        self.allowed_tools = Some(tools);
        self
    }

    /// Add a denied tool.
    pub fn deny_tool(mut self, tool: impl Into<String>) -> Self {
        self.denied_tools.push(tool.into());
        self
    }

    /// Set enabled state.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Set temperature.
    pub fn with_temperature(mut self, temperature: f32) -> Self {
        self.temperature = Some(temperature);
        self
    }

    /// Set model.
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Add metadata.
    pub fn with_metadata(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.metadata.insert(key.into(), value);
        self
    }

    /// Check if a tool is allowed for this agent.
    pub fn is_tool_allowed(&self, tool_name: &str) -> bool {
        // If denied, always false
        if self.denied_tools.iter().any(|t| t == tool_name) {
            return false;
        }

        // If allowed_tools is set, check if it's in the list
        if let Some(ref allowed) = self.allowed_tools {
            return allowed.iter().any(|t| t == tool_name);
        }

        // Default: allowed
        true
    }
}

/// Status of a task in the context.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum TaskStatus {
    /// Task is pending (not started).
    #[default]
    Pending,
    /// Task is currently in progress.
    InProgress,
    /// Task has been completed.
    Completed,
    /// Task was cancelled.
    Cancelled,
    /// Task failed.
    Failed,
}

/// Configuration for a task within the prompt context.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TaskConfig {
    /// Task identifier.
    pub id: String,
    /// Task description/name.
    pub description: String,
    /// Task status.
    pub status: TaskStatus,
    /// Task priority (higher = more important).
    pub priority: i32,
    /// Parent task ID (for subtasks).
    pub parent_id: Option<String>,
    /// Task dependencies.
    pub dependencies: Vec<String>,
    /// Additional metadata.
    pub metadata: HashMap<String, serde_json::Value>,
}

impl TaskConfig {
    /// Create a new task with the given ID and description.
    pub fn new(id: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            description: description.into(),
            ..Default::default()
        }
    }

    /// Set the task status.
    pub fn with_status(mut self, status: TaskStatus) -> Self {
        self.status = status;
        self
    }

    /// Set in progress.
    pub fn in_progress(self) -> Self {
        self.with_status(TaskStatus::InProgress)
    }

    /// Set completed.
    pub fn completed(self) -> Self {
        self.with_status(TaskStatus::Completed)
    }

    /// Set priority.
    pub fn with_priority(mut self, priority: i32) -> Self {
        self.priority = priority;
        self
    }

    /// Set parent ID.
    pub fn with_parent(mut self, parent_id: impl Into<String>) -> Self {
        self.parent_id = Some(parent_id.into());
        self
    }

    /// Add a dependency.
    pub fn depends_on(mut self, task_id: impl Into<String>) -> Self {
        self.dependencies.push(task_id.into());
        self
    }

    /// Add metadata.
    pub fn with_metadata(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.metadata.insert(key.into(), value);
        self
    }

    /// Check if this task is active (pending or in progress).
    pub fn is_active(&self) -> bool {
        matches!(self.status, TaskStatus::Pending | TaskStatus::InProgress)
    }
}

/// The main prompt context containing all information needed to build a system prompt.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PromptContext {
    /// Current working directory.
    pub cwd: Option<String>,
    /// Current date (auto-populated if not set).
    pub date: Option<String>,
    /// Operating system/platform.
    pub platform: Option<String>,
    /// Whether the current directory is a git repository.
    pub is_git_repo: Option<bool>,
    /// Current model name.
    pub model: Option<String>,
    /// Model provider ID.
    pub provider: Option<String>,
    /// Active agent configuration.
    pub agent: Option<AgentConfig>,
    /// Available subagents.
    pub subagents: Vec<AgentConfig>,
    /// Current tasks.
    pub tasks: Vec<TaskConfig>,
    /// Custom instructions from user.
    pub custom_instructions: Option<String>,
    /// User name.
    pub user_name: Option<String>,
    /// Session ID.
    pub session_id: Option<String>,
    /// Turn number in the conversation.
    pub turn_number: Option<u64>,
    /// Context window size (in tokens).
    pub context_window: Option<u32>,
    /// Current token usage.
    pub token_usage: Option<u64>,
    /// Additional environment variables.
    pub environment: HashMap<String, String>,
    /// Additional metadata.
    pub metadata: HashMap<String, serde_json::Value>,
}

impl PromptContext {
    /// Create a new empty context.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a context with common defaults populated.
    pub fn with_defaults() -> Self {
        Self {
            cwd: std::env::current_dir()
                .ok()
                .map(|p| p.display().to_string()),
            date: Some(chrono::Local::now().format("%a %b %d %Y").to_string()),
            platform: Some(std::env::consts::OS.to_string()),
            is_git_repo: Some(std::path::Path::new(".git").exists()),
            ..Default::default()
        }
    }

    /// Set the working directory.
    pub fn with_cwd(mut self, cwd: impl Into<String>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    /// Set the model.
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Set the provider.
    pub fn with_provider(mut self, provider: impl Into<String>) -> Self {
        self.provider = Some(provider.into());
        self
    }

    /// Set the active agent.
    pub fn with_agent(mut self, agent: AgentConfig) -> Self {
        self.agent = Some(agent);
        self
    }

    /// Add a subagent.
    pub fn add_subagent(mut self, subagent: AgentConfig) -> Self {
        self.subagents.push(subagent);
        self
    }

    /// Add a task.
    pub fn add_task(mut self, task: TaskConfig) -> Self {
        self.tasks.push(task);
        self
    }

    /// Set custom instructions.
    pub fn with_instructions(mut self, instructions: impl Into<String>) -> Self {
        self.custom_instructions = Some(instructions.into());
        self
    }

    /// Set the session ID.
    pub fn with_session_id(mut self, id: impl Into<String>) -> Self {
        self.session_id = Some(id.into());
        self
    }

    /// Set the turn number.
    pub fn with_turn_number(mut self, turn: u64) -> Self {
        self.turn_number = Some(turn);
        self
    }

    /// Set context window size.
    pub fn with_context_window(mut self, size: u32) -> Self {
        self.context_window = Some(size);
        self
    }

    /// Set current token usage.
    pub fn with_token_usage(mut self, tokens: u64) -> Self {
        self.token_usage = Some(tokens);
        self
    }

    /// Add an environment variable.
    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.environment.insert(key.into(), value.into());
        self
    }

    /// Add metadata.
    pub fn with_metadata(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.metadata.insert(key.into(), value);
        self
    }

    /// Get the number of active tasks.
    pub fn active_task_count(&self) -> usize {
        self.tasks.iter().filter(|t| t.is_active()).count()
    }

    /// Get the number of enabled subagents.
    pub fn enabled_subagent_count(&self) -> usize {
        self.subagents.iter().filter(|a| a.enabled).count()
    }

    /// Check if there's an active agent.
    pub fn has_agent(&self) -> bool {
        self.agent.as_ref().map(|a| a.enabled).unwrap_or(false)
    }

    /// Check if there are tasks.
    pub fn has_tasks(&self) -> bool {
        !self.tasks.is_empty()
    }

    /// Get task IDs.
    pub fn task_ids(&self) -> Vec<&str> {
        self.tasks.iter().map(|t| t.id.as_str()).collect()
    }

    /// Get subagent names.
    pub fn subagent_names(&self) -> Vec<&str> {
        self.subagents.iter().map(|a| a.name.as_str()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_config() {
        let agent = AgentConfig::new("build")
            .with_prompt("Build agent prompt")
            .with_mode("build")
            .deny_tool("Execute")
            .with_temperature(0.7);

        assert_eq!(agent.name, "build");
        assert!(agent.prompt.is_some());
        assert!(agent.enabled);
        assert!(!agent.is_tool_allowed("Execute"));
        assert!(agent.is_tool_allowed("Read"));
    }

    #[test]
    fn test_task_config() {
        let task = TaskConfig::new("task-1", "Implement feature")
            .with_priority(10)
            .in_progress()
            .depends_on("task-0");

        assert_eq!(task.id, "task-1");
        assert_eq!(task.status, TaskStatus::InProgress);
        assert!(task.is_active());
        assert_eq!(task.dependencies.len(), 1);
    }

    #[test]
    fn test_prompt_context() {
        let context = PromptContext::new()
            .with_cwd("/project")
            .with_model("claude-opus-4")
            .with_agent(AgentConfig::new("build"))
            .add_task(TaskConfig::new("t1", "Task 1").in_progress())
            .add_task(TaskConfig::new("t2", "Task 2"));

        assert!(context.has_agent());
        assert!(context.has_tasks());
        assert_eq!(context.active_task_count(), 2);
        assert_eq!(context.task_ids().len(), 2);
    }
}
