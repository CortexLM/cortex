//! Prompt injection hooks for modifying prompts before AI processing.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use super::types::{HookPriority, HookResult};
use crate::Result;

/// Input for prompt.inject hook - allows modifying prompts before AI processing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptInjectInput {
    /// Session ID
    pub session_id: String,
    /// Current agent
    pub agent: Option<String>,
    /// Current model
    pub model: Option<String>,
    /// User's original message
    pub user_message: String,
    /// Conversation history length
    pub history_length: usize,
    /// Current working directory
    pub cwd: Option<PathBuf>,
}

/// Output for prompt.inject hook (mutable).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptInjectOutput {
    /// System prompt to prepend
    pub system_prepend: Option<String>,
    /// System prompt to append
    pub system_append: Option<String>,
    /// Context to inject before user message
    pub context_prepend: Option<String>,
    /// Context to inject after user message
    pub context_append: Option<String>,
    /// Modified user message
    pub user_message: String,
    /// Extra context documents
    pub context_documents: Vec<ContextDocument>,
    /// Hook result
    pub result: HookResult,
}

impl PromptInjectOutput {
    /// Create a new output with the original user message.
    pub fn new(user_message: String) -> Self {
        Self {
            system_prepend: None,
            system_append: None,
            context_prepend: None,
            context_append: None,
            user_message,
            context_documents: Vec::new(),
            result: HookResult::Continue,
        }
    }

    /// Add a context document.
    pub fn add_context(&mut self, doc: ContextDocument) {
        self.context_documents.push(doc);
    }
}

/// Context document for prompt injection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextDocument {
    /// Document title
    pub title: String,
    /// Document content
    pub content: String,
    /// Source (file path, URL, etc.)
    pub source: Option<String>,
    /// Document type (code, docs, data, etc.)
    pub doc_type: ContextDocumentType,
    /// Priority (higher = more important)
    pub priority: i32,
}

/// Context document types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextDocumentType {
    /// Source code
    Code,
    /// Documentation
    Documentation,
    /// Data (JSON, CSV, etc.)
    Data,
    /// Configuration
    Config,
    /// Reference material
    Reference,
    /// Custom type
    Custom,
}

/// Handler for prompt.inject hook.
#[async_trait]
pub trait PromptInjectHook: Send + Sync {
    /// Get the priority of this hook.
    fn priority(&self) -> HookPriority {
        HookPriority::default()
    }

    /// Execute the hook.
    async fn execute(
        &self,
        input: &PromptInjectInput,
        output: &mut PromptInjectOutput,
    ) -> Result<()>;
}
