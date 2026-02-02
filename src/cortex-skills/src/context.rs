//! Skill context isolation.
//!
//! Provides isolated execution contexts for skills with tool filtering
//! and timeout management.

use std::collections::HashMap;
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::error::{SkillError, SkillResult};
use crate::skill::Skill;

/// A message in the skill context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    /// Message role (system, user, assistant).
    pub role: String,
    /// Message content.
    pub content: String,
}

impl Message {
    /// Creates a system message.
    pub fn system(content: &str) -> Self {
        Self {
            role: "system".to_string(),
            content: content.to_string(),
        }
    }

    /// Creates a user message.
    pub fn user(content: &str) -> Self {
        Self {
            role: "user".to_string(),
            content: content.to_string(),
        }
    }

    /// Creates an assistant message.
    pub fn assistant(content: &str) -> Self {
        Self {
            role: "assistant".to_string(),
            content: content.to_string(),
        }
    }
}

/// Isolated execution context for a skill.
///
/// The context tracks messages, tool results, and enforces skill-specific
/// restrictions like allowed tools and timeouts.
#[derive(Debug)]
pub struct SkillContext {
    /// The skill being executed.
    pub skill: Skill,
    /// Conversation messages within this context.
    messages: Vec<Message>,
    /// Tool execution results.
    tool_results: HashMap<String, serde_json::Value>,
    /// When the context was created.
    started_at: Instant,
    /// Whether the skill has been cancelled.
    cancelled: bool,
}

impl SkillContext {
    /// Creates a new skill context with the skill's system prompt.
    pub fn new(skill: Skill) -> Self {
        let system_prompt = skill.prompt.clone();
        Self {
            skill,
            messages: vec![Message::system(&system_prompt)],
            tool_results: HashMap::new(),
            started_at: Instant::now(),
            cancelled: false,
        }
    }

    /// Creates a context with a custom system message prepended.
    pub fn with_prefix(skill: Skill, prefix: &str) -> Self {
        let system_prompt = format!("{}\n\n{}", prefix, skill.prompt);
        Self {
            skill,
            messages: vec![Message::system(&system_prompt)],
            tool_results: HashMap::new(),
            started_at: Instant::now(),
            cancelled: false,
        }
    }

    /// Adds a user message to the context.
    pub fn add_user_message(&mut self, content: &str) {
        self.messages.push(Message::user(content));
    }

    /// Adds an assistant message to the context.
    pub fn add_assistant_message(&mut self, content: &str) {
        self.messages.push(Message::assistant(content));
    }

    /// Records a tool execution result.
    pub fn record_tool_result(&mut self, tool_name: &str, result: serde_json::Value) {
        self.tool_results.insert(tool_name.to_string(), result);
    }

    /// Returns the messages in this context.
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// Returns tool results.
    pub fn tool_results(&self) -> &HashMap<String, serde_json::Value> {
        &self.tool_results
    }

    /// Checks if a tool is allowed for this skill context.
    ///
    /// Returns `Ok(())` if allowed, or a descriptive error.
    pub fn check_tool(&self, tool: &str) -> SkillResult<()> {
        self.skill.check_tool(tool)
    }

    /// Validates and allows a tool call if permitted.
    ///
    /// This is a convenience method that both checks and returns the tool name
    /// for use in tool dispatching.
    pub fn allow_tool<'a>(&self, tool: &'a str) -> SkillResult<&'a str> {
        self.check_tool(tool)?;
        Ok(tool)
    }

    /// Checks if the skill has timed out.
    pub fn is_timed_out(&self) -> bool {
        if let Some(timeout) = self.skill.config.timeout {
            self.started_at.elapsed().as_secs() >= timeout
        } else {
            false
        }
    }

    /// Returns the elapsed time since context creation.
    pub fn elapsed(&self) -> std::time::Duration {
        self.started_at.elapsed()
    }

    /// Returns the remaining time before timeout, if a timeout is set.
    pub fn remaining_time(&self) -> Option<std::time::Duration> {
        self.skill.config.timeout.map(|timeout| {
            let elapsed = self.started_at.elapsed().as_secs();
            if elapsed >= timeout {
                std::time::Duration::from_secs(0)
            } else {
                std::time::Duration::from_secs(timeout - elapsed)
            }
        })
    }

    /// Checks timeout and returns an error if exceeded.
    pub fn check_timeout(&self) -> SkillResult<()> {
        if self.is_timed_out() {
            Err(SkillError::Timeout {
                skill: self.skill.name.clone(),
                timeout_secs: self.skill.config.timeout.unwrap_or(0),
            })
        } else {
            Ok(())
        }
    }

    /// Cancels the skill execution.
    pub fn cancel(&mut self) {
        self.cancelled = true;
    }

    /// Returns true if the skill has been cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled
    }

    /// Returns the model to use for this skill, if specified.
    pub fn model(&self) -> Option<&str> {
        self.skill.model()
    }

    /// Returns the skill ID.
    pub fn skill_id(&self) -> &str {
        &self.skill.id
    }

    /// Returns the skill name.
    pub fn skill_name(&self) -> &str {
        &self.skill.name
    }

    /// Returns the number of messages in the context.
    pub fn message_count(&self) -> usize {
        self.messages.len()
    }

    /// Clears tool results (useful for multi-turn conversations).
    pub fn clear_tool_results(&mut self) {
        self.tool_results.clear();
    }

    /// Resets the timeout timer.
    pub fn reset_timer(&mut self) {
        self.started_at = Instant::now();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skill::{SkillConfig, SkillMetadata};
    use std::path::PathBuf;

    fn make_test_skill() -> Skill {
        Skill::new(
            "test-skill".to_string(),
            "Test Skill".to_string(),
            "A test skill".to_string(),
            "1.0.0".to_string(),
            "You are a test assistant.".to_string(),
            SkillConfig {
                allowed_tools: vec!["Read".to_string(), "Grep".to_string()],
                denied_tools: vec!["Execute".to_string()],
                timeout: Some(60),
                ..Default::default()
            },
            SkillMetadata::default(),
            PathBuf::from("/test"),
        )
    }

    #[test]
    fn test_context_creation() {
        let skill = make_test_skill();
        let ctx = SkillContext::new(skill);

        assert_eq!(ctx.messages().len(), 1);
        assert_eq!(ctx.messages()[0].role, "system");
        assert!(ctx.messages()[0].content.contains("test assistant"));
    }

    #[test]
    fn test_context_with_prefix() {
        let skill = make_test_skill();
        let ctx = SkillContext::with_prefix(skill, "PREFIX:");

        assert!(ctx.messages()[0].content.starts_with("PREFIX:"));
    }

    #[test]
    fn test_add_messages() {
        let skill = make_test_skill();
        let mut ctx = SkillContext::new(skill);

        ctx.add_user_message("Hello");
        ctx.add_assistant_message("Hi there");

        assert_eq!(ctx.messages().len(), 3);
        assert_eq!(ctx.messages()[1].role, "user");
        assert_eq!(ctx.messages()[2].role, "assistant");
    }

    #[test]
    fn test_check_tool_allowed() {
        let skill = make_test_skill();
        let ctx = SkillContext::new(skill);

        assert!(ctx.check_tool("Read").is_ok());
        assert!(ctx.check_tool("Grep").is_ok());
    }

    #[test]
    fn test_check_tool_denied() {
        let skill = make_test_skill();
        let ctx = SkillContext::new(skill);

        let result = ctx.check_tool("Execute");
        assert!(result.is_err());
        assert!(matches!(result, Err(SkillError::ToolDenied { .. })));
    }

    #[test]
    fn test_check_tool_not_in_allowed() {
        let skill = make_test_skill();
        let ctx = SkillContext::new(skill);

        let result = ctx.check_tool("Create");
        assert!(result.is_err());
        assert!(matches!(result, Err(SkillError::ToolNotAllowed { .. })));
    }

    #[test]
    fn test_allow_tool() {
        let skill = make_test_skill();
        let ctx = SkillContext::new(skill);

        assert_eq!(ctx.allow_tool("Read").unwrap(), "Read");
        assert!(ctx.allow_tool("Execute").is_err());
    }

    #[test]
    fn test_tool_results() {
        let skill = make_test_skill();
        let mut ctx = SkillContext::new(skill);

        ctx.record_tool_result("Read", serde_json::json!({"content": "file data"}));

        assert!(ctx.tool_results().contains_key("Read"));
        assert_eq!(ctx.tool_results()["Read"]["content"], "file data");
    }

    #[test]
    fn test_timeout() {
        let mut skill = make_test_skill();
        skill.config.timeout = Some(0); // Immediate timeout

        let ctx = SkillContext::new(skill);

        // Small sleep to ensure timeout
        std::thread::sleep(std::time::Duration::from_millis(10));

        assert!(ctx.is_timed_out());
        assert!(ctx.check_timeout().is_err());
    }

    #[test]
    fn test_no_timeout() {
        let mut skill = make_test_skill();
        skill.config.timeout = None;

        let ctx = SkillContext::new(skill);
        assert!(!ctx.is_timed_out());
        assert!(ctx.check_timeout().is_ok());
    }

    #[test]
    fn test_remaining_time() {
        let skill = make_test_skill();
        let ctx = SkillContext::new(skill);

        let remaining = ctx.remaining_time();
        assert!(remaining.is_some());
        assert!(remaining.unwrap().as_secs() <= 60);
    }

    #[test]
    fn test_cancel() {
        let skill = make_test_skill();
        let mut ctx = SkillContext::new(skill);

        assert!(!ctx.is_cancelled());
        ctx.cancel();
        assert!(ctx.is_cancelled());
    }

    #[test]
    fn test_reset_timer() {
        let skill = make_test_skill();
        let mut ctx = SkillContext::new(skill);

        std::thread::sleep(std::time::Duration::from_millis(10));
        let elapsed1 = ctx.elapsed();

        ctx.reset_timer();
        let elapsed2 = ctx.elapsed();

        assert!(elapsed2 < elapsed1);
    }
}
