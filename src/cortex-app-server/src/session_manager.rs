//! Session manager - bridges WebSocket clients to cortex-core Sessions.
//!
//! This module creates real cortex-core Sessions and routes WebSocket messages
//! to the CLI's SQ/EQ (Submission Queue / Event Queue) system.

use std::collections::HashMap;
use std::sync::Arc;

use cortex_engine::{Config as CoreConfig, Session, SessionHandle};
use cortex_protocol::{
    AskForApproval, ConversationId, Event, EventMsg, Op, ReviewDecision, Submission, UserInput,
};
use tokio::sync::{RwLock, mpsc};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::storage::{SessionStorage, StoredMessage, StoredSession, StoredToolCall};
use crate::websocket::WsMessage;

/// Manages active CLI sessions connected to WebSocket clients.
pub struct SessionManager {
    /// Active sessions by session ID.
    sessions: RwLock<HashMap<String, ManagedSession>>,
    /// Persistent storage for sessions and messages.
    storage: Arc<SessionStorage>,
}

impl std::fmt::Debug for SessionManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionManager")
            .field("sessions", &self.sessions)
            .finish()
    }
}

/// A CLI session with its WebSocket connection.
pub struct ManagedSession {
    /// Session ID.
    pub id: String,
    /// Conversation ID from cortex-core.
    pub conversation_id: ConversationId,
    /// Handle to interact with the session.
    pub handle: SessionHandle,
    /// Task handle for the session runner.
    session_task: tokio::task::JoinHandle<()>,
    /// Task handle for the event forwarder.
    event_task: tokio::task::JoinHandle<()>,
    /// Shared WebSocket sender that can be updated when clients reconnect.
    ws_sender: Arc<tokio::sync::RwLock<Option<mpsc::Sender<WsMessage>>>>,
    /// User ID (optional).
    pub user_id: Option<String>,
    /// Working directory.
    pub cwd: std::path::PathBuf,
    /// Model being used.
    pub model: String,
}

impl std::fmt::Debug for ManagedSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ManagedSession")
            .field("id", &self.id)
            .field("conversation_id", &self.conversation_id)
            .field("user_id", &self.user_id)
            .field("cwd", &self.cwd)
            .field("model", &self.model)
            .finish()
    }
}

impl SessionManager {
    /// Create a new session manager.
    pub fn new() -> Self {
        let storage = SessionStorage::default_location().expect("Failed to create session storage");
        Self {
            sessions: RwLock::new(HashMap::new()),
            storage: Arc::new(storage),
        }
    }

    /// Get a reference to the storage.
    pub fn storage(&self) -> &Arc<SessionStorage> {
        &self.storage
    }

    /// Create a new CLI session and connect it to a WebSocket sender.
    pub async fn create_session(
        &self,
        ws_sender: mpsc::Sender<WsMessage>,
        options: CreateSessionOptions,
    ) -> Result<SessionInfo, SessionError> {
        let session_id = Uuid::new_v4().to_string();

        // Build cortex-core Config
        let config = CoreConfig {
            // In cloud/server mode, never ask for approval - auto-approve everything
            approval_policy: AskForApproval::Never,
            model: options.model.clone().unwrap_or_default(),
            cwd: options.cwd.clone().unwrap_or_default(),
            model_provider_id: options.provider.clone().unwrap_or_default(),
            ..Default::default()
        };

        // Create the real CLI session
        let (mut session, handle) =
            Session::new(config.clone()).map_err(|e| SessionError::Creation(e.to_string()))?;

        let conversation_id = handle.conversation_id;
        info!(
            session_id = %session_id,
            conversation_id = %conversation_id,
            model = %config.model,
            "Created CLI session"
        );

        // Spawn the session runner task
        let session_task = tokio::spawn(async move {
            if let Err(e) = session.run().await {
                error!("Session error: {}", e);
            }
        });

        // Clone event receiver for forwarding
        let event_rx = handle.event_rx.clone();
        let sid = session_id.clone();
        let storage = self.storage.clone();

        // Create shared WebSocket sender that can be updated when clients reconnect
        let shared_ws_sender = Arc::new(tokio::sync::RwLock::new(Some(ws_sender.clone())));
        let shared_ws_sender_clone = shared_ws_sender.clone();

        // Spawn the event forwarder task
        let event_task = tokio::spawn(async move {
            forward_events_shared(event_rx, shared_ws_sender_clone, sid, storage).await;
        });

        let managed = ManagedSession {
            id: session_id.clone(),
            conversation_id,
            handle,
            session_task,
            event_task,
            ws_sender: shared_ws_sender,
            user_id: options.user_id,
            cwd: config.cwd.clone(),
            model: config.model.clone(),
        };

        let info = SessionInfo {
            id: session_id.clone(),
            conversation_id: conversation_id.to_string(),
            model: managed.model.clone(),
            cwd: managed.cwd.clone(),
        };

        // Save session to persistent storage
        let stored_session = StoredSession {
            id: session_id.clone(),
            model: managed.model.clone(),
            cwd: managed.cwd.to_string_lossy().to_string(),
            created_at: chrono::Utc::now().timestamp(),
            updated_at: chrono::Utc::now().timestamp(),
            title: None,
        };
        if let Err(e) = self.storage.save_session(&stored_session) {
            warn!("Failed to save session to storage: {}", e);
        }

        // Store the session in memory
        let mut sessions = self.sessions.write().await;
        sessions.insert(session_id.clone(), managed);

        Ok(info)
    }

    /// Send a user message to a session.
    pub async fn send_message(
        &self,
        session_id: &str,
        content: String,
    ) -> Result<(), SessionError> {
        let sessions = self.sessions.read().await;
        let session = sessions
            .get(session_id)
            .ok_or_else(|| SessionError::NotFound(session_id.to_string()))?;

        // Save user message to storage
        let user_msg = StoredMessage {
            id: Uuid::new_v4().to_string(),
            role: "user".to_string(),
            content: content.clone(),
            timestamp: chrono::Utc::now().timestamp(),
            tool_calls: vec![],
        };
        if let Err(e) = self.storage.append_message(session_id, &user_msg) {
            warn!("Failed to save user message: {}", e);
        }

        // Update session title from first message
        if let Err(e) = self.storage.touch_session(session_id) {
            warn!("Failed to touch session: {}", e);
        }

        let submission = Submission {
            id: Uuid::new_v4().to_string(),
            op: Op::UserInput {
                items: vec![UserInput::Text { text: content }],
            },
        };

        session
            .handle
            .submission_tx
            .send(submission)
            .await
            .map_err(|e| SessionError::Send(e.to_string()))?;

        Ok(())
    }

    /// Update the WebSocket sender for a session (used when client reconnects).
    pub async fn update_ws_sender(
        &self,
        session_id: &str,
        new_sender: mpsc::Sender<WsMessage>,
    ) -> Result<(), SessionError> {
        let sessions = self.sessions.read().await;
        let session = sessions
            .get(session_id)
            .ok_or_else(|| SessionError::NotFound(session_id.to_string()))?;

        // Update the shared sender
        let mut sender_guard = session.ws_sender.write().await;
        *sender_guard = Some(new_sender);
        info!(session_id = %session_id, "Updated WebSocket sender for session");

        Ok(())
    }

    /// Interrupt the current task in a session.
    pub async fn interrupt(&self, session_id: &str) -> Result<(), SessionError> {
        let sessions = self.sessions.read().await;
        let session = sessions
            .get(session_id)
            .ok_or_else(|| SessionError::NotFound(session_id.to_string()))?;

        let submission = Submission {
            id: Uuid::new_v4().to_string(),
            op: Op::Interrupt,
        };

        session
            .handle
            .submission_tx
            .send(submission)
            .await
            .map_err(|e| SessionError::Send(e.to_string()))?;

        Ok(())
    }

    /// Approve a command execution.
    pub async fn approve_exec(
        &self,
        session_id: &str,
        call_id: String,
        approved: bool,
    ) -> Result<(), SessionError> {
        let sessions = self.sessions.read().await;
        let session = sessions
            .get(session_id)
            .ok_or_else(|| SessionError::NotFound(session_id.to_string()))?;

        let decision = if approved {
            ReviewDecision::Approved
        } else {
            ReviewDecision::Denied
        };

        let submission = Submission {
            id: Uuid::new_v4().to_string(),
            op: Op::ExecApproval {
                id: call_id,
                decision,
            },
        };

        session
            .handle
            .submission_tx
            .send(submission)
            .await
            .map_err(|e| SessionError::Send(e.to_string()))?;

        Ok(())
    }

    /// Submit design system selection - resumes session with design config.
    pub async fn submit_design_system(
        &self,
        session_id: &str,
        _call_id: String,
        config: serde_json::Value,
    ) -> Result<(), SessionError> {
        let sessions = self.sessions.read().await;
        let session = sessions
            .get(session_id)
            .ok_or_else(|| SessionError::NotFound(session_id.to_string()))?;

        // Send the design config as a user message to continue the conversation
        let config_str = serde_json::to_string_pretty(&config).unwrap_or_default();

        // Extract key info for a cleaner message
        let typography = config.get("typography");
        let colors = config.get("colors");

        let mut message = String::from("I've selected my design system:\n\n");

        if let Some(typo) = typography {
            if let Some(heading) = typo.get("heading") {
                message.push_str(&format!(
                    "**Heading:** {} @ {} ({}px)\n",
                    heading
                        .get("font")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Inter"),
                    heading
                        .get("weight")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(600),
                    heading.get("size").and_then(|v| v.as_u64()).unwrap_or(32)
                ));
            }
            if let Some(body) = typo.get("body") {
                message.push_str(&format!(
                    "**Body:** {} @ {} ({}px)\n",
                    body.get("font").and_then(|v| v.as_str()).unwrap_or("Inter"),
                    body.get("weight").and_then(|v| v.as_u64()).unwrap_or(400),
                    body.get("size").and_then(|v| v.as_u64()).unwrap_or(16)
                ));
            }
            if let Some(mono) = typo.get("mono") {
                message.push_str(&format!(
                    "**Code:** {}\n",
                    mono.get("font")
                        .and_then(|v| v.as_str())
                        .unwrap_or("JetBrains Mono")
                ));
            }
        }

        if let Some(cols) = colors.and_then(|c| c.as_array()) {
            message.push_str("\n**Colors:**\n");
            for c in cols {
                let name = c.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let hex = c.get("hex").and_then(|v| v.as_str()).unwrap_or("");
                message.push_str(&format!("- {}: {}\n", name, hex));
            }
        }

        message.push_str(&format!("\n```json\n{}\n```\n", config_str));
        message.push_str("\nNow create the frontend using ONLY these fonts and colors. Import fonts from Google Fonts. Use CSS variables for colors. Keep it minimal.");

        let submission = Submission {
            id: Uuid::new_v4().to_string(),
            op: Op::UserInput {
                items: vec![UserInput::text(message)],
            },
        };

        session
            .handle
            .submission_tx
            .send(submission)
            .await
            .map_err(|e| SessionError::Send(e.to_string()))?;

        info!(session_id = %session_id, "Design system selection submitted");

        Ok(())
    }

    /// Shutdown and remove a session.
    pub async fn destroy_session(&self, session_id: &str) -> Result<(), SessionError> {
        let mut sessions = self.sessions.write().await;

        if let Some(session) = sessions.remove(session_id) {
            // Send shutdown command
            let _ = session
                .handle
                .submission_tx
                .send(Submission {
                    id: Uuid::new_v4().to_string(),
                    op: Op::Shutdown,
                })
                .await;

            // Abort tasks
            session.session_task.abort();
            session.event_task.abort();

            info!(session_id = %session_id, "Session destroyed");
            Ok(())
        } else {
            Err(SessionError::NotFound(session_id.to_string()))
        }
    }

    /// Gracefully shutdown all active sessions.
    ///
    /// This method should be called during server shutdown to ensure all
    /// WebSocket connections receive proper close frames and all in-progress
    /// requests are terminated cleanly.
    pub async fn shutdown_all(&self) {
        let session_ids: Vec<String> = {
            let sessions = self.sessions.read().await;
            sessions.keys().cloned().collect()
        };

        if session_ids.is_empty() {
            info!("No active sessions to shutdown");
            return;
        }

        info!("Shutting down {} active sessions", session_ids.len());

        for session_id in session_ids {
            if let Err(e) = self.destroy_session(&session_id).await {
                warn!("Failed to shutdown session {}: {}", session_id, e);
            }
        }

        info!("All sessions shutdown complete");
    }

    /// List all active sessions.
    pub async fn list_sessions(&self) -> Vec<SessionInfo> {
        let sessions = self.sessions.read().await;
        sessions
            .values()
            .map(|s| SessionInfo {
                id: s.id.clone(),
                conversation_id: s.conversation_id.to_string(),
                model: s.model.clone(),
                cwd: s.cwd.clone(),
            })
            .collect()
    }

    /// Get session info.
    pub async fn get_session(&self, session_id: &str) -> Option<SessionInfo> {
        let sessions = self.sessions.read().await;
        sessions.get(session_id).map(|s| SessionInfo {
            id: s.id.clone(),
            conversation_id: s.conversation_id.to_string(),
            model: s.model.clone(),
            cwd: s.cwd.clone(),
        })
    }

    /// Update the model for a session.
    pub async fn update_model(&self, session_id: &str, model: &str) -> Result<(), SessionError> {
        let mut sessions = self.sessions.write().await;
        let session = sessions
            .get_mut(session_id)
            .ok_or_else(|| SessionError::NotFound(session_id.to_string()))?;

        // Update the model in the session
        session.model = model.to_string();

        // Send model update command to the CLI session using OverrideTurnContext
        let submission = Submission {
            id: Uuid::new_v4().to_string(),
            op: Op::OverrideTurnContext {
                cwd: None,
                approval_policy: None,
                sandbox_policy: None,
                model: Some(model.to_string()),
                effort: None,
                summary: None,
            },
        };

        session
            .handle
            .submission_tx
            .send(submission)
            .await
            .map_err(|e| SessionError::Send(e.to_string()))?;

        info!(session_id = %session_id, model = %model, "Model updated");
        Ok(())
    }

    /// Fork an existing session.
    pub async fn fork_session(
        &self,
        ws_sender: mpsc::Sender<WsMessage>,
        session_id: &str,
        message_index: usize,
    ) -> Result<SessionInfo, SessionError> {
        let (conversation_id, model, cwd, user_id) = {
            let sessions = self.sessions.read().await;
            let s = sessions
                .get(session_id)
                .ok_or_else(|| SessionError::NotFound(session_id.to_string()))?;
            (
                s.conversation_id,
                s.model.clone(),
                s.cwd.clone(),
                s.user_id.clone(),
            )
        };

        let new_session_id = Uuid::new_v4().to_string();

        // Build cortex-core Config from original
        let config = CoreConfig {
            model,
            cwd,
            approval_policy: AskForApproval::Never,
            ..Default::default()
        };

        // Create the real CLI session using fork
        let (mut session, handle) = Session::fork(config.clone(), conversation_id, message_index)
            .map_err(|e| SessionError::Creation(e.to_string()))?;

        let conversation_id = handle.conversation_id;
        info!(
            new_session_id = %new_session_id,
            parent_session_id = %session_id,
            conversation_id = %conversation_id,
            "Forked CLI session"
        );

        // Spawn the session runner task
        let session_task = tokio::spawn(async move {
            if let Err(e) = session.run().await {
                error!("Session error: {}", e);
            }
        });

        // Clone event receiver for forwarding
        let event_rx = handle.event_rx.clone();
        let sid = new_session_id.clone();
        let storage = self.storage.clone();

        // Create shared WebSocket sender
        let shared_ws_sender = Arc::new(tokio::sync::RwLock::new(Some(ws_sender.clone())));
        let shared_ws_sender_clone = shared_ws_sender.clone();

        // Spawn the event forwarder task
        let event_task = tokio::spawn(async move {
            forward_events_shared(event_rx, shared_ws_sender_clone, sid, storage).await;
        });

        let managed = ManagedSession {
            id: new_session_id.clone(),
            conversation_id,
            handle,
            session_task,
            event_task,
            ws_sender: shared_ws_sender,
            user_id,
            cwd: config.cwd.clone(),
            model: config.model.clone(),
        };

        let info = SessionInfo {
            id: new_session_id.clone(),
            conversation_id: conversation_id.to_string(),
            model: managed.model.clone(),
            cwd: managed.cwd.clone(),
        };

        // Save session to persistent storage
        let stored_session = StoredSession {
            id: new_session_id.clone(),
            model: managed.model.clone(),
            cwd: managed.cwd.to_string_lossy().to_string(),
            created_at: chrono::Utc::now().timestamp(),
            updated_at: chrono::Utc::now().timestamp(),
            title: Some(format!("Fork of {}", session_id)),
        };
        if let Err(e) = self.storage.save_session(&stored_session) {
            warn!("Failed to save session to storage: {}", e);
        }

        // Copy history to new storage
        if let Ok(history) = self.storage.read_history(session_id) {
            for (i, msg) in history.into_iter().enumerate() {
                if i > message_index {
                    break;
                }
                if let Err(e) = self.storage.append_message(&new_session_id, &msg) {
                    warn!("Failed to copy message to forked history: {}", e);
                }
            }
        }

        // Store the session in memory
        let mut sessions = self.sessions.write().await;
        sessions.insert(new_session_id.clone(), managed);

        Ok(info)
    }

    /// Get count of active sessions.
    pub async fn count(&self) -> usize {
        let sessions = self.sessions.read().await;
        sessions.len()
    }
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Options for creating a session.
#[derive(Debug, Clone, Default)]
pub struct CreateSessionOptions {
    pub user_id: Option<String>,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub cwd: Option<std::path::PathBuf>,
    pub system_prompt: Option<String>,
}

/// Session information.
#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub id: String,
    pub conversation_id: String,
    pub model: String,
    pub cwd: std::path::PathBuf,
}

/// Session errors.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("Failed to create session: {0}")]
    Creation(String),
    #[error("Session not found: {0}")]
    NotFound(String),
    #[error("Failed to send message: {0}")]
    Send(String),
    #[error("Invalid state: {0}")]
    InvalidState(String),
}

/// Forward events from CLI session to WebSocket.
async fn _forward_events(
    event_rx: async_channel::Receiver<Event>,
    ws_tx: mpsc::Sender<WsMessage>,
    session_id: String,
    storage: Arc<SessionStorage>,
) {
    let sid = session_id.as_str();
    let mut current_message = String::new();
    let mut current_tool_calls: Vec<StoredToolCall> = Vec::new();

    while let Ok(event) = event_rx.recv().await {
        // Track message content for storage
        match &event.msg {
            EventMsg::AgentMessageDelta(e) => {
                current_message.push_str(&e.delta);
            }
            EventMsg::ExecCommandEnd(e) => {
                current_tool_calls.push(StoredToolCall {
                    id: e.call_id.clone(),
                    name: "Execute".to_string(),
                    input: serde_json::json!({}),
                    output: Some(e.formatted_output.clone()),
                    success: e.exit_code == 0,
                    duration_ms: Some(e.duration_ms),
                });
            }
            EventMsg::TaskComplete(_) => {
                // Save assistant message to storage when task completes
                if !current_message.is_empty() || !current_tool_calls.is_empty() {
                    let assistant_msg = StoredMessage {
                        id: Uuid::new_v4().to_string(),
                        role: "assistant".to_string(),
                        content: current_message.clone(),
                        timestamp: chrono::Utc::now().timestamp(),
                        tool_calls: std::mem::take(&mut current_tool_calls),
                    };
                    if let Err(e) = storage.append_message(&session_id, &assistant_msg) {
                        warn!("Failed to save assistant message: {}", e);
                    }
                    current_message.clear();
                }
                // Update session title from content
                if let Err(e) = storage.touch_session(&session_id) {
                    warn!("Failed to touch session: {}", e);
                }
            }
            _ => {}
        }

        let ws_msg = convert_event_to_ws(&event);

        if let Some(msg) = ws_msg
            && ws_tx.send(msg).await.is_err()
        {
            warn!(session_id = %sid, "WebSocket sender closed");
            break;
        }
    }
    debug!(session_id = %sid, "Event forwarder finished");
}

/// Forward events from CLI session to a shared WebSocket sender.
/// The shared sender can be updated when clients reconnect.
async fn forward_events_shared(
    event_rx: async_channel::Receiver<Event>,
    ws_tx: Arc<tokio::sync::RwLock<Option<mpsc::Sender<WsMessage>>>>,
    session_id: String,
    storage: Arc<SessionStorage>,
) {
    let sid = session_id.as_str();
    let mut current_message = String::new();
    let mut current_tool_calls: Vec<StoredToolCall> = Vec::new();

    while let Ok(event) = event_rx.recv().await {
        // Track message content for storage
        match &event.msg {
            EventMsg::AgentMessageDelta(e) => {
                current_message.push_str(&e.delta);
            }
            EventMsg::ExecCommandBegin(e) => {
                // Store tool name for later
                debug!(session_id = %sid, tool_name = ?e.tool_name, "Tool execution started");
            }
            EventMsg::ExecCommandEnd(e) => {
                let tool_name = e
                    .command
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "Execute".to_string());
                current_tool_calls.push(StoredToolCall {
                    id: e.call_id.clone(),
                    name: tool_name,
                    input: serde_json::json!({}),
                    output: Some(e.formatted_output.clone()),
                    success: e.exit_code == 0,
                    duration_ms: Some(e.duration_ms),
                });
            }
            EventMsg::TaskComplete(_) => {
                // Save assistant message to storage when task completes
                if !current_message.is_empty() || !current_tool_calls.is_empty() {
                    let assistant_msg = StoredMessage {
                        id: Uuid::new_v4().to_string(),
                        role: "assistant".to_string(),
                        content: current_message.clone(),
                        timestamp: chrono::Utc::now().timestamp(),
                        tool_calls: std::mem::take(&mut current_tool_calls),
                    };
                    if let Err(e) = storage.append_message(&session_id, &assistant_msg) {
                        warn!("Failed to save assistant message: {}", e);
                    }
                    current_message.clear();
                }
                // Update session title from content
                if let Err(e) = storage.touch_session(&session_id) {
                    warn!("Failed to touch session: {}", e);
                }
            }
            _ => {}
        }

        let ws_msg = convert_event_to_ws(&event);

        if let Some(msg) = ws_msg {
            // Get current sender (may be None if client disconnected)
            let sender_guard = ws_tx.read().await;
            if let Some(sender) = sender_guard.as_ref() {
                if sender.send(msg).await.is_err() {
                    debug!(session_id = %sid, "WebSocket sender closed, waiting for reconnection");
                    // Don't break - the client might reconnect
                }
            } else {
                debug!(session_id = %sid, "No active WebSocket connection, event buffered");
            }
        }
    }
    debug!(session_id = %sid, "Event forwarder (shared) finished");
}

/// Convert a cortex-protocol Event to a WebSocket message.
fn convert_event_to_ws(event: &Event) -> Option<WsMessage> {
    match &event.msg {
        // Streaming content
        EventMsg::AgentMessageDelta(e) => Some(WsMessage::StreamChunk {
            content: e.delta.clone(),
        }),

        // Full message (end of stream)
        EventMsg::AgentMessage(e) => Some(WsMessage::AgentMessage {
            content: e.message.clone(),
        }),

        // User message echo
        EventMsg::UserMessage(e) => Some(WsMessage::MessageReceived {
            id: Uuid::new_v4().to_string(),
            role: "user".to_string(),
            content: e.message.clone(),
        }),

        // Tool execution start
        EventMsg::ExecCommandBegin(e) => Some(WsMessage::ToolCallBegin {
            call_id: e.call_id.clone(),
            tool_name: e.tool_name.clone().unwrap_or_else(|| "Execute".to_string()),
            arguments: e.tool_arguments.clone().unwrap_or_default(),
        }),

        // Tool execution end
        EventMsg::ExecCommandEnd(e) => Some(WsMessage::ToolCallEnd {
            call_id: e.call_id.clone(),
            tool_name: "Execute".to_string(),
            output: e.formatted_output.clone(),
            success: e.exit_code == 0,
            duration_ms: e.duration_ms,
            metadata: e.metadata.clone(),
        }),

        // Tool execution output chunk (streaming)
        EventMsg::ExecCommandOutputDelta(e) => {
            let stream = match e.stream {
                cortex_protocol::ExecOutputStream::Stdout => "stdout",
                cortex_protocol::ExecOutputStream::Stderr => "stderr",
            };
            Some(WsMessage::ToolCallOutputDelta {
                call_id: e.call_id.clone(),
                stream: stream.to_string(),
                chunk: e.chunk.clone(),
            })
        }

        // Approval request
        EventMsg::ExecApprovalRequest(e) => Some(WsMessage::ApprovalRequest {
            call_id: e.call_id.clone(),
            command: e.command.clone(),
            cwd: e.cwd.to_string_lossy().to_string(),
        }),

        // Task lifecycle
        EventMsg::TaskStarted(_) => Some(WsMessage::TaskStarted),

        EventMsg::TaskComplete(e) => Some(WsMessage::TaskComplete {
            message: e.last_agent_message.clone(),
        }),

        // Token usage
        EventMsg::TokenCount(e) => e.info.as_ref().map(|info| WsMessage::TokenUsage {
            input_tokens: info.last_token_usage.input_tokens as u32,
            output_tokens: info.last_token_usage.output_tokens as u32,
            total_tokens: info.last_token_usage.total_tokens as u32,
        }),

        // Errors
        EventMsg::Error(e) => Some(WsMessage::Error {
            code: "error".to_string(),
            message: e.message.clone(),
        }),

        EventMsg::Warning(e) => Some(WsMessage::Warning {
            message: e.message.clone(),
        }),

        // Session configured
        EventMsg::SessionConfigured(e) => Some(WsMessage::SessionConfigured {
            session_id: e.session_id.to_string(),
            model: e.model.clone(),
            cwd: e.cwd.to_string_lossy().to_string(),
        }),

        // Reasoning (thinking)
        EventMsg::AgentReasoningDelta(e) => Some(WsMessage::ReasoningDelta {
            delta: e.delta.clone(),
        }),

        // Turn aborted/cancelled
        EventMsg::TurnAborted(_) => Some(WsMessage::Cancelled),

        // Shutdown
        EventMsg::ShutdownComplete => Some(WsMessage::SessionClosed),

        // Other events we don't forward yet
        _ => {
            debug!(
                "Unhandled event type: {:?}",
                std::any::type_name::<EventMsg>()
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_session_manager_creation() {
        let manager = SessionManager::new();
        assert_eq!(manager.count().await, 0);
    }
}
