//! File operation hooks (before and after).

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use super::types::{HookPriority, HookResult};
use crate::Result;

/// File operation types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileOperation {
    Create,
    Read,
    Write,
    Delete,
    Rename,
    Move,
    Copy,
}

// ============================================================================
// File Operation Before Hook
// ============================================================================

/// Input for file.operation.before hook.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileOperationBeforeInput {
    /// Session ID
    pub session_id: String,
    /// Operation type
    pub operation: FileOperation,
    /// Source path
    pub path: PathBuf,
    /// Destination path (for rename/move/copy)
    pub dest_path: Option<PathBuf>,
    /// Tool that initiated the operation
    pub tool: Option<String>,
}

/// Output for file.operation.before hook (mutable).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileOperationBeforeOutput {
    /// Modified path
    pub path: PathBuf,
    /// Modified destination path
    pub dest_path: Option<PathBuf>,
    /// Whether to allow the operation
    pub allow: bool,
    /// Reason for denial (if any)
    pub deny_reason: Option<String>,
    /// Hook result
    pub result: HookResult,
}

impl FileOperationBeforeOutput {
    pub fn new(path: PathBuf, dest_path: Option<PathBuf>) -> Self {
        Self {
            path,
            dest_path,
            allow: true,
            deny_reason: None,
            result: HookResult::Continue,
        }
    }

    /// Deny the operation with a reason.
    pub fn deny(&mut self, reason: impl Into<String>) {
        self.allow = false;
        self.deny_reason = Some(reason.into());
        self.result = HookResult::Abort {
            reason: self.deny_reason.clone().unwrap_or_default(),
        };
    }
}

/// Handler for file.operation.before hook.
#[async_trait]
pub trait FileOperationBeforeHook: Send + Sync {
    fn priority(&self) -> HookPriority {
        HookPriority::default()
    }

    /// Get the file patterns this hook applies to (None = all files).
    fn patterns(&self) -> Option<Vec<String>> {
        None
    }

    /// Get the operations this hook applies to (None = all operations).
    fn operations(&self) -> Option<Vec<FileOperation>> {
        None
    }

    async fn execute(
        &self,
        input: &FileOperationBeforeInput,
        output: &mut FileOperationBeforeOutput,
    ) -> Result<()>;
}

// ============================================================================
// File Operation After Hook
// ============================================================================

/// Input for file.operation.after hook.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileOperationAfterInput {
    /// Session ID
    pub session_id: String,
    /// Operation type
    pub operation: FileOperation,
    /// Path
    pub path: PathBuf,
    /// Destination path (for rename/move/copy)
    pub dest_path: Option<PathBuf>,
    /// Whether the operation succeeded
    pub success: bool,
    /// Error message if failed
    pub error: Option<String>,
}

/// Output for file.operation.after hook (mutable).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileOperationAfterOutput {
    /// Additional actions to perform
    pub post_actions: Vec<FilePostAction>,
    /// Hook result
    pub result: HookResult,
}

impl FileOperationAfterOutput {
    pub fn new() -> Self {
        Self {
            post_actions: Vec::new(),
            result: HookResult::Continue,
        }
    }
}

impl Default for FileOperationAfterOutput {
    fn default() -> Self {
        Self::new()
    }
}

/// Post-operation actions.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FilePostAction {
    /// Refresh file in editor
    RefreshFile { path: PathBuf },
    /// Run linter
    RunLinter { path: PathBuf },
    /// Run formatter
    RunFormatter { path: PathBuf },
    /// Show notification
    Notify { message: String },
    /// Custom action
    Custom {
        action: String,
        data: serde_json::Value,
    },
}

/// Handler for file.operation.after hook.
#[async_trait]
pub trait FileOperationAfterHook: Send + Sync {
    fn priority(&self) -> HookPriority {
        HookPriority::default()
    }

    async fn execute(
        &self,
        input: &FileOperationAfterInput,
        output: &mut FileOperationAfterOutput,
    ) -> Result<()>;
}
