//! Conversation history management.

use std::collections::HashMap;

use serde::Serialize;

use crate::client::types::{Message, MessageContent, MessageRole};

/// A conversation with message history.
#[derive(Debug, Clone)]
pub struct Conversation {
    /// Messages in the conversation.
    messages: Vec<Message>,
    /// Conversation metadata.
    metadata: HashMap<String, serde_json::Value>,
    /// Total token count (estimated).
    token_count: u32,
    /// Turn count.
    turn_count: u32,
    /// Creation timestamp.
    created_at: std::time::Instant,
    /// Last update timestamp.
    updated_at: std::time::Instant,
}

impl Default for Conversation {
    fn default() -> Self {
        Self::new()
    }
}

impl Conversation {
    /// Create a new empty conversation.
    pub fn new() -> Self {
        let now = std::time::Instant::now();
        Self {
            messages: Vec::new(),
            metadata: HashMap::new(),
            token_count: 0,
            turn_count: 0,
            created_at: now,
            updated_at: now,
        }
    }

    /// Add a message to the conversation.
    pub fn add_message(&mut self, message: Message) {
        // Estimate tokens
        let tokens = estimate_tokens(&message);
        self.token_count += tokens;

        // Track turns
        if message.role == MessageRole::User {
            self.turn_count += 1;
        }

        self.messages.push(message);
        self.updated_at = std::time::Instant::now();
    }

    /// Get all messages.
    pub fn messages(&self) -> impl Iterator<Item = &Message> {
        self.messages.iter()
    }

    /// Get messages mutably.
    pub fn messages_mut(&mut self) -> &mut Vec<Message> {
        &mut self.messages
    }

    /// Get message count.
    pub fn len(&self) -> usize {
        self.messages.len()
    }

    /// Check if empty.
    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    /// Get total token count.
    pub fn token_count(&self) -> u32 {
        self.token_count
    }

    /// Get turn count.
    pub fn turn_count(&self) -> u32 {
        self.turn_count
    }

    /// Get metadata.
    pub fn metadata(&self) -> &HashMap<String, serde_json::Value> {
        &self.metadata
    }

    /// Get metadata mutably.
    pub fn metadata_mut(&mut self) -> &mut HashMap<String, serde_json::Value> {
        &mut self.metadata
    }

    /// Set metadata value.
    pub fn set_metadata(&mut self, key: impl Into<String>, value: serde_json::Value) {
        self.metadata.insert(key.into(), value);
    }

    /// Get metadata value.
    pub fn get_metadata(&self, key: &str) -> Option<&serde_json::Value> {
        self.metadata.get(key)
    }

    /// Clear conversation.
    pub fn clear(&mut self) {
        self.messages.clear();
        self.token_count = 0;
        self.turn_count = 0;
        self.updated_at = std::time::Instant::now();
    }

    /// Recompute turn count.
    pub fn recompute_turns(&mut self) {
        self.turn_count = self
            .messages
            .iter()
            .filter(|m| m.role == MessageRole::User)
            .count() as u32;
    }

    /// Get the last message.
    pub fn last_message(&self) -> Option<&Message> {
        self.messages.last()
    }

    /// Get the last user message.
    pub fn last_user_message(&self) -> Option<&Message> {
        self.messages
            .iter()
            .rev()
            .find(|m| m.role == MessageRole::User)
    }

    /// Get the last assistant message.
    pub fn last_assistant_message(&self) -> Option<&Message> {
        self.messages
            .iter()
            .rev()
            .find(|m| m.role == MessageRole::Assistant)
    }

    /// Get messages by role.
    pub fn messages_by_role(&self, role: MessageRole) -> Vec<&Message> {
        self.messages.iter().filter(|m| m.role == role).collect()
    }

    /// Get messages in range.
    pub fn messages_range(&self, start: usize, end: usize) -> &[Message] {
        let end = end.min(self.messages.len());
        let start = start.min(end);
        &self.messages[start..end]
    }

    /// Get the last N messages.
    pub fn last_n_messages(&self, n: usize) -> &[Message] {
        let start = self.messages.len().saturating_sub(n);
        &self.messages[start..]
    }

    /// Remove message at index.
    pub fn remove_message(&mut self, index: usize) -> Option<Message> {
        if index < self.messages.len() {
            let msg = self.messages.remove(index);
            self.token_count = self.token_count.saturating_sub(estimate_tokens(&msg));
            if msg.role == MessageRole::User {
                self.turn_count = self.turn_count.saturating_sub(1);
            }
            Some(msg)
        } else {
            None
        }
    }

    /// Insert message at index.
    pub fn insert_message(&mut self, index: usize, message: Message) {
        let tokens = estimate_tokens(&message);
        self.token_count += tokens;

        if message.role == MessageRole::User {
            self.turn_count += 1;
        }

        let index = index.min(self.messages.len());
        self.messages.insert(index, message);
        self.updated_at = std::time::Instant::now();
    }

    /// Truncate to N messages.
    pub fn truncate(&mut self, n: usize) {
        if n < self.messages.len() {
            let removed_tokens: u32 = self.messages[n..].iter().map(estimate_tokens).sum();
            let removed_turns: u32 = self.messages[n..]
                .iter()
                .filter(|m| m.role == MessageRole::User)
                .count() as u32;
            self.messages.truncate(n);
            self.token_count = self.token_count.saturating_sub(removed_tokens);
            self.turn_count = self.turn_count.saturating_sub(removed_turns);
            self.updated_at = std::time::Instant::now();
        }
    }

    /// Truncate to token limit.
    pub fn truncate_to_tokens(&mut self, max_tokens: u32) {
        while self.token_count > max_tokens && !self.messages.is_empty() {
            if let Some(msg) = self.messages.first() {
                let tokens = estimate_tokens(msg);
                if msg.role == MessageRole::User {
                    self.turn_count = self.turn_count.saturating_sub(1);
                }
                self.messages.remove(0);
                self.token_count = self.token_count.saturating_sub(tokens);
            }
        }
        self.updated_at = std::time::Instant::now();
    }

    /// Get duration since creation.
    pub fn duration(&self) -> std::time::Duration {
        self.created_at.elapsed()
    }

    /// Get duration since last update.
    pub fn idle_duration(&self) -> std::time::Duration {
        self.updated_at.elapsed()
    }

    /// Fork conversation (create a copy from a point).
    pub fn fork(&self, from_index: usize) -> Self {
        let messages: Vec<_> = self.messages[..from_index.min(self.messages.len())].to_vec();
        let token_count = messages.iter().map(estimate_tokens).sum();

        let now = std::time::Instant::now();
        Self {
            messages,
            metadata: self.metadata.clone(),
            token_count,
            turn_count: 0, // Reset for fork
            created_at: now,
            updated_at: now,
        }
    }

    /// Merge another conversation.
    pub fn merge(&mut self, other: &Conversation) {
        for msg in &other.messages {
            self.add_message(msg.clone());
        }
        for (key, value) in &other.metadata {
            self.metadata.insert(key.clone(), value.clone());
        }
    }

    /// Find messages containing text.
    pub fn search(&self, query: &str) -> Vec<(usize, &Message)> {
        self.messages
            .iter()
            .enumerate()
            .filter(|(_, m)| {
                m.content
                    .as_text()
                    .map(|t| t.contains(query))
                    .unwrap_or(false)
            })
            .collect()
    }

    /// Get summary statistics.
    pub fn stats(&self) -> ConversationStats {
        let user_messages = self
            .messages
            .iter()
            .filter(|m| m.role == MessageRole::User)
            .count();
        let assistant_messages = self
            .messages
            .iter()
            .filter(|m| m.role == MessageRole::Assistant)
            .count();
        let tool_messages = self
            .messages
            .iter()
            .filter(|m| m.role == MessageRole::Tool)
            .count();
        let system_messages = self
            .messages
            .iter()
            .filter(|m| m.role == MessageRole::System)
            .count();

        ConversationStats {
            total_messages: self.messages.len(),
            user_messages,
            assistant_messages,
            tool_messages,
            system_messages,
            total_tokens: self.token_count,
            turn_count: self.turn_count,
            duration_secs: self.duration().as_secs(),
        }
    }
}

/// Conversation statistics.
#[derive(Debug, Clone, Serialize)]
pub struct ConversationStats {
    /// Total messages.
    pub total_messages: usize,
    /// User messages.
    pub user_messages: usize,
    /// Assistant messages.
    pub assistant_messages: usize,
    /// Tool messages.
    pub tool_messages: usize,
    /// System messages.
    pub system_messages: usize,
    /// Total tokens.
    pub total_tokens: u32,
    /// Turn count.
    pub turn_count: u32,
    /// Duration in seconds.
    pub duration_secs: u64,
}

/// Builder for creating conversations.
#[derive(Debug, Default)]
pub struct ConversationBuilder {
    messages: Vec<Message>,
    metadata: HashMap<String, serde_json::Value>,
}

impl ConversationBuilder {
    /// Create a new builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a system message.
    pub fn system(mut self, content: impl Into<String>) -> Self {
        self.messages.push(Message::system(content));
        self
    }

    /// Add a user message.
    pub fn user(mut self, content: impl Into<String>) -> Self {
        self.messages.push(Message::user(content));
        self
    }

    /// Add an assistant message.
    pub fn assistant(mut self, content: impl Into<String>) -> Self {
        self.messages.push(Message::assistant(content));
        self
    }

    /// Add a message.
    pub fn message(mut self, message: Message) -> Self {
        self.messages.push(message);
        self
    }

    /// Add metadata.
    pub fn metadata(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.metadata.insert(key.into(), value);
        self
    }

    /// Build the conversation.
    pub fn build(self) -> Conversation {
        let mut conv = Conversation::new();
        for msg in self.messages {
            conv.add_message(msg);
        }
        conv.metadata = self.metadata;
        conv
    }
}

/// Estimate token count for a message.
fn estimate_tokens(message: &Message) -> u32 {
    let text = match &message.content {
        MessageContent::Text(s) => s.as_str(),
        MessageContent::Parts(parts) => {
            // Just count text parts for now
            return parts
                .iter()
                .filter_map(|p| {
                    match p {
                        crate::client::types::ContentPart::Text { text, .. } => {
                            Some(text.len() as u32 / 4)
                        }
                        _ => Some(100), // Estimate for images/documents
                    }
                })
                .sum();
        }
        MessageContent::ToolResult { content, .. } => content.as_str(),
        MessageContent::ToolCalls(calls) => {
            return calls.len() as u32 * 50; // Rough estimate
        }
    };

    // Rough estimate: ~4 characters per token
    (text.len() as u32 / 4) + 4 // +4 for role overhead
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_conversation_builder() {
        let conv = ConversationBuilder::new()
            .system("You are helpful")
            .user("Hello")
            .assistant("Hi!")
            .build();

        assert_eq!(conv.len(), 3);
    }

    #[test]
    fn test_conversation_search() {
        let mut conv = Conversation::new();
        conv.add_message(Message::user("Hello world"));
        conv.add_message(Message::assistant("Hi there!"));
        conv.add_message(Message::user("world peace"));

        let results = conv.search("world");
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_conversation_truncate() {
        let mut conv = Conversation::new();
        for i in 0..10 {
            conv.add_message(Message::user(format!("Message {}", i)));
        }

        assert_eq!(conv.len(), 10);
        conv.truncate(5);
        assert_eq!(conv.len(), 5);
    }

    #[test]
    fn test_conversation_fork() {
        let mut conv = Conversation::new();
        conv.add_message(Message::user("Hello"));
        conv.add_message(Message::assistant("Hi"));
        conv.add_message(Message::user("How are you?"));

        let fork = conv.fork(2);
        assert_eq!(fork.len(), 2);
    }
}

