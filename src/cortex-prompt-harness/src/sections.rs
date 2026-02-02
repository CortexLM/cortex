//! Prompt sections for structured system prompts.
//!
//! This module provides section types and builders for creating
//! structured parts of system prompts.

use serde::{Deserialize, Serialize};

use crate::context::{AgentConfig, PromptContext, TaskConfig, TaskStatus};
use crate::notifications::AgentNotification;

/// Priority level for prompt sections.
///
/// Higher priority sections appear earlier in the prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
pub enum SectionPriority {
    /// Critical - appears first (e.g., safety rules).
    Critical = 100,
    /// High priority - appears early.
    High = 75,
    /// Normal priority - default.
    #[default]
    Normal = 50,
    /// Low priority - appears late.
    Low = 25,
    /// Minimal - appears last.
    Minimal = 0,
}

/// A section of a system prompt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptSection {
    /// Section name (used as header).
    pub name: String,
    /// Section content.
    pub content: String,
    /// Priority (higher = earlier in prompt).
    pub priority: SectionPriority,
    /// Whether this section is enabled.
    pub enabled: bool,
    /// Whether to render as a markdown header.
    pub render_header: bool,
    /// Header level (1-6, default 2).
    pub header_level: u8,
}

impl PromptSection {
    /// Create a new section with name and content.
    pub fn new(name: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            content: content.into(),
            priority: SectionPriority::Normal,
            enabled: true,
            render_header: true,
            header_level: 2,
        }
    }

    /// Create a section without a header.
    pub fn content_only(content: impl Into<String>) -> Self {
        Self {
            name: String::new(),
            content: content.into(),
            priority: SectionPriority::Normal,
            enabled: true,
            render_header: false,
            header_level: 2,
        }
    }

    /// Set the priority.
    pub fn with_priority(mut self, priority: SectionPriority) -> Self {
        self.priority = priority;
        self
    }

    /// Set enabled state.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Set header level.
    pub fn with_header_level(mut self, level: u8) -> Self {
        self.header_level = level.clamp(1, 6);
        self
    }

    /// Disable header rendering.
    pub fn no_header(mut self) -> Self {
        self.render_header = false;
        self
    }

    /// Render the section to a string.
    pub fn render(&self) -> String {
        if !self.enabled {
            return String::new();
        }

        if self.render_header && !self.name.is_empty() {
            let hashes = "#".repeat(self.header_level as usize);
            format!("{} {}\n\n{}", hashes, self.name, self.content)
        } else {
            self.content.clone()
        }
    }
}

// ============================================================================
// Section Builders
// ============================================================================

/// Build the agent section for a system prompt.
pub fn build_agent_section(agent: &AgentConfig) -> PromptSection {
    let mut content = String::new();

    // Agent name and mode
    content.push_str(&format!("**Agent**: {}\n", agent.name));

    if let Some(ref mode) = agent.mode {
        content.push_str(&format!("**Mode**: {}\n", mode));
    }

    if let Some(ref description) = agent.description {
        content.push_str(&format!("**Description**: {}\n", description));
    }

    content.push('\n');

    // Agent prompt
    if let Some(ref prompt) = agent.prompt {
        content.push_str(prompt);
        content.push('\n');
    }

    // Tool restrictions
    if !agent.denied_tools.is_empty() {
        content.push_str("\n**Denied Tools**: ");
        content.push_str(&agent.denied_tools.join(", "));
        content.push('\n');
    }

    if let Some(ref allowed) = agent.allowed_tools {
        content.push_str("\n**Allowed Tools**: ");
        content.push_str(&allowed.join(", "));
        content.push('\n');
    }

    PromptSection::new("Agent Configuration", content).with_priority(SectionPriority::High)
}

/// Build a task section for a system prompt.
pub fn build_task_section(task: &TaskConfig) -> PromptSection {
    let status_emoji = match task.status {
        TaskStatus::Pending => "⏳",
        TaskStatus::InProgress => "🔄",
        TaskStatus::Completed => "✅",
        TaskStatus::Cancelled => "❌",
        TaskStatus::Failed => "⚠️",
    };

    let content = format!(
        "{} **{}** ({})\n{}",
        status_emoji,
        task.id,
        format!("{:?}", task.status).to_lowercase(),
        task.description
    );

    PromptSection::new(format!("Task: {}", task.id), content).with_priority(
        if task.status == TaskStatus::InProgress {
            SectionPriority::High
        } else {
            SectionPriority::Normal
        },
    )
}

/// Build a tasks summary section.
pub fn build_tasks_summary_section(tasks: &[TaskConfig]) -> PromptSection {
    if tasks.is_empty() {
        return PromptSection::new("Current Tasks", "No active tasks.")
            .with_priority(SectionPriority::Normal);
    }

    let mut content = String::new();

    // Count by status
    let pending = tasks
        .iter()
        .filter(|t| t.status == TaskStatus::Pending)
        .count();
    let in_progress = tasks
        .iter()
        .filter(|t| t.status == TaskStatus::InProgress)
        .count();
    let completed = tasks
        .iter()
        .filter(|t| t.status == TaskStatus::Completed)
        .count();

    content.push_str(&format!(
        "**Summary**: {} pending, {} in progress, {} completed\n\n",
        pending, in_progress, completed
    ));

    // List active tasks
    for task in tasks.iter().filter(|t| t.is_active()) {
        let status_emoji = if task.status == TaskStatus::InProgress {
            "🔄"
        } else {
            "⏳"
        };
        content.push_str(&format!(
            "- {} **{}**: {}\n",
            status_emoji, task.id, task.description
        ));
    }

    PromptSection::new("Current Tasks", content).with_priority(SectionPriority::High)
}

/// Build the environment section.
pub fn build_environment_section(context: &PromptContext) -> PromptSection {
    let mut content = String::new();

    if let Some(ref cwd) = context.cwd {
        content.push_str(&format!("- **Working Directory**: {}\n", cwd));
    }

    if let Some(ref platform) = context.platform {
        content.push_str(&format!("- **Platform**: {}\n", platform));
    }

    if let Some(ref date) = context.date {
        content.push_str(&format!("- **Date**: {}\n", date));
    }

    if let Some(is_git) = context.is_git_repo {
        content.push_str(&format!(
            "- **Git Repository**: {}\n",
            if is_git { "Yes" } else { "No" }
        ));
    }

    if let Some(ref model) = context.model {
        content.push_str(&format!("- **Model**: {}\n", model));
    }

    if let Some(window) = context.context_window {
        content.push_str(&format!("- **Context Window**: {} tokens\n", window));
    }

    if let Some(usage) = context.token_usage {
        content.push_str(&format!("- **Current Token Usage**: {} tokens\n", usage));
    }

    if let Some(turn) = context.turn_number {
        content.push_str(&format!("- **Turn**: {}\n", turn));
    }

    PromptSection::new("Environment", content).with_priority(SectionPriority::Low)
}

/// Build the notifications section for update messages.
pub fn build_notifications_section(notifications: &[AgentNotification]) -> PromptSection {
    if notifications.is_empty() {
        return PromptSection::new("Updates", "").enabled(false);
    }

    let mut content = String::new();
    content
        .push_str("**IMPORTANT: The following changes have occurred since the last message:**\n\n");

    for notification in notifications {
        let icon = notification.notification_type.icon();
        content.push_str(&format!("{} {}\n", icon, notification.message));
    }

    content.push_str("\nPlease acknowledge these changes in your response.\n");

    PromptSection::new("System Updates", content).with_priority(SectionPriority::Critical)
}

/// Build a custom instructions section.
pub fn build_custom_instructions_section(instructions: &str) -> PromptSection {
    PromptSection::new("Custom Instructions", instructions).with_priority(SectionPriority::High)
}

/// Build a subagents section.
pub fn build_subagents_section(subagents: &[AgentConfig]) -> PromptSection {
    if subagents.is_empty() {
        return PromptSection::new("Available Subagents", "No subagents available.")
            .with_priority(SectionPriority::Low)
            .enabled(false);
    }

    let mut content = String::new();
    content
        .push_str("You can delegate tasks to the following subagents using @mention syntax:\n\n");

    for agent in subagents.iter().filter(|a| a.enabled) {
        content.push_str(&format!("- **@{}**", agent.name));
        if let Some(ref desc) = agent.description {
            content.push_str(&format!(": {}", desc));
        }
        content.push('\n');
    }

    PromptSection::new("Available Subagents", content).with_priority(SectionPriority::Normal)
}

/// Build a tools section.
pub fn build_tools_section(allowed: &[String], denied: &[String]) -> PromptSection {
    let mut content = String::new();

    if !allowed.is_empty() {
        content.push_str("**Allowed Tools**:\n");
        for tool in allowed {
            content.push_str(&format!("- ✓ {}\n", tool));
        }
        content.push('\n');
    }

    if !denied.is_empty() {
        content.push_str("**Denied Tools**:\n");
        for tool in denied {
            content.push_str(&format!("- ✗ {}\n", tool));
        }
    }

    PromptSection::new("Tool Access", content).with_priority(SectionPriority::Normal)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_section_render() {
        let section = PromptSection::new("Test", "Content here");
        let rendered = section.render();

        assert!(rendered.contains("## Test"));
        assert!(rendered.contains("Content here"));
    }

    #[test]
    fn test_section_no_header() {
        let section = PromptSection::content_only("Just content");
        let rendered = section.render();

        assert!(!rendered.contains("##"));
        assert!(rendered.contains("Just content"));
    }

    #[test]
    fn test_section_disabled() {
        let section = PromptSection::new("Test", "Content").enabled(false);
        let rendered = section.render();

        assert!(rendered.is_empty());
    }

    #[test]
    fn test_agent_section() {
        let agent = AgentConfig::new("build")
            .with_prompt("Build things")
            .with_mode("build")
            .deny_tool("Execute");

        let section = build_agent_section(&agent);

        let rendered = section.render();
        assert!(rendered.contains("build"));
        assert!(rendered.contains("Execute"));
    }

    #[test]
    fn test_task_section() {
        let task = TaskConfig::new("task-1", "Do something").in_progress();

        let section = build_task_section(&task);
        let rendered = section.render();

        assert!(rendered.contains("task-1"));
        assert!(rendered.contains("🔄"));
    }

    #[test]
    fn test_environment_section() {
        let context = PromptContext::new()
            .with_cwd("/project")
            .with_model("claude-opus-4");

        let section = build_environment_section(&context);
        let rendered = section.render();

        assert!(rendered.contains("/project"));
        assert!(rendered.contains("claude-opus-4"));
    }

    #[test]
    fn test_notifications_section() {
        use crate::notifications::NotificationType;

        let notifications = vec![AgentNotification::new(
            NotificationType::AgentEnabled,
            "research",
            "Agent 'research' has been enabled.",
        )];

        let section = build_notifications_section(&notifications);
        let rendered = section.render();

        assert!(rendered.contains("research"));
        assert!(rendered.contains("enabled"));
    }
}
