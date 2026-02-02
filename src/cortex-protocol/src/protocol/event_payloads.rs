//! Event payload structures for agent -> user communication.

use std::collections::HashMap;
use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::config_types::ReasoningEffort;
use crate::conversation_id::ConversationId;
use crate::items::TurnItem;
use crate::models::ResponseItem;

use super::message_parts::{MessageWithParts, PartDelta, PartTiming};
use super::policies::{AskForApproval, SandboxPolicy};
use super::tokens::TokenUsage;

// ============================================================
// Session Lifecycle Events
// ============================================================

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct SessionConfiguredEvent {
    pub session_id: ConversationId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<ConversationId>,
    pub model: String,
    pub model_provider_id: String,
    pub approval_policy: AskForApproval,
    pub sandbox_policy: SandboxPolicy,
    pub cwd: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<ReasoningEffort>,
    pub history_log_id: u64,
    pub history_entry_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initial_messages: Option<Vec<super::events::EventMsg>>,
    pub rollout_path: PathBuf,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct TaskStartedEvent {
    pub model_context_window: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct TaskCompleteEvent {
    pub last_agent_message: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct TurnAbortedEvent {
    pub reason: TurnAbortReason,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TurnAbortReason {
    Interrupted,
    Replaced,
    ReviewEnded,
}

// ============================================================
// Error Events
// ============================================================

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ErrorEvent {
    pub message: String,
    #[serde(default)]
    pub cortex_error_info: Option<CortexErrorInfo>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CortexErrorInfo {
    ContextWindowExceeded,
    UsageLimitExceeded,
    HttpConnectionFailed { http_status_code: Option<u16> },
    ResponseStreamConnectionFailed { http_status_code: Option<u16> },
    InternalServerError,
    Unauthorized,
    BadRequest,
    SandboxError,
    ResponseStreamDisconnected { http_status_code: Option<u16> },
    ResponseTooManyFailedAttempts { http_status_code: Option<u16> },
    Other,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct WarningEvent {
    pub message: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct StreamErrorEvent {
    pub message: String,
    #[serde(default)]
    pub cortex_error_info: Option<CortexErrorInfo>,
}

// ============================================================
// Message Events
// ============================================================

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct AgentMessageEvent {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    pub message: String,
    /// Reason for message completion (e.g., "stop", "length" for truncation)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct AgentMessageDeltaEvent {
    pub delta: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct UserMessageEvent {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub images: Option<Vec<String>>,
}

// ============================================================
// Reasoning Events
// ============================================================

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct AgentReasoningEvent {
    pub text: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct AgentReasoningDeltaEvent {
    pub delta: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct AgentReasoningRawContentEvent {
    pub text: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct AgentReasoningRawContentDeltaEvent {
    pub delta: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct AgentReasoningSectionBreakEvent {
    #[serde(default)]
    pub item_id: String,
    #[serde(default)]
    pub summary_index: i64,
}

// ============================================================
// Execution Events
// ============================================================

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum ExecCommandSource {
    #[default]
    Agent,
    UserShell,
    UnifiedExecStartup,
    UnifiedExecInteraction,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ExecCommandBeginEvent {
    pub call_id: String,
    pub turn_id: String,
    pub command: Vec<String>,
    pub cwd: PathBuf,
    pub parsed_cmd: Vec<ParsedCommand>,
    #[serde(default)]
    pub source: ExecCommandSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interaction_input: Option<String>,
    /// Tool name (e.g., "Read", "Edit", "Execute")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    /// Tool arguments as JSON
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_arguments: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ParsedCommand {
    pub program: String,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExecOutputStream {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ExecCommandOutputDeltaEvent {
    pub call_id: String,
    pub stream: ExecOutputStream,
    /// Base64-encoded chunk of output data.
    pub chunk: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ExecCommandEndEvent {
    pub call_id: String,
    pub turn_id: String,
    pub command: Vec<String>,
    pub cwd: PathBuf,
    pub parsed_cmd: Vec<ParsedCommand>,
    #[serde(default)]
    pub source: ExecCommandSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interaction_input: Option<String>,
    pub stdout: String,
    pub stderr: String,
    #[serde(default)]
    pub aggregated_output: String,
    pub exit_code: i32,
    /// Duration in milliseconds.
    pub duration_ms: u64,
    pub formatted_output: String,
    /// Structured metadata from tool execution (file info, entries, etc.)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

// ============================================================
// Approval Events
// ============================================================

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ExecApprovalRequestEvent {
    pub call_id: String,
    pub turn_id: String,
    pub command: Vec<String>,
    pub cwd: PathBuf,
    #[serde(default)]
    pub sandbox_assessment: Option<SandboxCommandAssessment>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct SandboxCommandAssessment {
    pub risk_level: SandboxRiskLevel,
    pub explanation: String,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SandboxRiskLevel {
    Low,
    Medium,
    High,
}

// ============================================================
// Patch Events
// ============================================================

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct PatchApplyBeginEvent {
    pub call_id: String,
    #[serde(default)]
    pub turn_id: String,
    pub auto_approved: bool,
    pub changes: HashMap<PathBuf, FileChange>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct PatchApplyEndEvent {
    pub call_id: String,
    #[serde(default)]
    pub turn_id: String,
    pub stdout: String,
    pub stderr: String,
    pub success: bool,
    #[serde(default)]
    pub changes: HashMap<PathBuf, FileChange>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FileChange {
    Add {
        content: String,
    },
    Delete {
        content: String,
    },
    Update {
        unified_diff: String,
        move_path: Option<PathBuf>,
    },
}

// ============================================================
// History & Context Events
// ============================================================

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct TurnDiffEvent {
    pub unified_diff: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct GetHistoryEntryResponseEvent {
    pub offset: usize,
    pub log_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entry: Option<HistoryEntry>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct HistoryEntry {
    pub text: String,
    pub timestamp: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ListCustomPromptsResponseEvent {
    pub custom_prompts: Vec<CustomPrompt>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct CustomPrompt {
    pub name: String,
    pub prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct BackgroundEventEvent {
    pub message: String,
}

// ============================================================
// Undo/Redo Events
// ============================================================

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct UndoStartedEvent {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct UndoCompletedEvent {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct RedoStartedEvent {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct RedoCompletedEvent {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

// ============================================================
// Timeline & Forking Events
// ============================================================

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct SessionForkedEvent {
    pub new_session_id: ConversationId,
    pub parent_session_id: ConversationId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fork_point_message_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct TimelineUpdatedEvent {
    pub session_id: ConversationId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<ConversationId>,
    pub child_session_ids: Vec<ConversationId>,
}

// ============================================================
// Deprecation Event
// ============================================================

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct DeprecationNoticeEvent {
    pub summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
}

// ============================================================
// Web Search Events
// ============================================================

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct WebSearchBeginEvent {
    pub call_id: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct WebSearchEndEvent {
    pub call_id: String,
    pub query: String,
}

// ============================================================
// Image Event
// ============================================================

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ViewImageToolCallEvent {
    pub call_id: String,
    pub path: PathBuf,
}

// ============================================================
// Plan Events
// ============================================================

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct PlanUpdateEvent {
    pub plan: Vec<PlanItem>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct PlanItem {
    pub id: String,
    pub title: String,
    pub status: PlanItemStatus,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PlanItemStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
}

// ============================================================
// Share Events
// ============================================================

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct SessionSharedEvent {
    pub url: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct SessionUnsharedEvent {
    pub success: bool,
}

// ============================================================
// Unified Item Events
// ============================================================

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ItemStartedEvent {
    pub thread_id: ConversationId,
    pub turn_id: String,
    pub item: TurnItem,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ItemCompletedEvent {
    pub thread_id: ConversationId,
    pub turn_id: String,
    pub item: TurnItem,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct AgentMessageContentDeltaEvent {
    pub thread_id: String,
    pub turn_id: String,
    pub item_id: String,
    pub delta: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ReasoningContentDeltaEvent {
    pub thread_id: String,
    pub turn_id: String,
    pub item_id: String,
    pub delta: String,
    #[serde(default)]
    pub summary_index: i64,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ReasoningRawContentDeltaEvent {
    pub thread_id: String,
    pub turn_id: String,
    pub item_id: String,
    pub delta: String,
    #[serde(default)]
    pub content_index: i64,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct RawResponseItemEvent {
    pub item: ResponseItem,
}

// ============================================================
// Message Parts Events
// ============================================================

/// Event emitted when a new message with parts is created.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct MessageWithPartsCreatedEvent {
    /// Session ID.
    pub session_id: ConversationId,
    /// The new message.
    pub message: MessageWithParts,
}

/// Event emitted when a message with parts is completed.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct MessageWithPartsCompletedEvent {
    /// Session ID.
    pub session_id: ConversationId,
    /// Message ID.
    pub message_id: String,
    /// Token usage.
    pub tokens: TokenUsage,
    /// Cost in USD.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost: Option<f64>,
    /// Finish reason.
    pub finish_reason: String,
}

/// Event emitted when a message part is updated.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct PartUpdatedEvent {
    /// Session ID.
    pub session_id: ConversationId,
    /// Message ID.
    pub message_id: String,
    /// Part index.
    pub part_index: usize,
    /// Part ID.
    pub part_id: String,
    /// The updated part.
    pub part: super::message_parts::MessagePart,
    /// Timing information.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timing: Option<PartTiming>,
}

/// Event emitted when a message part is removed.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct PartRemovedEvent {
    /// Session ID.
    pub session_id: ConversationId,
    /// Message ID.
    pub message_id: String,
    /// Part index that was removed.
    pub part_index: usize,
    /// Part ID that was removed.
    pub part_id: String,
}

/// Event emitted for streaming part content deltas.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct PartDeltaEvent {
    /// Session ID.
    pub session_id: ConversationId,
    /// Message ID.
    pub message_id: String,
    /// Part index.
    pub part_index: usize,
    /// Part ID.
    pub part_id: String,
    /// The delta content.
    pub delta: PartDelta,
}
