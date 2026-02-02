//! Context management for conversations and sessions.
//!
//! Provides:
//! - Conversation history management
//! - Context window optimization
//! - Token counting and truncation
//! - Message compaction strategies
//! - File context injection
//! - System prompt management

pub mod compaction;
pub mod conversation;
pub mod file_context;
pub mod system_prompt;
pub mod token_budget;

pub use compaction::{CompactionStrategy, MessageCompactor};
pub use conversation::{Conversation, ConversationBuilder};
pub use file_context::{FileContext, FileContextBuilder};
pub use system_prompt::{SystemPrompt, SystemPromptBuilder};
pub use token_budget::{TokenBudget, TokenBudgetManager};

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::client::types::Message;
use crate::error::Result;

/// Context manager for a session.
#[derive(Debug)]
pub struct ContextManager {
    /// Current conversation.
    conversation: RwLock<Conversation>,
    /// Token budget manager.
    token_budget: TokenBudgetManager,
    /// Compaction strategy.
    compaction: CompactionStrategy,
    /// File contexts.
    file_contexts: RwLock<HashMap<PathBuf, FileContext>>,
    /// System prompt.
    system_prompt: RwLock<SystemPrompt>,
    /// Configuration.
    config: ContextConfig,
}

/// Context configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextConfig {
    /// Maximum tokens in context.
    pub max_tokens: u32,
    /// Reserve tokens for output.
    pub output_reserve: u32,
    /// Compaction threshold (0.0 - 1.0).
    pub compaction_threshold: f32,
    /// Enable automatic compaction.
    pub auto_compact: bool,
    /// Maximum file context size.
    pub max_file_context: u32,
    /// Enable caching.
    pub cache_enabled: bool,
    /// System prompt priority.
    pub system_prompt_priority: Priority,
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            max_tokens: 128000,
            output_reserve: 16384,
            compaction_threshold: 0.8,
            auto_compact: true,
            max_file_context: 32000,
            cache_enabled: true,
            system_prompt_priority: Priority::High,
        }
    }
}

/// Priority level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Priority {
    Low,
    Medium,
    High,
    Critical,
}

impl ContextManager {
    /// Create a new context manager.
    pub fn new(config: ContextConfig) -> Self {
        Self {
            conversation: RwLock::new(Conversation::new()),
            token_budget: TokenBudgetManager::new(config.max_tokens, config.output_reserve),
            compaction: CompactionStrategy::default(),
            file_contexts: RwLock::new(HashMap::new()),
            system_prompt: RwLock::new(SystemPrompt::default()),
            config,
        }
    }

    /// Add a message to the conversation.
    pub async fn add_message(&self, message: Message) -> Result<()> {
        let mut conv = self.conversation.write().await;
        conv.add_message(message);

        // Check if compaction is needed
        if self.config.auto_compact {
            let usage = self.token_budget.current_usage();
            if usage > self.config.compaction_threshold {
                self.compact().await?;
            }
        }

        Ok(())
    }

    /// Get all messages for API call.
    pub async fn get_messages(&self) -> Vec<Message> {
        let conv = self.conversation.read().await;
        let system = self.system_prompt.read().await;

        let mut messages = Vec::new();

        // Add system prompt first
        if let Some(content) = system.render() {
            messages.push(Message::system(content));
        }

        // Add conversation messages
        messages.extend(conv.messages().cloned());

        messages
    }

    /// Compact the context.
    pub async fn compact(&self) -> Result<()> {
        let mut conv = self.conversation.write().await;
        self.compaction.compact(&mut conv)?;
        Ok(())
    }

    /// Add file context.
    pub async fn add_file_context(&self, path: PathBuf, context: FileContext) {
        let mut contexts = self.file_contexts.write().await;
        contexts.insert(path, context);
    }

    /// Remove file context.
    pub async fn remove_file_context(&self, path: &PathBuf) {
        let mut contexts = self.file_contexts.write().await;
        contexts.remove(path);
    }

    /// Get file contexts.
    pub async fn get_file_contexts(&self) -> HashMap<PathBuf, FileContext> {
        let contexts = self.file_contexts.read().await;
        contexts.clone()
    }

    /// Set system prompt.
    pub async fn set_system_prompt(&self, prompt: SystemPrompt) {
        let mut system = self.system_prompt.write().await;
        *system = prompt;
    }

    /// Get token usage statistics.
    pub async fn token_stats(&self) -> TokenStats {
        let conv = self.conversation.read().await;
        let system = self.system_prompt.read().await;
        let files = self.file_contexts.read().await;

        let system_tokens = system.token_count();
        let conversation_tokens = conv.token_count();
        let file_tokens: u32 = files
            .values()
            .map(file_context::FileContext::token_count)
            .sum();
        let total = system_tokens + conversation_tokens + file_tokens;

        TokenStats {
            system_tokens,
            conversation_tokens,
            file_tokens,
            total_tokens: total,
            max_tokens: self.config.max_tokens,
            available_tokens: self.config.max_tokens.saturating_sub(total),
            usage_percent: (total as f32 / self.config.max_tokens as f32) * 100.0,
        }
    }

    /// Clear conversation history.
    pub async fn clear(&self) {
        let mut conv = self.conversation.write().await;
        conv.clear();
    }

    /// Export conversation.
    pub async fn export(&self) -> ConversationExport {
        let conv = self.conversation.read().await;
        let system = self.system_prompt.read().await;

        ConversationExport {
            system_prompt: system.clone(),
            messages: conv.messages().cloned().collect(),
            metadata: conv.metadata().clone(),
        }
    }

    /// Import conversation.
    pub async fn import(&self, export: ConversationExport) -> Result<()> {
        let mut conv = self.conversation.write().await;
        let mut system = self.system_prompt.write().await;

        *system = export.system_prompt;
        conv.clear();
        for msg in export.messages {
            conv.add_message(msg);
        }
        *conv.metadata_mut() = export.metadata;

        Ok(())
    }
}

/// Token usage statistics.
#[derive(Debug, Clone, Serialize)]
pub struct TokenStats {
    /// Tokens used by system prompt.
    pub system_tokens: u32,
    /// Tokens used by conversation.
    pub conversation_tokens: u32,
    /// Tokens used by file contexts.
    pub file_tokens: u32,
    /// Total tokens used.
    pub total_tokens: u32,
    /// Maximum tokens allowed.
    pub max_tokens: u32,
    /// Available tokens.
    pub available_tokens: u32,
    /// Usage percentage.
    pub usage_percent: f32,
}

/// Conversation export format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationExport {
    /// System prompt.
    pub system_prompt: SystemPrompt,
    /// Messages.
    pub messages: Vec<Message>,
    /// Metadata.
    pub metadata: HashMap<String, serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_context_manager() {
        let manager = ContextManager::new(ContextConfig::default());

        manager.add_message(Message::user("Hello")).await.unwrap();
        manager
            .add_message(Message::assistant("Hi there!"))
            .await
            .unwrap();

        let messages = manager.get_messages().await;
        assert!(messages.len() >= 2);
    }

    #[tokio::test]
    async fn test_token_stats() {
        let manager = ContextManager::new(ContextConfig::default());

        let stats = manager.token_stats().await;
        assert_eq!(stats.max_tokens, 128000);
        assert!(stats.usage_percent >= 0.0);
    }
}
