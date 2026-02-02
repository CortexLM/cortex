//! Event Queue types for agent -> user communication.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use strum_macros::Display;

use crate::approvals::{ApplyPatchApprovalRequestEvent, ElicitationRequestEvent};

use super::event_payloads::*;
use super::mcp::*;
use super::review::{ExitedReviewModeEvent, ReviewRequest};
use super::tokens::TokenCountEvent;

/// Event Queue Entry - events from agent to UI.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Event {
    /// Submission id this event correlates with.
    pub id: String,
    /// Event payload.
    pub msg: EventMsg,
}

/// Response events from the agent.
#[derive(Debug, Clone, Deserialize, Serialize, Display, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
#[non_exhaustive]
pub enum EventMsg {
    // Lifecycle
    // Note: Large variants are boxed to reduce enum size on stack
    SessionConfigured(Box<SessionConfiguredEvent>),
    TaskStarted(TaskStartedEvent),
    TaskComplete(TaskCompleteEvent),
    TurnAborted(TurnAbortedEvent),
    ShutdownComplete,

    // Messages
    AgentMessage(AgentMessageEvent),
    AgentMessageDelta(AgentMessageDeltaEvent),
    UserMessage(UserMessageEvent),

    // Reasoning
    AgentReasoning(AgentReasoningEvent),
    AgentReasoningDelta(AgentReasoningDeltaEvent),
    AgentReasoningRawContent(AgentReasoningRawContentEvent),
    AgentReasoningRawContentDelta(AgentReasoningRawContentDeltaEvent),
    AgentReasoningSectionBreak(AgentReasoningSectionBreakEvent),

    // Execution
    ExecCommandBegin(ExecCommandBeginEvent),
    ExecCommandOutputDelta(ExecCommandOutputDeltaEvent),
    ExecCommandEnd(Box<ExecCommandEndEvent>),

    // Approvals
    ExecApprovalRequest(ExecApprovalRequestEvent),
    ApplyPatchApprovalRequest(ApplyPatchApprovalRequestEvent),
    ElicitationRequest(ElicitationRequestEvent),

    // MCP
    McpToolCallBegin(McpToolCallBeginEvent),
    McpToolCallEnd(McpToolCallEndEvent),
    McpStartupUpdate(McpStartupUpdateEvent),
    McpStartupComplete(McpStartupCompleteEvent),
    McpListToolsResponse(Box<McpListToolsResponseEvent>),

    // Patches
    PatchApplyBegin(PatchApplyBeginEvent),
    PatchApplyEnd(PatchApplyEndEvent),

    // Tokens
    TokenCount(TokenCountEvent),

    // Errors
    Error(ErrorEvent),
    Warning(WarningEvent),
    StreamError(StreamErrorEvent),

    // History & Context
    TurnDiff(TurnDiffEvent),
    GetHistoryEntryResponse(GetHistoryEntryResponseEvent),
    ListCustomPromptsResponse(ListCustomPromptsResponseEvent),
    BackgroundEvent(BackgroundEventEvent),

    // Undo
    UndoStarted(UndoStartedEvent),
    UndoCompleted(UndoCompletedEvent),

    // Redo
    RedoStarted(RedoStartedEvent),
    RedoCompleted(RedoCompletedEvent),

    // Review
    EnteredReviewMode(ReviewRequest),
    ExitedReviewMode(ExitedReviewModeEvent),

    // Timeline & Forking
    SessionForked(SessionForkedEvent),
    TimelineUpdated(TimelineUpdatedEvent),

    // Deprecation
    DeprecationNotice(DeprecationNoticeEvent),

    // Web Search
    WebSearchBegin(WebSearchBeginEvent),
    WebSearchEnd(WebSearchEndEvent),

    // Image
    ViewImageToolCall(ViewImageToolCallEvent),

    // Plan
    PlanUpdate(PlanUpdateEvent),

    // Share
    SessionShared(SessionSharedEvent),
    SessionUnshared(SessionUnsharedEvent),

    // Items (new unified events)
    ItemStarted(ItemStartedEvent),
    ItemCompleted(ItemCompletedEvent),
    AgentMessageContentDelta(AgentMessageContentDeltaEvent),
    ReasoningContentDelta(ReasoningContentDeltaEvent),
    ReasoningRawContentDelta(ReasoningRawContentDeltaEvent),
    RawResponseItem(RawResponseItemEvent),

    // Message Parts (rich content)
    MessageWithPartsCreated(MessageWithPartsCreatedEvent),
    MessageWithPartsCompleted(MessageWithPartsCompletedEvent),
    PartUpdated(PartUpdatedEvent),
    PartRemoved(PartRemovedEvent),
    PartDelta(PartDeltaEvent),

    /// Catch-all for unknown event types during deserialization.
    /// This prevents deserialization failures when new event types are added.
    #[serde(other)]
    Unknown,
}
