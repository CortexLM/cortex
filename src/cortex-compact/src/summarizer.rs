//! Summarization logic for compaction.

use crate::{CompactionConfig, SUMMARY_PREFIX};
use serde::{Deserialize, Serialize};

/// A summary of conversation history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Summary {
    /// The summary text.
    pub text: String,
    /// Original token count.
    pub original_tokens: usize,
    /// Summary token count.
    pub summary_tokens: usize,
    /// Turns summarized.
    pub turns_summarized: usize,
}

impl Summary {
    pub fn new(
        text: impl Into<String>,
        original_tokens: usize,
        summary_tokens: usize,
        turns: usize,
    ) -> Self {
        Self {
            text: text.into(),
            original_tokens,
            summary_tokens,
            turns_summarized: turns,
        }
    }

    /// Format summary for insertion into conversation.
    pub fn formatted(&self) -> String {
        format!("{}{}", SUMMARY_PREFIX, self.text)
    }

    /// Compression ratio achieved.
    pub fn compression_ratio(&self) -> f32 {
        if self.original_tokens == 0 {
            return 1.0;
        }
        self.summary_tokens as f32 / self.original_tokens as f32
    }
}

/// Summarizer for conversation history.
pub struct Summarizer {
    config: CompactionConfig,
}

impl Summarizer {
    pub fn new(config: CompactionConfig) -> Self {
        Self { config }
    }

    /// Build the compaction prompt.
    pub fn build_prompt(&self, history: &[ConversationItem]) -> String {
        let mut prompt = String::new();

        prompt.push_str("# Conversation History to Summarize\n\n");

        for item in history {
            match item {
                ConversationItem::User { text } => {
                    prompt.push_str(&format!(
                        "**User**: {}\n\n",
                        truncate_text(text, self.config.max_user_message_tokens)
                    ));
                }
                ConversationItem::Assistant { text } => {
                    prompt.push_str(&format!("**Assistant**: {}\n\n", text));
                }
                ConversationItem::ToolUse { name, result } => {
                    if self.config.compact_tool_outputs {
                        prompt.push_str(&format!("**Tool ({})**: [output truncated]\n\n", name));
                    } else {
                        prompt.push_str(&format!(
                            "**Tool ({})**: {}\n\n",
                            name,
                            truncate_text(result, 500)
                        ));
                    }
                }
            }
        }

        prompt.push_str("\n---\n\n");
        prompt.push_str(crate::COMPACTION_PROMPT);

        prompt
    }

    /// Estimate tokens in text (rough approximation).
    pub fn estimate_tokens(&self, text: &str) -> usize {
        // Rough estimate: ~4 characters per token
        text.len() / 4
    }

    /// Select items to compact (preserve recent turns).
    pub fn select_items_to_compact<'a>(
        &self,
        items: &'a [ConversationItem],
    ) -> &'a [ConversationItem] {
        let preserve = self.config.preserve_recent_turns * 2; // Each turn has user + assistant
        if items.len() <= preserve {
            return &[];
        }
        &items[..items.len() - preserve]
    }
}

/// Item in conversation history.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ConversationItem {
    User { text: String },
    Assistant { text: String },
    ToolUse { name: String, result: String },
}

/// Truncate text to approximate token count.
fn truncate_text(text: &str, max_tokens: usize) -> String {
    let max_chars = max_tokens * 4;
    if text.len() <= max_chars {
        text.to_string()
    } else {
        format!("{}...[truncated]", &text[..max_chars])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_summary() {
        let summary = Summary::new("Test summary", 1000, 100, 5);
        assert_eq!(summary.compression_ratio(), 0.1);
    }

    #[test]
    fn test_summarizer() {
        let config = CompactionConfig::default();
        let summarizer = Summarizer::new(config);

        let items = vec![
            ConversationItem::User {
                text: "Hello".to_string(),
            },
            ConversationItem::Assistant {
                text: "Hi there!".to_string(),
            },
        ];

        let prompt = summarizer.build_prompt(&items);
        assert!(prompt.contains("Hello"));
        assert!(prompt.contains("Hi there"));
    }
}
