//! Specialized summarization strategy for conversation history.
//!
//! Provides a system prompt and logic for condensing message history
//! while preserving key decisions and state changes.

use crate::client::{Message, MessageRole};

/// The specialized summarization system prompt.
///
/// Guiding the model to preserve key decisions, state changes, and
/// important context for context maintenance.
pub const SUMMARIZATION_SYSTEM_PROMPT: &str = r#"You are a specialized summarization assistant. Your task is to condense the provided conversation history into a concise summary that preserves key decisions, state changes, and important context for maintaining the conversation's continuity.

Focus on:
1. Key decisions made by the user or assistant.
2. Major state changes (e.g., files created/modified, commands executed).
3. Important context that would be needed for future turns.
4. Core topics and goals discussed.

Avoid:
1. Verbatim repetition of long messages.
2. Unnecessary conversational filler.
3. Minor details that don't affect the overall progress.

The summary should be structured and easy to read, ensuring that an AI agent reading it can perfectly understand the current state of the task."#;

/// Strategy for summarizing conversation history.
#[derive(Debug, Clone)]
pub struct SummarizationStrategy {
    /// Number of recent messages to keep unsummarized.
    pub keep_recent: usize,
    /// Whether to preserve the system prompt.
    pub preserve_system: bool,
    /// Target token count for the summary.
    pub target_summary_tokens: usize,
}

impl Default for SummarizationStrategy {
    fn default() -> Self {
        Self {
            keep_recent: 10,
            preserve_system: true,
            target_summary_tokens: 500,
        }
    }
}

impl SummarizationStrategy {
    /// Create a new summarization strategy.
    pub fn new(keep_recent: usize, preserve_system: bool, target_summary_tokens: usize) -> Self {
        Self {
            keep_recent,
            preserve_system,
            target_summary_tokens,
        }
    }

    /// Identify messages that should be summarized.
    /// Returns a tuple of (messages_to_summarize, messages_to_keep).
    pub fn split_messages(&self, messages: &[Message]) -> (Vec<Message>, Vec<Message>) {
        if messages.is_empty() {
            return (Vec::new(), Vec::new());
        }

        let mut messages_to_summarize = Vec::new();
        let mut messages_to_keep = Vec::new();

        let mut start_idx = 0;
        if self.preserve_system && messages[0].role == MessageRole::System {
            messages_to_keep.push(messages[0].clone());
            start_idx = 1;
        }

        let remaining = &messages[start_idx..];
        let split_point = remaining.len().saturating_sub(self.keep_recent);

        messages_to_summarize.extend(remaining[..split_point].iter().cloned());
        messages_to_keep.extend(remaining[split_point..].iter().cloned());

        (messages_to_summarize, messages_to_keep)
    }

    /// Build the prompt for the summarization model.
    pub fn build_summarization_prompt(&self, messages_to_summarize: &[Message]) -> Vec<Message> {
        let mut prompt_messages = Vec::new();
        prompt_messages.push(Message::system(SUMMARIZATION_SYSTEM_PROMPT));

        let mut history_text = String::new();
        for msg in messages_to_summarize {
            let role = match msg.role {
                MessageRole::User => "User",
                MessageRole::Assistant => "Assistant",
                MessageRole::System => "System",
                MessageRole::Tool => "Tool",
            };
            let content = msg.content.as_text().unwrap_or("[Non-text content]");
            history_text.push_str(&format!("{}: {}\n\n", role, content));
        }

        prompt_messages.push(Message::user(format!(
            "Please summarize the following conversation history, focusing on key decisions and state changes:\n\n{}",
            history_text
        )));

        prompt_messages
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::{Message, MessageRole};

    #[test]
    fn test_split_messages() {
        let strategy = SummarizationStrategy::new(2, true, 500);
        let messages = vec![
            Message::system("System prompt"),
            Message::user("Message 1"),
            Message::assistant("Response 1"),
            Message::user("Message 2"),
            Message::assistant("Response 2"),
        ];

        let (to_summarize, to_keep) = strategy.split_messages(&messages);

        assert_eq!(to_summarize.len(), 2);
        assert_eq!(to_keep.len(), 3); // system + 2 recent

        assert_eq!(to_summarize[0].content.as_text().unwrap(), "Message 1");
        assert_eq!(to_keep[0].role, MessageRole::System);
        assert_eq!(to_keep[1].content.as_text().unwrap(), "Message 2");
    }

    #[test]
    fn test_build_summarization_prompt() {
        let strategy = SummarizationStrategy::default();
        let messages = vec![
            Message::user("I want to create a file"),
            Message::assistant("I created the file"),
        ];

        let prompt = strategy.build_summarization_prompt(&messages);

        assert_eq!(prompt.len(), 2);
        assert_eq!(prompt[0].role, MessageRole::System);
        assert!(
            prompt[0]
                .content
                .as_text()
                .unwrap()
                .contains("specialized summarization assistant")
        );
        assert!(
            prompt[1]
                .content
                .as_text()
                .unwrap()
                .contains("I want to create a file")
        );
    }
}
