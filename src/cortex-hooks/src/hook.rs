//! Hook definitions and types.
//!
//! SECURITY: This module handles hook command building with proper input sanitization.
//! All user-provided values (file paths, session IDs, etc.) are sanitized before
//! being substituted into command templates to prevent command injection attacks.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Sanitize a string value for safe use in command arguments.
/// This ensures that substituted values cannot break out of their intended context.
///
/// SECURITY: This function removes or replaces potentially dangerous characters
/// that could be used for command injection.
fn sanitize_for_command(input: &str) -> String {
    // Use a strict allowlist approach for safety
    // Allow common path and identifier characters only
    input
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric()
                || c == '/'
                || c == '\\'
                || c == '.'
                || c == '-'
                || c == '_'
                || c == ':'
                || c == ' '
            {
                c
            } else {
                // Replace potentially dangerous characters with underscore
                '_'
            }
        })
        .collect()
}

/// Type of hook trigger.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum HookType {
    // Existing events
    /// Triggered after a file is edited.
    FileEdited,
    /// Triggered after a file is created.
    FileCreated,
    /// Triggered after a file is deleted.
    FileDeleted,
    /// Triggered when a session completes.
    SessionCompleted,

    // New events (Claude Code inspired)
    /// Triggered before a tool is used.
    PreToolUse,
    /// Triggered after a tool is used successfully.
    PostToolUse,
    /// Triggered after a tool use failure.
    PostToolUseFailure,
    /// Triggered when a permission is requested.
    PermissionRequest,
    /// Triggered when a user prompt is submitted.
    UserPromptSubmit,
    /// Triggered for system notifications.
    Notification,
    /// Triggered when the agent stops responding.
    Stop,
    /// Triggered when a subagent starts.
    SubagentStart,
    /// Triggered when a subagent stops.
    SubagentStop,
    /// Triggered before context compaction.
    PreCompact,
    /// Triggered during setup (--init, --maintenance).
    Setup,
    /// Triggered at session start.
    SessionStart,
    /// Triggered at session end.
    SessionEnd,
}

impl HookType {
    pub fn as_str(&self) -> &'static str {
        match self {
            HookType::FileEdited => "FileEdited",
            HookType::FileCreated => "FileCreated",
            HookType::FileDeleted => "FileDeleted",
            HookType::SessionCompleted => "SessionCompleted",
            HookType::PreToolUse => "PreToolUse",
            HookType::PostToolUse => "PostToolUse",
            HookType::PostToolUseFailure => "PostToolUseFailure",
            HookType::PermissionRequest => "PermissionRequest",
            HookType::UserPromptSubmit => "UserPromptSubmit",
            HookType::Notification => "Notification",
            HookType::Stop => "Stop",
            HookType::SubagentStart => "SubagentStart",
            HookType::SubagentStop => "SubagentStop",
            HookType::PreCompact => "PreCompact",
            HookType::Setup => "Setup",
            HookType::SessionStart => "SessionStart",
            HookType::SessionEnd => "SessionEnd",
        }
    }

    /// Indicates if this event type can block/modify the execution flow.
    pub fn is_blocking(&self) -> bool {
        matches!(
            self,
            HookType::PreToolUse | HookType::PermissionRequest | HookType::UserPromptSubmit
        )
    }

    /// Returns the environment variables available for this hook type.
    pub fn available_env_vars(&self) -> &'static [&'static str] {
        match self {
            HookType::PreToolUse | HookType::PostToolUse | HookType::PostToolUseFailure => {
                &["TOOL_NAME", "TOOL_ARGS", "TOOL_RESULT", "FILE_PATH"]
            }
            HookType::SubagentStart | HookType::SubagentStop => {
                &["AGENT_ID", "AGENT_NAME", "PARENT_AGENT_ID"]
            }
            HookType::SessionStart | HookType::SessionEnd | HookType::SessionCompleted => {
                &["SESSION_ID", "SESSION_START_TIME"]
            }
            HookType::Stop => &["RESPONSE_LENGTH", "TOOLS_USED_COUNT"],
            HookType::FileEdited | HookType::FileCreated | HookType::FileDeleted => {
                &["FILE_PATH", "FILE_EXT"]
            }
            _ => &[],
        }
    }
}

/// Execution status of a hook.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookStatus {
    /// Hook completed successfully.
    Success,
    /// Hook failed.
    Failure,
    /// Async hook was started (fire-and-forget).
    AsyncStarted,
    /// Hook was skipped (e.g., already executed with `once` flag).
    Skipped,
    /// Hook timed out.
    Timeout,
}

/// A hook definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hook {
    /// Hook identifier.
    pub id: String,
    /// Hook type.
    pub hook_type: HookType,
    /// File pattern (glob) - for file hooks.
    pub pattern: Option<String>,
    /// Command to execute.
    pub command: Vec<String>,
    /// Environment variables.
    #[serde(default)]
    pub environment: HashMap<String, String>,
    /// Working directory.
    pub cwd: Option<PathBuf>,
    /// Timeout in seconds.
    pub timeout_secs: Option<u64>,
    /// Whether hook is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Whether to continue on error.
    #[serde(default)]
    pub continue_on_error: bool,

    // New fields for async and advanced execution
    /// If true, execute asynchronously (fire-and-forget, non-blocking).
    #[serde(default)]
    pub async_execution: bool,
    /// If true, execute only once per session.
    #[serde(default)]
    pub once: bool,
    /// Tool name matcher (for PreToolUse/PostToolUse hooks).
    #[serde(default)]
    pub tool_matcher: Option<String>,
}

fn default_true() -> bool {
    true
}

impl Hook {
    pub fn new(id: impl Into<String>, hook_type: HookType, command: Vec<String>) -> Self {
        Self {
            id: id.into(),
            hook_type,
            pattern: None,
            command,
            environment: HashMap::new(),
            cwd: None,
            timeout_secs: Some(30),
            enabled: true,
            continue_on_error: false,
            async_execution: false,
            once: false,
            tool_matcher: None,
        }
    }

    pub fn with_pattern(mut self, pattern: impl Into<String>) -> Self {
        self.pattern = Some(pattern.into());
        self
    }

    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.environment.insert(key.into(), value.into());
        self
    }

    pub fn with_cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    pub fn with_timeout(mut self, secs: u64) -> Self {
        self.timeout_secs = Some(secs);
        self
    }

    /// Configure the hook for asynchronous (fire-and-forget) execution.
    pub fn async_exec(mut self) -> Self {
        self.async_execution = true;
        self
    }

    /// Configure the hook to execute only once per session.
    pub fn once(mut self) -> Self {
        self.once = true;
        self
    }

    /// Configure a tool name matcher for PreToolUse/PostToolUse hooks.
    pub fn with_tool_matcher(mut self, matcher: impl Into<String>) -> Self {
        self.tool_matcher = Some(matcher.into());
        self
    }

    /// Check if this hook matches a tool name (for tool-related hooks).
    pub fn matches_tool(&self, tool_name: &str) -> bool {
        if let Some(ref matcher) = self.tool_matcher {
            // Support pipe-separated tool names like "Edit|Create|Execute"
            matcher.split('|').any(|m| m.trim() == tool_name)
        } else {
            true // No matcher means match all tools
        }
    }

    /// Check if this hook matches a file path.
    pub fn matches_file(&self, path: &str) -> bool {
        if let Some(ref pattern) = self.pattern {
            glob::Pattern::new(pattern)
                .map(|p| p.matches(path))
                .unwrap_or(false)
        } else {
            true
        }
    }

    /// Build the command with substitutions.
    ///
    /// SECURITY: All substituted values are sanitized to prevent command injection.
    /// The substitution placeholders ({file}, {path}, {session_id}, {message_id})
    /// are replaced with sanitized versions of the context values.
    pub fn build_command(&self, context: &HookContext) -> Vec<String> {
        self.command
            .iter()
            .map(|arg| {
                let mut result = arg.clone();

                // SECURITY: Sanitize file paths before substitution
                if let Some(ref file) = context.file_path {
                    let sanitized_path = sanitize_for_command(&file.to_string_lossy());
                    result = result.replace("{file}", &sanitized_path);
                    result = result.replace("{path}", &sanitized_path);
                }

                // SECURITY: Sanitize session_id before substitution
                if let Some(ref session_id) = context.session_id {
                    let sanitized_id = sanitize_for_command(session_id);
                    result = result.replace("{session_id}", &sanitized_id);
                }

                // SECURITY: Sanitize message_id before substitution
                if let Some(ref message_id) = context.message_id {
                    let sanitized_id = sanitize_for_command(message_id);
                    result = result.replace("{message_id}", &sanitized_id);
                }

                result
            })
            .collect()
    }
}

/// Context for hook execution.
#[derive(Debug, Clone, Default)]
pub struct HookContext {
    /// File path (for file hooks).
    pub file_path: Option<PathBuf>,
    /// Session ID.
    pub session_id: Option<String>,
    /// Message ID.
    pub message_id: Option<String>,
    /// Additional data.
    pub data: HashMap<String, String>,
    /// Tool name (for tool-related hooks).
    pub tool_name: Option<String>,
    /// Tool arguments (for tool-related hooks).
    pub tool_args: Option<String>,
    /// Tool result (for PostToolUse hooks).
    pub tool_result: Option<String>,
    /// Agent ID (for subagent hooks).
    pub agent_id: Option<String>,
    /// Agent name (for subagent hooks).
    pub agent_name: Option<String>,
    /// Parent agent ID (for subagent hooks).
    pub parent_agent_id: Option<String>,
}

impl HookContext {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_file(mut self, path: impl Into<PathBuf>) -> Self {
        self.file_path = Some(path.into());
        self
    }

    pub fn with_session(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    pub fn with_message(mut self, message_id: impl Into<String>) -> Self {
        self.message_id = Some(message_id.into());
        self
    }

    pub fn with_data(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.data.insert(key.into(), value.into());
        self
    }

    pub fn with_tool(mut self, name: impl Into<String>, args: impl Into<String>) -> Self {
        self.tool_name = Some(name.into());
        self.tool_args = Some(args.into());
        self
    }

    pub fn with_tool_result(mut self, result: impl Into<String>) -> Self {
        self.tool_result = Some(result.into());
        self
    }

    pub fn with_agent(
        mut self,
        agent_id: impl Into<String>,
        agent_name: impl Into<String>,
    ) -> Self {
        self.agent_id = Some(agent_id.into());
        self.agent_name = Some(agent_name.into());
        self
    }

    pub fn with_parent_agent(mut self, parent_id: impl Into<String>) -> Self {
        self.parent_agent_id = Some(parent_id.into());
        self
    }

    /// Convert context to environment variables for hook execution.
    pub fn as_env(&self) -> HashMap<String, String> {
        let mut env = HashMap::new();

        if let Some(ref path) = self.file_path {
            env.insert("FILE_PATH".to_string(), path.to_string_lossy().to_string());
            if let Some(ext) = path.extension() {
                env.insert("FILE_EXT".to_string(), ext.to_string_lossy().to_string());
            }
        }

        if let Some(ref session_id) = self.session_id {
            env.insert("SESSION_ID".to_string(), session_id.clone());
        }

        if let Some(ref message_id) = self.message_id {
            env.insert("MESSAGE_ID".to_string(), message_id.clone());
        }

        if let Some(ref tool_name) = self.tool_name {
            env.insert("TOOL_NAME".to_string(), tool_name.clone());
        }

        if let Some(ref tool_args) = self.tool_args {
            env.insert("TOOL_ARGS".to_string(), tool_args.clone());
        }

        if let Some(ref tool_result) = self.tool_result {
            env.insert("TOOL_RESULT".to_string(), tool_result.clone());
        }

        if let Some(ref agent_id) = self.agent_id {
            env.insert("AGENT_ID".to_string(), agent_id.clone());
        }

        if let Some(ref agent_name) = self.agent_name {
            env.insert("AGENT_NAME".to_string(), agent_name.clone());
        }

        if let Some(ref parent_id) = self.parent_agent_id {
            env.insert("PARENT_AGENT_ID".to_string(), parent_id.clone());
        }

        // Add custom data
        for (k, v) in &self.data {
            env.insert(k.clone(), v.clone());
        }

        env
    }
}

/// Result of hook execution.
#[derive(Debug, Clone)]
pub struct HookResult {
    /// Hook ID.
    pub hook_id: String,
    /// Whether hook succeeded.
    pub success: bool,
    /// Exit code.
    pub exit_code: Option<i32>,
    /// Stdout output.
    pub stdout: String,
    /// Stderr output.
    pub stderr: String,
    /// Duration in milliseconds.
    pub duration_ms: u64,
    /// Execution status.
    pub status: HookStatus,
}

impl HookResult {
    pub fn success(hook_id: impl Into<String>, duration_ms: u64) -> Self {
        Self {
            hook_id: hook_id.into(),
            success: true,
            exit_code: Some(0),
            stdout: String::new(),
            stderr: String::new(),
            duration_ms,
            status: HookStatus::Success,
        }
    }

    pub fn failure(hook_id: impl Into<String>, error: impl Into<String>, duration_ms: u64) -> Self {
        Self {
            hook_id: hook_id.into(),
            success: false,
            exit_code: None,
            stdout: String::new(),
            stderr: error.into(),
            duration_ms,
            status: HookStatus::Failure,
        }
    }

    /// Create a result for an async hook that was started.
    pub fn async_started(hook_id: impl Into<String>) -> Self {
        Self {
            hook_id: hook_id.into(),
            success: true,
            exit_code: None,
            stdout: String::new(),
            stderr: String::new(),
            duration_ms: 0,
            status: HookStatus::AsyncStarted,
        }
    }

    /// Create a result for a skipped hook.
    pub fn skipped(hook_id: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            hook_id: hook_id.into(),
            success: true,
            exit_code: None,
            stdout: String::new(),
            stderr: reason.into(),
            duration_ms: 0,
            status: HookStatus::Skipped,
        }
    }

    /// Create a result for a timed out hook.
    pub fn timeout(hook_id: impl Into<String>, duration_ms: u64) -> Self {
        Self {
            hook_id: hook_id.into(),
            success: false,
            exit_code: None,
            stdout: String::new(),
            stderr: "Hook execution timed out".to_string(),
            duration_ms,
            status: HookStatus::Timeout,
        }
    }

    pub fn with_output(mut self, stdout: String, stderr: String) -> Self {
        self.stdout = stdout;
        self.stderr = stderr;
        self
    }

    pub fn with_exit_code(mut self, code: i32) -> Self {
        self.exit_code = Some(code);
        self.success = code == 0;
        if code != 0 {
            self.status = HookStatus::Failure;
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hook_matches_file() {
        let hook =
            Hook::new("test", HookType::FileEdited, vec!["echo".to_string()]).with_pattern("*.rs");

        assert!(hook.matches_file("main.rs"));
        assert!(hook.matches_file("src/lib.rs"));
        assert!(!hook.matches_file("main.py"));
    }

    #[test]
    fn test_build_command() {
        let hook = Hook::new(
            "test",
            HookType::FileEdited,
            vec![
                "prettier".to_string(),
                "--write".to_string(),
                "{file}".to_string(),
            ],
        );

        let context = HookContext::new().with_file("/project/src/main.ts");

        let cmd = hook.build_command(&context);
        assert_eq!(cmd[2], "/project/src/main.ts");
    }

    #[test]
    fn test_build_command_sanitizes_injection_attempts() {
        let hook = Hook::new(
            "test",
            HookType::FileEdited,
            vec!["echo".to_string(), "{file}".to_string()],
        );

        // Attempt command injection via file path
        let context = HookContext::new().with_file("/project/$(rm -rf /)/main.ts");

        let cmd = hook.build_command(&context);
        // The dangerous characters should be sanitized
        assert!(!cmd[1].contains("$("));
        assert!(!cmd[1].contains(")"));
        // Should have underscores instead
        assert!(cmd[1].contains("_rm"));
    }

    #[test]
    fn test_sanitize_for_command() {
        // Test that dangerous characters are sanitized
        assert_eq!(sanitize_for_command("normal-file.txt"), "normal-file.txt");
        assert_eq!(sanitize_for_command("path/to/file"), "path/to/file");
        assert_eq!(sanitize_for_command("file$(cmd)"), "file__cmd_");
        assert_eq!(sanitize_for_command("file`cmd`"), "file_cmd_");
        assert_eq!(sanitize_for_command("file;rm -rf /"), "file_rm -rf /");
        assert_eq!(sanitize_for_command("file|cat"), "file_cat");
    }

    #[test]
    fn test_hook_type_blocking() {
        assert!(HookType::PreToolUse.is_blocking());
        assert!(HookType::PermissionRequest.is_blocking());
        assert!(HookType::UserPromptSubmit.is_blocking());
        assert!(!HookType::SessionStart.is_blocking());
        assert!(!HookType::PostToolUse.is_blocking());
        assert!(!HookType::FileEdited.is_blocking());
    }

    #[test]
    fn test_hook_type_env_vars() {
        let vars = HookType::PreToolUse.available_env_vars();
        assert!(vars.contains(&"TOOL_NAME"));
        assert!(vars.contains(&"TOOL_ARGS"));

        let vars = HookType::SubagentStart.available_env_vars();
        assert!(vars.contains(&"AGENT_ID"));
        assert!(vars.contains(&"AGENT_NAME"));

        let vars = HookType::SessionStart.available_env_vars();
        assert!(vars.contains(&"SESSION_ID"));
    }

    #[test]
    fn test_hook_async_and_once() {
        let hook = Hook::new("test", HookType::PostToolUse, vec!["echo".to_string()])
            .async_exec()
            .once();

        assert!(hook.async_execution);
        assert!(hook.once);
    }

    #[test]
    fn test_hook_tool_matcher() {
        let hook = Hook::new("test", HookType::PreToolUse, vec!["echo".to_string()])
            .with_tool_matcher("Edit|Create|Execute");

        assert!(hook.matches_tool("Edit"));
        assert!(hook.matches_tool("Create"));
        assert!(hook.matches_tool("Execute"));
        assert!(!hook.matches_tool("Read"));
        assert!(!hook.matches_tool("Delete"));
    }

    #[test]
    fn test_hook_context_as_env() {
        let context = HookContext::new()
            .with_file("/path/to/file.rs")
            .with_session("session-123")
            .with_tool("Execute", "ls -la")
            .with_agent("agent-1", "TestAgent")
            .with_parent_agent("parent-1")
            .with_data("CUSTOM_VAR", "custom_value");

        let env = context.as_env();

        assert_eq!(env.get("FILE_PATH"), Some(&"/path/to/file.rs".to_string()));
        assert_eq!(env.get("FILE_EXT"), Some(&"rs".to_string()));
        assert_eq!(env.get("SESSION_ID"), Some(&"session-123".to_string()));
        assert_eq!(env.get("TOOL_NAME"), Some(&"Execute".to_string()));
        assert_eq!(env.get("TOOL_ARGS"), Some(&"ls -la".to_string()));
        assert_eq!(env.get("AGENT_ID"), Some(&"agent-1".to_string()));
        assert_eq!(env.get("AGENT_NAME"), Some(&"TestAgent".to_string()));
        assert_eq!(env.get("PARENT_AGENT_ID"), Some(&"parent-1".to_string()));
        assert_eq!(env.get("CUSTOM_VAR"), Some(&"custom_value".to_string()));
    }

    #[test]
    fn test_hook_result_statuses() {
        let success = HookResult::success("test", 100);
        assert_eq!(success.status, HookStatus::Success);

        let failure = HookResult::failure("test", "error", 100);
        assert_eq!(failure.status, HookStatus::Failure);

        let async_started = HookResult::async_started("test");
        assert_eq!(async_started.status, HookStatus::AsyncStarted);

        let skipped = HookResult::skipped("test", "already executed");
        assert_eq!(skipped.status, HookStatus::Skipped);

        let timeout = HookResult::timeout("test", 5000);
        assert_eq!(timeout.status, HookStatus::Timeout);
    }
}
