//! Event Adapter - Converts cortex_protocol events to cortex-tui AppEvents.
//!
//! This module provides the translation layer between cortex-core's event system
//! and cortex-tui's event system. It handles the conversion of low-level protocol
//! events into high-level application events that the TUI can process.
//!
//! ## Architecture
//!
//! ```text
//! cortex_protocol::Event -> adapt_event() -> cortex_tui::AppEvent
//! ```
//!
//! ## Event Categories
//!
//! Events are organized into several categories:
//!
//! - **Session lifecycle**: Session configuration, task start/complete
//! - **Message events**: User messages, agent messages, streaming deltas
//! - **Tool execution**: Command begin/output/end events
//! - **Approval requests**: Tool approval, patch approval
//! - **Token usage**: Token counts and rate limits
//! - **MCP events**: Server connections and tool calls
//! - **Errors and warnings**: Error and warning messages
//!
//! ## Submodules
//!
//! - [`adapters`] - Individual adapter functions for event conversion
//! - [`categorize`] - Event categorization helpers for routing and priority
//! - [`approval`] - Approval state builders
//! - [`utils`] - Utility functions for data transformation
//!
//! ## Example
//!
//! ```rust,ignore
//! use cortex_tui::bridge::event_adapter::{adapt_event, adapt_events};
//! use cortex_protocol::Event;
//!
//! // Convert a single event
//! if let Some(app_event) = adapt_event(protocol_event) {
//!     event_bus.send(app_event).await?;
//! }
//!
//! // Convert multiple events
//! let app_events = adapt_events(protocol_events);
//! for event in app_events {
//!     event_bus.send(event).await?;
//! }
//! ```

mod adapters;
mod approval;
mod categorize;
mod utils;

use uuid::Uuid;

use crate::events::AppEvent;
use cortex_protocol::{Event, EventMsg, McpStartupStatus};

// Re-export adapter functions
use adapters::{
    adapt_agent_message, adapt_agent_message_delta, adapt_agent_reasoning_delta,
    adapt_apply_patch_approval_request, adapt_error, adapt_exec_approval_request,
    adapt_exec_command_begin, adapt_exec_command_end, adapt_exec_command_output_delta,
    adapt_mcp_tool_call_begin, adapt_mcp_tool_call_end, adapt_message_with_parts,
    adapt_session_configured, adapt_task_complete, adapt_token_count, adapt_turn_aborted,
    adapt_turn_diff, adapt_user_message, adapt_warning,
};

// Re-export public APIs
pub use approval::{create_approval_state, create_patch_approval_state};
pub use categorize::{
    is_high_priority_event, is_mcp_event, is_message_event, is_session_event, is_streaming_event,
    is_tool_event,
};
pub use utils::parse_uuid;

// ============================================================================
// MAIN CONVERSION FUNCTION
// ============================================================================

/// Convert a cortex_protocol Event to a cortex-tui AppEvent.
///
/// Returns `None` for events that should be ignored (e.g., ping/pong, debug).
/// This function handles all known event types and provides sensible defaults
/// for unknown events.
///
/// # Arguments
///
/// * `event` - The cortex_protocol event to convert
///
/// # Returns
///
/// * `Some(AppEvent)` - The converted application event
/// * `None` - If the event should be ignored
///
/// # Example
///
/// ```rust,ignore
/// let protocol_event = Event { id: "1".into(), msg: EventMsg::TaskStarted(...) };
/// if let Some(app_event) = adapt_event(protocol_event) {
///     // Handle the app event
/// }
/// ```
pub fn adapt_event(event: Event) -> Option<AppEvent> {
    match event.msg {
        // === Session lifecycle ===
        EventMsg::SessionConfigured(e) => adapt_session_configured(&e),
        EventMsg::TaskStarted(_) => Some(AppEvent::StreamingStarted),
        EventMsg::TaskComplete(e) => adapt_task_complete(&e),
        EventMsg::TurnAborted(e) => adapt_turn_aborted(&e),
        EventMsg::ShutdownComplete => Some(AppEvent::Quit),

        // === Message events ===
        EventMsg::UserMessage(e) => adapt_user_message(&e),
        EventMsg::AgentMessageDelta(e) => adapt_agent_message_delta(&e),
        EventMsg::AgentMessage(e) => adapt_agent_message(&e),

        // === Reasoning events ===
        EventMsg::AgentReasoning(e) => Some(AppEvent::StreamingChunk(e.text)),
        EventMsg::AgentReasoningDelta(e) => adapt_agent_reasoning_delta(&e),
        EventMsg::AgentReasoningRawContent(e) => Some(AppEvent::StreamingChunk(e.text)),
        EventMsg::AgentReasoningRawContentDelta(e) => Some(AppEvent::StreamingChunk(e.delta)),
        EventMsg::AgentReasoningSectionBreak(_) => None, // Internal event, ignore

        // === Tool execution ===
        EventMsg::ExecCommandBegin(e) => adapt_exec_command_begin(&e),
        EventMsg::ExecCommandOutputDelta(e) => adapt_exec_command_output_delta(&e),
        EventMsg::ExecCommandEnd(e) => adapt_exec_command_end(&e),

        // === Approval requests ===
        EventMsg::ExecApprovalRequest(e) => adapt_exec_approval_request(&e),
        EventMsg::ApplyPatchApprovalRequest(e) => adapt_apply_patch_approval_request(&e),
        EventMsg::ElicitationRequest(e) => Some(AppEvent::Info(format!(
            "MCP server '{}' requests input: {}",
            e.server_name, e.message
        ))),

        // === MCP events ===
        EventMsg::McpToolCallBegin(e) => adapt_mcp_tool_call_begin(&e),
        EventMsg::McpToolCallEnd(e) => adapt_mcp_tool_call_end(&e),
        EventMsg::McpStartupUpdate(e) => {
            let status_msg = match e.status {
                McpStartupStatus::Starting => format!("MCP server '{}' starting...", e.server),
                McpStartupStatus::Ready => format!("MCP server '{}' ready", e.server),
                McpStartupStatus::Failed { error } => {
                    format!("MCP server '{}' failed: {}", e.server, error)
                }
                McpStartupStatus::Cancelled => format!("MCP server '{}' cancelled", e.server),
            };
            Some(AppEvent::Info(status_msg))
        }
        EventMsg::McpStartupComplete(e) => {
            let msg = format!(
                "MCP startup complete: {} ready, {} failed, {} cancelled",
                e.ready.len(),
                e.failed.len(),
                e.cancelled.len()
            );
            Some(AppEvent::Info(msg))
        }
        EventMsg::McpListToolsResponse(_) => None, // Response event, handled separately

        // === Patch events ===
        EventMsg::PatchApplyBegin(e) => {
            let file_count = e.changes.len();
            Some(AppEvent::ToolStarted {
                name: "apply_patch".to_string(),
                args: serde_json::json!({
                    "call_id": e.call_id,
                    "file_count": file_count,
                    "auto_approved": e.auto_approved,
                }),
            })
        }
        EventMsg::PatchApplyEnd(e) => {
            let result = if e.success {
                "Patch applied successfully".to_string()
            } else {
                format!("Patch failed: {}", e.stderr)
            };
            Some(AppEvent::ToolCompleted {
                name: e.call_id,
                result,
            })
        }

        // === Token events ===
        EventMsg::TokenCount(e) => adapt_token_count(&e),

        // === Error events ===
        EventMsg::Error(e) => adapt_error(&e),
        EventMsg::Warning(e) => adapt_warning(&e),
        EventMsg::StreamError(e) => Some(AppEvent::StreamingError(e.message)),

        // === History & Context events ===
        EventMsg::TurnDiff(e) => adapt_turn_diff(&e),
        EventMsg::GetHistoryEntryResponse(_) => None, // Response event, handled separately
        EventMsg::ListCustomPromptsResponse(_) => None, // Response event, handled separately
        EventMsg::BackgroundEvent(e) => Some(AppEvent::Info(e.message)),

        // === Undo/Redo events ===
        EventMsg::UndoStarted(e) => Some(AppEvent::Info(
            e.message.unwrap_or_else(|| "Undo started...".to_string()),
        )),
        EventMsg::UndoCompleted(e) => {
            if e.success {
                Some(AppEvent::Info(
                    e.message.unwrap_or_else(|| "Undo completed".to_string()),
                ))
            } else {
                Some(AppEvent::Error(
                    e.message.unwrap_or_else(|| "Undo failed".to_string()),
                ))
            }
        }
        EventMsg::RedoStarted(e) => Some(AppEvent::Info(
            e.message.unwrap_or_else(|| "Redo started...".to_string()),
        )),
        EventMsg::RedoCompleted(e) => {
            if e.success {
                Some(AppEvent::Info(
                    e.message.unwrap_or_else(|| "Redo completed".to_string()),
                ))
            } else {
                Some(AppEvent::Error(
                    e.message.unwrap_or_else(|| "Redo failed".to_string()),
                ))
            }
        }

        // === Review events ===
        EventMsg::EnteredReviewMode(r) => Some(AppEvent::Info(format!(
            "Entered review mode: {}",
            r.user_facing_hint
        ))),
        EventMsg::ExitedReviewMode(_) => Some(AppEvent::Info("Exited review mode".to_string())),

        // === Timeline & Forking ===
        EventMsg::SessionForked(e) => Some(AppEvent::SessionCreated(
            parse_uuid(&e.new_session_id.to_string()).unwrap_or_else(Uuid::new_v4),
        )),
        EventMsg::TimelineUpdated(_) => None, // Internal event

        // === Deprecation ===
        EventMsg::DeprecationNotice(e) => Some(AppEvent::Warning(format!(
            "Deprecation: {}{}",
            e.summary,
            e.details.map(|d| format!(" - {}", d)).unwrap_or_default()
        ))),

        // === Web Search ===
        EventMsg::WebSearchBegin(e) => Some(AppEvent::ToolStarted {
            name: "web_search".to_string(),
            args: serde_json::json!({ "call_id": e.call_id }),
        }),
        EventMsg::WebSearchEnd(e) => Some(AppEvent::ToolCompleted {
            name: e.call_id,
            result: format!("Search completed: {}", e.query),
        }),

        // === Image ===
        EventMsg::ViewImageToolCall(e) => Some(AppEvent::ToolStarted {
            name: "view_image".to_string(),
            args: serde_json::json!({
                "call_id": e.call_id,
                "path": e.path.display().to_string(),
            }),
        }),

        // === Plan ===
        EventMsg::PlanUpdate(e) => {
            let summary = e
                .plan
                .iter()
                .map(|item| format!("[{:?}] {}", item.status, item.title))
                .collect::<Vec<_>>()
                .join("\n");
            Some(AppEvent::Info(format!("Plan updated:\n{}", summary)))
        }

        // === Share events ===
        EventMsg::SessionShared(e) => Some(AppEvent::Info(format!("Session shared: {}", e.url))),
        EventMsg::SessionUnshared(e) => {
            if e.success {
                Some(AppEvent::Info("Session unshared".to_string()))
            } else {
                Some(AppEvent::Error("Failed to unshare session".to_string()))
            }
        }

        // === Unified item events ===
        EventMsg::ItemStarted(e) => {
            // Convert TurnItem to appropriate event
            Some(AppEvent::Info(format!(
                "Item started in turn {}",
                e.turn_id
            )))
        }
        EventMsg::ItemCompleted(e) => Some(AppEvent::Info(format!(
            "Item completed in turn {}",
            e.turn_id
        ))),
        EventMsg::AgentMessageContentDelta(e) => Some(AppEvent::StreamingChunk(e.delta)),
        EventMsg::ReasoningContentDelta(e) => Some(AppEvent::StreamingChunk(e.delta)),
        EventMsg::ReasoningRawContentDelta(e) => Some(AppEvent::StreamingChunk(e.delta)),
        EventMsg::RawResponseItem(_) => None, // Internal event

        // === Message Parts events ===
        EventMsg::MessageWithPartsCreated(e) => {
            let content = e.message.get_text_content();
            adapt_message_with_parts(e.message.role, content)
        }
        EventMsg::MessageWithPartsCompleted(e) => Some(AppEvent::Info(format!(
            "Message {} completed",
            e.message_id
        ))),
        EventMsg::PartUpdated(_) => None, // Fine-grained update, handled by parts system
        EventMsg::PartRemoved(_) => None, // Fine-grained update, handled by parts system
        EventMsg::PartDelta(e) => {
            // Extract delta content based on type
            let content = match e.delta {
                cortex_protocol::PartDelta::Text { content } => content,
                cortex_protocol::PartDelta::Reasoning { content } => content,
                cortex_protocol::PartDelta::ToolOutput { output } => output,
            };
            Some(AppEvent::StreamingChunk(content))
        }

        // === Events to ignore ===
        EventMsg::Unknown => {
            tracing::debug!("Received unknown event type");
            None
        }

        // Catch-all for any future event types added to the non-exhaustive enum
        #[allow(unreachable_patterns)]
        _ => {
            tracing::debug!("Unhandled event type in adapt_event");
            None
        }
    }
}

// ============================================================================
// BATCH EVENT PROCESSING
// ============================================================================

/// Process multiple events, filtering out None results.
///
/// This is useful for processing a batch of protocol events and converting
/// them to application events in one call.
///
/// # Arguments
///
/// * `events` - A vector of cortex_protocol events
///
/// # Returns
///
/// A vector of converted AppEvents (events that returned None are excluded).
///
/// # Example
///
/// ```rust,ignore
/// let protocol_events = vec![event1, event2, event3];
/// let app_events = adapt_events(protocol_events);
/// // app_events contains only successfully converted events
/// ```
pub fn adapt_events(events: Vec<Event>) -> Vec<AppEvent> {
    events.into_iter().filter_map(adapt_event).collect()
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use cortex_core::widgets::chat::MessageRole;
    use cortex_protocol::{
        AgentMessageDeltaEvent, AskForApproval, ConversationId, ExecApprovalRequestEvent,
        SandboxPolicy, SessionConfiguredEvent, TaskCompleteEvent, TaskStartedEvent,
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
    fn test_adapt_task_started() {
        let event = make_event(EventMsg::TaskStarted(TaskStartedEvent {
            model_context_window: Some(100000),
        }));
        let result = adapt_event(event);
        assert!(matches!(result, Some(AppEvent::StreamingStarted)));
    }

    #[test]
    fn test_adapt_task_complete() {
        let event = make_event(EventMsg::TaskComplete(TaskCompleteEvent {
            last_agent_message: Some("Done!".to_string()),
        }));
        let result = adapt_event(event);
        assert!(matches!(result, Some(AppEvent::MessageReceived(_))));
    }

    #[test]
    fn test_adapt_agent_message_delta() {
        let event = make_event(EventMsg::AgentMessageDelta(AgentMessageDeltaEvent {
            delta: "Hello ".to_string(),
        }));
        let result = adapt_event(event);
        assert!(matches!(result, Some(AppEvent::StreamingChunk(s)) if s == "Hello "));
    }

    #[test]
    fn test_adapt_user_message() {
        let event = make_event(EventMsg::UserMessage(UserMessageEvent {
            id: Some("msg-1".to_string()),
            parent_id: None,
            message: "Hi there".to_string(),
            images: None,
        }));
        let result = adapt_event(event);
        assert!(
            matches!(result, Some(AppEvent::MessageReceived(m)) if m.role == MessageRole::User)
        );
    }

    #[test]
    fn test_adapt_exec_approval_request() {
        let event = make_event(EventMsg::ExecApprovalRequest(ExecApprovalRequestEvent {
            call_id: "call-1".to_string(),
            turn_id: "turn-1".to_string(),
            command: vec!["rm".to_string(), "-rf".to_string(), "/tmp/test".to_string()],
            cwd: PathBuf::from("/home/user"),
            sandbox_assessment: None,
        }));
        let result = adapt_event(event);
        assert!(matches!(
            result,
            Some(AppEvent::ToolApprovalRequired { .. })
        ));
    }

    #[test]
    fn test_adapt_session_configured() {
        let event = make_event(EventMsg::SessionConfigured(Box::new(
            SessionConfiguredEvent {
                session_id: ConversationId::new(),
                parent_session_id: None,
                model: "claude-3".to_string(),
                model_provider_id: "anthropic".to_string(),
                approval_policy: AskForApproval::OnRequest,
                sandbox_policy: SandboxPolicy::default(),
                cwd: PathBuf::from("/home/user"),
                reasoning_effort: None,
                history_log_id: 1,
                history_entry_count: 0,
                initial_messages: None,
                rollout_path: PathBuf::from("/tmp/rollout"),
            },
        )));
        let result = adapt_event(event);
        assert!(matches!(result, Some(AppEvent::SessionCreated(_))));
    }

    #[test]
    fn test_adapt_events_batch() {
        let events = vec![
            make_event(EventMsg::TaskStarted(TaskStartedEvent {
                model_context_window: Some(100000),
            })),
            make_event(EventMsg::AgentMessageDelta(AgentMessageDeltaEvent {
                delta: "Hello".to_string(),
            })),
            make_event(EventMsg::Unknown), // Should be filtered out
        ];
        let results = adapt_events(events);
        assert_eq!(results.len(), 2);
    }
}
