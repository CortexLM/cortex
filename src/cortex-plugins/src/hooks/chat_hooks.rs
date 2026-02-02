//! Chat message hooks.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::types::{HookPriority, HookResult};
use crate::Result;

/// Input for chat.message hook.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessageInput {
    /// Session ID
    pub session_id: String,
    /// Message ID
    pub message_id: Option<String>,
    /// Message role (user, assistant, system)
    pub role: String,
    /// Agent name
    pub agent: Option<String>,
    /// Model name
    pub model: Option<String>,
}

/// Output for chat.message hook (mutable).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessageOutput {
    /// Message content
    pub content: String,
    /// Message parts (for multipart messages)
    pub parts: Vec<MessagePart>,
    /// Hook result
    pub result: HookResult,
}

impl ChatMessageOutput {
    /// Create a new output with the message content.
    pub fn new(content: String) -> Self {
        Self {
            content,
            parts: Vec::new(),
            result: HookResult::Continue,
        }
    }
}

/// Message part for multipart messages.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MessagePart {
    /// Text content
    Text { content: String },
    /// Image content
    Image { url: String, alt: Option<String> },
    /// Tool call
    ToolCall {
        tool: String,
        args: serde_json::Value,
    },
}

/// Handler for chat.message hook.
#[async_trait]
pub trait ChatMessageHook: Send + Sync {
    /// Get the priority of this hook.
    fn priority(&self) -> HookPriority {
        HookPriority::default()
    }

    /// Execute the hook.
    async fn execute(&self, input: &ChatMessageInput, output: &mut ChatMessageOutput)
    -> Result<()>;
}
