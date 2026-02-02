//! User input interception hooks.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use super::types::{HookPriority, HookResult};
use crate::Result;

/// Input for input.intercept hook - intercepts user input before processing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputInterceptInput {
    /// Session ID
    pub session_id: String,
    /// Raw input text
    pub text: String,
    /// Whether it's a command (starts with /)
    pub is_command: bool,
    /// Cursor position
    pub cursor_position: usize,
}

/// Output for input.intercept hook (mutable).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputInterceptOutput {
    /// Modified input text
    pub text: String,
    /// Whether to process the input
    pub process: bool,
    /// Alternative action to take
    pub action: Option<InputAction>,
    /// Autocomplete suggestions
    pub suggestions: Vec<InputSuggestion>,
    /// Hook result
    pub result: HookResult,
}

impl InputInterceptOutput {
    pub fn new(text: String) -> Self {
        Self {
            text,
            process: true,
            action: None,
            suggestions: Vec::new(),
            result: HookResult::Continue,
        }
    }
}

/// Input actions that can be triggered by hooks.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InputAction {
    /// Expand text (e.g., snippet expansion)
    Expand { text: String },
    /// Show quick pick
    QuickPick { items: Vec<QuickPickItem> },
    /// Open file
    OpenFile { path: PathBuf },
    /// Show help
    ShowHelp { topic: Option<String> },
    /// Custom action
    Custom {
        action: String,
        data: serde_json::Value,
    },
}

/// Quick pick item.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuickPickItem {
    /// Label
    pub label: String,
    /// Description
    pub description: Option<String>,
    /// Detail
    pub detail: Option<String>,
    /// Value to insert
    pub value: String,
}

/// Input suggestion for autocomplete.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputSuggestion {
    /// Suggestion text
    pub text: String,
    /// Label to display
    pub label: String,
    /// Description
    pub description: Option<String>,
    /// Kind of suggestion
    pub kind: SuggestionKind,
    /// Sort priority
    pub sort_priority: i32,
}

/// Suggestion kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SuggestionKind {
    Command,
    File,
    Snippet,
    Variable,
    Keyword,
    Custom,
}

/// Handler for input.intercept hook.
#[async_trait]
pub trait InputInterceptHook: Send + Sync {
    fn priority(&self) -> HookPriority {
        HookPriority::default()
    }

    async fn execute(
        &self,
        input: &InputInterceptInput,
        output: &mut InputInterceptOutput,
    ) -> Result<()>;
}
