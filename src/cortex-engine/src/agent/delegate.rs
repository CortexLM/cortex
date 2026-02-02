//! Agent delegate for handling callbacks and events.

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::tools::spec::ToolResult;

use super::{AgentEvent, ApprovalResponse, RiskLevel, TokenUsage};

/// Delegate trait for agent callbacks.
#[async_trait]
pub trait AgentDelegate: Send + Sync {
    /// Called when a turn starts.
    async fn on_turn_started(&self, _turn_id: u64, _user_message: &str) {}

    /// Called when the model starts thinking.
    async fn on_thinking(&self) {}

    /// Called when text is generated.
    async fn on_text_delta(&self, _content: &str) {}

    /// Called when reasoning content is generated.
    async fn on_reasoning_delta(&self, _content: &str) {}

    /// Called when a tool call starts.
    async fn on_tool_call_started(&self, _id: &str, _name: &str, _arguments: &str) {}

    /// Called when a tool call completes.
    async fn on_tool_call_completed(&self, _id: &str, _name: &str, _result: &ToolResult) {}

    /// Called when a tool call needs approval.
    /// Returns whether the call should be approved.
    async fn on_approval_needed(
        &self,
        _id: &str,
        _name: &str,
        _arguments: &str,
        _risk_level: RiskLevel,
    ) -> ApprovalResponse {
        ApprovalResponse::Approve
    }

    /// Called when a turn completes.
    async fn on_turn_completed(&self, _turn_id: u64, _response: &str, _usage: &TokenUsage) {}

    /// Called when a turn is interrupted.
    async fn on_turn_interrupted(&self, _turn_id: u64) {}

    /// Called when context is compacted.
    async fn on_context_compacted(&self, _messages_removed: usize, _tokens_saved: u32) {}

    /// Called when an error occurs.
    async fn on_error(&self, _message: &str, _recoverable: bool) {}

    /// Called when session is saved.
    async fn on_session_saved(&self, _path: &std::path::Path) {}

    /// Called on shutdown.
    async fn on_shutdown(&self) {}
}

/// No-op delegate that does nothing.
#[allow(dead_code)]
pub struct NoOpDelegate;

#[async_trait]
impl AgentDelegate for NoOpDelegate {}

/// Channel delegate that sends events to a channel.
#[allow(dead_code)]
pub struct ChannelDelegate {
    tx: mpsc::Sender<AgentEvent>,
}

#[allow(dead_code)]
impl ChannelDelegate {
    pub fn new(tx: mpsc::Sender<AgentEvent>) -> Self {
        Self { tx }
    }

    async fn send(&self, event: AgentEvent) {
        let _ = self.tx.send(event).await;
    }
}

#[async_trait]
impl AgentDelegate for ChannelDelegate {
    async fn on_turn_started(&self, turn_id: u64, user_message: &str) {
        self.send(AgentEvent::TurnStarted {
            turn_id,
            user_message: user_message.to_string(),
        })
        .await;
    }

    async fn on_thinking(&self) {
        self.send(AgentEvent::Thinking).await;
    }

    async fn on_text_delta(&self, content: &str) {
        self.send(AgentEvent::TextDelta {
            content: content.to_string(),
        })
        .await;
    }

    async fn on_reasoning_delta(&self, content: &str) {
        self.send(AgentEvent::ReasoningDelta {
            content: content.to_string(),
        })
        .await;
    }

    async fn on_tool_call_started(&self, id: &str, name: &str, arguments: &str) {
        self.send(AgentEvent::ToolCallStarted {
            id: id.to_string(),
            name: name.to_string(),
            arguments: arguments.to_string(),
        })
        .await;
    }

    async fn on_tool_call_completed(&self, id: &str, name: &str, result: &ToolResult) {
        self.send(AgentEvent::ToolCallCompleted {
            id: id.to_string(),
            name: name.to_string(),
            result: result.clone(),
        })
        .await;
    }

    async fn on_turn_completed(&self, turn_id: u64, response: &str, usage: &TokenUsage) {
        self.send(AgentEvent::TurnCompleted {
            turn_id,
            response: response.to_string(),
            token_usage: usage.clone(),
        })
        .await;
    }

    async fn on_turn_interrupted(&self, turn_id: u64) {
        self.send(AgentEvent::TurnInterrupted { turn_id }).await;
    }

    async fn on_context_compacted(&self, messages_removed: usize, tokens_saved: u32) {
        self.send(AgentEvent::ContextCompacted {
            messages_removed,
            tokens_saved,
        })
        .await;
    }

    async fn on_error(&self, message: &str, recoverable: bool) {
        self.send(AgentEvent::Error {
            message: message.to_string(),
            recoverable,
        })
        .await;
    }

    async fn on_shutdown(&self) {
        self.send(AgentEvent::ShutdownComplete).await;
    }
}

/// Composite delegate that dispatches to multiple delegates.
#[allow(dead_code)]
pub struct CompositeDelegate {
    delegates: Vec<Arc<dyn AgentDelegate>>,
}

#[allow(dead_code)]
impl CompositeDelegate {
    pub fn new() -> Self {
        Self {
            delegates: Vec::new(),
        }
    }

    pub fn add(mut self, delegate: Arc<dyn AgentDelegate>) -> Self {
        self.delegates.push(delegate);
        self
    }
}

impl Default for CompositeDelegate {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AgentDelegate for CompositeDelegate {
    async fn on_turn_started(&self, turn_id: u64, user_message: &str) {
        for d in &self.delegates {
            d.on_turn_started(turn_id, user_message).await;
        }
    }

    async fn on_thinking(&self) {
        for d in &self.delegates {
            d.on_thinking().await;
        }
    }

    async fn on_text_delta(&self, content: &str) {
        for d in &self.delegates {
            d.on_text_delta(content).await;
        }
    }

    async fn on_reasoning_delta(&self, content: &str) {
        for d in &self.delegates {
            d.on_reasoning_delta(content).await;
        }
    }

    async fn on_tool_call_started(&self, id: &str, name: &str, arguments: &str) {
        for d in &self.delegates {
            d.on_tool_call_started(id, name, arguments).await;
        }
    }

    async fn on_tool_call_completed(&self, id: &str, name: &str, result: &ToolResult) {
        for d in &self.delegates {
            d.on_tool_call_completed(id, name, result).await;
        }
    }

    async fn on_approval_needed(
        &self,
        id: &str,
        name: &str,
        arguments: &str,
        risk_level: RiskLevel,
    ) -> ApprovalResponse {
        // First delegate that returns non-Approve wins
        for d in &self.delegates {
            let response = d.on_approval_needed(id, name, arguments, risk_level).await;
            if !matches!(response, ApprovalResponse::Approve) {
                return response;
            }
        }
        ApprovalResponse::Approve
    }

    async fn on_turn_completed(&self, turn_id: u64, response: &str, usage: &TokenUsage) {
        for d in &self.delegates {
            d.on_turn_completed(turn_id, response, usage).await;
        }
    }

    async fn on_turn_interrupted(&self, turn_id: u64) {
        for d in &self.delegates {
            d.on_turn_interrupted(turn_id).await;
        }
    }

    async fn on_context_compacted(&self, messages_removed: usize, tokens_saved: u32) {
        for d in &self.delegates {
            d.on_context_compacted(messages_removed, tokens_saved).await;
        }
    }

    async fn on_error(&self, message: &str, recoverable: bool) {
        for d in &self.delegates {
            d.on_error(message, recoverable).await;
        }
    }

    async fn on_session_saved(&self, path: &std::path::Path) {
        for d in &self.delegates {
            d.on_session_saved(path).await;
        }
    }

    async fn on_shutdown(&self) {
        for d in &self.delegates {
            d.on_shutdown().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_no_op_delegate() {
        let delegate = NoOpDelegate;
        delegate.on_turn_started(1, "test").await;
        delegate.on_thinking().await;
    }

    #[tokio::test]
    async fn test_channel_delegate() {
        let (tx, mut rx) = mpsc::channel(10);
        let delegate = ChannelDelegate::new(tx);

        delegate.on_turn_started(1, "Hello").await;

        let event = rx.recv().await.unwrap();
        assert!(matches!(event, AgentEvent::TurnStarted { turn_id: 1, .. }));
    }

    #[tokio::test]
    async fn test_composite_delegate() {
        let (tx, mut rx) = mpsc::channel(10);

        let delegate = CompositeDelegate::new()
            .add(Arc::new(NoOpDelegate))
            .add(Arc::new(ChannelDelegate::new(tx)));

        delegate.on_thinking().await;

        let event = rx.recv().await.unwrap();
        assert!(matches!(event, AgentEvent::Thinking));
    }
}
