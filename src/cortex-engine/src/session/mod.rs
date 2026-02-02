//! Session runner - manages the lifecycle of a conversation session.
//!
//! This module provides the core session management that connects
//! the TUI/CLI to the agent loop.

mod agent_loop;
mod handlers;
mod lifecycle;
mod prompt;
mod types;

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use async_channel::{Receiver, Sender};

use cortex_protocol::{ConversationId, Event, Submission, TokenUsage};

use crate::client::{Message, ModelClient};
use crate::config::Config;
use crate::rollout::RolloutRecorder;
use crate::tools::ToolRouter;

pub use lifecycle::list_sessions;
pub use prompt::build_system_prompt;
pub use types::{SessionHandle, SessionInfo, TokenCounter};

/// A running session that handles conversation with the model.
pub struct Session {
    /// Session configuration.
    pub(crate) config: Config,
    /// Conversation ID.
    pub(crate) conversation_id: ConversationId,
    /// Model client.
    pub(crate) client: Box<dyn ModelClient>,
    /// Tool router.
    pub(crate) tool_router: ToolRouter,
    /// Conversation messages.
    pub(crate) messages: Vec<Message>,
    /// Submission receiver (from UI).
    pub(crate) submission_rx: Receiver<Submission>,
    /// Event sender (to UI).
    pub(crate) event_tx: Sender<Event>,
    /// Current turn ID.
    pub(crate) turn_id: u64,
    /// Token usage tracking.
    pub(crate) total_usage: TokenUsage,
    /// Token counter for precise context window tracking.
    pub(crate) token_counter: Arc<TokenCounter>,
    /// Whether session is running.
    pub(crate) running: bool,
    /// Rollout recorder for persistence.
    pub(crate) recorder: Option<RolloutRecorder>,
    /// Pending approval requests (call_id -> (tool_call, args)).
    pub(crate) pending_approvals: std::collections::HashMap<String, types::PendingToolCall>,
    /// Cancellation flag for interrupting current request.
    pub(crate) cancelled: Arc<AtomicBool>,
    /// Snapshot manager for undo/redo.
    #[allow(dead_code)]
    pub(crate) snapshot_manager: Arc<crate::tasks::snapshot::SnapshotManager>,
    /// Undo history.
    pub(crate) undo_history: crate::tasks::undo::UndoHistory,
    /// Redo history.
    pub(crate) redo_history: crate::tasks::undo::RedoHistory,
    /// Current turn's undo actions.
    #[allow(dead_code)]
    pub(crate) current_undo_actions: Vec<crate::tasks::undo::UndoAction>,
    /// Share service for generating public URLs.
    pub(crate) share_service: crate::share_service::ShareService,
    /// LSP integration.
    pub(crate) lsp: Arc<crate::integrations::LspIntegration>,
}

impl Session {
    /// Emit an event to the event channel and optionally record it.
    pub(crate) async fn emit(&mut self, msg: cortex_protocol::EventMsg) {
        // Skip rollout recording for delta events (too frequent, causes latency)
        let skip_recording = matches!(msg, cortex_protocol::EventMsg::AgentMessageDelta(_));

        let event = Event {
            id: self.turn_id.to_string(),
            msg,
        };

        // Record event to rollout file (skip deltas for performance)
        if !skip_recording && let Some(recorder) = &mut self.recorder {
            let _ = recorder.record_event(&event);
        }

        // Non-blocking send - never wait
        let _ = self.event_tx.try_send(event);
    }
}
