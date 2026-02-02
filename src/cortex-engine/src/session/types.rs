//! Session types - TokenCounter, PendingToolCall, SessionHandle, SessionInfo.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use async_channel::{Receiver, Sender};

use cortex_protocol::{ConversationId, Event, Submission};

use crate::client::types::MessageContent;
use crate::client::{Message, ToolDefinition as ClientToolDefinition};
use crate::error::Result;

/// Simple token counter for session context tracking.
/// Uses approximation-based counting (4 chars per token).
pub struct TokenCounter;

impl Default for TokenCounter {
    fn default() -> Self {
        Self::new()
    }
}

impl TokenCounter {
    /// Create a new token counter.
    pub fn new() -> Self {
        Self
    }

    /// Count tokens in messages (approximate).
    pub async fn count_messages(&self, _model: &str, messages: &[Message]) -> Result<usize> {
        let mut total = 0usize;
        for msg in messages {
            // Base overhead per message (~4 tokens)
            total += 4;
            // Count content tokens
            let text = match &msg.content {
                MessageContent::Text(t) => t.len(),
                MessageContent::Parts(parts) => parts
                    .iter()
                    .map(|p| match p {
                        crate::client::types::ContentPart::Text { text, .. } => text.len(),
                        _ => 85, // Image tokens approximation
                    })
                    .sum(),
                MessageContent::ToolResult { content, .. } => content.len(),
                MessageContent::ToolCalls(calls) => {
                    calls.iter().map(|c| c.name.len() + c.arguments.len()).sum()
                }
            };
            // Approximate: 4 chars per token
            total += (text as f64 / 4.0).ceil() as usize;
        }
        // Message separator overhead
        total += messages.len() * 3;
        Ok(total)
    }

    /// Count tokens in tool definitions (approximate).
    pub async fn count_tools(&self, _model: &str, tools: &[ClientToolDefinition]) -> Result<usize> {
        let mut total = 0usize;
        for tool in tools {
            let json = serde_json::to_string(tool).unwrap_or_default();
            // Approximate: 4 chars per token + 10 overhead
            total += (json.len() as f64 / 4.0).ceil() as usize + 10;
        }
        // Base overhead for having tools
        if !tools.is_empty() {
            total += 9;
        }
        Ok(total)
    }
}

/// A tool call waiting for approval.
#[derive(Debug, Clone)]
pub(crate) struct PendingToolCall {
    pub tool_name: String,
    pub arguments: serde_json::Value,
    pub tool_call_id: String,
}

/// Handle for interacting with a session.
#[derive(Clone)]
pub struct SessionHandle {
    /// Submission sender.
    pub submission_tx: Sender<Submission>,
    /// Event receiver.
    pub event_rx: Receiver<Event>,
    /// Conversation ID.
    pub conversation_id: ConversationId,
    /// Cancellation flag.
    pub cancelled: Arc<AtomicBool>,
}

/// Session info for listing.
#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub id: String,
    pub timestamp: String,
    pub model: Option<String>,
    pub cwd: PathBuf,
    pub message_count: usize,
    pub git_branch: Option<String>,
}
