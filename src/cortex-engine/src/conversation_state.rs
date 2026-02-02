//! Conversation state machine.
//!
//! Manages the state and transitions of agent conversations.

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::error::{CortexError, Result};

/// Conversation phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum ConversationPhase {
    /// Initial state, awaiting first input.
    #[default]
    Idle,
    /// Waiting for user input.
    AwaitingInput,
    /// Processing user input.
    ProcessingInput,
    /// Calling the model.
    CallingModel,
    /// Streaming model response.
    StreamingResponse,
    /// Executing tool calls.
    ExecutingTools,
    /// Waiting for tool approval.
    AwaitingApproval,
    /// Processing tool results.
    ProcessingResults,
    /// Compacting context.
    Compacting,
    /// Error occurred.
    Error,
    /// Conversation ended.
    Ended,
    /// Paused by user.
    Paused,
}

impl ConversationPhase {
    /// Check if this is a terminal state.
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Ended | Self::Error)
    }

    /// Check if this state allows user input.
    pub fn accepts_input(&self) -> bool {
        matches!(self, Self::Idle | Self::AwaitingInput | Self::Paused)
    }

    /// Check if this state is active/processing.
    pub fn is_active(&self) -> bool {
        matches!(
            self,
            Self::ProcessingInput
                | Self::CallingModel
                | Self::StreamingResponse
                | Self::ExecutingTools
                | Self::ProcessingResults
                | Self::Compacting
        )
    }
}

/// Conversation event.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum ConversationEvent {
    /// User submitted input.
    UserInput { text: String },
    /// Started calling model.
    ModelCallStart,
    /// Model response chunk received.
    ModelChunk { delta: String },
    /// Model response complete.
    ModelComplete { response: String },
    /// Tool call requested.
    ToolCallRequested {
        tool: String,
        args: serde_json::Value,
    },
    /// Tool call approved.
    ToolApproved { tool_call_id: String },
    /// Tool call denied.
    ToolDenied {
        tool_call_id: String,
        reason: String,
    },
    /// Tool call completed.
    ToolCompleted {
        tool_call_id: String,
        result: String,
    },
    /// Tool call failed.
    ToolFailed { tool_call_id: String, error: String },
    /// Context compaction started.
    CompactionStart,
    /// Context compaction completed.
    CompactionComplete { tokens_saved: u32 },
    /// Error occurred.
    Error { message: String },
    /// User requested pause.
    Pause,
    /// User requested resume.
    Resume,
    /// User requested abort.
    Abort,
    /// Conversation ended normally.
    End,
}

/// State transition result.
#[derive(Debug, Clone)]
pub struct TransitionResult {
    /// Previous phase.
    pub from: ConversationPhase,
    /// New phase.
    pub to: ConversationPhase,
    /// Whether transition was valid.
    pub valid: bool,
    /// Error message if invalid.
    pub error: Option<String>,
}

impl TransitionResult {
    /// Create a valid transition.
    fn valid(from: ConversationPhase, to: ConversationPhase) -> Self {
        Self {
            from,
            to,
            valid: true,
            error: None,
        }
    }

    /// Create an invalid transition.
    fn invalid(from: ConversationPhase, to: ConversationPhase, error: impl Into<String>) -> Self {
        Self {
            from,
            to,
            valid: false,
            error: Some(error.into()),
        }
    }
}

/// Conversation state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationState {
    /// Conversation ID.
    pub id: String,
    /// Current phase.
    pub phase: ConversationPhase,
    /// Turn count.
    pub turn_count: u32,
    /// Total tokens used.
    pub total_tokens: u64,
    /// Created timestamp.
    pub created_at: u64,
    /// Last activity timestamp.
    pub last_activity: u64,
    /// Current tool calls.
    pub pending_tool_calls: Vec<PendingToolCall>,
    /// Error if in error state.
    pub error: Option<String>,
    /// Metadata.
    pub metadata: HashMap<String, String>,
}

impl ConversationState {
    /// Create a new conversation state.
    pub fn new(id: impl Into<String>) -> Self {
        let now = timestamp_now();
        Self {
            id: id.into(),
            phase: ConversationPhase::Idle,
            turn_count: 0,
            total_tokens: 0,
            created_at: now,
            last_activity: now,
            pending_tool_calls: Vec::new(),
            error: None,
            metadata: HashMap::new(),
        }
    }

    /// Apply an event and transition state.
    pub fn apply(&mut self, event: ConversationEvent) -> TransitionResult {
        let from = self.phase;
        self.last_activity = timestamp_now();

        let result = match event {
            ConversationEvent::UserInput { .. } => self.handle_user_input(from),
            ConversationEvent::ModelCallStart => self.handle_model_call_start(from),
            ConversationEvent::ModelChunk { .. } => self.handle_model_chunk(from),
            ConversationEvent::ModelComplete { .. } => self.handle_model_complete(from),
            ConversationEvent::ToolCallRequested { tool, args } => {
                self.handle_tool_requested(from, &tool, args)
            }
            ConversationEvent::ToolApproved { tool_call_id } => {
                self.handle_tool_approved(from, &tool_call_id)
            }
            ConversationEvent::ToolDenied {
                tool_call_id,
                reason,
            } => self.handle_tool_denied(from, &tool_call_id, &reason),
            ConversationEvent::ToolCompleted { tool_call_id, .. } => {
                self.handle_tool_completed(from, &tool_call_id)
            }
            ConversationEvent::ToolFailed {
                tool_call_id,
                error,
            } => self.handle_tool_failed(from, &tool_call_id, &error),
            ConversationEvent::CompactionStart => self.handle_compaction_start(from),
            ConversationEvent::CompactionComplete { tokens_saved } => {
                self.handle_compaction_complete(from, tokens_saved)
            }
            ConversationEvent::Error { message } => self.handle_error(from, &message),
            ConversationEvent::Pause => self.handle_pause(from),
            ConversationEvent::Resume => self.handle_resume(from),
            ConversationEvent::Abort => self.handle_abort(from),
            ConversationEvent::End => self.handle_end(from),
        };

        if result.valid {
            self.phase = result.to;
        }

        result
    }

    fn handle_user_input(&mut self, from: ConversationPhase) -> TransitionResult {
        if from.accepts_input() {
            self.turn_count += 1;
            TransitionResult::valid(from, ConversationPhase::ProcessingInput)
        } else {
            TransitionResult::invalid(
                from,
                ConversationPhase::ProcessingInput,
                "Cannot accept input in current state",
            )
        }
    }

    fn handle_model_call_start(&self, from: ConversationPhase) -> TransitionResult {
        if from == ConversationPhase::ProcessingInput
            || from == ConversationPhase::ProcessingResults
        {
            TransitionResult::valid(from, ConversationPhase::CallingModel)
        } else {
            TransitionResult::invalid(
                from,
                ConversationPhase::CallingModel,
                "Cannot call model in current state",
            )
        }
    }

    fn handle_model_chunk(&self, from: ConversationPhase) -> TransitionResult {
        if from == ConversationPhase::CallingModel || from == ConversationPhase::StreamingResponse {
            TransitionResult::valid(from, ConversationPhase::StreamingResponse)
        } else {
            TransitionResult::invalid(
                from,
                ConversationPhase::StreamingResponse,
                "Not expecting model chunks",
            )
        }
    }

    fn handle_model_complete(&mut self, from: ConversationPhase) -> TransitionResult {
        if from == ConversationPhase::CallingModel || from == ConversationPhase::StreamingResponse {
            if self.pending_tool_calls.is_empty() {
                TransitionResult::valid(from, ConversationPhase::AwaitingInput)
            } else {
                TransitionResult::valid(from, ConversationPhase::ExecutingTools)
            }
        } else {
            TransitionResult::invalid(
                from,
                ConversationPhase::AwaitingInput,
                "Not expecting model completion",
            )
        }
    }

    fn handle_tool_requested(
        &mut self,
        from: ConversationPhase,
        tool: &str,
        args: serde_json::Value,
    ) -> TransitionResult {
        if from == ConversationPhase::StreamingResponse || from == ConversationPhase::CallingModel {
            self.pending_tool_calls.push(PendingToolCall {
                id: generate_id(),
                tool: tool.to_string(),
                arguments: args,
                status: ToolCallStatus::Pending,
            });
            TransitionResult::valid(from, ConversationPhase::AwaitingApproval)
        } else {
            TransitionResult::invalid(
                from,
                ConversationPhase::AwaitingApproval,
                "Not expecting tool requests",
            )
        }
    }

    fn handle_tool_approved(&mut self, from: ConversationPhase, id: &str) -> TransitionResult {
        if from == ConversationPhase::AwaitingApproval {
            if let Some(tc) = self.pending_tool_calls.iter_mut().find(|tc| tc.id == id) {
                tc.status = ToolCallStatus::Running;
            }
            TransitionResult::valid(from, ConversationPhase::ExecutingTools)
        } else {
            TransitionResult::invalid(
                from,
                ConversationPhase::ExecutingTools,
                "Not waiting for approval",
            )
        }
    }

    fn handle_tool_denied(
        &mut self,
        from: ConversationPhase,
        id: &str,
        _reason: &str,
    ) -> TransitionResult {
        if from == ConversationPhase::AwaitingApproval {
            self.pending_tool_calls.retain(|tc| tc.id != id);
            if self.pending_tool_calls.is_empty() {
                TransitionResult::valid(from, ConversationPhase::ProcessingResults)
            } else {
                TransitionResult::valid(from, ConversationPhase::AwaitingApproval)
            }
        } else {
            TransitionResult::invalid(from, from, "Not waiting for approval")
        }
    }

    fn handle_tool_completed(&mut self, from: ConversationPhase, id: &str) -> TransitionResult {
        if from == ConversationPhase::ExecutingTools {
            self.pending_tool_calls.retain(|tc| tc.id != id);
            if self.pending_tool_calls.is_empty() {
                TransitionResult::valid(from, ConversationPhase::ProcessingResults)
            } else {
                TransitionResult::valid(from, ConversationPhase::ExecutingTools)
            }
        } else {
            TransitionResult::invalid(from, from, "Not executing tools")
        }
    }

    fn handle_tool_failed(
        &mut self,
        from: ConversationPhase,
        id: &str,
        _error: &str,
    ) -> TransitionResult {
        if from == ConversationPhase::ExecutingTools {
            self.pending_tool_calls.retain(|tc| tc.id != id);
            if self.pending_tool_calls.is_empty() {
                TransitionResult::valid(from, ConversationPhase::ProcessingResults)
            } else {
                TransitionResult::valid(from, ConversationPhase::ExecutingTools)
            }
        } else {
            TransitionResult::invalid(from, from, "Not executing tools")
        }
    }

    fn handle_compaction_start(&self, from: ConversationPhase) -> TransitionResult {
        TransitionResult::valid(from, ConversationPhase::Compacting)
    }

    fn handle_compaction_complete(
        &self,
        from: ConversationPhase,
        _tokens_saved: u32,
    ) -> TransitionResult {
        if from == ConversationPhase::Compacting {
            TransitionResult::valid(from, ConversationPhase::AwaitingInput)
        } else {
            TransitionResult::invalid(from, ConversationPhase::AwaitingInput, "Not compacting")
        }
    }

    fn handle_error(&mut self, from: ConversationPhase, message: &str) -> TransitionResult {
        self.error = Some(message.to_string());
        TransitionResult::valid(from, ConversationPhase::Error)
    }

    fn handle_pause(&self, from: ConversationPhase) -> TransitionResult {
        if from.is_active() || from == ConversationPhase::AwaitingInput {
            TransitionResult::valid(from, ConversationPhase::Paused)
        } else {
            TransitionResult::invalid(
                from,
                ConversationPhase::Paused,
                "Cannot pause in current state",
            )
        }
    }

    fn handle_resume(&self, from: ConversationPhase) -> TransitionResult {
        if from == ConversationPhase::Paused {
            TransitionResult::valid(from, ConversationPhase::AwaitingInput)
        } else {
            TransitionResult::invalid(from, ConversationPhase::AwaitingInput, "Not paused")
        }
    }

    fn handle_abort(&mut self, from: ConversationPhase) -> TransitionResult {
        self.pending_tool_calls.clear();
        TransitionResult::valid(from, ConversationPhase::Ended)
    }

    fn handle_end(&mut self, from: ConversationPhase) -> TransitionResult {
        if from == ConversationPhase::AwaitingInput || from == ConversationPhase::Idle {
            TransitionResult::valid(from, ConversationPhase::Ended)
        } else {
            TransitionResult::invalid(from, ConversationPhase::Ended, "Conversation still active")
        }
    }

    /// Get duration.
    pub fn duration(&self) -> Duration {
        let now = timestamp_now();
        Duration::from_secs(now.saturating_sub(self.created_at))
    }

    /// Add tokens.
    pub fn add_tokens(&mut self, count: u64) {
        self.total_tokens += count;
    }

    /// Set metadata.
    pub fn set_metadata(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.metadata.insert(key.into(), value.into());
    }
}

/// Pending tool call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingToolCall {
    /// Call ID.
    pub id: String,
    /// Tool name.
    pub tool: String,
    /// Arguments.
    pub arguments: serde_json::Value,
    /// Status.
    pub status: ToolCallStatus,
}

/// Tool call status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum ToolCallStatus {
    /// Pending approval.
    #[default]
    Pending,
    /// Approved and running.
    Running,
    /// Completed.
    Completed,
    /// Failed.
    Failed,
    /// Denied.
    Denied,
}

/// Conversation state manager.
pub struct ConversationStateManager {
    /// Conversations indexed by ID.
    conversations: RwLock<HashMap<String, ConversationState>>,
}

impl ConversationStateManager {
    /// Create a new manager.
    pub fn new() -> Self {
        Self {
            conversations: RwLock::new(HashMap::new()),
        }
    }

    /// Create a new conversation.
    pub async fn create(&self, id: impl Into<String>) -> ConversationState {
        let state = ConversationState::new(id);
        let id = state.id.clone();
        self.conversations
            .write()
            .await
            .insert(id.clone(), state.clone());
        state
    }

    /// Get a conversation.
    pub async fn get(&self, id: &str) -> Option<ConversationState> {
        self.conversations.read().await.get(id).cloned()
    }

    /// Apply an event to a conversation.
    pub async fn apply_event(
        &self,
        id: &str,
        event: ConversationEvent,
    ) -> Result<TransitionResult> {
        let mut conversations = self.conversations.write().await;
        let state = conversations
            .get_mut(id)
            .ok_or_else(|| CortexError::NotFound(format!("Conversation not found: {id}")))?;
        Ok(state.apply(event))
    }

    /// Remove a conversation.
    pub async fn remove(&self, id: &str) {
        self.conversations.write().await.remove(id);
    }

    /// List all conversations.
    pub async fn list(&self) -> Vec<ConversationState> {
        self.conversations.read().await.values().cloned().collect()
    }

    /// Get count.
    pub async fn count(&self) -> usize {
        self.conversations.read().await.len()
    }
}

impl Default for ConversationStateManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Generate unique ID.
fn generate_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros())
        .unwrap_or(0);
    format!("tc_{ts:x}")
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
    fn test_state_transitions() {
        let mut state = ConversationState::new("test");

        // Start idle
        assert_eq!(state.phase, ConversationPhase::Idle);

        // User input
        let result = state.apply(ConversationEvent::UserInput {
            text: "Hello".to_string(),
        });
        assert!(result.valid);
        assert_eq!(state.phase, ConversationPhase::ProcessingInput);

        // Model call
        let result = state.apply(ConversationEvent::ModelCallStart);
        assert!(result.valid);
        assert_eq!(state.phase, ConversationPhase::CallingModel);

        // Model complete
        let result = state.apply(ConversationEvent::ModelComplete {
            response: "Hi".to_string(),
        });
        assert!(result.valid);
        assert_eq!(state.phase, ConversationPhase::AwaitingInput);
    }

    #[test]
    fn test_tool_flow() {
        let mut state = ConversationState::new("test");

        state.apply(ConversationEvent::UserInput {
            text: "Read file".to_string(),
        });
        state.apply(ConversationEvent::ModelCallStart);
        state.apply(ConversationEvent::ToolCallRequested {
            tool: "read_file".to_string(),
            args: serde_json::json!({"path": "/test"}),
        });

        assert_eq!(state.phase, ConversationPhase::AwaitingApproval);
        assert_eq!(state.pending_tool_calls.len(), 1);

        let id = state.pending_tool_calls[0].id.clone();
        state.apply(ConversationEvent::ToolApproved {
            tool_call_id: id.clone(),
        });
        assert_eq!(state.phase, ConversationPhase::ExecutingTools);

        state.apply(ConversationEvent::ToolCompleted {
            tool_call_id: id,
            result: "content".to_string(),
        });
        assert_eq!(state.phase, ConversationPhase::ProcessingResults);
    }

    #[test]
    fn test_invalid_transition() {
        let mut state = ConversationState::new("test");

        // Can't start model call from idle
        let result = state.apply(ConversationEvent::ModelCallStart);
        assert!(!result.valid);
        assert_eq!(state.phase, ConversationPhase::Idle);
    }

    #[tokio::test]
    async fn test_state_manager() {
        let manager = ConversationStateManager::new();

        let state = manager.create("conv-1").await;
        assert_eq!(state.id, "conv-1");

        assert_eq!(manager.count().await, 1);

        manager
            .apply_event(
                "conv-1",
                ConversationEvent::UserInput {
                    text: "Hi".to_string(),
                },
            )
            .await
            .unwrap();

        let updated = manager.get("conv-1").await.unwrap();
        assert_eq!(updated.phase, ConversationPhase::ProcessingInput);
    }
}
