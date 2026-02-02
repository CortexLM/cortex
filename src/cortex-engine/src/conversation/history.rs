//! Conversation history.

use crate::client::{Message, MessageRole};

/// Entry in conversation history.
#[derive(Debug, Clone)]
pub struct HistoryEntry {
    pub message: Message,
    pub turn_id: u64,
}

/// Conversation history.
#[derive(Debug, Default)]
pub struct ConversationHistory {
    messages: Vec<Message>,
    current_turn: u64,
}

impl ConversationHistory {
    /// Create a new conversation history.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a message.
    pub fn add_message(&mut self, message: Message) {
        // Increment turn on user messages
        if message.role == MessageRole::User {
            self.current_turn += 1;
        }
        self.messages.push(message);
    }

    /// Get all messages.
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// Get message count.
    pub fn len(&self) -> usize {
        self.messages.len()
    }

    /// Check if empty.
    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    /// Clear history.
    pub fn clear(&mut self) {
        self.messages.clear();
        self.current_turn = 0;
    }

    /// Undo the last turn.
    pub fn undo(&mut self) -> bool {
        if self.messages.is_empty() {
            return false;
        }

        // Find the last user message
        let last_user_idx = self
            .messages
            .iter()
            .rposition(|m| m.role == MessageRole::User);

        if let Some(idx) = last_user_idx {
            // Remove everything from the last user message onwards
            self.messages.truncate(idx);
            self.current_turn = self.current_turn.saturating_sub(1);
            true
        } else {
            false
        }
    }

    /// Get the last assistant message.
    pub fn last_assistant_message(&self) -> Option<&Message> {
        self.messages
            .iter()
            .rev()
            .find(|m| m.role == MessageRole::Assistant)
    }

    /// Get the last user message.
    pub fn last_user_message(&self) -> Option<&Message> {
        self.messages
            .iter()
            .rev()
            .find(|m| m.role == MessageRole::User)
    }

    /// Get current turn number.
    pub fn current_turn(&self) -> u64 {
        self.current_turn
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_history() {
        let mut history = ConversationHistory::new();

        history.add_message(Message::system("You are a helpful assistant."));
        history.add_message(Message::user("Hello!"));
        history.add_message(Message::assistant("Hi there!"));

        assert_eq!(history.len(), 3);
        assert_eq!(history.current_turn(), 1);
    }

    #[test]
    fn test_undo() {
        let mut history = ConversationHistory::new();

        history.add_message(Message::user("First"));
        history.add_message(Message::assistant("Response 1"));
        history.add_message(Message::user("Second"));
        history.add_message(Message::assistant("Response 2"));

        assert!(history.undo());
        assert_eq!(history.len(), 2);
        assert_eq!(history.current_turn(), 1);
    }
}
