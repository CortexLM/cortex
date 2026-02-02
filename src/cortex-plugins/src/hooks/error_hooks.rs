//! Error handling hooks.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

use super::types::{HookPriority, HookResult};
use crate::Result;

/// Input for error.handle hook - when an error occurs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorHandleInput {
    /// Session ID
    pub session_id: String,
    /// Error source
    pub source: ErrorSource,
    /// Error code (if any)
    pub code: Option<String>,
    /// Error message
    pub message: String,
    /// Stack trace (if available)
    pub stack: Option<String>,
    /// Context data
    pub context: HashMap<String, serde_json::Value>,
}

/// Error sources.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorSource {
    /// Tool execution error
    Tool { tool: String },
    /// AI/model error
    Ai { model: String },
    /// Command error
    Command { command: String },
    /// Plugin error
    Plugin { plugin_id: String },
    /// File operation error
    File { path: PathBuf },
    /// Network error
    Network { url: String },
    /// Permission error
    Permission { resource: String },
    /// Parse error
    Parse { input: String },
    /// Unknown error
    Unknown,
}

/// Output for error.handle hook (mutable).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorHandleOutput {
    /// Whether error was handled
    pub handled: bool,
    /// User-friendly message to show
    pub user_message: Option<String>,
    /// Recovery action
    pub recovery: Option<ErrorRecovery>,
    /// Whether to retry the operation
    pub retry: bool,
    /// Whether to suppress the error
    pub suppress: bool,
    /// Hook result
    pub result: HookResult,
}

impl ErrorHandleOutput {
    pub fn new() -> Self {
        Self {
            handled: false,
            user_message: None,
            recovery: None,
            retry: false,
            suppress: false,
            result: HookResult::Continue,
        }
    }

    /// Handle the error with a user message.
    pub fn handle(&mut self, message: impl Into<String>) {
        self.handled = true;
        self.user_message = Some(message.into());
    }
}

impl Default for ErrorHandleOutput {
    fn default() -> Self {
        Self::new()
    }
}

/// Error recovery actions.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ErrorRecovery {
    /// Suggest an alternative command
    SuggestCommand { command: String, args: Vec<String> },
    /// Show documentation
    ShowDocs { topic: String },
    /// Retry with different parameters
    RetryWith {
        params: HashMap<String, serde_json::Value>,
    },
    /// Open a file
    OpenFile { path: PathBuf },
    /// Custom recovery
    Custom {
        action: String,
        data: serde_json::Value,
    },
}

/// Handler for error.handle hook.
#[async_trait]
pub trait ErrorHandleHook: Send + Sync {
    fn priority(&self) -> HookPriority {
        HookPriority::default()
    }

    /// Get error sources this hook handles (None = all).
    fn sources(&self) -> Option<Vec<String>> {
        None
    }

    async fn execute(&self, input: &ErrorHandleInput, output: &mut ErrorHandleOutput)
    -> Result<()>;
}
