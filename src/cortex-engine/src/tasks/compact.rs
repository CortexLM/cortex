//! Context compaction task.
//!
//! Handles compacting conversation context to stay within token limits.

use serde::{Deserialize, Serialize};

use super::{TaskMeta, TaskType};

/// Compaction strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum CompactionStrategy {
    /// Summarize older messages.
    #[default]
    Summarize,
    /// Drop older messages.
    Truncate,
    /// Keep only important messages.
    SelectImportant,
    /// Combine similar messages.
    Merge,
    /// Use sliding window.
    SlidingWindow,
    /// Hybrid approach.
    Hybrid,
}

/// Compact task for context compaction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactTask {
    /// Task metadata.
    pub meta: TaskMeta,
    /// Current context.
    pub context: Vec<ContextMessage>,
    /// Target token count.
    pub target_tokens: u32,
    /// Current token count.
    pub current_tokens: u32,
    /// Compaction strategy.
    pub strategy: CompactionStrategy,
    /// Preserve system messages.
    pub preserve_system: bool,
    /// Preserve tool results.
    pub preserve_tools: bool,
    /// Number of recent messages to always keep.
    pub keep_recent: usize,
}

impl CompactTask {
    /// Create a new compact task.
    pub fn new(id: impl Into<String>, context: Vec<ContextMessage>) -> Self {
        let current_tokens = context.iter().map(|m| m.tokens).sum();

        Self {
            meta: TaskMeta::new(id, TaskType::Compact),
            context,
            target_tokens: 8000,
            current_tokens,
            strategy: CompactionStrategy::Summarize,
            preserve_system: true,
            preserve_tools: true,
            keep_recent: 5,
        }
    }

    /// Set target token count.
    pub fn with_target(mut self, tokens: u32) -> Self {
        self.target_tokens = tokens;
        self
    }

    /// Set strategy.
    pub fn with_strategy(mut self, strategy: CompactionStrategy) -> Self {
        self.strategy = strategy;
        self
    }

    /// Set keep recent count.
    pub fn with_keep_recent(mut self, count: usize) -> Self {
        self.keep_recent = count;
        self
    }

    /// Calculate tokens to remove.
    pub fn tokens_to_remove(&self) -> u32 {
        self.current_tokens.saturating_sub(self.target_tokens)
    }

    /// Check if compaction is needed.
    pub fn needs_compaction(&self) -> bool {
        self.current_tokens > self.target_tokens
    }

    /// Execute compaction with truncate strategy.
    pub fn compact_truncate(&self) -> CompactResult {
        let mut kept = Vec::new();
        let mut removed = Vec::new();
        let mut tokens_kept = 0u32;

        let total = self.context.len();
        let keep_from = total.saturating_sub(self.keep_recent);

        for (i, msg) in self.context.iter().enumerate() {
            // Always keep system messages and recent messages
            let should_keep = (self.preserve_system && msg.role == MessageRole::System)
                || i >= keep_from
                || tokens_kept + msg.tokens <= self.target_tokens;

            if should_keep && tokens_kept + msg.tokens <= self.target_tokens {
                kept.push(msg.clone());
                tokens_kept += msg.tokens;
            } else {
                removed.push(msg.clone());
            }
        }

        CompactResult {
            original_tokens: self.current_tokens,
            final_tokens: tokens_kept,
            messages_kept: kept.len(),
            messages_removed: removed.len(),
            kept,
            removed,
            summary: None,
        }
    }

    /// Execute compaction with sliding window.
    pub fn compact_sliding_window(&self, window_size: usize) -> CompactResult {
        let mut kept = Vec::new();
        let mut removed = Vec::new();

        // Keep system messages
        for msg in &self.context {
            if self.preserve_system && msg.role == MessageRole::System {
                kept.push(msg.clone());
            }
        }

        // Keep last window_size messages
        let non_system: Vec<_> = self
            .context
            .iter()
            .filter(|m| m.role != MessageRole::System)
            .cloned()
            .collect();

        let window_start = non_system.len().saturating_sub(window_size);

        for (i, msg) in non_system.into_iter().enumerate() {
            if i >= window_start {
                kept.push(msg);
            } else {
                removed.push(msg);
            }
        }

        let tokens_kept = kept.iter().map(|m| m.tokens).sum();

        CompactResult {
            original_tokens: self.current_tokens,
            final_tokens: tokens_kept,
            messages_kept: kept.len(),
            messages_removed: removed.len(),
            kept,
            removed,
            summary: None,
        }
    }

    /// Create a summary prompt for summarization strategy.
    pub fn summarization_prompt(&self) -> String {
        let messages_to_summarize: Vec<_> = self
            .context
            .iter()
            .take(self.context.len().saturating_sub(self.keep_recent))
            .filter(|m| m.role != MessageRole::System)
            .collect();

        let mut prompt = String::from(
            "Summarize the following conversation history concisely, \
             preserving key decisions, code changes, and important context:\n\n",
        );

        for msg in messages_to_summarize {
            prompt.push_str(&format!("{}: {}\n\n", msg.role, msg.content));
        }

        prompt.push_str("\nProvide a concise summary that captures the essential information.");
        prompt
    }
}

/// Context message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextMessage {
    /// Message ID.
    pub id: String,
    /// Role.
    pub role: MessageRole,
    /// Content.
    pub content: String,
    /// Token count.
    pub tokens: u32,
    /// Timestamp.
    pub timestamp: u64,
    /// Importance score (0-1).
    pub importance: f32,
    /// Whether this is a tool result.
    pub is_tool_result: bool,
}

impl ContextMessage {
    /// Create a new message.
    pub fn new(
        id: impl Into<String>,
        role: MessageRole,
        content: impl Into<String>,
        tokens: u32,
    ) -> Self {
        Self {
            id: id.into(),
            role,
            content: content.into(),
            tokens,
            timestamp: timestamp_now(),
            importance: 0.5,
            is_tool_result: false,
        }
    }

    /// Set importance.
    pub fn with_importance(mut self, importance: f32) -> Self {
        self.importance = importance.clamp(0.0, 1.0);
        self
    }

    /// Mark as tool result.
    pub fn as_tool_result(mut self) -> Self {
        self.is_tool_result = true;
        self
    }
}

/// Message role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    System,
    User,
    Assistant,
    Tool,
}

impl std::fmt::Display for MessageRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::System => write!(f, "System"),
            Self::User => write!(f, "User"),
            Self::Assistant => write!(f, "Assistant"),
            Self::Tool => write!(f, "Tool"),
        }
    }
}

/// Compaction result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactResult {
    /// Original token count.
    pub original_tokens: u32,
    /// Final token count.
    pub final_tokens: u32,
    /// Number of messages kept.
    pub messages_kept: usize,
    /// Number of messages removed.
    pub messages_removed: usize,
    /// Kept messages.
    pub kept: Vec<ContextMessage>,
    /// Removed messages.
    pub removed: Vec<ContextMessage>,
    /// Summary if summarization was used.
    pub summary: Option<String>,
}

impl CompactResult {
    /// Get tokens saved.
    pub fn tokens_saved(&self) -> u32 {
        self.original_tokens.saturating_sub(self.final_tokens)
    }

    /// Get compression ratio.
    pub fn compression_ratio(&self) -> f32 {
        if self.original_tokens > 0 {
            self.final_tokens as f32 / self.original_tokens as f32
        } else {
            1.0
        }
    }

    /// With summary.
    pub fn with_summary(mut self, summary: impl Into<String>) -> Self {
        self.summary = Some(summary.into());
        self
    }
}

/// Get current timestamp.
fn timestamp_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_messages(count: usize, tokens_each: u32) -> Vec<ContextMessage> {
        (0..count)
            .map(|i| {
                ContextMessage::new(
                    format!("msg-{}", i),
                    if i == 0 {
                        MessageRole::System
                    } else {
                        MessageRole::User
                    },
                    format!("Message {}", i),
                    tokens_each,
                )
            })
            .collect()
    }

    #[test]
    fn test_compact_task() {
        let messages = create_messages(10, 100);
        let task = CompactTask::new("compact-1", messages)
            .with_target(500)
            .with_strategy(CompactionStrategy::Truncate);

        assert!(task.needs_compaction());
        assert_eq!(task.tokens_to_remove(), 500);
    }

    #[test]
    fn test_truncate_strategy() {
        let messages = create_messages(10, 100);
        let task = CompactTask::new("compact-1", messages)
            .with_target(500)
            .with_keep_recent(3);

        let result = task.compact_truncate();
        assert!(result.final_tokens <= 500);
        assert!(result.messages_kept > 0);
    }

    #[test]
    fn test_sliding_window() {
        let messages = create_messages(10, 100);
        let task = CompactTask::new("compact-1", messages);

        let result = task.compact_sliding_window(5);
        assert!(result.messages_kept <= 6); // 5 + system
    }

    #[test]
    fn test_compression_ratio() {
        let result = CompactResult {
            original_tokens: 1000,
            final_tokens: 500,
            messages_kept: 5,
            messages_removed: 5,
            kept: Vec::new(),
            removed: Vec::new(),
            summary: None,
        };

        assert_eq!(result.tokens_saved(), 500);
        assert!((result.compression_ratio() - 0.5).abs() < 0.001);
    }
}
