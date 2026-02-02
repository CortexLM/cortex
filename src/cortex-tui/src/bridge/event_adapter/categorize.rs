//! Event categorization helpers.
//!
//! This module provides functions to classify cortex_protocol events into
//! logical categories for routing and priority handling.

use cortex_protocol::{Event, EventMsg};

/// Check if an event indicates streaming is active.
///
/// Returns true for events that represent ongoing streaming content,
/// such as message deltas or command output.
///
/// # Arguments
///
/// * `event` - Reference to the event to check
///
/// # Returns
///
/// `true` if the event is a streaming event.
pub fn is_streaming_event(event: &Event) -> bool {
    matches!(
        event.msg,
        EventMsg::AgentMessageDelta(_)
            | EventMsg::AgentReasoningDelta(_)
            | EventMsg::AgentReasoningRawContentDelta(_)
            | EventMsg::ExecCommandOutputDelta(_)
            | EventMsg::AgentMessageContentDelta(_)
            | EventMsg::ReasoningContentDelta(_)
            | EventMsg::ReasoningRawContentDelta(_)
            | EventMsg::PartDelta(_)
    )
}

/// Check if an event requires immediate UI update.
///
/// High-priority events typically require user attention or indicate
/// significant state changes (approval requests, errors, task completion).
///
/// # Arguments
///
/// * `event` - Reference to the event to check
///
/// # Returns
///
/// `true` if the event should be processed with high priority.
pub fn is_high_priority_event(event: &Event) -> bool {
    matches!(
        event.msg,
        EventMsg::ExecApprovalRequest(_)
            | EventMsg::ApplyPatchApprovalRequest(_)
            | EventMsg::ElicitationRequest(_)
            | EventMsg::Error(_)
            | EventMsg::StreamError(_)
            | EventMsg::TaskComplete(_)
            | EventMsg::TurnAborted(_)
            | EventMsg::ShutdownComplete
    )
}

/// Check if an event indicates a tool is executing.
///
/// Tool events include command execution, MCP tool calls, patch applications,
/// and web searches.
///
/// # Arguments
///
/// * `event` - Reference to the event to check
///
/// # Returns
///
/// `true` if the event is related to tool execution.
pub fn is_tool_event(event: &Event) -> bool {
    matches!(
        event.msg,
        EventMsg::ExecCommandBegin(_)
            | EventMsg::ExecCommandOutputDelta(_)
            | EventMsg::ExecCommandEnd(_)
            | EventMsg::ExecApprovalRequest(_)
            | EventMsg::McpToolCallBegin(_)
            | EventMsg::McpToolCallEnd(_)
            | EventMsg::PatchApplyBegin(_)
            | EventMsg::PatchApplyEnd(_)
            | EventMsg::ApplyPatchApprovalRequest(_)
            | EventMsg::WebSearchBegin(_)
            | EventMsg::WebSearchEnd(_)
            | EventMsg::ViewImageToolCall(_)
    )
}

/// Check if an event is a session lifecycle event.
///
/// # Arguments
///
/// * `event` - Reference to the event to check
///
/// # Returns
///
/// `true` if the event relates to session lifecycle.
pub fn is_session_event(event: &Event) -> bool {
    matches!(
        event.msg,
        EventMsg::SessionConfigured(_)
            | EventMsg::TaskStarted(_)
            | EventMsg::TaskComplete(_)
            | EventMsg::TurnAborted(_)
            | EventMsg::ShutdownComplete
            | EventMsg::SessionForked(_)
            | EventMsg::SessionShared(_)
            | EventMsg::SessionUnshared(_)
    )
}

/// Check if an event is a message event.
///
/// # Arguments
///
/// * `event` - Reference to the event to check
///
/// # Returns
///
/// `true` if the event relates to messages.
pub fn is_message_event(event: &Event) -> bool {
    matches!(
        event.msg,
        EventMsg::UserMessage(_)
            | EventMsg::AgentMessage(_)
            | EventMsg::AgentMessageDelta(_)
            | EventMsg::AgentReasoning(_)
            | EventMsg::AgentReasoningDelta(_)
            | EventMsg::MessageWithPartsCreated(_)
            | EventMsg::MessageWithPartsCompleted(_)
            | EventMsg::PartDelta(_)
    )
}

/// Check if an event is an MCP-related event.
///
/// # Arguments
///
/// * `event` - Reference to the event to check
///
/// # Returns
///
/// `true` if the event relates to MCP servers or tools.
pub fn is_mcp_event(event: &Event) -> bool {
    matches!(
        event.msg,
        EventMsg::McpToolCallBegin(_)
            | EventMsg::McpToolCallEnd(_)
            | EventMsg::McpStartupUpdate(_)
            | EventMsg::McpStartupComplete(_)
            | EventMsg::McpListToolsResponse(_)
            | EventMsg::ElicitationRequest(_)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use cortex_protocol::{
        AgentMessageDeltaEvent, ErrorEvent, ExecCommandBeginEvent, TaskStartedEvent,
        UserMessageEvent,
    };
    use std::path::PathBuf;

    fn make_event(msg: EventMsg) -> Event {
        Event {
            id: "test-123".to_string(),
            msg,
        }
    }

    #[test]
    fn test_is_streaming_event() {
        let streaming = make_event(EventMsg::AgentMessageDelta(AgentMessageDeltaEvent {
            delta: "test".to_string(),
        }));
        assert!(is_streaming_event(&streaming));

        let non_streaming = make_event(EventMsg::TaskStarted(TaskStartedEvent {
            model_context_window: None,
        }));
        assert!(!is_streaming_event(&non_streaming));
    }

    #[test]
    fn test_is_high_priority_event() {
        let high = make_event(EventMsg::Error(ErrorEvent {
            message: "error".to_string(),
            cortex_error_info: None,
        }));
        assert!(is_high_priority_event(&high));

        let low = make_event(EventMsg::AgentMessageDelta(AgentMessageDeltaEvent {
            delta: "test".to_string(),
        }));
        assert!(!is_high_priority_event(&low));
    }

    #[test]
    fn test_is_tool_event() {
        let tool = make_event(EventMsg::ExecCommandBegin(ExecCommandBeginEvent {
            call_id: "1".to_string(),
            turn_id: "1".to_string(),
            command: vec!["ls".to_string()],
            cwd: PathBuf::from("/"),
            parsed_cmd: vec![],
            source: cortex_protocol::ExecCommandSource::Agent,
            interaction_input: None,
            tool_name: None,
            tool_arguments: None,
        }));
        assert!(is_tool_event(&tool));
    }

    #[test]
    fn test_is_session_event() {
        let session_event = make_event(EventMsg::TaskStarted(TaskStartedEvent {
            model_context_window: None,
        }));
        assert!(is_session_event(&session_event));

        let non_session = make_event(EventMsg::AgentMessageDelta(AgentMessageDeltaEvent {
            delta: "test".to_string(),
        }));
        assert!(!is_session_event(&non_session));
    }

    #[test]
    fn test_is_message_event() {
        let msg_event = make_event(EventMsg::UserMessage(UserMessageEvent {
            id: None,
            parent_id: None,
            message: "test".to_string(),
            images: None,
        }));
        assert!(is_message_event(&msg_event));
    }

    #[test]
    fn test_is_mcp_event() {
        let mcp_event = make_event(EventMsg::McpStartupComplete(
            cortex_protocol::McpStartupCompleteEvent::default(),
        ));
        assert!(is_mcp_event(&mcp_event));
    }
}
