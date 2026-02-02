//! Message compaction strategies for context management.

use serde::{Deserialize, Serialize};

use super::conversation::Conversation;
use crate::client::types::{Message, MessageRole};
use crate::error::Result;

/// Compaction strategy for reducing context size.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionStrategy {
    /// Strategy type.
    pub strategy: StrategyType,
    /// Target reduction ratio (0.0 - 1.0).
    pub target_ratio: f32,
    /// Preserve recent messages count.
    pub preserve_recent: usize,
    /// Preserve system messages.
    pub preserve_system: bool,
    /// Preserve tool calls and results.
    pub preserve_tools: bool,
    /// Maximum summary length.
    pub max_summary_length: usize,
}

impl Default for CompactionStrategy {
    fn default() -> Self {
        Self {
            strategy: StrategyType::Sliding,
            target_ratio: 0.5,
            preserve_recent: 10,
            preserve_system: true,
            preserve_tools: true,
            max_summary_length: 500,
        }
    }
}

/// Strategy type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StrategyType {
    /// Remove oldest messages.
    Sliding,
    /// Summarize old messages.
    Summarize,
    /// Remove by importance.
    Importance,
    /// Hybrid approach.
    Hybrid,
    /// Keep only recent turns.
    TurnBased,
    /// Custom compaction.
    Custom,
}

impl CompactionStrategy {
    /// Create a sliding window strategy.
    pub fn sliding(preserve_recent: usize) -> Self {
        Self {
            strategy: StrategyType::Sliding,
            preserve_recent,
            ..Self::default()
        }
    }

    /// Create a summarization strategy.
    pub fn summarize(max_summary_length: usize) -> Self {
        Self {
            strategy: StrategyType::Summarize,
            max_summary_length,
            ..Self::default()
        }
    }

    /// Create a turn-based strategy.
    pub fn turn_based(preserve_turns: usize) -> Self {
        Self {
            strategy: StrategyType::TurnBased,
            preserve_recent: preserve_turns * 2, // User + Assistant per turn
            ..Self::default()
        }
    }

    /// Compact a conversation.
    pub fn compact(&self, conversation: &mut Conversation) -> Result<()> {
        match self.strategy {
            StrategyType::Sliding => self.compact_sliding(conversation),
            StrategyType::Summarize => self.compact_summarize(conversation),
            StrategyType::Importance => self.compact_importance(conversation),
            StrategyType::Hybrid => self.compact_hybrid(conversation),
            StrategyType::TurnBased => self.compact_turn_based(conversation),
            StrategyType::Custom => Ok(()), // No-op for custom
        }
    }

    /// Sliding window compaction.
    fn compact_sliding(&self, conversation: &mut Conversation) -> Result<()> {
        let messages = conversation.messages_mut();
        let total = messages.len();

        if total <= self.preserve_recent {
            return Ok(());
        }

        // Find messages to keep
        let mut keep_indices: Vec<usize> = Vec::new();

        // Keep system messages if configured
        if self.preserve_system {
            for (i, msg) in messages.iter().enumerate() {
                if msg.role == MessageRole::System {
                    keep_indices.push(i);
                }
            }
        }

        // Keep recent messages
        let start_recent = total.saturating_sub(self.preserve_recent);
        for i in start_recent..total {
            if !keep_indices.contains(&i) {
                keep_indices.push(i);
            }
        }

        keep_indices.sort();

        // Create new message list
        let new_messages: Vec<Message> = keep_indices
            .into_iter()
            .filter_map(|i| messages.get(i).cloned())
            .collect();

        *messages = new_messages;
        Ok(())
    }

    /// Summarization compaction (placeholder - would need LLM call).
    fn compact_summarize(&self, conversation: &mut Conversation) -> Result<()> {
        let messages = conversation.messages_mut();
        let total = messages.len();

        if total <= self.preserve_recent + 1 {
            return Ok(());
        }

        // Calculate how many messages to summarize
        let summarize_count = total.saturating_sub(self.preserve_recent);
        if summarize_count == 0 {
            return Ok(());
        }

        // Extract messages to summarize
        let to_summarize: Vec<_> = messages.drain(..summarize_count).collect();

        // Create a simple summary (in real implementation, would use LLM)
        let summary = create_simple_summary(&to_summarize, self.max_summary_length);

        // Insert summary as system message at start
        messages.insert(
            0,
            Message::system(format!("[Conversation summary]\n{summary}")),
        );

        Ok(())
    }

    /// Importance-based compaction.
    fn compact_importance(&self, conversation: &mut Conversation) -> Result<()> {
        let messages = conversation.messages_mut();
        let total_len = messages.len();
        let capacity = messages.capacity();

        // Score each message by importance
        let mut scored: Vec<(usize, f32, Message)> = messages
            .drain(..)
            .enumerate()
            .map(|(i, msg)| {
                let score = calculate_importance(&msg, i, total_len);
                (i, score, msg)
            })
            .collect();

        // Sort by importance (descending)
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // Calculate target count
        let target_count = (capacity as f32 * self.target_ratio) as usize;
        let target_count = target_count.max(self.preserve_recent);

        // Keep top N by importance, then restore order
        scored.truncate(target_count);
        scored.sort_by_key(|(i, _, _)| *i);

        // Restore messages
        *messages = scored.into_iter().map(|(_, _, msg)| msg).collect();

        Ok(())
    }

    /// Hybrid compaction.
    fn compact_hybrid(&self, conversation: &mut Conversation) -> Result<()> {
        // First pass: summarize old messages
        self.compact_summarize(conversation)?;

        // Second pass: importance-based trimming if still too large
        if conversation.len() > self.preserve_recent * 2 {
            self.compact_importance(conversation)?;
        }

        Ok(())
    }

    /// Turn-based compaction.
    fn compact_turn_based(&self, conversation: &mut Conversation) -> Result<()> {
        let messages = conversation.messages_mut();

        // Group messages into turns
        let mut turns: Vec<Vec<Message>> = Vec::new();
        let mut current_turn: Vec<Message> = Vec::new();

        for msg in messages.drain(..) {
            if msg.role == MessageRole::User && !current_turn.is_empty() {
                turns.push(std::mem::take(&mut current_turn));
            }
            current_turn.push(msg);
        }
        if !current_turn.is_empty() {
            turns.push(current_turn);
        }

        // Keep recent turns
        let preserve_turns = self.preserve_recent / 2;
        let start = turns.len().saturating_sub(preserve_turns);

        // Reconstruct messages
        *messages = turns.into_iter().skip(start).flatten().collect();

        Ok(())
    }
}

/// Message compactor for more complex compaction operations.
#[derive(Debug)]
pub struct MessageCompactor {
    /// Strategies to apply in order.
    strategies: Vec<CompactionStrategy>,
    /// Target token count.
    target_tokens: u32,
    /// Minimum messages to keep.
    min_messages: usize,
}

impl MessageCompactor {
    /// Create a new compactor.
    pub fn new(target_tokens: u32) -> Self {
        Self {
            strategies: vec![CompactionStrategy::default()],
            target_tokens,
            min_messages: 2,
        }
    }

    /// Add a strategy.
    pub fn add_strategy(mut self, strategy: CompactionStrategy) -> Self {
        self.strategies.push(strategy);
        self
    }

    /// Set minimum messages.
    pub fn min_messages(mut self, min: usize) -> Self {
        self.min_messages = min;
        self
    }

    /// Compact until target is reached.
    pub fn compact(&self, conversation: &mut Conversation) -> Result<CompactionResult> {
        let initial_messages = conversation.len();
        let initial_tokens = conversation.token_count();

        for strategy in &self.strategies {
            if conversation.token_count() <= self.target_tokens {
                break;
            }
            if conversation.len() <= self.min_messages {
                break;
            }
            strategy.compact(conversation)?;
        }

        Ok(CompactionResult {
            messages_removed: initial_messages - conversation.len(),
            tokens_saved: initial_tokens - conversation.token_count(),
            final_messages: conversation.len(),
            final_tokens: conversation.token_count(),
        })
    }
}

/// Result of compaction.
#[derive(Debug, Clone, Serialize)]
pub struct CompactionResult {
    /// Messages removed.
    pub messages_removed: usize,
    /// Tokens saved.
    pub tokens_saved: u32,
    /// Final message count.
    pub final_messages: usize,
    /// Final token count.
    pub final_tokens: u32,
}

/// Calculate message importance score.
fn calculate_importance(message: &Message, index: usize, total: usize) -> f32 {
    let mut score = 0.0f32;

    // Base score by role
    score += match message.role {
        MessageRole::System => 10.0,
        MessageRole::User => 5.0,
        MessageRole::Assistant => 4.0,
        MessageRole::Tool => 3.0,
    };

    // Recency bonus
    let recency = index as f32 / total as f32;
    score += recency * 5.0;

    // Length penalty (very long messages get lower score)
    let content_len = message.content.as_text().map(str::len).unwrap_or(0);
    if content_len > 2000 {
        score -= 2.0;
    }

    // Tool results are important
    if message.tool_calls.is_some() {
        score += 3.0;
    }

    score
}

/// Create a simple summary of messages.
fn create_simple_summary(messages: &[Message], max_length: usize) -> String {
    let mut summary = String::new();

    for msg in messages {
        if let Some(text) = msg.content.as_text() {
            let role = match msg.role {
                MessageRole::User => "User",
                MessageRole::Assistant => "Assistant",
                MessageRole::System => "System",
                MessageRole::Tool => "Tool",
            };

            // Truncate long messages
            let text = if text.len() > 100 {
                format!("{}...", &text[..97])
            } else {
                text.to_string()
            };

            summary.push_str(&format!("{role}: {text}\n"));

            if summary.len() >= max_length {
                break;
            }
        }
    }

    if summary.len() > max_length {
        summary.truncate(max_length - 3);
        summary.push_str("...");
    }

    summary
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::conversation::ConversationBuilder;

    #[test]
    fn test_sliding_compaction() {
        let mut conv = ConversationBuilder::new()
            .user("Hello")
            .assistant("Hi")
            .user("How are you?")
            .assistant("Good!")
            .user("What's new?")
            .assistant("Nothing much")
            .build();

        let strategy = CompactionStrategy::sliding(4);
        strategy.compact(&mut conv).unwrap();

        assert_eq!(conv.len(), 4);
    }

    #[test]
    fn test_turn_based_compaction() {
        let mut conv = ConversationBuilder::new()
            .user("Turn 1 user")
            .assistant("Turn 1 assistant")
            .user("Turn 2 user")
            .assistant("Turn 2 assistant")
            .user("Turn 3 user")
            .assistant("Turn 3 assistant")
            .build();

        let mut strategy = CompactionStrategy::turn_based(2);
        strategy.preserve_recent = 4;
        strategy.compact(&mut conv).unwrap();

        assert!(conv.len() <= 4);
    }

    #[test]
    fn test_importance_calculation() {
        let msg = Message::system("Important");
        let score = calculate_importance(&msg, 0, 10);
        assert!(score > 0.0);
    }
}
