//! Session lifecycle hooks (start and end).

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use super::prompt_hooks::ContextDocument;
use super::types::{HookPriority, HookResult};
use crate::Result;

// ============================================================================
// Session Start Hook
// ============================================================================

/// Input for session.start hook.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionStartInput {
    /// Session ID
    pub session_id: String,
    /// Agent name
    pub agent: Option<String>,
    /// Model name
    pub model: Option<String>,
    /// Working directory
    pub cwd: PathBuf,
    /// Is this a resumed session?
    pub resumed: bool,
}

/// Output for session.start hook (mutable).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionStartOutput {
    /// Initial system prompt additions
    pub system_prompt_additions: Vec<String>,
    /// Initial context to provide
    pub initial_context: Vec<ContextDocument>,
    /// Greeting message
    pub greeting: Option<String>,
    /// Hook result
    pub result: HookResult,
}

impl SessionStartOutput {
    pub fn new() -> Self {
        Self {
            system_prompt_additions: Vec::new(),
            initial_context: Vec::new(),
            greeting: None,
            result: HookResult::Continue,
        }
    }
}

impl Default for SessionStartOutput {
    fn default() -> Self {
        Self::new()
    }
}

/// Handler for session.start hook.
#[async_trait]
pub trait SessionStartHook: Send + Sync {
    fn priority(&self) -> HookPriority {
        HookPriority::default()
    }

    async fn execute(
        &self,
        input: &SessionStartInput,
        output: &mut SessionStartOutput,
    ) -> Result<()>;
}

// ============================================================================
// Session End Hook
// ============================================================================

/// Input for session.end hook.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionEndInput {
    /// Session ID
    pub session_id: String,
    /// Duration in seconds
    pub duration_secs: u64,
    /// Total messages
    pub total_messages: usize,
    /// Total tokens used
    pub total_tokens: Option<u64>,
    /// Whether session was saved
    pub saved: bool,
}

/// Output for session.end hook (mutable).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionEndOutput {
    /// Summary to generate
    pub generate_summary: bool,
    /// Actions to perform
    pub actions: Vec<SessionEndAction>,
    /// Hook result
    pub result: HookResult,
}

impl SessionEndOutput {
    pub fn new() -> Self {
        Self {
            generate_summary: false,
            actions: Vec::new(),
            result: HookResult::Continue,
        }
    }
}

impl Default for SessionEndOutput {
    fn default() -> Self {
        Self::new()
    }
}

/// Session end actions.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionEndAction {
    /// Save session summary
    SaveSummary { path: PathBuf },
    /// Export chat
    ExportChat { format: String, path: PathBuf },
    /// Show statistics
    ShowStats,
    /// Custom action
    Custom {
        action: String,
        data: serde_json::Value,
    },
}

/// Handler for session.end hook.
#[async_trait]
pub trait SessionEndHook: Send + Sync {
    fn priority(&self) -> HookPriority {
        HookPriority::default()
    }

    async fn execute(&self, input: &SessionEndInput, output: &mut SessionEndOutput) -> Result<()>;
}
