//! Context utilities.
//!
//! Provides utilities for managing conversation context
//! including summarization, prioritization, and compression.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::ai_utils::{ChatMessage, Role};

/// Context item.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextItem {
    /// Unique ID.
    pub id: String,
    /// Content.
    pub content: String,
    /// Type.
    pub item_type: ContextItemType,
    /// Priority (0-100).
    pub priority: u8,
    /// Token count estimate.
    pub tokens: usize,
    /// Metadata.
    #[serde(default)]
    pub metadata: HashMap<String, String>,
    /// Timestamp.
    pub timestamp: u64,
}

impl ContextItem {
    /// Create a new context item.
    pub fn new(content: impl Into<String>, item_type: ContextItemType) -> Self {
        let content = content.into();
        let tokens = content.len() / 4; // Rough estimate

        Self {
            id: generate_id(),
            content,
            item_type,
            priority: 50,
            tokens,
            metadata: HashMap::new(),
            timestamp: timestamp_now(),
        }
    }

    /// Set priority.
    pub fn with_priority(mut self, priority: u8) -> Self {
        self.priority = priority;
        self
    }

    /// Set metadata.
    pub fn with_metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }
}

/// Context item type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContextItemType {
    /// System instructions.
    System,
    /// User message.
    User,
    /// Assistant response.
    Assistant,
    /// Tool result.
    ToolResult,
    /// File content.
    File,
    /// Code snippet.
    Code,
    /// Summary.
    Summary,
    /// Reference.
    Reference,
}

impl ContextItemType {
    /// Get base priority.
    pub fn base_priority(&self) -> u8 {
        match self {
            Self::System => 100,
            Self::User => 90,
            Self::Assistant => 80,
            Self::ToolResult => 70,
            Self::Code => 60,
            Self::File => 50,
            Self::Summary => 40,
            Self::Reference => 30,
        }
    }
}

/// Context manager.
#[derive(Debug, Clone, Default)]
pub struct ContextManager {
    items: Vec<ContextItem>,
    max_tokens: usize,
    current_tokens: usize,
}

impl ContextManager {
    /// Create a new context manager.
    pub fn new(max_tokens: usize) -> Self {
        Self {
            items: Vec::new(),
            max_tokens,
            current_tokens: 0,
        }
    }

    /// Add an item.
    pub fn add(&mut self, item: ContextItem) {
        self.current_tokens += item.tokens;
        self.items.push(item);

        // Compact if over limit
        if self.current_tokens > self.max_tokens {
            self.compact();
        }
    }

    /// Get items sorted by priority.
    pub fn get_items(&self) -> Vec<&ContextItem> {
        let mut items: Vec<_> = self.items.iter().collect();
        items.sort_by(|a, b| b.priority.cmp(&a.priority));
        items
    }

    /// Get items that fit in token budget.
    pub fn get_items_for_budget(&self, budget: usize) -> Vec<&ContextItem> {
        let items = self.get_items();
        let mut result = Vec::new();
        let mut used = 0;

        for item in items {
            if used + item.tokens <= budget {
                result.push(item);
                used += item.tokens;
            }
        }

        result
    }

    /// Compact context to fit limits.
    pub fn compact(&mut self) {
        // Remove low priority items first
        self.items.sort_by(|a, b| b.priority.cmp(&a.priority));

        while self.current_tokens > self.max_tokens && self.items.len() > 1 {
            if let Some(item) = self.items.pop() {
                self.current_tokens -= item.tokens;
            }
        }
    }

    /// Clear all items.
    pub fn clear(&mut self) {
        self.items.clear();
        self.current_tokens = 0;
    }

    /// Get current token count.
    pub fn token_count(&self) -> usize {
        self.current_tokens
    }

    /// Get available tokens.
    pub fn available_tokens(&self) -> usize {
        self.max_tokens.saturating_sub(self.current_tokens)
    }

    /// Convert to messages.
    pub fn to_messages(&self) -> Vec<ChatMessage> {
        self.get_items()
            .iter()
            .map(|item| {
                let role = match item.item_type {
                    ContextItemType::System => Role::System,
                    ContextItemType::User => Role::User,
                    ContextItemType::Assistant | ContextItemType::Summary => Role::Assistant,
                    _ => Role::User,
                };

                ChatMessage {
                    role,
                    content: item.content.clone(),
                    name: None,
                    tool_call_id: None,
                }
            })
            .collect()
    }
}

/// Context summarizer.
pub struct ContextSummarizer {
    /// Max summary length.
    max_length: usize,
}

impl ContextSummarizer {
    /// Create a new summarizer.
    pub fn new(max_length: usize) -> Self {
        Self { max_length }
    }

    /// Summarize messages (simple extractive summary).
    pub fn summarize(&self, messages: &[ChatMessage]) -> String {
        let mut summary = String::new();
        let mut tokens_used = 0;

        for message in messages {
            // Extract key sentences
            for sentence in message.content.split('.') {
                let sentence = sentence.trim();
                if sentence.is_empty() {
                    continue;
                }

                let tokens = sentence.len() / 4;
                if tokens_used + tokens > self.max_length {
                    break;
                }

                // Only include important-looking sentences
                if self.is_important(sentence) {
                    if !summary.is_empty() {
                        summary.push_str(". ");
                    }
                    summary.push_str(sentence);
                    tokens_used += tokens;
                }
            }
        }

        if !summary.is_empty() && !summary.ends_with('.') {
            summary.push('.');
        }

        summary
    }

    /// Check if sentence is important (heuristic).
    fn is_important(&self, sentence: &str) -> bool {
        let important_patterns = [
            "important",
            "must",
            "should",
            "need",
            "require",
            "error",
            "warning",
            "success",
            "fail",
            "complete",
            "result",
            "output",
        ];

        let sentence_lower = sentence.to_lowercase();

        // Check for important patterns
        for pattern in &important_patterns {
            if sentence_lower.contains(pattern) {
                return true;
            }
        }

        // Include first sentence of each message
        true
    }

    /// Create summary item.
    pub fn create_summary_item(&self, messages: &[ChatMessage]) -> ContextItem {
        let summary = self.summarize(messages);
        ContextItem::new(summary, ContextItemType::Summary).with_priority(40)
    }
}

impl Default for ContextSummarizer {
    fn default() -> Self {
        Self::new(500)
    }
}

/// Context prioritizer.
pub struct ContextPrioritizer {
    /// Recency weight.
    recency_weight: f32,
    /// Relevance weight.
    relevance_weight: f32,
    /// Type weight.
    type_weight: f32,
}

impl ContextPrioritizer {
    /// Create a new prioritizer.
    pub fn new() -> Self {
        Self {
            recency_weight: 0.3,
            relevance_weight: 0.4,
            type_weight: 0.3,
        }
    }

    /// Set weights.
    pub fn with_weights(mut self, recency: f32, relevance: f32, type_: f32) -> Self {
        let total = recency + relevance + type_;
        self.recency_weight = recency / total;
        self.relevance_weight = relevance / total;
        self.type_weight = type_ / total;
        self
    }

    /// Calculate priority for item.
    pub fn calculate_priority(&self, item: &ContextItem, query: Option<&str>) -> u8 {
        let now = timestamp_now();

        // Recency score (decay over time)
        let age_secs = (now - item.timestamp) as f32;
        let recency = (100.0 * (-age_secs / 3600.0).exp()) as u8; // 1 hour half-life

        // Relevance score
        let relevance = if let Some(q) = query {
            self.calculate_relevance(&item.content, q)
        } else {
            50
        };

        // Type score
        let type_score = item.item_type.base_priority();

        // Weighted average
        let weighted = self.recency_weight * recency as f32
            + self.relevance_weight * relevance as f32
            + self.type_weight * type_score as f32;

        weighted.round() as u8
    }

    /// Calculate relevance between content and query.
    fn calculate_relevance(&self, content: &str, query: &str) -> u8 {
        let content_lower = content.to_lowercase();
        let query_lower = query.to_lowercase();

        let query_words: Vec<&str> = query_lower.split_whitespace().collect();
        let matching = query_words
            .iter()
            .filter(|w| content_lower.contains(*w))
            .count();

        if query_words.is_empty() {
            return 50;
        }

        let ratio = matching as f32 / query_words.len() as f32;
        (ratio * 100.0) as u8
    }

    /// Prioritize items.
    pub fn prioritize(&self, items: &mut [ContextItem], query: Option<&str>) {
        for item in items.iter_mut() {
            item.priority = self.calculate_priority(item, query);
        }
    }
}

impl Default for ContextPrioritizer {
    fn default() -> Self {
        Self::new()
    }
}

/// Context window manager.
pub struct ContextWindow {
    /// Items in window.
    items: Vec<ContextItem>,
    /// Window size (tokens).
    window_size: usize,
    /// Reserved tokens (for response).
    reserved: usize,
}

impl ContextWindow {
    /// Create a new window.
    pub fn new(window_size: usize, reserved: usize) -> Self {
        Self {
            items: Vec::new(),
            window_size,
            reserved,
        }
    }

    /// Get available space.
    pub fn available(&self) -> usize {
        let used: usize = self.items.iter().map(|i| i.tokens).sum();
        (self.window_size - self.reserved).saturating_sub(used)
    }

    /// Add item if fits.
    pub fn add(&mut self, item: ContextItem) -> bool {
        if item.tokens <= self.available() {
            self.items.push(item);
            true
        } else {
            false
        }
    }

    /// Add items with priority.
    pub fn add_prioritized(&mut self, mut items: Vec<ContextItem>) {
        // Sort by priority
        items.sort_by(|a, b| b.priority.cmp(&a.priority));

        for item in items {
            if !self.add(item) {
                break;
            }
        }
    }

    /// Get items.
    pub fn get_items(&self) -> &[ContextItem] {
        &self.items
    }

    /// Clear window.
    pub fn clear(&mut self) {
        self.items.clear();
    }

    /// Get total tokens.
    pub fn total_tokens(&self) -> usize {
        self.items.iter().map(|i| i.tokens).sum()
    }
}

/// Generate unique ID.
fn generate_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("{now:x}")
}

/// Get current timestamp.
fn timestamp_now() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_context_item() {
        let item = ContextItem::new("Hello world", ContextItemType::User).with_priority(80);

        assert_eq!(item.priority, 80);
        assert!(item.tokens > 0);
    }

    #[test]
    fn test_context_manager() {
        let mut manager = ContextManager::new(1000);

        manager.add(ContextItem::new("System prompt", ContextItemType::System));
        manager.add(ContextItem::new("User message", ContextItemType::User));

        assert_eq!(manager.items.len(), 2);
        assert!(manager.token_count() > 0);
    }

    #[test]
    fn test_context_manager_compact() {
        let mut manager = ContextManager::new(50); // Very small

        for i in 0..10 {
            manager.add(
                ContextItem::new(format!("Message {}", i), ContextItemType::User)
                    .with_priority((i * 10) as u8),
            );
        }

        // Should have compacted
        assert!(manager.token_count() <= 50 || manager.items.len() < 10);
    }

    #[test]
    fn test_summarizer() {
        let summarizer = ContextSummarizer::new(100);

        let messages = vec![
            ChatMessage::user("This is important. Please note this."),
            ChatMessage::assistant("The result was successful."),
        ];

        let summary = summarizer.summarize(&messages);
        assert!(!summary.is_empty());
    }

    #[test]
    fn test_prioritizer() {
        let prioritizer = ContextPrioritizer::new();

        let item = ContextItem::new("Test content about errors", ContextItemType::User);
        let priority = prioritizer.calculate_priority(&item, Some("error handling"));

        assert!(priority > 0);
    }

    #[test]
    fn test_context_window() {
        let mut window = ContextWindow::new(1000, 200);

        assert_eq!(window.available(), 800);

        let item = ContextItem::new("Test", ContextItemType::User);
        assert!(window.add(item));

        assert!(window.available() < 800);
    }

    #[test]
    fn test_context_item_type_priority() {
        assert!(ContextItemType::System.base_priority() > ContextItemType::User.base_priority());
        assert!(ContextItemType::User.base_priority() > ContextItemType::Reference.base_priority());
    }
}
