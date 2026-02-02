//! Turn state management.
//!
//! Tracks the state of individual turns within a session, including
//! tool calls, responses, and timing information.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::error::{CortexError, Result};

/// Turn state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnState {
    /// Turn ID.
    pub id: String,
    /// Session ID.
    pub session_id: String,
    /// Turn number within session.
    pub turn_number: u32,
    /// Turn phase.
    pub phase: TurnPhase,
    /// User input.
    pub input: String,
    /// Model response.
    pub response: Option<String>,
    /// Tool calls made.
    pub tool_calls: Vec<TurnToolCall>,
    /// Created timestamp.
    pub created_at: u64,
    /// Started timestamp.
    pub started_at: Option<u64>,
    /// Completed timestamp.
    pub completed_at: Option<u64>,
    /// Input tokens.
    pub input_tokens: u32,
    /// Output tokens.
    pub output_tokens: u32,
    /// Reasoning tokens.
    pub reasoning_tokens: u32,
    /// Error if failed.
    pub error: Option<String>,
    /// Turn metadata.
    pub metadata: TurnMetadata,
}

impl TurnState {
    /// Create a new turn state.
    pub fn new(
        id: impl Into<String>,
        session_id: impl Into<String>,
        turn_number: u32,
        input: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            session_id: session_id.into(),
            turn_number,
            phase: TurnPhase::Pending,
            input: input.into(),
            response: None,
            tool_calls: Vec::new(),
            created_at: timestamp_now(),
            started_at: None,
            completed_at: None,
            input_tokens: 0,
            output_tokens: 0,
            reasoning_tokens: 0,
            error: None,
            metadata: TurnMetadata::default(),
        }
    }

    /// Start the turn.
    pub fn start(&mut self) {
        self.phase = TurnPhase::Processing;
        self.started_at = Some(timestamp_now());
    }

    /// Set as waiting for tool results.
    pub fn wait_for_tools(&mut self) {
        self.phase = TurnPhase::WaitingTools;
    }

    /// Set as waiting for user approval.
    pub fn wait_for_approval(&mut self, tool_call_id: String) {
        self.phase = TurnPhase::WaitingApproval { tool_call_id };
    }

    /// Resume processing.
    pub fn resume(&mut self) {
        self.phase = TurnPhase::Processing;
    }

    /// Complete the turn.
    pub fn complete(&mut self, response: impl Into<String>) {
        self.phase = TurnPhase::Completed;
        self.response = Some(response.into());
        self.completed_at = Some(timestamp_now());
    }

    /// Fail the turn.
    pub fn fail(&mut self, error: impl Into<String>) {
        self.phase = TurnPhase::Failed;
        self.error = Some(error.into());
        self.completed_at = Some(timestamp_now());
    }

    /// Cancel the turn.
    pub fn cancel(&mut self) {
        self.phase = TurnPhase::Cancelled;
        self.completed_at = Some(timestamp_now());
    }

    /// Add a tool call.
    pub fn add_tool_call(&mut self, tool_call: TurnToolCall) {
        self.tool_calls.push(tool_call);
    }

    /// Set token usage.
    pub fn set_tokens(&mut self, input: u32, output: u32, reasoning: u32) {
        self.input_tokens = input;
        self.output_tokens = output;
        self.reasoning_tokens = reasoning;
    }

    /// Get total tokens.
    pub fn total_tokens(&self) -> u32 {
        self.input_tokens + self.output_tokens + self.reasoning_tokens
    }

    /// Get turn duration.
    pub fn duration(&self) -> Option<Duration> {
        match (self.started_at, self.completed_at) {
            (Some(start), Some(end)) if end >= start => Some(Duration::from_secs(end - start)),
            _ => None,
        }
    }

    /// Check if turn is finished.
    pub fn is_finished(&self) -> bool {
        matches!(
            self.phase,
            TurnPhase::Completed | TurnPhase::Failed | TurnPhase::Cancelled
        )
    }

    /// Check if turn succeeded.
    pub fn is_success(&self) -> bool {
        matches!(self.phase, TurnPhase::Completed)
    }

    /// Get pending tool call count.
    pub fn pending_tool_calls(&self) -> usize {
        self.tool_calls
            .iter()
            .filter(|tc| tc.status == ToolCallStatus::Pending)
            .count()
    }
}

/// Turn phase.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
#[derive(Default)]
pub enum TurnPhase {
    /// Turn is pending start.
    #[default]
    Pending,
    /// Turn is processing.
    Processing,
    /// Waiting for tool results.
    WaitingTools,
    /// Waiting for user approval.
    WaitingApproval { tool_call_id: String },
    /// Turn completed successfully.
    Completed,
    /// Turn failed.
    Failed,
    /// Turn was cancelled.
    Cancelled,
}

/// Tool call within a turn.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnToolCall {
    /// Tool call ID.
    pub id: String,
    /// Tool name.
    pub name: String,
    /// Arguments.
    pub arguments: serde_json::Value,
    /// Status.
    pub status: ToolCallStatus,
    /// Result.
    pub result: Option<String>,
    /// Error if failed.
    pub error: Option<String>,
    /// Started timestamp.
    pub started_at: Option<u64>,
    /// Completed timestamp.
    pub completed_at: Option<u64>,
    /// Whether approval was required.
    pub required_approval: bool,
    /// Whether it was auto-approved.
    pub auto_approved: bool,
}

impl TurnToolCall {
    /// Create a new tool call.
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        arguments: serde_json::Value,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            arguments,
            status: ToolCallStatus::Pending,
            result: None,
            error: None,
            started_at: None,
            completed_at: None,
            required_approval: false,
            auto_approved: false,
        }
    }

    /// Start the tool call.
    pub fn start(&mut self) {
        self.status = ToolCallStatus::Running;
        self.started_at = Some(timestamp_now());
    }

    /// Complete the tool call.
    pub fn complete(&mut self, result: impl Into<String>) {
        self.status = ToolCallStatus::Completed;
        self.result = Some(result.into());
        self.completed_at = Some(timestamp_now());
    }

    /// Fail the tool call.
    pub fn fail(&mut self, error: impl Into<String>) {
        self.status = ToolCallStatus::Failed;
        self.error = Some(error.into());
        self.completed_at = Some(timestamp_now());
    }

    /// Deny the tool call.
    pub fn deny(&mut self, reason: impl Into<String>) {
        self.status = ToolCallStatus::Denied;
        self.error = Some(reason.into());
        self.completed_at = Some(timestamp_now());
    }

    /// Get duration.
    pub fn duration(&self) -> Option<Duration> {
        match (self.started_at, self.completed_at) {
            (Some(start), Some(end)) if end >= start => Some(Duration::from_secs(end - start)),
            _ => None,
        }
    }
}

/// Tool call status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum ToolCallStatus {
    /// Pending execution.
    #[default]
    Pending,
    /// Waiting for approval.
    WaitingApproval,
    /// Running.
    Running,
    /// Completed successfully.
    Completed,
    /// Failed.
    Failed,
    /// Denied by user.
    Denied,
    /// Timed out.
    Timeout,
}

/// Turn metadata.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TurnMetadata {
    /// Model used.
    pub model: Option<String>,
    /// Temperature.
    pub temperature: Option<f32>,
    /// Max tokens.
    pub max_tokens: Option<u32>,
    /// Custom data.
    pub custom: HashMap<String, String>,
}

/// Turn manager for a session.
#[derive(Debug, Clone)]
pub struct TurnManager {
    /// Turn states indexed by ID.
    turns: Arc<RwLock<HashMap<String, TurnState>>>,
    /// Turn order.
    order: Arc<RwLock<Vec<String>>>,
    /// Current turn ID.
    current: Arc<RwLock<Option<String>>>,
}

impl TurnManager {
    /// Create a new turn manager.
    pub fn new() -> Self {
        Self {
            turns: Arc::new(RwLock::new(HashMap::new())),
            order: Arc::new(RwLock::new(Vec::new())),
            current: Arc::new(RwLock::new(None)),
        }
    }

    /// Create a new turn.
    pub async fn create(&self, session_id: &str, input: &str) -> TurnState {
        let order = self.order.read().await;
        let turn_number = order.len() as u32 + 1;
        let id = format!("turn_{turn_number}");

        TurnState::new(&id, session_id, turn_number, input)
    }

    /// Start a turn.
    pub async fn start(&self, mut turn: TurnState) -> String {
        turn.start();
        let id = turn.id.clone();

        self.turns.write().await.insert(id.clone(), turn);
        self.order.write().await.push(id.clone());
        *self.current.write().await = Some(id.clone());

        id
    }

    /// Get a turn.
    pub async fn get(&self, id: &str) -> Option<TurnState> {
        self.turns.read().await.get(id).cloned()
    }

    /// Get current turn.
    pub async fn current(&self) -> Option<TurnState> {
        let current = self.current.read().await.clone()?;
        self.get(&current).await
    }

    /// Update a turn.
    pub async fn update(&self, id: &str, f: impl FnOnce(&mut TurnState)) -> Result<()> {
        let mut turns = self.turns.write().await;
        let turn = turns
            .get_mut(id)
            .ok_or_else(|| CortexError::NotFound(format!("Turn not found: {id}")))?;
        f(turn);

        // Clear current if finished
        if turn.is_finished() {
            let mut current = self.current.write().await;
            if current.as_deref() == Some(id) {
                *current = None;
            }
        }

        Ok(())
    }

    /// Complete current turn.
    pub async fn complete_current(&self, response: &str) -> Result<()> {
        let id = self
            .current
            .read()
            .await
            .clone()
            .ok_or_else(|| CortexError::NotFound("No current turn".to_string()))?;

        self.update(&id, |turn| {
            turn.complete(response);
        })
        .await
    }

    /// Get all turns.
    pub async fn all(&self) -> Vec<TurnState> {
        let turns = self.turns.read().await;
        let order = self.order.read().await;

        order
            .iter()
            .filter_map(|id| turns.get(id).cloned())
            .collect()
    }

    /// Get turn count.
    pub async fn count(&self) -> usize {
        self.order.read().await.len()
    }

    /// Get last N turns.
    pub async fn last_n(&self, n: usize) -> Vec<TurnState> {
        let turns = self.turns.read().await;
        let order = self.order.read().await;

        order
            .iter()
            .rev()
            .take(n)
            .filter_map(|id| turns.get(id).cloned())
            .collect()
    }
}

impl Default for TurnManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Turn event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TurnEvent {
    /// Turn started.
    Started { turn_id: String },
    /// Response text received.
    ResponseDelta { turn_id: String, delta: String },
    /// Tool call requested.
    ToolCallRequested {
        turn_id: String,
        tool_call: TurnToolCall,
    },
    /// Tool call completed.
    ToolCallCompleted {
        turn_id: String,
        tool_call_id: String,
    },
    /// Waiting for approval.
    WaitingApproval {
        turn_id: String,
        tool_call_id: String,
    },
    /// Turn completed.
    Completed { turn_id: String },
    /// Turn failed.
    Failed { turn_id: String, error: String },
    /// Turn cancelled.
    Cancelled { turn_id: String },
}

/// Get current timestamp.
fn timestamp_now() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_turn_state() {
        let mut turn = TurnState::new("turn-1", "session-1", 1, "Hello");
        assert_eq!(turn.phase, TurnPhase::Pending);

        turn.start();
        assert_eq!(turn.phase, TurnPhase::Processing);

        turn.complete("Response");
        assert!(turn.is_success());
        assert_eq!(turn.response, Some("Response".to_string()));
    }

    #[test]
    fn test_turn_tool_call() {
        let mut call =
            TurnToolCall::new("call-1", "read_file", serde_json::json!({"path": "/test"}));
        assert_eq!(call.status, ToolCallStatus::Pending);

        call.start();
        assert_eq!(call.status, ToolCallStatus::Running);

        call.complete("file content");
        assert_eq!(call.status, ToolCallStatus::Completed);
        assert_eq!(call.result, Some("file content".to_string()));
    }

    #[tokio::test]
    async fn test_turn_manager() {
        let manager = TurnManager::new();

        let turn = manager.create("session-1", "Hello").await;
        let id = manager.start(turn).await;

        assert_eq!(manager.count().await, 1);

        let current = manager.current().await.unwrap();
        assert_eq!(current.id, id);

        manager.complete_current("Response").await.unwrap();
        assert!(manager.current().await.is_none());
    }
}
