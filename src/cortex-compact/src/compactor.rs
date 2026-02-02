//! Compaction execution.

use crate::summarizer::ConversationItem;
use crate::{CompactionConfig, Result, Summarizer, Summary};
use serde::{Deserialize, Serialize};
use tracing::info;

/// Result of compaction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionResult {
    /// Whether compaction was performed.
    pub compacted: bool,
    /// The summary if compaction was performed.
    pub summary: Option<Summary>,
    /// Tokens before compaction.
    pub tokens_before: usize,
    /// Tokens after compaction.
    pub tokens_after: usize,
    /// Items removed.
    pub items_removed: usize,
}

impl CompactionResult {
    pub fn no_compaction(current_tokens: usize) -> Self {
        Self {
            compacted: false,
            summary: None,
            tokens_before: current_tokens,
            tokens_after: current_tokens,
            items_removed: 0,
        }
    }

    pub fn success(
        summary: Summary,
        tokens_before: usize,
        tokens_after: usize,
        items_removed: usize,
    ) -> Self {
        Self {
            compacted: true,
            summary: Some(summary),
            tokens_before,
            tokens_after,
            items_removed,
        }
    }

    /// Tokens saved by compaction.
    pub fn tokens_saved(&self) -> usize {
        self.tokens_before.saturating_sub(self.tokens_after)
    }
}

/// Compactor for conversation history.
pub struct Compactor {
    config: CompactionConfig,
    summarizer: Summarizer,
}

impl Compactor {
    pub fn new(config: CompactionConfig) -> Self {
        let summarizer = Summarizer::new(config.clone());
        Self { config, summarizer }
    }

    /// Check if compaction is needed.
    pub fn needs_compaction(&self, current_tokens: usize, max_tokens: usize) -> bool {
        self.config.should_compact(current_tokens, max_tokens)
    }

    /// Perform compaction on conversation history.
    /// Returns the items to keep and the compaction result.
    pub fn compact(
        &self,
        items: Vec<ConversationItem>,
        summary_text: String,
        current_tokens: usize,
    ) -> Result<(Vec<ConversationItem>, CompactionResult)> {
        let to_compact = self.summarizer.select_items_to_compact(&items);

        if to_compact.is_empty() {
            return Ok((items, CompactionResult::no_compaction(current_tokens)));
        }

        let items_removed = to_compact.len();
        let tokens_in_compacted: usize = to_compact
            .iter()
            .map(|i| self.summarizer.estimate_tokens(&item_text(i)))
            .sum();

        let summary_tokens = self.summarizer.estimate_tokens(&summary_text);

        let summary = Summary::new(
            summary_text,
            tokens_in_compacted,
            summary_tokens,
            items_removed / 2, // Approximate turns
        );

        // Build new items list: summary + preserved items
        let preserved_start =
            items.len() - (self.config.preserve_recent_turns * 2).min(items.len());
        let mut new_items = vec![ConversationItem::Assistant {
            text: summary.formatted(),
        }];
        new_items.extend(items.into_iter().skip(preserved_start));

        let tokens_after = current_tokens - tokens_in_compacted + summary_tokens;

        let result =
            CompactionResult::success(summary, current_tokens, tokens_after, items_removed);

        info!(
            "Compacted conversation: {} tokens -> {} tokens ({} items removed)",
            result.tokens_before, result.tokens_after, result.items_removed
        );

        Ok((new_items, result))
    }

    /// Build the prompt to send to the model for summarization.
    pub fn build_summarization_prompt(&self, items: &[ConversationItem]) -> String {
        let to_compact = self.summarizer.select_items_to_compact(items);
        self.summarizer.build_prompt(to_compact)
    }

    /// Get config reference.
    pub fn config(&self) -> &CompactionConfig {
        &self.config
    }
}

fn item_text(item: &ConversationItem) -> String {
    match item {
        ConversationItem::User { text } => text.clone(),
        ConversationItem::Assistant { text } => text.clone(),
        ConversationItem::ToolUse { name, result } => format!("{}: {}", name, result),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compactor() {
        let config = CompactionConfig::default();
        let compactor = Compactor::new(config);

        // Create some conversation items
        let items: Vec<ConversationItem> = (0..10)
            .flat_map(|i| {
                vec![
                    ConversationItem::User {
                        text: format!("User message {}", i),
                    },
                    ConversationItem::Assistant {
                        text: format!("Assistant response {}", i),
                    },
                ]
            })
            .collect();

        let prompt = compactor.build_summarization_prompt(&items);
        assert!(prompt.contains("User message"));
    }
}
