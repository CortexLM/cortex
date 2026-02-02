//! Command execution hooks (before and after).

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::types::{HookPriority, HookResult};
use crate::Result;

// ============================================================================
// Command Execute Before Hook
// ============================================================================

/// Input for command.execute.before hook - before slash command execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandExecuteBeforeInput {
    /// Session ID
    pub session_id: String,
    /// Command name (without /)
    pub command: String,
    /// Command arguments
    pub args: Vec<String>,
    /// Raw input
    pub raw_input: String,
}

/// Output for command.execute.before hook (mutable).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandExecuteBeforeOutput {
    /// Modified command name
    pub command: String,
    /// Modified arguments
    pub args: Vec<String>,
    /// Whether to allow execution
    pub allow: bool,
    /// Alternative output (skip execution and show this instead)
    pub alternative_output: Option<String>,
    /// Hook result
    pub result: HookResult,
}

impl CommandExecuteBeforeOutput {
    pub fn new(command: String, args: Vec<String>) -> Self {
        Self {
            command,
            args,
            allow: true,
            alternative_output: None,
            result: HookResult::Continue,
        }
    }

    /// Replace the command output.
    pub fn replace_output(&mut self, output: impl Into<String>) {
        self.allow = false;
        self.alternative_output = Some(output.into());
        self.result = HookResult::Replace {
            result: serde_json::json!({ "output": self.alternative_output }),
        };
    }
}

/// Handler for command.execute.before hook.
#[async_trait]
pub trait CommandExecuteBeforeHook: Send + Sync {
    fn priority(&self) -> HookPriority {
        HookPriority::default()
    }

    /// Get command patterns this hook applies to (None = all commands).
    fn patterns(&self) -> Option<Vec<String>> {
        None
    }

    async fn execute(
        &self,
        input: &CommandExecuteBeforeInput,
        output: &mut CommandExecuteBeforeOutput,
    ) -> Result<()>;
}

// ============================================================================
// Command Execute After Hook
// ============================================================================

/// Input for command.execute.after hook.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandExecuteAfterInput {
    /// Session ID
    pub session_id: String,
    /// Command name
    pub command: String,
    /// Command arguments
    pub args: Vec<String>,
    /// Whether execution succeeded
    pub success: bool,
    /// Duration in milliseconds
    pub duration_ms: u64,
}

/// Output for command.execute.after hook (mutable).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandExecuteAfterOutput {
    /// Command output
    pub output: String,
    /// Additional messages to show
    pub messages: Vec<String>,
    /// Hook result
    pub result: HookResult,
}

impl CommandExecuteAfterOutput {
    pub fn new(output: String) -> Self {
        Self {
            output,
            messages: Vec::new(),
            result: HookResult::Continue,
        }
    }
}

/// Handler for command.execute.after hook.
#[async_trait]
pub trait CommandExecuteAfterHook: Send + Sync {
    fn priority(&self) -> HookPriority {
        HookPriority::default()
    }

    async fn execute(
        &self,
        input: &CommandExecuteAfterInput,
        output: &mut CommandExecuteAfterOutput,
    ) -> Result<()>;
}
