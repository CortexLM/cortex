//! Chat message types and roles.
//!
//! Core data structures for representing chat messages and their metadata.

use crate::style::CortexStyle;
use ratatui::prelude::*;

// ============================================================
// MESSAGE ROLE
// ============================================================

/// Identifies the sender of a chat message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageRole {
    /// Message from the user
    User,
    /// Message from the AI assistant
    Assistant,
    /// System-level message (instructions, context)
    System,
    /// Output from a tool invocation
    Tool,
}

impl MessageRole {
    /// Returns the display prefix for this role.
    pub fn prefix(&self) -> &'static str {
        match self {
            MessageRole::User => "> ",
            MessageRole::Assistant => "",
            MessageRole::System => "System: ",
            MessageRole::Tool => "", // Tool messages use tool_name as prefix
        }
    }

    /// Returns the style for this role.
    pub fn style(&self) -> Style {
        match self {
            MessageRole::User => CortexStyle::user_message(),
            MessageRole::Assistant => CortexStyle::assistant_message(),
            MessageRole::System => CortexStyle::system_message(),
            MessageRole::Tool => CortexStyle::info(),
        }
    }
}

// ============================================================
// MESSAGE
// ============================================================

/// A single chat message with metadata.
#[derive(Debug, Clone)]
pub struct Message {
    /// The role of the message sender
    pub role: MessageRole,
    /// The message content
    pub content: String,
    /// Optional timestamp string
    pub timestamp: Option<String>,
    /// Whether this message is currently streaming
    pub is_streaming: bool,
    /// Tool name (for Tool role messages)
    pub tool_name: Option<String>,
}

impl Message {
    /// Creates a new user message.
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::User,
            content: content.into(),
            timestamp: None,
            is_streaming: false,
            tool_name: None,
        }
    }

    /// Creates a new assistant message.
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::Assistant,
            content: content.into(),
            timestamp: None,
            is_streaming: false,
            tool_name: None,
        }
    }

    /// Creates a new system message.
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::System,
            content: content.into(),
            timestamp: None,
            is_streaming: false,
            tool_name: None,
        }
    }

    /// Creates a new tool output message.
    pub fn tool(name: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::Tool,
            content: content.into(),
            timestamp: None,
            is_streaming: false,
            tool_name: Some(name.into()),
        }
    }

    /// Marks this message as currently streaming.
    pub fn streaming(mut self) -> Self {
        self.is_streaming = true;
        self
    }

    /// Adds a timestamp to this message.
    pub fn with_timestamp(mut self, ts: impl Into<String>) -> Self {
        self.timestamp = Some(ts.into());
        self
    }

    /// Returns the display prefix for this message.
    pub fn prefix(&self) -> String {
        match self.role {
            MessageRole::Tool => {
                if let Some(ref name) = self.tool_name {
                    format!("[{}]: ", name)
                } else {
                    "[tool]: ".to_string()
                }
            }
            _ => self.role.prefix().to_string(),
        }
    }
}

// ============================================================
// STYLED SEGMENT
// ============================================================

/// A segment of text with associated styling.
#[derive(Debug, Clone)]
pub struct StyledSegment {
    pub text: String,
    pub style: Style,
}

// ============================================================
// TESTS
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_role_prefix() {
        assert_eq!(MessageRole::User.prefix(), "> ");
        assert_eq!(MessageRole::Assistant.prefix(), "");
        assert_eq!(MessageRole::System.prefix(), "System: ");
        assert_eq!(MessageRole::Tool.prefix(), "");
    }

    #[test]
    fn test_message_creation() {
        let user_msg = Message::user("Hello");
        assert_eq!(user_msg.role, MessageRole::User);
        assert_eq!(user_msg.content, "Hello");
        assert!(!user_msg.is_streaming);

        let assistant_msg = Message::assistant("Hi there");
        assert_eq!(assistant_msg.role, MessageRole::Assistant);

        let system_msg = Message::system("Context");
        assert_eq!(system_msg.role, MessageRole::System);

        let tool_msg = Message::tool("search", "Results...");
        assert_eq!(tool_msg.role, MessageRole::Tool);
        assert_eq!(tool_msg.tool_name, Some("search".to_string()));
    }

    #[test]
    fn test_message_streaming() {
        let msg = Message::assistant("Hi").streaming();
        assert!(msg.is_streaming);
    }

    #[test]
    fn test_message_timestamp() {
        let msg = Message::user("Hi").with_timestamp("12:00");
        assert_eq!(msg.timestamp, Some("12:00".to_string()));
    }

    #[test]
    fn test_tool_message_prefix() {
        let msg = Message::tool("search", "Results");
        assert_eq!(msg.prefix(), "[search]: ");

        let mut msg_no_name = Message::tool("", "Results");
        msg_no_name.tool_name = None;
        assert_eq!(msg_no_name.prefix(), "[tool]: ");
    }
}
