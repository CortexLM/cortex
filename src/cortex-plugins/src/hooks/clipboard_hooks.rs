//! Clipboard operation hooks (copy and paste).

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::types::{HookPriority, HookResult};
use crate::Result;

// ============================================================================
// Clipboard Copy Hook
// ============================================================================

/// Input for clipboard.copy hook.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipboardCopyInput {
    /// Session ID
    pub session_id: String,
    /// Content being copied
    pub content: String,
    /// Source of the copy (output, code, selection)
    pub source: ClipboardSource,
}

/// Clipboard sources.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClipboardSource {
    /// AI output
    Output,
    /// Code block
    Code { language: Option<String> },
    /// User selection
    Selection,
    /// Command result
    CommandResult,
}

/// Output for clipboard.copy hook (mutable).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipboardCopyOutput {
    /// Modified content to copy
    pub content: String,
    /// Whether to allow the copy
    pub allow: bool,
    /// Hook result
    pub result: HookResult,
}

impl ClipboardCopyOutput {
    pub fn new(content: String) -> Self {
        Self {
            content,
            allow: true,
            result: HookResult::Continue,
        }
    }
}

/// Handler for clipboard.copy hook.
#[async_trait]
pub trait ClipboardCopyHook: Send + Sync {
    fn priority(&self) -> HookPriority {
        HookPriority::default()
    }

    async fn execute(
        &self,
        input: &ClipboardCopyInput,
        output: &mut ClipboardCopyOutput,
    ) -> Result<()>;
}

// ============================================================================
// Clipboard Paste Hook
// ============================================================================

/// Input for clipboard.paste hook.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipboardPasteInput {
    /// Session ID
    pub session_id: String,
    /// Content being pasted
    pub content: String,
}

/// Output for clipboard.paste hook (mutable).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipboardPasteOutput {
    /// Modified content to paste
    pub content: String,
    /// Whether to process as file content
    pub process_as_file: bool,
    /// Detected language for code
    pub language: Option<String>,
    /// Hook result
    pub result: HookResult,
}

impl ClipboardPasteOutput {
    pub fn new(content: String) -> Self {
        Self {
            content,
            process_as_file: false,
            language: None,
            result: HookResult::Continue,
        }
    }
}

/// Handler for clipboard.paste hook.
#[async_trait]
pub trait ClipboardPasteHook: Send + Sync {
    fn priority(&self) -> HookPriority {
        HookPriority::default()
    }

    async fn execute(
        &self,
        input: &ClipboardPasteInput,
        output: &mut ClipboardPasteOutput,
    ) -> Result<()>;
}
