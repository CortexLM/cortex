//! Cortex Prompt Harness - Centralized system prompt management.
//!
//! This crate provides a centralized harness for building and managing system prompts
//! in Cortex CLI. It handles:
//!
//! - **Dynamic prompt construction**: Build system prompts based on context (tasks, agents, etc.)
//! - **Agent state tracking**: Track agent state changes between messages
//! - **Update notifications**: Inject notifications about agent changes into the next prompt
//! - **Centralized prompts**: Single source of truth for all system prompts (see [`prompts`] module)
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────┐
//! │                     Prompt Harness                                   │
//! │  ┌──────────────┐  ┌───────────────┐  ┌────────────────────────┐   │
//! │  │PromptContext │  │ StateTracker  │  │   UpdateNotifier       │   │
//! │  │  (config)    │  │  (changes)    │  │   (injections)         │   │
//! │  └──────┬───────┘  └───────┬───────┘  └──────────┬─────────────┘   │
//! │         │                  │                     │                 │
//! │         ▼                  ▼                     ▼                 │
//! │  ┌──────────────────────────────────────────────────────────────┐   │
//! │  │                  SystemPromptBuilder                         │   │
//! │  │  • Base prompt template                                      │   │
//! │  │  • Agent-specific sections                                   │   │
//! │  │  • Task context sections                                     │   │
//! │  │  • Update notifications                                      │   │
//! │  └──────────────────────────────────────────────────────────────┘   │
//! └─────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Centralized Prompts
//!
//! All system prompts are centralized in the [`prompts`] module for better
//! visibility and maintainability:
//!
//! ```rust
//! use cortex_prompt_harness::prompts;
//!
//! // Core prompts
//! let main_prompt = prompts::core::CORTEX_MAIN_PROMPT;
//! let tui_prompt = prompts::core::TUI_SYSTEM_PROMPT_TEMPLATE;
//!
//! // Agent prompts
//! let explore = prompts::agents::EXPLORE_AGENT_PROMPT;
//! let research = prompts::agents::RESEARCH_AGENT_PROMPT;
//!
//! // Task prompts
//! let summarization = prompts::tasks::SUMMARIZATION_PROMPT;
//! let compaction = prompts::tasks::COMPACTION_PROMPT;
//! ```
//!
//! # Example
//!
//! ```rust
//! use cortex_prompt_harness::{PromptHarness, PromptContext, AgentConfig};
//!
//! // Create harness
//! let mut harness = PromptHarness::new();
//!
//! // Set context
//! let context = PromptContext::new()
//!     .with_cwd("/project")
//!     .with_model("claude-opus-4")
//!     .with_agent(AgentConfig::new("build").with_prompt("Build agent prompt"));
//!
//! // Build system prompt for first message
//! let prompt = harness.build_system_prompt(&context);
//!
//! // Later, agent state changes...
//! harness.notify_agent_enabled("research");
//!
//! // Next prompt will include the update notification
//! let prompt_with_update = harness.build_system_prompt(&context);
//! ```

pub mod builder;
pub mod context;
pub mod notifications;
pub mod prompts;
pub mod sections;
pub mod state;
pub mod tracker;

// Re-exports
pub use builder::SystemPromptBuilder;
pub use context::{AgentConfig, PromptContext, TaskConfig};
pub use notifications::{AgentNotification, NotificationKind, NotificationType, UpdateNotifier};
pub use sections::{PromptSection, SectionPriority};
pub use state::{AgentState, PromptState};
pub use tracker::{ChangeEvent, StateChange, StateTracker};

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use thiserror::Error;

/// Errors that can occur in the prompt harness.
#[derive(Error, Debug)]
pub enum HarnessError {
    #[error("Invalid context: {0}")]
    InvalidContext(String),
    #[error("Builder error: {0}")]
    BuilderError(String),
    #[error("State tracking error: {0}")]
    StateError(String),
    #[error("Notification error: {0}")]
    NotificationError(String),
}

/// Result type for harness operations.
pub type Result<T> = std::result::Result<T, HarnessError>;

/// The main prompt harness for managing system prompts.
///
/// This is the central entry point for the prompt harness system.
/// It coordinates the builder, state tracker, and notifier to produce
/// dynamic system prompts.
#[derive(Debug)]
pub struct PromptHarness {
    /// State tracker for monitoring agent changes.
    tracker: StateTracker,
    /// Update notifier for pending notifications.
    notifier: UpdateNotifier,
    /// Cached sections for reuse.
    cached_sections: Arc<RwLock<HashMap<String, PromptSection>>>,
    /// Base prompt template.
    base_template: Option<String>,
    /// Custom variables for template substitution.
    variables: HashMap<String, String>,
}

impl Default for PromptHarness {
    fn default() -> Self {
        Self::new()
    }
}

impl PromptHarness {
    /// Create a new prompt harness.
    pub fn new() -> Self {
        Self {
            tracker: StateTracker::new(),
            notifier: UpdateNotifier::new(),
            cached_sections: Arc::new(RwLock::new(HashMap::new())),
            base_template: None,
            variables: HashMap::new(),
        }
    }

    /// Create a harness with a base template.
    pub fn with_template(template: impl Into<String>) -> Self {
        Self {
            base_template: Some(template.into()),
            ..Self::default()
        }
    }

    /// Set the base template.
    pub fn set_template(&mut self, template: impl Into<String>) {
        self.base_template = Some(template.into());
    }

    /// Set a variable for template substitution.
    pub fn set_variable(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.variables.insert(key.into(), value.into());
    }

    /// Build the system prompt for the given context.
    ///
    /// This method:
    /// 1. Creates a new builder with the base template
    /// 2. Adds context-specific sections (agent, tasks, environment)
    /// 3. Checks for pending notifications and adds them
    /// 4. Returns the final rendered prompt
    ///
    /// The notifications are cleared after being included, so they
    /// only appear once in the next prompt after a change.
    pub fn build_system_prompt(&mut self, context: &PromptContext) -> String {
        let mut builder = if let Some(ref template) = self.base_template {
            SystemPromptBuilder::with_base(template)
        } else {
            SystemPromptBuilder::new()
        };

        // Add variables
        for (key, value) in &self.variables {
            builder = builder.variable(key, value);
        }

        // Add context variables
        builder = builder
            .variable("cwd", context.cwd.as_deref().unwrap_or("."))
            .variable("model", context.model.as_deref().unwrap_or("unknown"))
            .variable(
                "date",
                chrono::Local::now().format("%a %b %d %Y").to_string(),
            )
            .variable("platform", std::env::consts::OS);

        // Add agent section if present
        if let Some(ref agent) = context.agent {
            builder = builder.section(sections::build_agent_section(agent));
        }

        // Add task sections if present
        for task in &context.tasks {
            builder = builder.section(sections::build_task_section(task));
        }

        // Add environment section
        builder = builder.section(sections::build_environment_section(context));

        // Add pending notifications (and clear them)
        let notifications = self.notifier.drain_notifications();
        if !notifications.is_empty() {
            builder = builder.section(sections::build_notifications_section(&notifications));
        }

        // Update tracker with current state
        self.tracker.update_state(context);

        builder.build()
    }

    /// Build a system prompt with explicit notification injection.
    ///
    /// Use this when you want to inject a notification into a specific prompt
    /// without adding it to the queue.
    pub fn build_with_notification(
        &mut self,
        context: &PromptContext,
        notification: AgentNotification,
    ) -> String {
        self.notifier.add(notification);
        self.build_system_prompt(context)
    }

    /// Notify that an agent has been enabled.
    pub fn notify_agent_enabled(&mut self, agent_name: &str) {
        self.notifier.add(AgentNotification::new(
            NotificationType::AgentEnabled,
            agent_name,
            format!("Agent '{}' has been enabled and is now active.", agent_name),
        ));
    }

    /// Notify that an agent has been disabled.
    pub fn notify_agent_disabled(&mut self, agent_name: &str) {
        self.notifier.add(AgentNotification::new(
            NotificationType::AgentDisabled,
            agent_name,
            format!(
                "Agent '{}' has been disabled and is no longer active.",
                agent_name
            ),
        ));
    }

    /// Notify that an agent's configuration has changed.
    pub fn notify_agent_config_changed(&mut self, agent_name: &str, changes: &str) {
        self.notifier.add(AgentNotification::new(
            NotificationType::AgentConfigChanged,
            agent_name,
            format!(
                "Agent '{}' configuration has been modified: {}",
                agent_name, changes
            ),
        ));
    }

    /// Notify that a task has been added.
    pub fn notify_task_added(&mut self, task_name: &str) {
        self.notifier.add(AgentNotification::new(
            NotificationType::TaskAdded,
            task_name,
            format!("New task '{}' has been added to the context.", task_name),
        ));
    }

    /// Notify that a task has been completed.
    pub fn notify_task_completed(&mut self, task_name: &str) {
        self.notifier.add(AgentNotification::new(
            NotificationType::TaskCompleted,
            task_name,
            format!("Task '{}' has been completed.", task_name),
        ));
    }

    /// Notify that a task has been removed.
    pub fn notify_task_removed(&mut self, task_name: &str) {
        self.notifier.add(AgentNotification::new(
            NotificationType::TaskRemoved,
            task_name,
            format!("Task '{}' has been removed from the context.", task_name),
        ));
    }

    /// Notify about a model change.
    pub fn notify_model_changed(&mut self, old_model: &str, new_model: &str) {
        self.notifier.add(AgentNotification {
            notification_type: NotificationType::ModelChanged,
            subject: "model".to_string(),
            message: format!(
                "Model has been changed from '{}' to '{}'.",
                old_model, new_model
            ),
            timestamp: chrono::Utc::now(),
            metadata: Some({
                let mut map = serde_json::Map::new();
                map.insert(
                    "old_model".to_string(),
                    serde_json::Value::String(old_model.to_string()),
                );
                map.insert(
                    "new_model".to_string(),
                    serde_json::Value::String(new_model.to_string()),
                );
                serde_json::Value::Object(map)
            }),
        });
    }

    /// Notify about a working directory change.
    pub fn notify_cwd_changed(&mut self, old_cwd: &str, new_cwd: &str) {
        self.notifier.add(AgentNotification {
            notification_type: NotificationType::EnvironmentChanged,
            subject: "cwd".to_string(),
            message: format!(
                "Working directory has been changed from '{}' to '{}'.",
                old_cwd, new_cwd
            ),
            timestamp: chrono::Utc::now(),
            metadata: Some({
                let mut map = serde_json::Map::new();
                map.insert(
                    "old_cwd".to_string(),
                    serde_json::Value::String(old_cwd.to_string()),
                );
                map.insert(
                    "new_cwd".to_string(),
                    serde_json::Value::String(new_cwd.to_string()),
                );
                serde_json::Value::Object(map)
            }),
        });
    }

    /// Add a custom notification.
    pub fn notify_custom(&mut self, notification: AgentNotification) {
        self.notifier.add(notification);
    }

    /// Notify that an MCP server has been added/connected.
    pub fn notify_mcp_server_added(&mut self, server_name: &str, tool_count: usize) {
        self.notifier
            .add(AgentNotification::mcp_server_added(server_name, tool_count));
    }

    /// Notify that an MCP server has been removed/disconnected.
    pub fn notify_mcp_server_removed(&mut self, server_name: &str) {
        self.notifier
            .add(AgentNotification::mcp_server_removed(server_name));
    }

    /// Notify about tools discovered from an MCP server.
    pub fn notify_mcp_tools_discovered(&mut self, server_name: &str, tool_names: &[String]) {
        self.notifier.add(AgentNotification::mcp_tools_discovered(
            server_name,
            tool_names,
        ));
    }

    /// Check if there are pending notifications.
    pub fn has_pending_notifications(&self) -> bool {
        self.notifier.has_pending()
    }

    /// Get the number of pending notifications.
    pub fn pending_notification_count(&self) -> usize {
        self.notifier.count()
    }

    /// Get a reference to the state tracker.
    pub fn tracker(&self) -> &StateTracker {
        &self.tracker
    }

    /// Get a mutable reference to the state tracker.
    pub fn tracker_mut(&mut self) -> &mut StateTracker {
        &mut self.tracker
    }

    /// Compare current context with previous state and auto-generate notifications.
    ///
    /// This method detects changes between the current context and the
    /// previously tracked state, automatically generating appropriate
    /// notifications.
    pub fn detect_and_notify_changes(&mut self, context: &PromptContext) {
        let changes = self.tracker.detect_changes(context);

        for change in changes {
            match change {
                StateChange::AgentEnabled(name) => self.notify_agent_enabled(&name),
                StateChange::AgentDisabled(name) => self.notify_agent_disabled(&name),
                StateChange::AgentConfigChanged(name, details) => {
                    self.notify_agent_config_changed(&name, &details);
                }
                StateChange::TaskAdded(name) => self.notify_task_added(&name),
                StateChange::TaskRemoved(name) => self.notify_task_removed(&name),
                StateChange::TaskCompleted(name) => self.notify_task_completed(&name),
                StateChange::ModelChanged(old, new) => self.notify_model_changed(&old, &new),
                StateChange::CwdChanged(old, new) => self.notify_cwd_changed(&old, &new),
                StateChange::Custom(msg) => {
                    self.notifier.add(AgentNotification::new(
                        NotificationType::Custom,
                        "system",
                        msg,
                    ));
                }
            }
        }
    }

    /// Register a section for caching.
    pub fn cache_section(&mut self, key: impl Into<String>, section: PromptSection) {
        if let Ok(mut cache) = self.cached_sections.write() {
            cache.insert(key.into(), section);
        }
    }

    /// Get a cached section.
    pub fn get_cached_section(&self, key: &str) -> Option<PromptSection> {
        self.cached_sections
            .read()
            .ok()
            .and_then(|cache| cache.get(key).cloned())
    }

    /// Clear the section cache.
    pub fn clear_cache(&mut self) {
        if let Ok(mut cache) = self.cached_sections.write() {
            cache.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_harness_creation() {
        let harness = PromptHarness::new();
        assert!(!harness.has_pending_notifications());
    }

    #[test]
    fn test_harness_with_template() {
        let harness = PromptHarness::with_template("You are a helpful assistant.");
        assert!(harness.base_template.is_some());
    }

    #[test]
    fn test_notification_queue() {
        let mut harness = PromptHarness::new();

        harness.notify_agent_enabled("research");
        assert!(harness.has_pending_notifications());
        assert_eq!(harness.pending_notification_count(), 1);

        harness.notify_task_added("analyze-code");
        assert_eq!(harness.pending_notification_count(), 2);
    }

    #[test]
    fn test_build_clears_notifications() {
        let mut harness = PromptHarness::with_template("Base prompt");
        let context = PromptContext::new();

        harness.notify_agent_enabled("test");
        assert!(harness.has_pending_notifications());

        let _ = harness.build_system_prompt(&context);
        assert!(!harness.has_pending_notifications());
    }

    #[test]
    fn test_variable_substitution() {
        let mut harness = PromptHarness::with_template("Hello {{name}}!");
        harness.set_variable("name", "World");

        let context = PromptContext::new();
        let prompt = harness.build_system_prompt(&context);

        assert!(prompt.contains("Hello World!"));
    }
}
