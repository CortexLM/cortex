//! Tool execution hooks (before and after).

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::types::{HookPriority, HookResult};
use crate::Result;

// ============================================================================
// Tool Execute Before Hook
// ============================================================================

/// Input for tool.execute.before hook.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolExecuteBeforeInput {
    /// Tool name
    pub tool: String,
    /// Session ID
    pub session_id: String,
    /// Call ID
    pub call_id: String,
    /// Tool arguments
    pub args: serde_json::Value,
}

/// Output for tool.execute.before hook (mutable).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolExecuteBeforeOutput {
    /// Modified tool arguments
    pub args: serde_json::Value,
    /// Hook result
    pub result: HookResult,
}

impl ToolExecuteBeforeOutput {
    /// Create a new output with the original args.
    pub fn new(args: serde_json::Value) -> Self {
        Self {
            args,
            result: HookResult::Continue,
        }
    }
}

/// Handler for tool.execute.before hook.
#[async_trait]
pub trait ToolExecuteBeforeHook: Send + Sync {
    /// Get the priority of this hook.
    fn priority(&self) -> HookPriority {
        HookPriority::default()
    }

    /// Get the tool pattern this hook applies to (None = all tools).
    fn pattern(&self) -> Option<&str> {
        None
    }

    /// Execute the hook.
    async fn execute(
        &self,
        input: &ToolExecuteBeforeInput,
        output: &mut ToolExecuteBeforeOutput,
    ) -> Result<()>;
}

// ============================================================================
// Tool Execute After Hook
// ============================================================================

/// Input for tool.execute.after hook.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolExecuteAfterInput {
    /// Tool name
    pub tool: String,
    /// Session ID
    pub session_id: String,
    /// Call ID
    pub call_id: String,
    /// Whether the tool execution succeeded
    pub success: bool,
    /// Execution duration in milliseconds
    pub duration_ms: u64,
}

/// Output for tool.execute.after hook (mutable).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolExecuteAfterOutput {
    /// Tool output title
    pub title: Option<String>,
    /// Tool output content
    pub output: String,
    /// Additional metadata
    pub metadata: HashMap<String, serde_json::Value>,
    /// Hook result
    pub result: HookResult,
}

impl ToolExecuteAfterOutput {
    /// Create a new output with the tool output.
    pub fn new(output: String) -> Self {
        Self {
            title: None,
            output,
            metadata: HashMap::new(),
            result: HookResult::Continue,
        }
    }
}

/// Handler for tool.execute.after hook.
#[async_trait]
pub trait ToolExecuteAfterHook: Send + Sync {
    /// Get the priority of this hook.
    fn priority(&self) -> HookPriority {
        HookPriority::default()
    }

    /// Get the tool pattern this hook applies to (None = all tools).
    fn pattern(&self) -> Option<&str> {
        None
    }

    /// Execute the hook.
    async fn execute(
        &self,
        input: &ToolExecuteAfterInput,
        output: &mut ToolExecuteAfterOutput,
    ) -> Result<()>;
}
