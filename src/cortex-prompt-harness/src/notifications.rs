//! Notification system for agent and context updates.
//!
//! This module provides types and utilities for tracking and injecting
//! notifications about changes that occurred between messages.

use std::collections::VecDeque;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Type of notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationType {
    /// An agent was enabled.
    AgentEnabled,
    /// An agent was disabled.
    AgentDisabled,
    /// An agent's configuration changed.
    AgentConfigChanged,
    /// A task was added.
    TaskAdded,
    /// A task was completed.
    TaskCompleted,
    /// A task was removed.
    TaskRemoved,
    /// A task's status changed.
    TaskStatusChanged,
    /// The model was changed.
    ModelChanged,
    /// The environment changed (cwd, etc.).
    EnvironmentChanged,
    /// A tool was enabled.
    ToolEnabled,
    /// A tool was disabled.
    ToolDisabled,
    /// Custom notification.
    Custom,
    /// An MCP server was added/connected.
    McpServerAdded,
    /// An MCP server was removed/disconnected.
    McpServerRemoved,
    /// New tools were discovered from an MCP server.
    McpToolsDiscovered,
}

impl NotificationType {
    /// Get an icon/emoji for this notification type.
    pub fn icon(&self) -> &'static str {
        match self {
            Self::AgentEnabled => "🟢",
            Self::AgentDisabled => "🔴",
            Self::AgentConfigChanged => "⚙️",
            Self::TaskAdded => "➕",
            Self::TaskCompleted => "✅",
            Self::TaskRemoved => "➖",
            Self::TaskStatusChanged => "🔄",
            Self::ModelChanged => "🤖",
            Self::EnvironmentChanged => "📁",
            Self::ToolEnabled => "🔧",
            Self::ToolDisabled => "🚫",
            Self::Custom => "📢",
            Self::McpServerAdded => "🔌",
            Self::McpServerRemoved => "🔇",
            Self::McpToolsDiscovered => "🛠️",
        }
    }

    /// Get the category for grouping.
    pub fn category(&self) -> &'static str {
        match self {
            Self::AgentEnabled | Self::AgentDisabled | Self::AgentConfigChanged => "Agent",
            Self::TaskAdded | Self::TaskCompleted | Self::TaskRemoved | Self::TaskStatusChanged => {
                "Task"
            }
            Self::ModelChanged | Self::EnvironmentChanged => "Environment",
            Self::ToolEnabled | Self::ToolDisabled => "Tool",
            Self::Custom => "General",
            Self::McpServerAdded | Self::McpServerRemoved | Self::McpToolsDiscovered => "MCP",
        }
    }
}

/// Kind of notification for filtering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum NotificationKind {
    /// Informational notification.
    #[default]
    Info,
    /// Warning notification.
    Warning,
    /// Error notification.
    Error,
    /// Success notification.
    Success,
}

/// A notification about a change that should be communicated to the agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentNotification {
    /// Type of notification.
    pub notification_type: NotificationType,
    /// Subject of the notification (e.g., agent name, task ID).
    pub subject: String,
    /// Human-readable message.
    pub message: String,
    /// Timestamp when the notification was created.
    pub timestamp: DateTime<Utc>,
    /// Optional additional metadata.
    pub metadata: Option<serde_json::Value>,
}

impl AgentNotification {
    /// Create a new notification.
    pub fn new(
        notification_type: NotificationType,
        subject: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            notification_type,
            subject: subject.into(),
            message: message.into(),
            timestamp: Utc::now(),
            metadata: None,
        }
    }

    /// Create a notification with metadata.
    pub fn with_metadata(mut self, metadata: serde_json::Value) -> Self {
        self.metadata = Some(metadata);
        self
    }

    /// Create an agent enabled notification.
    pub fn agent_enabled(name: &str) -> Self {
        Self::new(
            NotificationType::AgentEnabled,
            name,
            format!("Agent '{}' has been enabled and is now active.", name),
        )
    }

    /// Create an agent disabled notification.
    pub fn agent_disabled(name: &str) -> Self {
        Self::new(
            NotificationType::AgentDisabled,
            name,
            format!(
                "Agent '{}' has been disabled and is no longer active.",
                name
            ),
        )
    }

    /// Create a task added notification.
    pub fn task_added(id: &str, description: &str) -> Self {
        Self::new(
            NotificationType::TaskAdded,
            id,
            format!("New task '{}' has been added: {}", id, description),
        )
    }

    /// Create a task completed notification.
    pub fn task_completed(id: &str) -> Self {
        Self::new(
            NotificationType::TaskCompleted,
            id,
            format!("Task '{}' has been completed.", id),
        )
    }

    /// Create a model changed notification.
    pub fn model_changed(old: &str, new: &str) -> Self {
        Self::new(
            NotificationType::ModelChanged,
            "model",
            format!("Model changed from '{}' to '{}'.", old, new),
        )
    }

    /// Create a custom notification.
    pub fn custom(subject: &str, message: &str) -> Self {
        Self::new(NotificationType::Custom, subject, message)
    }

    /// Create an MCP server added notification.
    pub fn mcp_server_added(server_name: &str, tool_count: usize) -> Self {
        Self::new(
            NotificationType::McpServerAdded,
            server_name,
            format!(
                "MCP server '{}' has been connected with {} tools available.",
                server_name, tool_count
            ),
        )
    }

    /// Create an MCP server removed notification.
    pub fn mcp_server_removed(server_name: &str) -> Self {
        Self::new(
            NotificationType::McpServerRemoved,
            server_name,
            format!("MCP server '{}' has been disconnected.", server_name),
        )
    }

    /// Create an MCP tools discovered notification with tool names.
    pub fn mcp_tools_discovered(server_name: &str, tool_names: &[String]) -> Self {
        let tools_preview = if tool_names.len() <= 5 {
            tool_names.join(", ")
        } else {
            format!(
                "{} and {} more",
                tool_names[..5].join(", "),
                tool_names.len() - 5
            )
        };
        Self::new(
            NotificationType::McpToolsDiscovered,
            server_name,
            format!(
                "MCP server '{}' provides the following tools: {}",
                server_name, tools_preview
            ),
        )
    }

    /// Get the icon for this notification.
    pub fn icon(&self) -> &'static str {
        self.notification_type.icon()
    }

    /// Format for display in prompt.
    pub fn format_for_prompt(&self) -> String {
        format!("{} {}", self.icon(), self.message)
    }
}

/// Manager for pending notifications.
///
/// Notifications are queued and then drained when building the next prompt.
/// This ensures that updates are communicated to the agent in the next
/// message, not at the time when the change occurred.
#[derive(Debug, Default)]
pub struct UpdateNotifier {
    /// Queue of pending notifications.
    queue: VecDeque<AgentNotification>,
    /// Maximum number of notifications to keep.
    max_notifications: usize,
    /// Whether to deduplicate notifications.
    deduplicate: bool,
}

impl UpdateNotifier {
    /// Create a new notifier with default settings.
    pub fn new() -> Self {
        Self {
            queue: VecDeque::new(),
            max_notifications: 50,
            deduplicate: true,
        }
    }

    /// Create a notifier with a custom max size.
    pub fn with_max_size(max: usize) -> Self {
        Self {
            max_notifications: max,
            ..Self::new()
        }
    }

    /// Set whether to deduplicate notifications.
    pub fn deduplicate(mut self, dedupe: bool) -> Self {
        self.deduplicate = dedupe;
        self
    }

    /// Add a notification to the queue.
    pub fn add(&mut self, notification: AgentNotification) {
        // Deduplicate if enabled
        if self.deduplicate {
            // Remove any existing notification with same type and subject
            self.queue.retain(|n| {
                !(n.notification_type == notification.notification_type
                    && n.subject == notification.subject)
            });
        }

        // Add to queue
        self.queue.push_back(notification);

        // Trim if over max
        while self.queue.len() > self.max_notifications {
            self.queue.pop_front();
        }
    }

    /// Check if there are pending notifications.
    pub fn has_pending(&self) -> bool {
        !self.queue.is_empty()
    }

    /// Get the number of pending notifications.
    pub fn count(&self) -> usize {
        self.queue.len()
    }

    /// Peek at pending notifications without removing them.
    pub fn peek(&self) -> Vec<&AgentNotification> {
        self.queue.iter().collect()
    }

    /// Drain all pending notifications.
    ///
    /// Returns the notifications and clears the queue.
    pub fn drain_notifications(&mut self) -> Vec<AgentNotification> {
        self.queue.drain(..).collect()
    }

    /// Clear all pending notifications.
    pub fn clear(&mut self) {
        self.queue.clear();
    }

    /// Get notifications filtered by type.
    pub fn filter_by_type(&self, notification_type: NotificationType) -> Vec<&AgentNotification> {
        self.queue
            .iter()
            .filter(|n| n.notification_type == notification_type)
            .collect()
    }

    /// Get notifications filtered by category.
    pub fn filter_by_category(&self, category: &str) -> Vec<&AgentNotification> {
        self.queue
            .iter()
            .filter(|n| n.notification_type.category() == category)
            .collect()
    }

    /// Format all pending notifications for display.
    pub fn format_all(&self) -> String {
        self.queue
            .iter()
            .map(|n| n.format_for_prompt())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_notification_creation() {
        let notification = AgentNotification::new(
            NotificationType::AgentEnabled,
            "test-agent",
            "Agent test-agent has been enabled.",
        );

        assert_eq!(
            notification.notification_type,
            NotificationType::AgentEnabled
        );
        assert_eq!(notification.subject, "test-agent");
    }

    #[test]
    fn test_notification_helpers() {
        let notification = AgentNotification::agent_enabled("research");
        assert!(notification.message.contains("research"));
        assert!(notification.message.contains("enabled"));
    }

    #[test]
    fn test_notifier_add() {
        let mut notifier = UpdateNotifier::new();

        notifier.add(AgentNotification::agent_enabled("test"));
        assert!(notifier.has_pending());
        assert_eq!(notifier.count(), 1);
    }

    #[test]
    fn test_notifier_drain() {
        let mut notifier = UpdateNotifier::new();

        notifier.add(AgentNotification::agent_enabled("agent1"));
        notifier.add(AgentNotification::task_added("task1", "Do something"));

        assert_eq!(notifier.count(), 2);

        let notifications = notifier.drain_notifications();
        assert_eq!(notifications.len(), 2);
        assert!(!notifier.has_pending());
    }

    #[test]
    fn test_notifier_deduplication() {
        let mut notifier = UpdateNotifier::new().deduplicate(true);

        notifier.add(AgentNotification::agent_enabled("test"));
        notifier.add(AgentNotification::agent_enabled("test")); // Duplicate

        assert_eq!(notifier.count(), 1);
    }

    #[test]
    fn test_notifier_no_deduplication() {
        let mut notifier = UpdateNotifier::new().deduplicate(false);

        notifier.add(AgentNotification::agent_enabled("test"));
        notifier.add(AgentNotification::agent_enabled("test"));

        assert_eq!(notifier.count(), 2);
    }

    #[test]
    fn test_notifier_max_size() {
        let mut notifier = UpdateNotifier::with_max_size(3);

        for i in 0..5 {
            notifier.add(AgentNotification::custom(&format!("sub{}", i), "msg"));
        }

        assert_eq!(notifier.count(), 3);
    }

    #[test]
    fn test_filter_by_type() {
        let mut notifier = UpdateNotifier::new();

        notifier.add(AgentNotification::agent_enabled("a1"));
        notifier.add(AgentNotification::task_added("t1", "desc"));
        notifier.add(AgentNotification::agent_enabled("a2"));

        let agent_notifications = notifier.filter_by_type(NotificationType::AgentEnabled);
        assert_eq!(agent_notifications.len(), 2);
    }

    #[test]
    fn test_notification_icon() {
        assert_eq!(NotificationType::AgentEnabled.icon(), "🟢");
        assert_eq!(NotificationType::AgentDisabled.icon(), "🔴");
        assert_eq!(NotificationType::TaskCompleted.icon(), "✅");
    }

    #[test]
    fn test_mcp_notifications() {
        let notification = AgentNotification::mcp_server_added("github", 5);
        assert_eq!(
            notification.notification_type,
            NotificationType::McpServerAdded
        );
        assert!(notification.message.contains("github"));
        assert!(notification.message.contains("5"));

        let notification = AgentNotification::mcp_server_removed("github");
        assert_eq!(
            notification.notification_type,
            NotificationType::McpServerRemoved
        );
        assert!(notification.message.contains("disconnected"));

        let tools = vec!["tool1".to_string(), "tool2".to_string()];
        let notification = AgentNotification::mcp_tools_discovered("github", &tools);
        assert_eq!(
            notification.notification_type,
            NotificationType::McpToolsDiscovered
        );
    }

    #[test]
    fn test_mcp_notification_icons() {
        assert_eq!(NotificationType::McpServerAdded.icon(), "🔌");
        assert_eq!(NotificationType::McpServerRemoved.icon(), "🔇");
        assert_eq!(NotificationType::McpToolsDiscovered.icon(), "🛠️");
    }

    #[test]
    fn test_mcp_notification_category() {
        assert_eq!(NotificationType::McpServerAdded.category(), "MCP");
        assert_eq!(NotificationType::McpServerRemoved.category(), "MCP");
        assert_eq!(NotificationType::McpToolsDiscovered.category(), "MCP");
    }
}
