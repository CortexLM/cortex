//! Event adapter helper functions.
//!
//! This module contains individual adapter functions that convert specific
//! cortex_protocol event types to cortex-tui AppEvents.

use std::path::PathBuf;

use crate::events::AppEvent;
use cortex_core::widgets::{Message, MessageRole};
use cortex_protocol::{
    AgentMessageDeltaEvent, AgentMessageEvent, AgentReasoningDeltaEvent,
    ApplyPatchApprovalRequestEvent, ErrorEvent, ExecApprovalRequestEvent, ExecCommandBeginEvent,
    ExecCommandEndEvent, ExecCommandOutputDeltaEvent, McpToolCallBeginEvent, McpToolCallEndEvent,
    SessionConfiguredEvent, TaskCompleteEvent, TokenCountEvent, TurnAbortedEvent, TurnDiffEvent,
    UserMessageEvent, WarningEvent,
};

use super::utils::{decode_output_chunk, format_command, parse_uuid};

// ============================================================================
// SESSION LIFECYCLE ADAPTERS
// ============================================================================

/// Adapt a SessionConfigured event.
pub fn adapt_session_configured(e: &SessionConfiguredEvent) -> Option<AppEvent> {
    let session_uuid = parse_uuid(&e.session_id.to_string())?;
    Some(AppEvent::SessionCreated(session_uuid))
}

/// Adapt a TaskComplete event.
pub fn adapt_task_complete(e: &TaskCompleteEvent) -> Option<AppEvent> {
    if let Some(ref msg) = e.last_agent_message {
        let message = Message::assistant(msg);
        Some(AppEvent::MessageReceived(message))
    } else {
        Some(AppEvent::StreamingCompleted)
    }
}

/// Adapt a TurnAborted event.
pub fn adapt_turn_aborted(e: &TurnAbortedEvent) -> Option<AppEvent> {
    let reason = match e.reason {
        cortex_protocol::TurnAbortReason::Interrupted => "Turn interrupted by user",
        cortex_protocol::TurnAbortReason::Replaced => "Turn replaced by new request",
        cortex_protocol::TurnAbortReason::ReviewEnded => "Review mode ended",
    };
    Some(AppEvent::StreamingError(reason.to_string()))
}

// ============================================================================
// MESSAGE ADAPTERS
// ============================================================================

/// Adapt a UserMessage event.
pub fn adapt_user_message(e: &UserMessageEvent) -> Option<AppEvent> {
    let message = Message::user(&e.message);
    Some(AppEvent::MessageReceived(message))
}

/// Adapt an AgentMessageDelta event.
pub fn adapt_agent_message_delta(e: &AgentMessageDeltaEvent) -> Option<AppEvent> {
    Some(AppEvent::StreamingChunk(e.delta.clone()))
}

/// Adapt an AgentMessage event.
pub fn adapt_agent_message(e: &AgentMessageEvent) -> Option<AppEvent> {
    let message = Message::assistant(&e.message);
    Some(AppEvent::MessageReceived(message))
}

/// Adapt an AgentReasoningDelta event.
pub fn adapt_agent_reasoning_delta(e: &AgentReasoningDeltaEvent) -> Option<AppEvent> {
    // Reasoning deltas can be treated as streaming chunks
    // Could create a separate ReasoningChunk event type if needed
    Some(AppEvent::StreamingChunk(e.delta.clone()))
}

// ============================================================================
// TOOL EXECUTION ADAPTERS
// ============================================================================

/// Adapt an ExecCommandBegin event.
pub fn adapt_exec_command_begin(e: &ExecCommandBeginEvent) -> Option<AppEvent> {
    let name = e.tool_name.clone().unwrap_or_else(|| "shell".to_string());
    let args = e.tool_arguments.clone().unwrap_or_else(|| {
        serde_json::json!({
            "command": e.command.join(" "),
            "cwd": e.cwd.display().to_string(),
        })
    });
    Some(AppEvent::ToolStarted { name, args })
}

/// Adapt an ExecCommandOutputDelta event.
pub fn adapt_exec_command_output_delta(e: &ExecCommandOutputDeltaEvent) -> Option<AppEvent> {
    let chunk = decode_output_chunk(&e.chunk);
    if chunk.is_empty() {
        return None;
    }
    Some(AppEvent::ToolProgress {
        name: e.call_id.clone(),
        status: chunk,
    })
}

/// Adapt an ExecCommandEnd event.
pub fn adapt_exec_command_end(e: &ExecCommandEndEvent) -> Option<AppEvent> {
    let result = if e.exit_code == 0 {
        format!("Completed in {}ms", e.duration_ms)
    } else {
        format!("Exit code: {} ({}ms)", e.exit_code, e.duration_ms)
    };
    Some(AppEvent::ToolCompleted {
        name: e.call_id.clone(),
        result,
    })
}

// ============================================================================
// APPROVAL REQUEST ADAPTERS
// ============================================================================

/// Adapt an ExecApprovalRequest event.
pub fn adapt_exec_approval_request(e: &ExecApprovalRequestEvent) -> Option<AppEvent> {
    let name = format_command(&e.command);
    let args = serde_json::json!({
        "command": e.command,
        "cwd": e.cwd.display().to_string(),
        "call_id": e.call_id,
        "turn_id": e.turn_id,
    });
    let diff = e
        .sandbox_assessment
        .as_ref()
        .map(|a| format!("Risk: {:?}\n{}", a.risk_level, a.explanation));

    Some(AppEvent::ToolApprovalRequired { name, args, diff })
}

/// Adapt an ApplyPatchApprovalRequest event.
pub fn adapt_apply_patch_approval_request(e: &ApplyPatchApprovalRequestEvent) -> Option<AppEvent> {
    let name = format!("apply_patch ({})", e.summary.total_files());
    let args = serde_json::json!({
        "call_id": e.call_id,
        "turn_id": e.turn_id,
        "files": e.files.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
        "summary": {
            "files_added": e.summary.files_added,
            "files_modified": e.summary.files_modified,
            "files_deleted": e.summary.files_deleted,
            "lines_added": e.summary.lines_added,
            "lines_removed": e.summary.lines_removed,
        },
    });
    let diff = Some(e.patch.clone());

    Some(AppEvent::ToolApprovalRequired { name, args, diff })
}

// ============================================================================
// MCP ADAPTERS
// ============================================================================

/// Adapt an MCP tool call begin event.
pub fn adapt_mcp_tool_call_begin(e: &McpToolCallBeginEvent) -> Option<AppEvent> {
    Some(AppEvent::ToolStarted {
        name: format!("{}:{}", e.invocation.server, e.invocation.tool),
        args: e
            .invocation
            .arguments
            .clone()
            .unwrap_or(serde_json::Value::Null),
    })
}

/// Adapt an MCP tool call end event.
pub fn adapt_mcp_tool_call_end(e: &McpToolCallEndEvent) -> Option<AppEvent> {
    let result = match &e.result {
        Ok(r) => {
            if r.is_error == Some(true) {
                format!("MCP tool error ({}ms)", e.duration_ms)
            } else {
                format!("Completed in {}ms", e.duration_ms)
            }
        }
        Err(err) => format!("Error: {}", err),
    };
    Some(AppEvent::ToolCompleted {
        name: e.call_id.clone(),
        result,
    })
}

// ============================================================================
// TOKEN & ERROR ADAPTERS
// ============================================================================

/// Adapt a TokenCount event.
pub fn adapt_token_count(e: &TokenCountEvent) -> Option<AppEvent> {
    if let Some(ref info) = e.info {
        let msg = format!(
            "Tokens: {} in, {} out ({} total)",
            info.last_token_usage.input_tokens,
            info.last_token_usage.output_tokens,
            info.total_token_usage.total_tokens
        );
        Some(AppEvent::Info(msg))
    } else {
        None
    }
}

/// Adapt an Error event.
pub fn adapt_error(e: &ErrorEvent) -> Option<AppEvent> {
    Some(AppEvent::Error(e.message.clone()))
}

/// Adapt a Warning event.
pub fn adapt_warning(e: &WarningEvent) -> Option<AppEvent> {
    Some(AppEvent::Warning(e.message.clone()))
}

// ============================================================================
// DIFF ADAPTERS
// ============================================================================

/// Adapt a TurnDiff event.
pub fn adapt_turn_diff(e: &TurnDiffEvent) -> Option<AppEvent> {
    // Extract file path from diff header if possible
    // Unified diffs typically start with "--- a/path" or "--- /path"
    let path = e
        .unified_diff
        .lines()
        .find(|line| line.starts_with("---"))
        .and_then(|line| {
            line.strip_prefix("--- ")
                .map(|p| p.trim_start_matches("a/").trim_start_matches("b/"))
        })
        .unwrap_or("unknown");

    Some(AppEvent::FileAdded(PathBuf::from(path)))
}

// ============================================================================
// MESSAGE PARTS ADAPTERS
// ============================================================================

/// Adapt a MessageWithPartsCreated event to an AppEvent.
pub fn adapt_message_with_parts(
    role: cortex_protocol::MessageRole,
    content: String,
) -> Option<AppEvent> {
    let msg_role = match role {
        cortex_protocol::MessageRole::User => MessageRole::User,
        cortex_protocol::MessageRole::Assistant => MessageRole::Assistant,
        cortex_protocol::MessageRole::System => MessageRole::System,
    };
    let message = match msg_role {
        MessageRole::User => Message::user(content),
        MessageRole::Assistant => Message::assistant(content).streaming(),
        MessageRole::System => Message::system(content),
        MessageRole::Tool => Message::tool("unknown", content),
    };
    Some(AppEvent::MessageReceived(message))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_adapt_task_complete() {
        let e = TaskCompleteEvent {
            last_agent_message: Some("Done!".to_string()),
        };
        let result = adapt_task_complete(&e);
        assert!(matches!(result, Some(AppEvent::MessageReceived(_))));
    }

    #[test]
    fn test_adapt_agent_message_delta() {
        let e = AgentMessageDeltaEvent {
            delta: "Hello ".to_string(),
        };
        let result = adapt_agent_message_delta(&e);
        assert!(matches!(result, Some(AppEvent::StreamingChunk(s)) if s == "Hello "));
    }

    #[test]
    fn test_adapt_user_message() {
        let e = UserMessageEvent {
            id: Some("msg-1".to_string()),
            parent_id: None,
            message: "Hi there".to_string(),
            images: None,
        };
        let result = adapt_user_message(&e);
        assert!(
            matches!(result, Some(AppEvent::MessageReceived(m)) if m.role == MessageRole::User)
        );
    }

    #[test]
    fn test_adapt_error() {
        let e = ErrorEvent {
            message: "Something went wrong".to_string(),
            cortex_error_info: None,
        };
        let result = adapt_error(&e);
        assert!(matches!(result, Some(AppEvent::Error(s)) if s == "Something went wrong"));
    }

    #[test]
    fn test_adapt_warning() {
        let e = WarningEvent {
            message: "Be careful".to_string(),
        };
        let result = adapt_warning(&e);
        assert!(matches!(result, Some(AppEvent::Warning(s)) if s == "Be careful"));
    }

    #[test]
    fn test_adapt_exec_command_begin() {
        let e = ExecCommandBeginEvent {
            call_id: "call-1".to_string(),
            turn_id: "turn-1".to_string(),
            command: vec!["ls".to_string(), "-la".to_string()],
            cwd: PathBuf::from("/home/user"),
            parsed_cmd: vec![],
            source: cortex_protocol::ExecCommandSource::Agent,
            interaction_input: None,
            tool_name: None,
            tool_arguments: None,
        };
        let result = adapt_exec_command_begin(&e);
        assert!(matches!(result, Some(AppEvent::ToolStarted { name, .. }) if name == "shell"));
    }

    #[test]
    fn test_adapt_exec_approval_request() {
        let e = ExecApprovalRequestEvent {
            call_id: "call-1".to_string(),
            turn_id: "turn-1".to_string(),
            command: vec!["rm".to_string(), "-rf".to_string(), "/tmp/test".to_string()],
            cwd: PathBuf::from("/home/user"),
            sandbox_assessment: None,
        };
        let result = adapt_exec_approval_request(&e);
        assert!(matches!(
            result,
            Some(AppEvent::ToolApprovalRequired { .. })
        ));
    }
}
