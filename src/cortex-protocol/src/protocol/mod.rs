//! Core protocol types for Cortex session communication.
//!
//! Uses a SQ (Submission Queue) / EQ (Event Queue) pattern to asynchronously
//! communicate between user and agent.

#![allow(clippy::collapsible_if)]

mod event_payloads;
mod events;
mod mcp;
mod message_parts;
mod policies;
mod review;
mod submission;
mod tokens;

#[cfg(test)]
mod tests;

// ============================================================
// Constants
// ============================================================

/// Open/close tags for special user-input blocks.
pub const USER_INSTRUCTIONS_OPEN_TAG: &str = "<user_instructions>";
pub const USER_INSTRUCTIONS_CLOSE_TAG: &str = "</user_instructions>";
pub const ENVIRONMENT_CONTEXT_OPEN_TAG: &str = "<environment_context>";
pub const ENVIRONMENT_CONTEXT_CLOSE_TAG: &str = "</environment_context>";
pub const USER_MESSAGE_BEGIN: &str = "## My request for Cortex:";

// ============================================================
// Re-exports: Submission Queue
// ============================================================

pub use submission::{Op, Submission};

// ============================================================
// Re-exports: Event Queue
// ============================================================

pub use events::{Event, EventMsg};

// ============================================================
// Re-exports: Policies
// ============================================================

pub use policies::{
    AskForApproval, ElicitationAction, ReviewDecision, SandboxPolicy, WritableRoot,
};

// ============================================================
// Re-exports: Message Parts
// ============================================================

pub use message_parts::{
    FileAttachment, FilePartSource, IndexedPart, LineRange, MessagePart, MessagePartError,
    MessageRole, MessageWithParts, PartDelta, PartTiming, SubtaskStatus, TextRange, ToolState,
};

// ============================================================
// Re-exports: Tokens
// ============================================================

pub use tokens::{
    CreditsSnapshot, RateLimitSnapshot, RateLimitWindow, TokenCountEvent, TokenUsage,
    TokenUsageInfo,
};

// ============================================================
// Re-exports: MCP
// ============================================================

pub use mcp::{
    McpAuthStatus, McpContent, McpInvocation, McpListToolsResponseEvent, McpResource,
    McpResourceTemplate, McpStartupCompleteEvent, McpStartupFailure, McpStartupStatus,
    McpStartupUpdateEvent, McpToolCallBeginEvent, McpToolCallEndEvent, McpToolDefinition,
    McpToolResult,
};

// ============================================================
// Re-exports: Review
// ============================================================

pub use review::{
    ExitedReviewModeEvent, ReviewCodeLocation, ReviewFinding, ReviewLineRange, ReviewOutputEvent,
    ReviewRequest,
};

// ============================================================
// Re-exports: Event Payloads
// ============================================================

pub use event_payloads::{
    AgentMessageContentDeltaEvent, AgentMessageDeltaEvent, AgentMessageEvent,
    AgentReasoningDeltaEvent, AgentReasoningEvent, AgentReasoningRawContentDeltaEvent,
    AgentReasoningRawContentEvent, AgentReasoningSectionBreakEvent, BackgroundEventEvent,
    CortexErrorInfo, CustomPrompt, DeprecationNoticeEvent, ErrorEvent, ExecApprovalRequestEvent,
    ExecCommandBeginEvent, ExecCommandEndEvent, ExecCommandOutputDeltaEvent, ExecCommandSource,
    ExecOutputStream, FileChange, GetHistoryEntryResponseEvent, HistoryEntry, ItemCompletedEvent,
    ItemStartedEvent, ListCustomPromptsResponseEvent, MessageWithPartsCompletedEvent,
    MessageWithPartsCreatedEvent, ParsedCommand, PartDeltaEvent, PartRemovedEvent,
    PartUpdatedEvent, PatchApplyBeginEvent, PatchApplyEndEvent, PlanItem, PlanItemStatus,
    PlanUpdateEvent, RawResponseItemEvent, ReasoningContentDeltaEvent,
    ReasoningRawContentDeltaEvent, RedoCompletedEvent, RedoStartedEvent, SandboxCommandAssessment,
    SandboxRiskLevel, SessionConfiguredEvent, SessionForkedEvent, SessionSharedEvent,
    SessionUnsharedEvent, StreamErrorEvent, TaskCompleteEvent, TaskStartedEvent,
    TimelineUpdatedEvent, TurnAbortReason, TurnAbortedEvent, TurnDiffEvent, UndoCompletedEvent,
    UndoStartedEvent, UserMessageEvent, ViewImageToolCallEvent, WarningEvent, WebSearchBeginEvent,
    WebSearchEndEvent,
};
