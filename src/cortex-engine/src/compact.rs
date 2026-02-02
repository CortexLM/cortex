//! Context compaction utilities.
//!
//! Provides various strategies for compacting conversation context
//! to fit within token limits.

use serde::{Deserialize, Serialize};

/// Compaction strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum CompactionStrategy {
    /// Truncate oldest messages.
    TruncateOldest,
    /// Summarize old messages.
    Summarize,
    /// Remove tool results.
    RemoveToolResults,
    /// Sliding window.
    SlidingWindow,
    /// Smart compaction based on importance.
    #[default]
    Smart,
    /// Hybrid approach.
    Hybrid,
}

/// Compaction configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionConfig {
    /// Strategy to use.
    pub strategy: CompactionStrategy,
    /// Target token count.
    pub target_tokens: u32,
    /// Maximum tokens allowed.
    pub max_tokens: u32,
    /// Minimum messages to keep.
    pub min_messages: usize,
    /// Keep system message.
    pub keep_system: bool,
    /// Keep recent N messages.
    pub keep_recent: usize,
    /// Summarization model.
    pub summary_model: Option<String>,
    /// Summary max tokens.
    pub summary_max_tokens: u32,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            strategy: CompactionStrategy::Smart,
            target_tokens: 100000,
            max_tokens: 128000,
            min_messages: 4,
            keep_system: true,
            keep_recent: 10,
            summary_model: Some("gpt-4o-mini".to_string()),
            summary_max_tokens: 500,
        }
    }
}

/// Message for compaction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionMessage {
    /// Message role.
    pub role: String,
    /// Message content.
    pub content: String,
    /// Token count.
    pub tokens: u32,
    /// Is system message.
    pub is_system: bool,
    /// Is tool result.
    pub is_tool_result: bool,
    /// Importance score (0-1).
    pub importance: f32,
    /// Timestamp.
    pub timestamp: u64,
}

impl CompactionMessage {
    /// Create a new message.
    pub fn new(role: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            content: content.into(),
            tokens: 0,
            is_system: false,
            is_tool_result: false,
            importance: 0.5,
            timestamp: timestamp_now(),
        }
    }

    /// Set token count.
    pub fn with_tokens(mut self, tokens: u32) -> Self {
        self.tokens = tokens;
        self
    }

    /// Mark as system message.
    pub fn as_system(mut self) -> Self {
        self.is_system = true;
        self.importance = 1.0;
        self
    }

    /// Mark as tool result.
    pub fn as_tool_result(mut self) -> Self {
        self.is_tool_result = true;
        self.importance = 0.3;
        self
    }

    /// Set importance.
    pub fn with_importance(mut self, importance: f32) -> Self {
        self.importance = importance.clamp(0.0, 1.0);
        self
    }
}

/// Compaction result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionResult {
    /// Compacted messages.
    pub messages: Vec<CompactionMessage>,
    /// Tokens before.
    pub tokens_before: u32,
    /// Tokens after.
    pub tokens_after: u32,
    /// Messages removed.
    pub messages_removed: usize,
    /// Summary added.
    pub summary_added: bool,
    /// Strategy used.
    pub strategy: CompactionStrategy,
}

impl CompactionResult {
    /// Get tokens saved.
    pub fn tokens_saved(&self) -> u32 {
        self.tokens_before.saturating_sub(self.tokens_after)
    }

    /// Get compression ratio.
    pub fn compression_ratio(&self) -> f64 {
        if self.tokens_before > 0 {
            self.tokens_after as f64 / self.tokens_before as f64
        } else {
            1.0
        }
    }
}

/// Context compactor.
pub struct Compactor {
    /// Configuration.
    config: CompactionConfig,
}

impl Compactor {
    /// Create a new compactor.
    pub fn new(config: CompactionConfig) -> Self {
        Self { config }
    }

    /// Create with default config.
    pub fn default_compactor() -> Self {
        Self::new(CompactionConfig::default())
    }

    /// Check if compaction is needed.
    pub fn needs_compaction(&self, messages: &[CompactionMessage]) -> bool {
        let total_tokens: u32 = messages.iter().map(|m| m.tokens).sum();
        total_tokens > self.config.max_tokens
    }

    /// Compact messages.
    pub fn compact(&self, messages: Vec<CompactionMessage>) -> CompactionResult {
        let tokens_before: u32 = messages.iter().map(|m| m.tokens).sum();

        if tokens_before <= self.config.target_tokens {
            return CompactionResult {
                messages,
                tokens_before,
                tokens_after: tokens_before,
                messages_removed: 0,
                summary_added: false,
                strategy: self.config.strategy,
            };
        }

        match self.config.strategy {
            CompactionStrategy::TruncateOldest => self.truncate_oldest(messages, tokens_before),
            CompactionStrategy::SlidingWindow => self.sliding_window(messages, tokens_before),
            CompactionStrategy::RemoveToolResults => {
                self.remove_tool_results(messages, tokens_before)
            }
            CompactionStrategy::Smart => self.smart_compact(messages, tokens_before),
            CompactionStrategy::Summarize => self.summarize(messages, tokens_before),
            CompactionStrategy::Hybrid => self.hybrid_compact(messages, tokens_before),
        }
    }

    /// Truncate oldest messages.
    fn truncate_oldest(
        &self,
        messages: Vec<CompactionMessage>,
        tokens_before: u32,
    ) -> CompactionResult {
        let mut result = Vec::new();
        let mut current_tokens = 0u32;
        let mut removed = 0usize;

        // Keep system message
        if self.config.keep_system
            && let Some(sys_msg) = messages.iter().find(|m| m.is_system)
        {
            result.push(sys_msg.clone());
            current_tokens += sys_msg.tokens;
        }

        // Take from the end (most recent)
        let non_system: Vec<_> = messages.iter().filter(|m| !m.is_system).collect();

        for msg in non_system.iter().rev() {
            if current_tokens + msg.tokens <= self.config.target_tokens {
                result.insert(if result.is_empty() { 0 } else { 1 }, (*msg).clone());
                current_tokens += msg.tokens;
            } else {
                removed += 1;
            }
        }

        CompactionResult {
            messages: result,
            tokens_before,
            tokens_after: current_tokens,
            messages_removed: removed,
            summary_added: false,
            strategy: CompactionStrategy::TruncateOldest,
        }
    }

    /// Sliding window approach.
    fn sliding_window(
        &self,
        messages: Vec<CompactionMessage>,
        tokens_before: u32,
    ) -> CompactionResult {
        let mut result = Vec::new();
        let mut current_tokens = 0u32;

        // Keep system message
        if self.config.keep_system
            && let Some(sys_msg) = messages.iter().find(|m| m.is_system)
        {
            result.push(sys_msg.clone());
            current_tokens += sys_msg.tokens;
        }

        // Keep last N messages
        let non_system: Vec<_> = messages.iter().filter(|m| !m.is_system).collect();

        let keep_count = self.config.keep_recent.min(non_system.len());
        let start_idx = non_system.len().saturating_sub(keep_count);

        for msg in non_system.iter().skip(start_idx) {
            if current_tokens + msg.tokens <= self.config.target_tokens {
                result.push((*msg).clone());
                current_tokens += msg.tokens;
            }
        }

        let removed = messages.len() - result.len();

        CompactionResult {
            messages: result,
            tokens_before,
            tokens_after: current_tokens,
            messages_removed: removed,
            summary_added: false,
            strategy: CompactionStrategy::SlidingWindow,
        }
    }

    /// Remove tool results first.
    fn remove_tool_results(
        &self,
        messages: Vec<CompactionMessage>,
        tokens_before: u32,
    ) -> CompactionResult {
        let result: Vec<_> = messages
            .iter()
            .filter(|m| !m.is_tool_result)
            .cloned()
            .collect();

        let tokens_after: u32 = result.iter().map(|m| m.tokens).sum();
        let removed = messages.len() - result.len();

        // If still over limit, apply truncation
        if tokens_after > self.config.target_tokens {
            let truncate_result = self.truncate_oldest(result, tokens_after);
            return CompactionResult {
                messages: truncate_result.messages,
                tokens_before,
                tokens_after: truncate_result.tokens_after,
                messages_removed: removed + truncate_result.messages_removed,
                summary_added: false,
                strategy: CompactionStrategy::RemoveToolResults,
            };
        }

        CompactionResult {
            messages: result,
            tokens_before,
            tokens_after,
            messages_removed: removed,
            summary_added: false,
            strategy: CompactionStrategy::RemoveToolResults,
        }
    }

    /// Smart compaction based on importance.
    fn smart_compact(
        &self,
        mut messages: Vec<CompactionMessage>,
        tokens_before: u32,
    ) -> CompactionResult {
        // Sort by importance (keep high importance)
        messages.sort_by(|a, b| {
            b.importance
                .partial_cmp(&a.importance)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let mut result = Vec::new();
        let mut current_tokens = 0u32;
        let mut removed = 0usize;

        for msg in messages {
            if current_tokens + msg.tokens <= self.config.target_tokens || msg.is_system {
                current_tokens += msg.tokens;
                result.push(msg);
            } else {
                removed += 1;
            }
        }

        // Re-sort by timestamp
        result.sort_by_key(|m| m.timestamp);

        CompactionResult {
            messages: result,
            tokens_before,
            tokens_after: current_tokens,
            messages_removed: removed,
            summary_added: false,
            strategy: CompactionStrategy::Smart,
        }
    }

    /// Summarize old messages.
    fn summarize(&self, messages: Vec<CompactionMessage>, tokens_before: u32) -> CompactionResult {
        let mut result = Vec::new();
        let mut current_tokens = 0u32;

        // Keep system message
        if self.config.keep_system
            && let Some(sys_msg) = messages.iter().find(|m| m.is_system)
        {
            result.push(sys_msg.clone());
            current_tokens += sys_msg.tokens;
        }

        // Keep recent messages
        let non_system: Vec<_> = messages.iter().filter(|m| !m.is_system).collect();

        let keep_count = self.config.keep_recent.min(non_system.len());

        // Messages to summarize
        let to_summarize: Vec<_> = non_system
            .iter()
            .take(non_system.len().saturating_sub(keep_count))
            .cloned()
            .collect();

        // Create summary message (placeholder - would call model in real impl)
        if !to_summarize.is_empty() {
            let summary = create_summary_placeholder(&to_summarize);
            let summary_msg = CompactionMessage::new("system", &summary)
                .with_tokens(self.config.summary_max_tokens)
                .with_importance(0.8);
            result.push(summary_msg);
            current_tokens += self.config.summary_max_tokens;
        }

        // Add recent messages
        for msg in non_system
            .iter()
            .skip(non_system.len().saturating_sub(keep_count))
        {
            if current_tokens + msg.tokens <= self.config.target_tokens {
                result.push((*msg).clone());
                current_tokens += msg.tokens;
            }
        }

        let removed = messages.len() - result.len() + if to_summarize.is_empty() { 0 } else { 1 };

        CompactionResult {
            messages: result,
            tokens_before,
            tokens_after: current_tokens,
            messages_removed: removed,
            summary_added: !to_summarize.is_empty(),
            strategy: CompactionStrategy::Summarize,
        }
    }

    /// Hybrid compaction.
    fn hybrid_compact(
        &self,
        messages: Vec<CompactionMessage>,
        tokens_before: u32,
    ) -> CompactionResult {
        // First, remove tool results
        let after_tools = self.remove_tool_results(messages, tokens_before);

        if after_tools.tokens_after <= self.config.target_tokens {
            return after_tools;
        }

        // Then apply smart compaction
        self.smart_compact(after_tools.messages, after_tools.tokens_after)
    }
}

impl Default for Compactor {
    fn default() -> Self {
        Self::default_compactor()
    }
}

/// Create a placeholder summary.
fn create_summary_placeholder(messages: &[&CompactionMessage]) -> String {
    let count = messages.len();
    let roles: Vec<_> = messages.iter().map(|m| m.role.as_str()).collect();
    let user_count = roles.iter().filter(|r| **r == "user").count();
    let assistant_count = roles.iter().filter(|r| **r == "assistant").count();

    format!(
        "[Summary of {count} previous messages: {user_count} user messages, {assistant_count} assistant responses. \
        The conversation covered various topics and tasks.]"
    )
}

/// Get current timestamp.
fn timestamp_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Token counter trait.
pub trait TokenCounter: Send + Sync {
    /// Count tokens in text.
    fn count(&self, text: &str) -> u32;

    /// Count tokens in messages.
    fn count_messages(&self, messages: &[CompactionMessage]) -> u32 {
        messages.iter().map(|m| m.tokens).sum()
    }
}

/// Simple token counter (approximation).
pub struct SimpleTokenCounter {
    /// Chars per token (approximation).
    chars_per_token: f32,
}

impl SimpleTokenCounter {
    /// Create a new counter.
    pub fn new() -> Self {
        Self {
            chars_per_token: 4.0,
        }
    }
}

impl Default for SimpleTokenCounter {
    fn default() -> Self {
        Self::new()
    }
}

impl TokenCounter for SimpleTokenCounter {
    fn count(&self, text: &str) -> u32 {
        (text.len() as f32 / self.chars_per_token).ceil() as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_messages(count: usize, tokens_each: u32) -> Vec<CompactionMessage> {
        (0..count)
            .map(|i| {
                CompactionMessage::new(
                    if i % 2 == 0 { "user" } else { "assistant" },
                    format!("Message {}", i),
                )
                .with_tokens(tokens_each)
            })
            .collect()
    }

    #[test]
    fn test_no_compaction_needed() {
        let config = CompactionConfig {
            target_tokens: 1000,
            max_tokens: 1000,
            ..Default::default()
        };
        let compactor = Compactor::new(config);

        let messages = create_messages(5, 100);
        let result = compactor.compact(messages);

        assert_eq!(result.tokens_before, 500);
        assert_eq!(result.tokens_after, 500);
        assert_eq!(result.messages_removed, 0);
    }

    #[test]
    fn test_truncate_oldest() {
        let config = CompactionConfig {
            strategy: CompactionStrategy::TruncateOldest,
            target_tokens: 300,
            max_tokens: 300,
            keep_system: false,
            ..Default::default()
        };
        let compactor = Compactor::new(config);

        let messages = create_messages(10, 100);
        let result = compactor.compact(messages);

        assert!(result.tokens_after <= 300);
        assert!(result.messages_removed > 0);
    }

    #[test]
    fn test_sliding_window() {
        let config = CompactionConfig {
            strategy: CompactionStrategy::SlidingWindow,
            target_tokens: 500,
            max_tokens: 500,
            keep_recent: 5,
            keep_system: false,
            ..Default::default()
        };
        let compactor = Compactor::new(config);

        let messages = create_messages(10, 100);
        let result = compactor.compact(messages);

        assert_eq!(result.messages.len(), 5);
    }

    #[test]
    fn test_remove_tool_results() {
        let config = CompactionConfig {
            strategy: CompactionStrategy::RemoveToolResults,
            target_tokens: 500,
            max_tokens: 1000,
            keep_system: false,
            ..Default::default()
        };
        let compactor = Compactor::new(config);

        let mut messages = create_messages(5, 100);
        messages.push(
            CompactionMessage::new("tool", "Tool result")
                .as_tool_result()
                .with_tokens(200),
        );

        let result = compactor.compact(messages);

        assert!(!result.messages.iter().any(|m| m.is_tool_result));
    }

    #[test]
    fn test_compression_ratio() {
        let result = CompactionResult {
            messages: vec![],
            tokens_before: 1000,
            tokens_after: 500,
            messages_removed: 5,
            summary_added: false,
            strategy: CompactionStrategy::Smart,
        };

        assert_eq!(result.tokens_saved(), 500);
        assert!((result.compression_ratio() - 0.5).abs() < 0.01);
    }

    #[test]
    fn test_simple_token_counter() {
        let counter = SimpleTokenCounter::new();

        // ~4 chars per token
        assert_eq!(counter.count("Hello"), 2);
        assert_eq!(counter.count("Hello World!"), 3);
    }
}
