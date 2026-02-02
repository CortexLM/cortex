//! Message handler for processing model responses.

use super::{AgentEvent, AgentProfile, RiskLevel, ToolPermission};
use crate::client::types::{Message, MessageContent, ToolCall};
use crate::error::{CortexError, Result};
use cortex_common::strip_ansi_codes;
use std::path::Path;
use tokio::sync::mpsc;

/// Message handler for processing and transforming messages.
#[derive(Default)]
pub struct MessageHandler {
    /// Message transformers.
    transformers: Vec<Box<dyn MessageTransformer>>,
    /// Message filters.
    filters: Vec<Box<dyn MessageFilter>>,
}

impl std::fmt::Debug for MessageHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MessageHandler")
            .field(
                "transformers",
                &format!("{} transformers", self.transformers.len()),
            )
            .field("filters", &format!("{} filters", self.filters.len()))
            .finish()
    }
}

impl MessageHandler {
    /// Create a new message handler.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a transformer.
    pub fn add_transformer<T: MessageTransformer + 'static>(&mut self, transformer: T) {
        self.transformers.push(Box::new(transformer));
    }

    /// Add a filter.
    pub fn add_filter<F: MessageFilter + 'static>(&mut self, filter: F) {
        self.filters.push(Box::new(filter));
    }

    /// Process a message through all transformers.
    pub fn transform(&self, mut message: Message) -> Message {
        for transformer in &self.transformers {
            message = transformer.transform(message);
        }
        message
    }

    /// Check if a message passes all filters.
    pub fn filter(&self, message: &Message) -> bool {
        self.filters.iter().all(|f| f.filter(message))
    }

    /// Process a batch of messages.
    pub fn process(&self, messages: Vec<Message>) -> Vec<Message> {
        messages
            .into_iter()
            .filter(|m| self.filter(m))
            .map(|m| self.transform(m))
            .collect()
    }

    /// Validates and handles tool execution based on agent profile permissions.
    ///
    /// If a tool is 'deny', returns an error immediately.
    /// If 'ask', triggers a protocol 'ExecApprovalRequest' via the event sender.
    /// Returns Ok(true) if execution should proceed, Ok(false) if waiting for approval.
    pub async fn handle_tool_execution(
        &self,
        profile: &AgentProfile,
        tool_call: &ToolCall,
        _turn_id: &str,
        _cwd: &Path,
        event_tx: &mpsc::UnboundedSender<AgentEvent>,
    ) -> Result<bool> {
        let permission = profile
            .tool_permissions
            .get(&tool_call.function.name)
            .cloned()
            .unwrap_or(ToolPermission::Allow);

        match permission {
            ToolPermission::Allow => Ok(true),
            ToolPermission::Deny => {
                let error_msg = format!(
                    "Tool '{}' is explicitly denied by profile '{}'",
                    tool_call.function.name, profile.name
                );
                Err(CortexError::Internal(error_msg))
            }
            ToolPermission::Ask => {
                // Trigger protocol ExecApprovalRequest by emitting ToolCallPending event
                // This will be picked up by the session handler and converted to a protocol event
                let _ = event_tx.send(AgentEvent::ToolCallPending {
                    id: tool_call.id.clone(),
                    name: tool_call.function.name.clone(),
                    arguments: tool_call.function.arguments.clone(),
                    risk_level: RiskLevel::Medium, // Tools marked as 'ask' are considered medium risk by default
                });

                Ok(false)
            }
        }
    }
}

/// Trait for message transformers.
pub trait MessageTransformer: Send + Sync {
    /// Transform a message.
    fn transform(&self, message: Message) -> Message;
}

/// Trait for message filters.
pub trait MessageFilter: Send + Sync {
    /// Returns true if the message should be kept.
    fn filter(&self, message: &Message) -> bool;
}

/// Truncate message content to a maximum length.
#[derive(Debug)]
#[allow(dead_code)]
pub struct TruncateTransformer {
    max_length: usize,
    suffix: String,
}

#[allow(dead_code)]
impl TruncateTransformer {
    pub fn new(max_length: usize) -> Self {
        Self {
            max_length,
            suffix: "... [truncated]".to_string(),
        }
    }

    pub fn with_suffix(mut self, suffix: impl Into<String>) -> Self {
        self.suffix = suffix.into();
        self
    }
}

impl MessageTransformer for TruncateTransformer {
    fn transform(&self, mut message: Message) -> Message {
        if let MessageContent::Text(ref mut text) = message.content
            && text.len() > self.max_length
        {
            text.truncate(self.max_length - self.suffix.len());
            text.push_str(&self.suffix);
        }
        message
    }
}

/// Strip ANSI escape codes from messages.
#[derive(Debug, Default)]
#[allow(dead_code)]
pub struct StripAnsiTransformer;

impl MessageTransformer for StripAnsiTransformer {
    fn transform(&self, mut message: Message) -> Message {
        if let MessageContent::Text(ref mut text) = message.content {
            *text = strip_ansi_codes(text);
        }
        message
    }
}

/// Filter out empty messages.
#[derive(Debug, Default)]
#[allow(dead_code)]
pub struct NonEmptyFilter;

impl MessageFilter for NonEmptyFilter {
    fn filter(&self, message: &Message) -> bool {
        match &message.content {
            MessageContent::Text(text) => !text.trim().is_empty(),
            MessageContent::Parts(parts) => !parts.is_empty(),
            MessageContent::ToolResult { content, .. } => !content.trim().is_empty(),
            MessageContent::ToolCalls(calls) => !calls.is_empty(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_transformer() {
        let transformer = TruncateTransformer::new(20);
        let msg = Message::user("This is a very long message that should be truncated");
        let result = transformer.transform(msg);

        if let MessageContent::Text(text) = result.content {
            assert!(text.len() <= 20);
            assert!(text.ends_with("[truncated]"));
        }
    }

    #[test]
    fn test_strip_ansi() {
        let text = "\x1b[31mRed\x1b[0m Normal";
        let result = strip_ansi_codes(text);
        assert_eq!(result, "Red Normal");
    }

    #[test]
    fn test_message_handler() {
        let mut handler = MessageHandler::new();
        handler.add_filter(NonEmptyFilter);
        handler.add_transformer(StripAnsiTransformer);

        let messages = vec![Message::user("\x1b[31mHello\x1b[0m"), Message::user("")];

        let processed = handler.process(messages);
        assert_eq!(processed.len(), 1);

        if let MessageContent::Text(text) = &processed[0].content {
            assert_eq!(text, "Hello");
        }
    }
}
