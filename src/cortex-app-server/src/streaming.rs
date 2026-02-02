//! HTTP Streaming API for CLI sessions.
//!
//! Provides Server-Sent Events (SSE) streaming for real-time communication
//! with cortex-core CLI sessions. All session management happens server-side.

use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Path, State},
    response::sse::{Event, KeepAlive, Sse},
    routing::{get, post},
};
use cortex_engine::{Config as CoreConfig, Session, SessionHandle};
use cortex_protocol::{
    ConversationId, Event as CliEvent, EventMsg, Op, ReviewDecision, Submission, UserInput,
};
use futures::stream::Stream;
use serde::{Deserialize, Serialize};
use tokio::sync::{RwLock, mpsc};
use tokio_stream::wrappers::ReceiverStream;
use tracing::{debug, error, info};
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// Create streaming API routes.
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        // CLI Session management
        .route("/cli/sessions", post(create_cli_session))
        .route("/cli/sessions", get(list_cli_sessions))
        .route("/cli/sessions/:id", get(get_cli_session))
        .route(
            "/cli/sessions/:id",
            axum::routing::delete(delete_cli_session),
        )
        // Message streaming
        .route("/cli/sessions/:id/chat", post(chat_stream))
        .route("/cli/sessions/:id/events", get(session_events_stream))
        // Approval handling
        .route("/cli/sessions/:id/approve", post(approve_command))
        // Interrupt
        .route("/cli/sessions/:id/interrupt", post(interrupt_session))
        // Fork
        .route("/cli/sessions/:id/fork", post(fork_cli_session))
}

// ============================================================================
// Types
// ============================================================================

/// Managed CLI session.
pub struct CliSession {
    pub id: String,
    pub conversation_id: ConversationId,
    pub handle: SessionHandle,
    pub model: String,
    pub cwd: std::path::PathBuf,
    pub created_at: std::time::Instant,
    session_task: tokio::task::JoinHandle<()>,
}

impl std::fmt::Debug for CliSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CliSession")
            .field("id", &self.id)
            .field("conversation_id", &self.conversation_id)
            .field("model", &self.model)
            .field("cwd", &self.cwd)
            .finish()
    }
}

/// CLI session manager stored in AppState.
#[derive(Debug, Default)]
pub struct CliSessionManager {
    sessions: RwLock<HashMap<String, CliSession>>,
}

impl CliSessionManager {
    pub fn new() -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
        }
    }

    pub async fn get(&self, id: &str) -> Option<SessionHandle> {
        let sessions = self.sessions.read().await;
        sessions.get(id).map(|s| s.handle.clone())
    }

    pub async fn count(&self) -> usize {
        let sessions = self.sessions.read().await;
        sessions.len()
    }

    /// Gracefully shutdown all active CLI sessions.
    ///
    /// This method should be called during server shutdown to ensure all
    /// streaming connections receive proper close frames and all in-progress
    /// requests are terminated cleanly.
    pub async fn shutdown_all(&self) {
        let session_ids: Vec<String> = {
            let sessions = self.sessions.read().await;
            sessions.keys().cloned().collect()
        };

        if session_ids.is_empty() {
            info!("No active CLI sessions to shutdown");
            return;
        }

        info!("Shutting down {} active CLI sessions", session_ids.len());

        let mut sessions = self.sessions.write().await;
        for session_id in session_ids {
            if let Some(session) = sessions.remove(&session_id) {
                // Send shutdown command
                let _ = session
                    .handle
                    .submission_tx
                    .send(Submission {
                        id: Uuid::new_v4().to_string(),
                        op: Op::Shutdown,
                    })
                    .await;

                // Abort the session task
                session.session_task.abort();

                debug!(session_id = %session_id, "CLI session shutdown");
            }
        }

        info!("All CLI sessions shutdown complete");
    }
}

// ============================================================================
// Request/Response Types
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct CreateCliSessionRequest {
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct CliSessionResponse {
    pub id: String,
    pub conversation_id: String,
    pub model: String,
    pub cwd: String,
    pub status: String,
}

#[derive(Debug, Deserialize)]
pub struct ChatRequest {
    pub content: String,
}

#[derive(Debug, Deserialize)]
pub struct ApproveRequest {
    pub call_id: String,
    pub approved: bool,
}

/// SSE event types sent to client.
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    /// Session is ready.
    SessionReady {
        session_id: String,
        model: String,
        cwd: String,
    },
    /// User message acknowledged.
    MessageReceived { id: String },
    /// Task started processing.
    TaskStarted,
    /// Streaming text delta.
    Delta { content: String },
    /// Full message (replaces deltas).
    Message { content: String },
    /// Reasoning/thinking delta.
    Reasoning { content: String },
    /// Tool call started.
    ToolStart {
        call_id: String,
        tool_name: String,
        arguments: serde_json::Value,
    },
    /// Tool call output delta.
    ToolOutput { call_id: String, content: String },
    /// Tool call completed.
    ToolEnd {
        call_id: String,
        tool_name: String,
        output: String,
        success: bool,
        duration_ms: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        metadata: Option<serde_json::Value>,
    },
    /// Command requires approval.
    ApprovalRequired {
        call_id: String,
        command: Vec<String>,
        cwd: String,
    },
    /// Token usage update.
    TokenUsage {
        input_tokens: i64,
        output_tokens: i64,
        total_tokens: i64,
    },
    /// Task completed.
    TaskComplete { message: Option<String> },
    /// Warning message.
    Warning { message: String },
    /// Error occurred.
    Error { message: String },
    /// Session closed.
    SessionClosed,
    /// Keep-alive ping.
    Ping { timestamp: u64 },
}

#[derive(Debug, Deserialize)]
pub struct ForkCliSessionRequest {
    pub message_index: usize,
}

// ============================================================================
// Handlers
// ============================================================================

/// Create a new CLI session.
async fn create_cli_session(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateCliSessionRequest>,
) -> AppResult<Json<CliSessionResponse>> {
    let session_id = Uuid::new_v4().to_string();

    // Build cortex-core Config
    let mut config = CoreConfig::default();

    if let Some(model) = &req.model {
        config.model = model.clone();
    }
    if let Some(cwd) = &req.cwd {
        config.cwd = std::path::PathBuf::from(cwd);
    }
    if let Some(provider) = &req.provider {
        config.model_provider_id = provider.clone();
    }

    // Create the real CLI session
    let (mut session, handle) = Session::new(config.clone())
        .map_err(|e| AppError::Internal(format!("Failed to create session: {e}")))?;

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

    let cli_session = CliSession {
        id: session_id.clone(),
        conversation_id,
        handle,
        model: config.model.clone(),
        cwd: config.cwd.clone(),
        created_at: std::time::Instant::now(),
        session_task,
    };

    let response = CliSessionResponse {
        id: session_id.clone(),
        conversation_id: conversation_id.to_string(),
        model: cli_session.model.clone(),
        cwd: cli_session.cwd.to_string_lossy().to_string(),
        status: "ready".to_string(),
    };

    // Store the session
    let mut sessions = state.cli_session_manager.sessions.write().await;
    sessions.insert(session_id, cli_session);

    Ok(Json(response))
}

/// Fork a CLI session.
async fn fork_cli_session(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<ForkCliSessionRequest>,
) -> AppResult<Json<CliSessionResponse>> {
    let (conversation_id, model, cwd) = {
        let sessions = state.cli_session_manager.sessions.read().await;
        let s = sessions
            .get(&id)
            .ok_or_else(|| AppError::NotFound(format!("Session not found: {id}")))?;
        (s.conversation_id, s.model.clone(), s.cwd.clone())
    };

    let new_session_id = Uuid::new_v4().to_string();

    // Build cortex-core Config from original
    let config = CoreConfig {
        model,
        cwd,
        ..Default::default()
    };

    // Create the real CLI session using fork
    let (mut session, handle) =
        Session::fork(config.clone(), conversation_id, req.message_index)
            .map_err(|e| AppError::Internal(format!("Failed to fork session: {e}")))?;

    let conversation_id = handle.conversation_id;

    info!(
        new_session_id = %new_session_id,
        parent_session_id = %id,
        conversation_id = %conversation_id,
        "Forked CLI session"
    );

    // Spawn the session runner task
    let session_task = tokio::spawn(async move {
        if let Err(e) = session.run().await {
            error!("Session error: {}", e);
        }
    });

    let cli_session = CliSession {
        id: new_session_id.clone(),
        conversation_id,
        handle,
        model: config.model.clone(),
        cwd: config.cwd.clone(),
        created_at: std::time::Instant::now(),
        session_task,
    };

    let response = CliSessionResponse {
        id: new_session_id.clone(),
        conversation_id: conversation_id.to_string(),
        model: cli_session.model.clone(),
        cwd: cli_session.cwd.to_string_lossy().to_string(),
        status: "ready".to_string(),
    };

    // Store the session
    let mut sessions = state.cli_session_manager.sessions.write().await;
    sessions.insert(new_session_id, cli_session);

    Ok(Json(response))
}

/// List all CLI sessions.
async fn list_cli_sessions(State(state): State<Arc<AppState>>) -> Json<Vec<CliSessionResponse>> {
    let sessions = state.cli_session_manager.sessions.read().await;
    let list: Vec<_> = sessions
        .values()
        .map(|s| CliSessionResponse {
            id: s.id.clone(),
            conversation_id: s.conversation_id.to_string(),
            model: s.model.clone(),
            cwd: s.cwd.to_string_lossy().to_string(),
            status: "active".to_string(),
        })
        .collect();
    Json(list)
}

/// Get a CLI session by ID.
async fn get_cli_session(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> AppResult<Json<CliSessionResponse>> {
    let sessions = state.cli_session_manager.sessions.read().await;
    let session = sessions
        .get(&id)
        .ok_or_else(|| AppError::NotFound(format!("Session not found: {id}")))?;

    Ok(Json(CliSessionResponse {
        id: session.id.clone(),
        conversation_id: session.conversation_id.to_string(),
        model: session.model.clone(),
        cwd: session.cwd.to_string_lossy().to_string(),
        status: "active".to_string(),
    }))
}

/// Delete a CLI session.
async fn delete_cli_session(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> AppResult<Json<serde_json::Value>> {
    let mut sessions = state.cli_session_manager.sessions.write().await;

    if let Some(session) = sessions.remove(&id) {
        // Send shutdown command
        let _ = session
            .handle
            .submission_tx
            .send(Submission {
                id: Uuid::new_v4().to_string(),
                op: Op::Shutdown,
            })
            .await;

        // Abort the session task
        session.session_task.abort();

        info!(session_id = %id, "CLI session deleted");
        Ok(Json(serde_json::json!({ "deleted": true })))
    } else {
        Err(AppError::NotFound(format!("Session not found: {id}")))
    }
}

/// Send a chat message and stream the response via SSE.
async fn chat_stream(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    Json(req): Json<ChatRequest>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, AppError> {
    // Get session handle
    let handle = {
        let sessions = state.cli_session_manager.sessions.read().await;
        sessions
            .get(&session_id)
            .map(|s| s.handle.clone())
            .ok_or_else(|| AppError::NotFound(format!("Session not found: {session_id}")))?
    };

    // Create SSE channel
    let (tx, rx) = mpsc::channel::<Result<Event, Infallible>>(100);

    // Send user message to CLI
    let submission = Submission {
        id: Uuid::new_v4().to_string(),
        op: Op::UserInput {
            items: vec![UserInput::Text {
                text: req.content.clone(),
            }],
        },
    };

    handle
        .submission_tx
        .send(submission)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to send message: {e}")))?;

    // Spawn task to forward CLI events to SSE
    let event_rx = handle.event_rx.clone();
    tokio::spawn(async move {
        forward_cli_events_to_sse(event_rx, tx).await;
    });

    // Return SSE stream
    let stream = ReceiverStream::new(rx);
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

/// Subscribe to session events via SSE (long-polling alternative).
async fn session_events_stream(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, AppError> {
    // Get session handle
    let handle = {
        let sessions = state.cli_session_manager.sessions.read().await;
        sessions
            .get(&session_id)
            .map(|s| s.handle.clone())
            .ok_or_else(|| AppError::NotFound(format!("Session not found: {session_id}")))?
    };

    // Create SSE channel
    let (tx, rx) = mpsc::channel::<Result<Event, Infallible>>(100);

    // Send initial ready event
    let _ = tx
        .send(Ok(Event::default().event("message").data(
            serde_json::to_string(&StreamEvent::Ping {
                timestamp: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
            })
            .unwrap(),
        )))
        .await;

    // Spawn task to forward CLI events to SSE
    let event_rx = handle.event_rx.clone();
    tokio::spawn(async move {
        forward_cli_events_to_sse(event_rx, tx).await;
    });

    // Return SSE stream
    let stream = ReceiverStream::new(rx);
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

/// Approve or deny a command execution.
async fn approve_command(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
    Json(req): Json<ApproveRequest>,
) -> AppResult<Json<serde_json::Value>> {
    let handle = {
        let sessions = state.cli_session_manager.sessions.read().await;
        sessions
            .get(&session_id)
            .map(|s| s.handle.clone())
            .ok_or_else(|| AppError::NotFound(format!("Session not found: {session_id}")))?
    };

    let decision = if req.approved {
        ReviewDecision::Approved
    } else {
        ReviewDecision::Denied
    };

    let submission = Submission {
        id: Uuid::new_v4().to_string(),
        op: Op::ExecApproval {
            id: req.call_id,
            decision,
        },
    };

    handle
        .submission_tx
        .send(submission)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to send approval: {e}")))?;

    Ok(Json(serde_json::json!({ "approved": req.approved })))
}

/// Interrupt the current task.
async fn interrupt_session(
    State(state): State<Arc<AppState>>,
    Path(session_id): Path<String>,
) -> AppResult<Json<serde_json::Value>> {
    let handle = {
        let sessions = state.cli_session_manager.sessions.read().await;
        sessions
            .get(&session_id)
            .map(|s| s.handle.clone())
            .ok_or_else(|| AppError::NotFound(format!("Session not found: {session_id}")))?
    };

    let submission = Submission {
        id: Uuid::new_v4().to_string(),
        op: Op::Interrupt,
    };

    handle
        .submission_tx
        .send(submission)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to send interrupt: {e}")))?;

    Ok(Json(serde_json::json!({ "interrupted": true })))
}

// ============================================================================
// Event Forwarding
// ============================================================================

/// Forward CLI events to SSE channel.
async fn forward_cli_events_to_sse(
    event_rx: async_channel::Receiver<CliEvent>,
    tx: mpsc::Sender<Result<Event, Infallible>>,
) {
    while let Ok(event) = event_rx.recv().await {
        if let Some(stream_event) = convert_cli_event(&event) {
            let json = match serde_json::to_string(&stream_event) {
                Ok(j) => j,
                Err(e) => {
                    error!("Failed to serialize event: {}", e);
                    continue;
                }
            };

            let sse_event = Event::default().event("message").data(json);

            if tx.send(Ok(sse_event)).await.is_err() {
                debug!("SSE client disconnected");
                break;
            }

            // Check for terminal events
            if matches!(
                stream_event,
                StreamEvent::TaskComplete { .. }
                    | StreamEvent::SessionClosed
                    | StreamEvent::Error { .. }
            ) {
                break;
            }
        }
    }
}

/// Convert CLI event to stream event.
fn convert_cli_event(event: &CliEvent) -> Option<StreamEvent> {
    match &event.msg {
        EventMsg::AgentMessageDelta(e) => Some(StreamEvent::Delta {
            content: e.delta.clone(),
        }),

        EventMsg::AgentMessage(e) => Some(StreamEvent::Message {
            content: e.message.clone(),
        }),

        EventMsg::AgentReasoningDelta(e) => Some(StreamEvent::Reasoning {
            content: e.delta.clone(),
        }),

        EventMsg::ExecCommandBegin(e) => Some(StreamEvent::ToolStart {
            call_id: e.call_id.clone(),
            tool_name: e.tool_name.clone().unwrap_or_else(|| "Execute".to_string()),
            arguments: e.tool_arguments.clone().unwrap_or_default(),
        }),

        EventMsg::ExecCommandOutputDelta(e) => {
            // Decode base64 output
            let content =
                base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &e.chunk)
                    .ok()
                    .and_then(|bytes| String::from_utf8(bytes).ok())
                    .unwrap_or_default();

            Some(StreamEvent::ToolOutput {
                call_id: e.call_id.clone(),
                content,
            })
        }

        EventMsg::ExecCommandEnd(e) => Some(StreamEvent::ToolEnd {
            call_id: e.call_id.clone(),
            tool_name: "Execute".to_string(),
            output: e.formatted_output.clone(),
            success: e.exit_code == 0,
            duration_ms: e.duration_ms,
            metadata: e.metadata.clone(),
        }),

        EventMsg::ExecApprovalRequest(e) => Some(StreamEvent::ApprovalRequired {
            call_id: e.call_id.clone(),
            command: e.command.clone(),
            cwd: e.cwd.to_string_lossy().to_string(),
        }),

        EventMsg::TaskStarted(_) => Some(StreamEvent::TaskStarted),

        EventMsg::TaskComplete(e) => Some(StreamEvent::TaskComplete {
            message: e.last_agent_message.clone(),
        }),

        EventMsg::TokenCount(e) => e.info.as_ref().map(|info| StreamEvent::TokenUsage {
            input_tokens: info.last_token_usage.input_tokens,
            output_tokens: info.last_token_usage.output_tokens,
            total_tokens: info.last_token_usage.total_tokens,
        }),

        EventMsg::Warning(e) => Some(StreamEvent::Warning {
            message: e.message.clone(),
        }),

        EventMsg::Error(e) => Some(StreamEvent::Error {
            message: e.message.clone(),
        }),

        EventMsg::ShutdownComplete => Some(StreamEvent::SessionClosed),

        EventMsg::SessionConfigured(e) => Some(StreamEvent::SessionReady {
            session_id: e.session_id.to_string(),
            model: e.model.clone(),
            cwd: e.cwd.to_string_lossy().to_string(),
        }),

        _ => None,
    }
}
