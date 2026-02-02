//! Submission Queue types for user -> agent communication.

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::config_types::ReasoningEffort;
use crate::config_types::ReasoningSummary;
use crate::user_input::UserInput;

use super::policies::{AskForApproval, ElicitationAction, ReviewDecision, SandboxPolicy};
use super::review::ReviewRequest;

/// Submission Queue Entry - requests from user to agent.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct Submission {
    /// Unique id for this Submission to correlate with Events.
    pub id: String,
    /// Payload operation.
    pub op: Op,
}

/// Submission operation types.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Op {
    /// Abort current task.
    Interrupt,

    /// Input from the user.
    UserInput { items: Vec<UserInput> },

    /// Full turn with context for a conversation.
    UserTurn {
        items: Vec<UserInput>,
        cwd: PathBuf,
        approval_policy: AskForApproval,
        sandbox_policy: SandboxPolicy,
        model: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        effort: Option<ReasoningEffort>,
        summary: ReasoningSummary,
        final_output_json_schema: Option<serde_json::Value>,
    },

    /// Override parts of the persistent turn context.
    OverrideTurnContext {
        #[serde(skip_serializing_if = "Option::is_none")]
        cwd: Option<PathBuf>,
        #[serde(skip_serializing_if = "Option::is_none")]
        approval_policy: Option<AskForApproval>,
        #[serde(skip_serializing_if = "Option::is_none")]
        sandbox_policy: Option<SandboxPolicy>,
        #[serde(skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        effort: Option<Option<ReasoningEffort>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        summary: Option<ReasoningSummary>,
    },

    /// Approve a command execution.
    ExecApproval {
        id: String,
        decision: ReviewDecision,
    },

    /// Approve a code patch.
    PatchApproval {
        id: String,
        decision: ReviewDecision,
    },

    /// Resolve an MCP elicitation request.
    ResolveElicitation {
        server_name: String,
        request_id: String,
        decision: ElicitationAction,
    },

    /// Add entry to persistent history.
    AddToHistory { text: String },

    /// Request history entry.
    GetHistoryEntryRequest { offset: usize, log_id: u64 },

    /// List available MCP tools.
    ListMcpTools,

    /// Reload all MCP servers.
    ReloadMcpServers,

    /// Enable an MCP server.
    EnableMcpServer { name: String },

    /// Disable an MCP server.
    DisableMcpServer { name: String },

    /// List available custom prompts.
    ListCustomPrompts,

    /// Compact conversation context.
    Compact,

    /// Undo last turn.
    Undo,

    /// Redo last undone turn.
    Redo,

    /// Fork current session at a specific point.
    ForkSession {
        #[serde(skip_serializing_if = "Option::is_none")]
        fork_point_message_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        message_index: Option<usize>,
    },

    /// Request session timeline info.
    GetSessionTimeline,

    /// Request code review.
    Review { review_request: ReviewRequest },

    /// Shutdown the agent.
    Shutdown,

    /// Switch current agent profile.
    SwitchAgent { name: String },

    /// Execute a user shell command (!cmd).
    RunUserShellCommand { command: String },

    /// Share current session.
    Share,

    /// Unshare current session.
    Unshare,
}
