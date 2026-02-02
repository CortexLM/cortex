//! Agent state management.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::client::types::Message;

use super::{AgentConfig, ConversationTurn, TurnStatus};

/// Agent state.
#[derive(Debug)]
pub struct AgentState {
    /// Current state type.
    pub state: StateType,
    /// Current turn ID.
    pub turn_id: u64,
    /// Total tokens used in session.
    pub tokens_used: u64,
    /// Conversation history.
    pub messages: Vec<Message>,
    /// Turn history.
    pub turns: Vec<ConversationTurn>,
    /// Working directory.
    pub working_dir: PathBuf,
    /// Environment variables.
    pub env: HashMap<String, String>,
    /// Session start time.
    pub started_at: Instant,
    /// Last activity time.
    pub last_activity: Instant,
    /// Custom state data.
    pub custom: HashMap<String, serde_json::Value>,
}

impl Default for AgentState {
    fn default() -> Self {
        let now = Instant::now();
        Self {
            state: StateType::Idle,
            turn_id: 0,
            tokens_used: 0,
            messages: Vec::new(),
            turns: Vec::new(),
            working_dir: std::env::current_dir().unwrap_or_default(),
            env: std::env::vars().collect(),
            started_at: now,
            last_activity: now,
            custom: HashMap::new(),
        }
    }
}

impl AgentState {
    /// Create new agent state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create from config.
    pub fn from_config(config: &AgentConfig) -> Self {
        Self {
            working_dir: config.working_directory.clone(),
            ..Self::default()
        }
    }

    /// Transition to a new state.
    pub fn transition(&mut self, new_state: StateType) {
        self.state = new_state;
        self.last_activity = Instant::now();
    }

    /// Check if in given state.
    pub fn is(&self, state: StateType) -> bool {
        self.state == state
    }

    /// Check if idle.
    pub fn is_idle(&self) -> bool {
        matches!(self.state, StateType::Idle)
    }

    /// Check if processing.
    pub fn is_processing(&self) -> bool {
        matches!(self.state, StateType::Processing | StateType::ToolExecution)
    }

    /// Get session duration.
    pub fn session_duration(&self) -> Duration {
        self.started_at.elapsed()
    }

    /// Get idle duration.
    pub fn idle_duration(&self) -> Duration {
        self.last_activity.elapsed()
    }

    /// Start a new turn.
    pub fn start_turn(&mut self) -> u64 {
        self.turn_id += 1;
        self.transition(StateType::Processing);
        self.turn_id
    }

    /// Complete the current turn.
    pub fn complete_turn(&mut self, turn: ConversationTurn) {
        self.tokens_used += turn.token_usage.total_tokens as u64;
        self.turns.push(turn);
        self.transition(StateType::Idle);
    }

    /// Add message.
    pub fn add_message(&mut self, message: Message) {
        self.messages.push(message);
        self.last_activity = Instant::now();
    }

    /// Clear conversation.
    pub fn clear(&mut self) {
        self.messages.clear();
        self.turns.clear();
        self.tokens_used = 0;
        self.turn_id = 0;
        self.transition(StateType::Idle);
    }

    /// Get custom value.
    pub fn get_custom<T: serde::de::DeserializeOwned>(&self, key: &str) -> Option<T> {
        self.custom
            .get(key)
            .and_then(|v| serde_json::from_value(v.clone()).ok())
    }

    /// Set custom value.
    pub fn set_custom<T: serde::Serialize>(&mut self, key: impl Into<String>, value: T) {
        if let Ok(v) = serde_json::to_value(value) {
            self.custom.insert(key.into(), v);
        }
    }

    /// Snapshot for persistence.
    pub fn snapshot(&self) -> StateSnapshot {
        StateSnapshot {
            turn_id: self.turn_id,
            tokens_used: self.tokens_used,
            turns: self.turns.clone(),
            working_dir: self.working_dir.clone(),
            custom: self.custom.clone(),
        }
    }

    /// Restore from snapshot.
    pub fn restore(&mut self, snapshot: StateSnapshot) {
        self.turn_id = snapshot.turn_id;
        self.tokens_used = snapshot.tokens_used;
        self.turns = snapshot.turns;
        self.working_dir = snapshot.working_dir;
        self.custom = snapshot.custom;
    }
}

/// State type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StateType {
    /// Agent is idle, waiting for input.
    Idle,
    /// Agent is processing user input.
    Processing,
    /// Agent is waiting for model response.
    WaitingForModel,
    /// Agent is executing a tool.
    ToolExecution,
    /// Agent is waiting for user approval.
    WaitingForApproval,
    /// Agent is compacting context.
    Compacting,
    /// Agent is shutting down.
    ShuttingDown,
    /// Agent has encountered an error.
    Error,
}

/// Turn state.
#[derive(Debug, Clone)]
pub struct TurnState {
    /// Turn ID.
    pub id: u64,
    /// Start time.
    pub started_at: Instant,
    /// Current iteration.
    pub iteration: u32,
    /// Tool calls made.
    pub tool_calls: Vec<String>,
    /// Tokens used so far.
    pub tokens_used: u32,
    /// Current status.
    pub status: TurnStatus,
    /// Errors encountered.
    pub errors: Vec<String>,
}

impl TurnState {
    /// Create new turn state.
    pub fn new(id: u64) -> Self {
        Self {
            id,
            started_at: Instant::now(),
            iteration: 0,
            tool_calls: Vec::new(),
            tokens_used: 0,
            status: TurnStatus::InProgress,
            errors: Vec::new(),
        }
    }

    /// Increment iteration.
    pub fn next_iteration(&mut self) {
        self.iteration += 1;
    }

    /// Record tool call.
    pub fn record_tool_call(&mut self, name: &str) {
        self.tool_calls.push(name.to_string());
    }

    /// Record tokens.
    pub fn record_tokens(&mut self, tokens: u32) {
        self.tokens_used += tokens;
    }

    /// Record error.
    pub fn record_error(&mut self, error: impl Into<String>) {
        self.errors.push(error.into());
    }

    /// Get duration.
    pub fn duration(&self) -> Duration {
        self.started_at.elapsed()
    }

    /// Complete turn.
    pub fn complete(&mut self) {
        self.status = TurnStatus::Completed;
    }

    /// Mark as failed.
    pub fn fail(&mut self) {
        self.status = TurnStatus::Failed;
    }

    /// Mark as cancelled.
    pub fn cancel(&mut self) {
        self.status = TurnStatus::Cancelled;
    }
}

/// State snapshot for persistence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateSnapshot {
    /// Current turn ID.
    pub turn_id: u64,
    /// Total tokens used.
    pub tokens_used: u64,
    /// Turn history.
    pub turns: Vec<ConversationTurn>,
    /// Working directory.
    pub working_dir: PathBuf,
    /// Custom data.
    pub custom: HashMap<String, serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_state_default() {
        let state = AgentState::new();
        assert!(state.is_idle());
        assert_eq!(state.turn_id, 0);
    }

    #[test]
    fn test_state_transitions() {
        let mut state = AgentState::new();

        state.transition(StateType::Processing);
        assert!(state.is(StateType::Processing));

        state.transition(StateType::ToolExecution);
        assert!(state.is(StateType::ToolExecution));
        assert!(state.is_processing());
    }

    #[test]
    fn test_turn_state() {
        let mut turn = TurnState::new(1);

        turn.next_iteration();
        turn.record_tool_call("shell");
        turn.record_tokens(100);

        assert_eq!(turn.iteration, 1);
        assert_eq!(turn.tool_calls.len(), 1);
        assert_eq!(turn.tokens_used, 100);
    }

    #[test]
    fn test_custom_state() {
        let mut state = AgentState::new();

        state.set_custom("key", "value");
        let value: Option<String> = state.get_custom("key");
        assert_eq!(value, Some("value".to_string()));
    }
}
