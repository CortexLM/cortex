//! Conversation manager.

use std::path::PathBuf;

use cortex_protocol::ConversationId;

use super::history::ConversationHistory;
use crate::client::{CompletionResponse, Message};

/// Manages conversation state.
pub struct ConversationManager {
    id: ConversationId,
    history: ConversationHistory,
    model: String,
    cwd: PathBuf,
}

impl ConversationManager {
    /// Create a new conversation manager.
    pub fn new(model: impl Into<String>, cwd: PathBuf) -> Self {
        Self {
            id: ConversationId::new(),
            history: ConversationHistory::new(),
            model: model.into(),
            cwd,
        }
    }

    /// Create from an existing conversation ID.
    pub fn with_id(id: ConversationId, model: impl Into<String>, cwd: PathBuf) -> Self {
        Self {
            id,
            history: ConversationHistory::new(),
            model: model.into(),
            cwd,
        }
    }

    /// Get the conversation ID.
    pub fn id(&self) -> &ConversationId {
        &self.id
    }

    /// Get the model.
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Get the working directory.
    pub fn cwd(&self) -> &PathBuf {
        &self.cwd
    }

    /// Add a user message.
    pub fn add_user_message(&mut self, content: impl Into<String>) {
        self.history.add_message(Message::user(content));
    }

    /// Add a system message.
    pub fn add_system_message(&mut self, content: impl Into<String>) {
        self.history.add_message(Message::system(content));
    }

    /// Add an assistant message.
    pub fn add_assistant_message(&mut self, content: impl Into<String>) {
        self.history.add_message(Message::assistant(content));
    }

    /// Add a tool result.
    pub fn add_tool_result(&mut self, tool_call_id: impl Into<String>, content: impl Into<String>) {
        self.history
            .add_message(Message::tool_result(tool_call_id, content));
    }

    /// Add a completion response.
    pub fn add_response(&mut self, response: &CompletionResponse) {
        if let Some(msg) = &response.message {
            self.history.add_message(msg.clone());
        }
    }

    /// Get all messages.
    pub fn messages(&self) -> &[Message] {
        self.history.messages()
    }

    /// Get message count.
    pub fn message_count(&self) -> usize {
        self.history.len()
    }

    /// Clear history.
    pub fn clear(&mut self) {
        self.history.clear();
    }

    /// Undo the last turn (remove last assistant message and its tool calls).
    pub fn undo(&mut self) -> bool {
        self.history.undo()
    }

    /// Get the last assistant message.
    pub fn last_assistant_message(&self) -> Option<&Message> {
        self.history.last_assistant_message()
    }
}
