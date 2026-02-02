//! Context compaction for managing conversation length.
//!
//! When the conversation approaches the token limit, older messages
//! are summarized to make room for new interactions.

use crate::client::Message;

/// Configuration for context compaction.
#[derive(Debug, Clone)]
pub struct CompactionConfig {
    /// Maximum context window size in tokens.
    pub max_tokens: usize,
    /// Threshold at which to trigger compaction (percentage of max_tokens).
    pub threshold_percent: f32,
    /// Number of recent messages to always preserve.
    pub preserve_recent: usize,
    /// Whether to preserve the system prompt.
    pub preserve_system: bool,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            max_tokens: 128_000,
            threshold_percent: 0.8,
            preserve_recent: 10,
            preserve_system: true,
        }
    }
}

impl CompactionConfig {
    /// Get the token threshold for triggering compaction.
    pub fn threshold_tokens(&self) -> usize {
        (self.max_tokens as f32 * self.threshold_percent) as usize
    }
}

/// Result of compaction analysis.
#[derive(Debug)]
pub struct CompactionAnalysis {
    /// Whether compaction is needed.
    pub needs_compaction: bool,
    /// Current token count.
    pub current_tokens: usize,
    /// Number of messages that would be compacted.
    pub messages_to_compact: usize,
    /// Estimated tokens after compaction.
    pub estimated_tokens_after: usize,
}

/// Manages context compaction for a conversation.
pub struct ContextCompactor {
    config: CompactionConfig,
}

impl ContextCompactor {
    /// Create a new context compactor.
    pub fn new(config: CompactionConfig) -> Self {
        Self { config }
    }

    /// Create with default config and custom max tokens.
    pub fn with_max_tokens(max_tokens: usize) -> Self {
        Self {
            config: CompactionConfig {
                max_tokens,
                ..Default::default()
            },
        }
    }

    /// Analyze whether compaction is needed.
    pub fn analyze(&self, messages: &[Message], current_tokens: usize) -> CompactionAnalysis {
        let threshold = self.config.threshold_tokens();
        let needs_compaction = current_tokens >= threshold;

        let (messages_to_compact, estimated_reduction) = if needs_compaction {
            self.calculate_compaction_scope(messages, current_tokens)
        } else {
            (0, 0)
        };

        CompactionAnalysis {
            needs_compaction,
            current_tokens,
            messages_to_compact,
            estimated_tokens_after: current_tokens.saturating_sub(estimated_reduction),
        }
    }

    /// Calculate which messages to compact.
    fn calculate_compaction_scope(
        &self,
        messages: &[Message],
        current_tokens: usize,
    ) -> (usize, usize) {
        let total_messages = messages.len();

        // Skip system message if preserving
        let start_idx = if self.config.preserve_system && !messages.is_empty() {
            if messages[0].role == crate::client::MessageRole::System {
                1
            } else {
                0
            }
        } else {
            0
        };

        // Calculate how many messages we can compact
        let end_idx = total_messages.saturating_sub(self.config.preserve_recent);

        if end_idx <= start_idx {
            return (0, 0);
        }

        let messages_to_compact = end_idx - start_idx;

        // Estimate token reduction (rough: assume 70% reduction from summarization)
        let tokens_per_message = current_tokens / total_messages.max(1);
        let estimated_reduction = (messages_to_compact * tokens_per_message * 70) / 100;

        (messages_to_compact, estimated_reduction)
    }

    /// Perform compaction on messages.
    /// Returns the compacted messages and a summary of what was compacted.
    pub fn compact(&self, messages: Vec<Message>) -> (Vec<Message>, Option<String>) {
        let _total = messages.len();

        // Skip system message if present and we're preserving it
        let (system_msg, remaining) = if self.config.preserve_system && !messages.is_empty() {
            if messages[0].role == crate::client::MessageRole::System {
                (Some(messages[0].clone()), &messages[1..])
            } else {
                (None, &messages[..])
            }
        } else {
            (None, &messages[..])
        };

        // Calculate how many to keep from the end
        let keep_count = self.config.preserve_recent.min(remaining.len());

        if remaining.len() <= keep_count {
            // Nothing to compact
            return (messages, None);
        }

        // Split into compact and keep
        let compact_count = remaining.len() - keep_count;
        let to_compact = &remaining[..compact_count];
        let to_keep = &remaining[compact_count..];

        // Generate summary of compacted messages
        let summary = generate_summary(to_compact);

        // Build new message list
        let mut result = Vec::new();

        // Add system message if we had one
        if let Some(sys) = system_msg {
            result.push(sys);
        }

        // Add summary message
        result.push(Message::system(format!(
            "[Conversation summary - {compact_count} earlier messages compacted]\n\n{summary}"
        )));

        // Add preserved messages
        result.extend(to_keep.iter().cloned());

        let compaction_info = format!(
            "Compacted {} messages ({} remaining)",
            compact_count,
            result.len()
        );

        (result, Some(compaction_info))
    }
}

/// Generate a summary of messages for compaction.
fn generate_summary(messages: &[Message]) -> String {
    let mut summary_parts = Vec::new();

    // Track topics discussed
    let mut user_queries = Vec::new();
    let mut assistant_actions = Vec::new();
    let mut files_mentioned = std::collections::HashSet::new();

    for msg in messages {
        let content = msg.content.as_text().unwrap_or("");

        match msg.role {
            crate::client::MessageRole::User => {
                // Extract the first line as a query summary
                if let Some(first_line) = content.lines().next() {
                    let truncated = if first_line.len() > 100 {
                        format!("{}...", &first_line[..100])
                    } else {
                        first_line.to_string()
                    };
                    user_queries.push(truncated);
                }
            }
            crate::client::MessageRole::Assistant => {
                // Look for action patterns
                if content.contains("Created") || content.contains("created") {
                    assistant_actions.push("Created files".to_string());
                }
                if content.contains("Modified")
                    || content.contains("modified")
                    || content.contains("Updated")
                {
                    assistant_actions.push("Modified files".to_string());
                }
                if content.contains("Executed")
                    || content.contains("ran")
                    || content.contains("Running")
                {
                    assistant_actions.push("Executed commands".to_string());
                }

                // Extract file paths
                for word in content.split_whitespace() {
                    if word.contains('/')
                        && (word.ends_with(".rs")
                            || word.ends_with(".py")
                            || word.ends_with(".js")
                            || word.ends_with(".ts")
                            || word.ends_with(".toml")
                            || word.ends_with(".json")
                            || word.ends_with(".md"))
                    {
                        files_mentioned.insert(
                            word.trim_matches(|c: char| {
                                !c.is_alphanumeric() && c != '/' && c != '.' && c != '_' && c != '-'
                            })
                            .to_string(),
                        );
                    }
                }
            }
            _ => {}
        }
    }

    // Build summary
    if !user_queries.is_empty() {
        let queries: Vec<_> = user_queries.iter().take(5).collect();
        summary_parts.push(format!(
            "User asked about:\n{}",
            queries
                .iter()
                .map(|q| format!("  - {q}"))
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }

    if !assistant_actions.is_empty() {
        let unique_actions: std::collections::HashSet<_> = assistant_actions.into_iter().collect();
        summary_parts.push(format!(
            "Assistant performed:\n{}",
            unique_actions
                .iter()
                .map(|a| format!("  - {a}"))
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }

    if !files_mentioned.is_empty() {
        let files: Vec<_> = files_mentioned.iter().take(10).collect();
        summary_parts.push(format!(
            "Files involved:\n{}",
            files
                .iter()
                .map(|f| format!("  - {f}"))
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }

    if summary_parts.is_empty() {
        format!(
            "{} messages were exchanged covering various topics.",
            messages.len()
        )
    } else {
        summary_parts.join("\n\n")
    }
}

/// Estimate token count for a message (rough approximation).
pub fn estimate_tokens(text: &str) -> usize {
    // Rough estimate: ~4 characters per token for English text
    // This is a simplification; real tokenization is more complex
    text.len().div_ceil(4)
}

/// Estimate token count for a list of messages.
pub fn estimate_message_tokens(messages: &[Message]) -> usize {
    messages
        .iter()
        .map(|m| {
            let content_tokens = estimate_tokens(m.content.as_text().unwrap_or(""));
            // Add overhead for role, formatting, etc.
            content_tokens + 4
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compaction_config_threshold() {
        let config = CompactionConfig {
            max_tokens: 100_000,
            threshold_percent: 0.8,
            ..Default::default()
        };
        assert_eq!(config.threshold_tokens(), 80_000);
    }

    #[test]
    fn test_estimate_tokens() {
        assert_eq!(estimate_tokens("hello"), 2); // 5 chars -> ~2 tokens
        assert_eq!(estimate_tokens("hello world"), 3); // 11 chars -> ~3 tokens
    }

    #[test]
    fn test_compaction_not_needed_for_small_context() {
        let compactor = ContextCompactor::new(CompactionConfig::default());
        let messages = vec![
            Message::system("You are helpful"),
            Message::user("Hello"),
            Message::assistant("Hi there!"),
        ];

        let analysis = compactor.analyze(&messages, 100);
        assert!(!analysis.needs_compaction);
    }

    #[test]
    fn test_compaction_needed_for_large_context() {
        let config = CompactionConfig {
            max_tokens: 1000,
            threshold_percent: 0.8,
            preserve_recent: 2,
            preserve_system: true,
        };
        let compactor = ContextCompactor::new(config);

        let messages = vec![
            Message::system("System"),
            Message::user("Query 1"),
            Message::assistant("Response 1"),
            Message::user("Query 2"),
            Message::assistant("Response 2"),
            Message::user("Query 3"),
            Message::assistant("Response 3"),
        ];

        let analysis = compactor.analyze(&messages, 900);
        assert!(analysis.needs_compaction);
        assert!(analysis.messages_to_compact > 0);
    }

    #[test]
    fn test_compact_preserves_recent() {
        let config = CompactionConfig {
            max_tokens: 1000,
            threshold_percent: 0.8,
            preserve_recent: 2,
            preserve_system: true,
        };
        let compactor = ContextCompactor::new(config);

        let messages = vec![
            Message::system("System"),
            Message::user("Old query"),
            Message::assistant("Old response"),
            Message::user("Recent query"),
            Message::assistant("Recent response"),
        ];

        let (compacted, info) = compactor.compact(messages);

        // Should have: system + summary + 2 recent
        assert!(info.is_some());
        assert!(compacted.len() <= 4);
    }
}
