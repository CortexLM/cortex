//! Turn management for agent conversations.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::{TokenUsage, ToolCallRecord, UserInputItem};
use crate::client::types::Message;

/// A conversation turn.
#[derive(Debug, Clone)]
pub struct Turn {
    /// Turn ID.
    pub id: u64,
    /// User input items.
    pub user_items: Vec<UserInputItem>,
    /// User message text.
    pub user_message: String,
    /// Assistant response.
    pub assistant_response: Option<String>,
    /// Tool calls made.
    pub tool_calls: Vec<ToolCallRecord>,
    /// Messages in this turn.
    pub messages: Vec<Message>,
    /// Token usage.
    pub token_usage: TokenUsage,
    /// Start time.
    pub started_at: Instant,
    /// End time.
    pub ended_at: Option<Instant>,
    /// Status.
    pub status: TurnStatus,
    /// Error message if failed.
    pub error: Option<String>,
    /// Metadata.
    pub metadata: std::collections::HashMap<String, serde_json::Value>,
}

impl Turn {
    /// Create a new turn.
    pub fn new(id: u64, user_items: Vec<UserInputItem>) -> Self {
        let user_message = user_items
            .iter()
            .filter_map(|item| match item {
                UserInputItem::Text { content } => Some(content.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");

        Self {
            id,
            user_items,
            user_message,
            assistant_response: None,
            tool_calls: Vec::new(),
            messages: Vec::new(),
            token_usage: TokenUsage::default(),
            started_at: Instant::now(),
            ended_at: None,
            status: TurnStatus::InProgress,
            error: None,
            metadata: std::collections::HashMap::new(),
        }
    }

    /// Add tool call record.
    pub fn add_tool_call(&mut self, record: ToolCallRecord) {
        self.tool_calls.push(record);
    }

    /// Add message.
    pub fn add_message(&mut self, message: Message) {
        self.messages.push(message);
    }

    /// Set assistant response.
    pub fn set_response(&mut self, response: impl Into<String>) {
        self.assistant_response = Some(response.into());
    }

    /// Update token usage.
    pub fn update_tokens(&mut self, usage: TokenUsage) {
        self.token_usage.input_tokens += usage.input_tokens;
        self.token_usage.output_tokens += usage.output_tokens;
        self.token_usage.total_tokens += usage.total_tokens;
        self.token_usage.cached_tokens += usage.cached_tokens;
        self.token_usage.reasoning_tokens += usage.reasoning_tokens;
    }

    /// Complete the turn.
    pub fn complete(&mut self) {
        self.status = TurnStatus::Completed;
        self.ended_at = Some(Instant::now());
    }

    /// Mark as failed.
    pub fn fail(&mut self, error: impl Into<String>) {
        self.status = TurnStatus::Failed;
        self.error = Some(error.into());
        self.ended_at = Some(Instant::now());
    }

    /// Cancel the turn.
    pub fn cancel(&mut self) {
        self.status = TurnStatus::Cancelled;
        self.ended_at = Some(Instant::now());
    }

    /// Get duration.
    pub fn duration(&self) -> Duration {
        match self.ended_at {
            Some(end) => end.duration_since(self.started_at),
            None => self.started_at.elapsed(),
        }
    }

    /// Check if completed.
    pub fn is_completed(&self) -> bool {
        matches!(self.status, TurnStatus::Completed)
    }

    /// Check if failed.
    pub fn is_failed(&self) -> bool {
        matches!(self.status, TurnStatus::Failed)
    }

    /// Check if in progress.
    pub fn is_in_progress(&self) -> bool {
        matches!(self.status, TurnStatus::InProgress)
    }

    /// Get tool call count.
    pub fn tool_call_count(&self) -> usize {
        self.tool_calls.len()
    }

    /// Set metadata.
    pub fn set_metadata(&mut self, key: impl Into<String>, value: serde_json::Value) {
        self.metadata.insert(key.into(), value);
    }

    /// Get metadata.
    pub fn get_metadata(&self, key: &str) -> Option<&serde_json::Value> {
        self.metadata.get(key)
    }

    /// Convert to result.
    pub fn into_result(self) -> TurnResult {
        let duration_ms = self.duration().as_millis() as u64;
        TurnResult {
            turn_id: self.id,
            user_message: self.user_message,
            assistant_response: self.assistant_response,
            tool_calls: self.tool_calls,
            token_usage: self.token_usage,
            duration_ms,
            status: self.status,
            error: self.error,
        }
    }
}

/// Turn status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum TurnStatus {
    /// Turn is in progress.
    #[default]
    InProgress,
    /// Turn completed successfully.
    Completed,
    /// Turn failed.
    Failed,
    /// Turn was cancelled.
    Cancelled,
    /// Turn was interrupted.
    Interrupted,
    /// Turn is waiting for approval.
    WaitingForApproval,
}

/// Turn result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnResult {
    /// Turn ID.
    pub turn_id: u64,
    /// User message.
    pub user_message: String,
    /// Assistant response.
    pub assistant_response: Option<String>,
    /// Tool calls made.
    pub tool_calls: Vec<ToolCallRecord>,
    /// Token usage.
    pub token_usage: TokenUsage,
    /// Duration in milliseconds.
    pub duration_ms: u64,
    /// Status.
    pub status: TurnStatus,
    /// Error if failed.
    pub error: Option<String>,
}

impl TurnResult {
    /// Check if successful.
    pub fn is_success(&self) -> bool {
        matches!(self.status, TurnStatus::Completed)
    }

    /// Get response text.
    pub fn response(&self) -> Option<&str> {
        self.assistant_response.as_deref()
    }
}

/// Turn builder.
#[derive(Debug, Default)]
#[allow(dead_code)]
pub struct TurnBuilder {
    id: u64,
    user_items: Vec<UserInputItem>,
    metadata: std::collections::HashMap<String, serde_json::Value>,
}

#[allow(dead_code)]
impl TurnBuilder {
    /// Create new builder.
    pub fn new(id: u64) -> Self {
        Self {
            id,
            ..Self::default()
        }
    }

    /// Add text input.
    pub fn text(mut self, content: impl Into<String>) -> Self {
        self.user_items.push(UserInputItem::Text {
            content: content.into(),
        });
        self
    }

    /// Add file input.
    pub fn file(mut self, path: impl Into<String>) -> Self {
        self.user_items.push(UserInputItem::File {
            path: path.into(),
            content: None,
        });
        self
    }

    /// Add image input.
    pub fn image(mut self, url: impl Into<String>) -> Self {
        self.user_items
            .push(UserInputItem::Image { url: url.into() });
        self
    }

    /// Add metadata.
    pub fn metadata(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.metadata.insert(key.into(), value);
        self
    }

    /// Build the turn.
    pub fn build(self) -> Turn {
        let mut turn = Turn::new(self.id, self.user_items);
        turn.metadata = self.metadata;
        turn
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_turn_creation() {
        let turn = Turn::new(
            1,
            vec![UserInputItem::Text {
                content: "Hello".into(),
            }],
        );

        assert_eq!(turn.id, 1);
        assert_eq!(turn.user_message, "Hello");
        assert!(turn.is_in_progress());
    }

    #[test]
    fn test_turn_completion() {
        let mut turn = Turn::new(1, vec![]);

        turn.set_response("Response");
        turn.complete();

        assert!(turn.is_completed());
        assert_eq!(turn.assistant_response, Some("Response".to_string()));
    }

    #[test]
    fn test_turn_failure() {
        let mut turn = Turn::new(1, vec![]);

        turn.fail("Error occurred");

        assert!(turn.is_failed());
        assert_eq!(turn.error, Some("Error occurred".to_string()));
    }

    #[test]
    fn test_turn_builder() {
        let turn = TurnBuilder::new(1).text("Hello").file("test.rs").build();

        assert_eq!(turn.id, 1);
        assert_eq!(turn.user_items.len(), 2);
    }

    #[test]
    fn test_turn_result() {
        let mut turn = Turn::new(
            1,
            vec![UserInputItem::Text {
                content: "Test".into(),
            }],
        );
        turn.set_response("OK");
        turn.complete();

        let result = turn.into_result();
        assert!(result.is_success());
        assert_eq!(result.response(), Some("OK"));
    }
}
