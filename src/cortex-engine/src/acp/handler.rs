//! ACP request handlers.
//!
//! This module contains the business logic for handling ACP protocol requests,
//! including session management, prompt processing, and event forwarding.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{RwLock, broadcast};
use tracing::{debug, error, info, warn};

use crate::acp::protocol::{AcpError, AcpRequestId, AcpResponse};
use crate::acp::types::*;
use crate::config::Config;
use crate::session::{Session, SessionHandle};
use cortex_protocol::{EventMsg, Op, Submission, UserInput};

/// Session state tracked by the ACP handler.
pub struct AcpSessionState {
    /// The session handle.
    pub handle: SessionHandle,
    /// Cancel token for this session.
    pub cancel_tx: broadcast::Sender<()>,
    /// Session metadata.
    pub metadata: SessionMetadata,
}

/// Session metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionMetadata {
    /// Session ID.
    pub session_id: String,
    /// Working directory.
    pub cwd: String,
    /// Current model.
    pub model: Option<String>,
    /// Current agent.
    pub agent: Option<String>,
    /// Creation timestamp.
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// ACP request handler.
pub struct AcpHandler {
    /// Active sessions.
    sessions: Arc<RwLock<HashMap<String, AcpSessionState>>>,
    /// Configuration.
    config: Config,
    /// Notification sender for streaming updates.
    notification_tx: broadcast::Sender<AcpNotificationEvent>,
}

/// Notification event wrapper.
#[derive(Debug, Clone)]
pub struct AcpNotificationEvent {
    /// The method name.
    pub method: String,
    /// The notification params.
    pub params: Value,
}

impl AcpHandler {
    /// Create a new handler.
    pub fn new(config: Config) -> Self {
        let (notification_tx, _) = broadcast::channel(1024);
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            config,
            notification_tx,
        }
    }

    /// Subscribe to notifications.
    pub fn subscribe(&self) -> broadcast::Receiver<AcpNotificationEvent> {
        self.notification_tx.subscribe()
    }

    /// Handle initialize request.
    pub async fn handle_initialize(&self, params: InitializeRequest) -> Result<InitializeResponse> {
        debug!(
            "Initialize request: version={}, client={}",
            params.protocol_version, params.client_info.name
        );

        Ok(InitializeResponse {
            protocol_version: PROTOCOL_VERSION,
            agent_capabilities: AgentCapabilities {
                load_session: true,
                mcp_capabilities: Some(McpCapabilities {
                    http: true,
                    sse: true,
                }),
                prompt_capabilities: PromptCapabilities {
                    embedded_context: true,
                    image: true,
                },
            },
            agent_info: AgentInfo {
                name: "Cortex".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
            auth_methods: vec![],
        })
    }

    /// Handle session/new request.
    pub async fn handle_session_new(
        &self,
        params: NewSessionRequest,
    ) -> Result<NewSessionResponse> {
        info!("Creating new session with cwd: {}", params.cwd);

        let mut config = self.config.clone();
        config.cwd = params.cwd.clone().into();

        let (mut session, handle) = Session::new(config)?;
        let session_id = handle.conversation_id.to_string();

        let (cancel_tx, _) = broadcast::channel(1);

        let metadata = SessionMetadata {
            session_id: session_id.clone(),
            cwd: params.cwd,
            model: Some(self.config.model.clone()),
            agent: None,
            created_at: chrono::Utc::now(),
        };

        let state = AcpSessionState {
            handle: handle.clone(),
            cancel_tx: cancel_tx.clone(),
            metadata: metadata.clone(),
        };

        // Store session
        self.sessions
            .write()
            .await
            .insert(session_id.clone(), state);

        // Clone session_id before spawning the session runner
        let session_id_for_runner = session_id.clone();

        // Spawn session runner
        tokio::spawn(async move {
            if let Err(e) = session.run().await {
                error!("Session {} error: {}", session_id_for_runner, e);
            }
        });

        // Spawn event forwarder
        let session_id_clone = session_id.clone();
        let notification_tx = self.notification_tx.clone();
        let event_rx = handle.event_rx.clone();
        let mut cancel_rx = cancel_tx.subscribe();

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    result = event_rx.recv() => {
                        match result {
                            Ok(event) => {
                                if let Some(notification) = event_to_notification(&session_id_clone, event.msg) {
                                    let _ = notification_tx.send(notification);
                                }
                            }
                            Err(_) => break,
                        }
                    }
                    _ = cancel_rx.recv() => {
                        debug!("Session {} event forwarder cancelled", session_id_clone);
                        break;
                    }
                }
            }
        });

        Ok(NewSessionResponse {
            session_id,
            models: Some(SessionModels {
                current_model_id: self.config.model.clone(),
                available_models: vec![
                    ModelInfo {
                        model_id: "claude-sonnet-4-20250514".to_string(),
                        name: "Claude Sonnet 4".to_string(),
                    },
                    ModelInfo {
                        model_id: "gpt-4o".to_string(),
                        name: "GPT-4o".to_string(),
                    },
                ],
            }),
            modes: Some(SessionModes {
                current_mode_id: "default".to_string(),
                available_modes: vec![
                    ModeInfo {
                        id: "default".to_string(),
                        name: "Default".to_string(),
                        description: "Standard agent mode".to_string(),
                    },
                    ModeInfo {
                        id: "plan".to_string(),
                        name: "Plan".to_string(),
                        description: "Planning mode with confirmation".to_string(),
                    },
                ],
            }),
        })
    }

    /// Handle session/load request.
    pub async fn handle_session_load(
        &self,
        params: LoadSessionRequest,
    ) -> Result<LoadSessionResponse> {
        info!("Loading session: {}", params.session_id);

        // Check if session already loaded
        let sessions = self.sessions.read().await;
        if let Some(state) = sessions.get(&params.session_id) {
            return Ok(LoadSessionResponse {
                session_id: params.session_id,
                models: Some(SessionModels {
                    current_model_id: state.metadata.model.clone().unwrap_or_default(),
                    available_models: vec![],
                }),
                modes: None,
            });
        }
        drop(sessions);

        // Try to resume from storage
        let conversation_id: cortex_protocol::ConversationId = params
            .session_id
            .parse()
            .map_err(|_| anyhow::anyhow!("Invalid session ID"))?;

        let config = self.config.clone();
        let (mut session, handle) = Session::resume(config, conversation_id)?;
        let session_id = handle.conversation_id.to_string();

        let (cancel_tx, _) = broadcast::channel(1);

        let metadata = SessionMetadata {
            session_id: session_id.clone(),
            cwd: self.config.cwd.display().to_string(),
            model: Some(self.config.model.clone()),
            agent: None,
            created_at: chrono::Utc::now(),
        };

        let state = AcpSessionState {
            handle: handle.clone(),
            cancel_tx: cancel_tx.clone(),
            metadata,
        };

        self.sessions
            .write()
            .await
            .insert(session_id.clone(), state);

        // Spawn session runner
        tokio::spawn(async move {
            if let Err(e) = session.run().await {
                error!("Session error: {}", e);
            }
        });

        // Spawn event forwarder
        let notification_tx = self.notification_tx.clone();
        let event_rx = handle.event_rx.clone();
        let session_id_clone = session_id.clone();
        let mut cancel_rx = cancel_tx.subscribe();

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    result = event_rx.recv() => {
                        match result {
                            Ok(event) => {
                                if let Some(notification) = event_to_notification(&session_id_clone, event.msg) {
                                    let _ = notification_tx.send(notification);
                                }
                            }
                            Err(_) => break,
                        }
                    }
                    _ = cancel_rx.recv() => {
                        break;
                    }
                }
            }
        });

        Ok(LoadSessionResponse {
            session_id,
            models: None,
            modes: None,
        })
    }

    /// Handle session/list request.
    pub async fn handle_session_list(
        &self,
        _params: ListSessionsRequest,
    ) -> Result<ListSessionsResponse> {
        let sessions = crate::list_sessions(&self.config.cortex_home)?;

        let session_infos: Vec<SessionListInfo> = sessions
            .into_iter()
            .map(|s| SessionListInfo {
                session_id: s.id,
                title: None,
                cwd: s.cwd.display().to_string(),
                created_at: s.timestamp,
                message_count: s.message_count,
            })
            .collect();

        Ok(ListSessionsResponse {
            sessions: session_infos,
        })
    }

    /// Handle session/prompt request.
    pub async fn handle_session_prompt(&self, params: PromptRequest) -> Result<PromptResponse> {
        let sessions = self.sessions.read().await;
        let state = sessions
            .get(&params.session_id)
            .context("Session not found")?;
        let handle = state.handle.clone();
        drop(sessions);

        // Convert prompt content to user inputs
        let mut user_inputs = Vec::new();
        for content in params.prompt {
            match content {
                PromptContent::Text { text } => {
                    user_inputs.push(UserInput::Text { text });
                }
                PromptContent::Image {
                    data,
                    uri,
                    mime_type,
                } => {
                    if let Some(data) = data {
                        user_inputs.push(UserInput::Image {
                            media_type: mime_type,
                            data,
                        });
                    } else if let Some(_uri) = uri {
                        // Image URI fetching not yet implemented - skipping
                        warn!("Image URI not yet supported, skipping");
                    }
                }
                PromptContent::Resource { resource } => match resource {
                    Resource::Text { text } => {
                        user_inputs.push(UserInput::Text { text });
                    }
                },
                PromptContent::ResourceLink { uri } => {
                    // Include the URI as text context
                    user_inputs.push(UserInput::Text {
                        text: format!("Resource: {uri}"),
                    });
                }
            }
        }

        let submission = Submission {
            id: uuid::Uuid::new_v4().to_string(),
            op: Op::UserInput { items: user_inputs },
        };

        handle.submission_tx.send(submission).await?;

        // Wait for turn completion
        let event_rx = handle.event_rx.clone();
        while let Ok(event) = event_rx.recv().await {
            match event.msg {
                EventMsg::TaskComplete(_) => break,
                EventMsg::Error(_) => break,
                _ => {}
            }
        }

        Ok(PromptResponse {
            stop_reason: StopReason::EndTurn,
        })
    }

    /// Handle session/cancel request.
    pub async fn handle_session_cancel(&self, params: CancelRequest) -> Result<CancelResponse> {
        let sessions = self.sessions.read().await;
        let state = sessions
            .get(&params.session_id)
            .context("Session not found")?;

        // Signal cancellation
        let _ = state.cancel_tx.send(());

        // Send interrupt submission
        let submission = Submission {
            id: uuid::Uuid::new_v4().to_string(),
            op: Op::Interrupt,
        };
        let _ = state.handle.submission_tx.send(submission).await;

        Ok(CancelResponse { cancelled: true })
    }

    /// Handle models/list request.
    pub async fn handle_models_list(&self) -> Result<ModelsListResponse> {
        Ok(ModelsListResponse {
            models: vec![
                ModelInfo {
                    model_id: "claude-sonnet-4-20250514".to_string(),
                    name: "Claude Sonnet 4".to_string(),
                },
                ModelInfo {
                    model_id: "claude-3-5-sonnet-20241022".to_string(),
                    name: "Claude 3.5 Sonnet".to_string(),
                },
                ModelInfo {
                    model_id: "gpt-4o".to_string(),
                    name: "GPT-4o".to_string(),
                },
                ModelInfo {
                    model_id: "gpt-4o-mini".to_string(),
                    name: "GPT-4o Mini".to_string(),
                },
            ],
        })
    }

    /// Handle agents/list request.
    pub async fn handle_agents_list(&self) -> Result<AgentsListResponse> {
        Ok(AgentsListResponse {
            agents: vec![AgentInfo {
                name: "Cortex".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            }],
        })
    }

    /// Process a JSON-RPC request and return a response.
    pub async fn process_request(
        &self,
        id: AcpRequestId,
        method: &str,
        params: Value,
    ) -> AcpResponse {
        match method {
            "initialize" => match serde_json::from_value::<InitializeRequest>(params) {
                Ok(req) => match self.handle_initialize(req).await {
                    Ok(resp) => AcpResponse::success(id, serde_json::to_value(resp).unwrap()),
                    Err(e) => AcpResponse::error(id, AcpError::internal(e.to_string())),
                },
                Err(e) => AcpResponse::error(id, AcpError::invalid_params(e.to_string())),
            },
            "session/new" => match serde_json::from_value::<NewSessionRequest>(params) {
                Ok(req) => match self.handle_session_new(req).await {
                    Ok(resp) => AcpResponse::success(id, serde_json::to_value(resp).unwrap()),
                    Err(e) => AcpResponse::error(id, AcpError::internal(e.to_string())),
                },
                Err(e) => AcpResponse::error(id, AcpError::invalid_params(e.to_string())),
            },
            "session/load" => match serde_json::from_value::<LoadSessionRequest>(params) {
                Ok(req) => match self.handle_session_load(req).await {
                    Ok(resp) => AcpResponse::success(id, serde_json::to_value(resp).unwrap()),
                    Err(e) => AcpResponse::error(id, AcpError::session_not_found(&e.to_string())),
                },
                Err(e) => AcpResponse::error(id, AcpError::invalid_params(e.to_string())),
            },
            "session/list" => match serde_json::from_value::<ListSessionsRequest>(params) {
                Ok(req) => match self.handle_session_list(req).await {
                    Ok(resp) => AcpResponse::success(id, serde_json::to_value(resp).unwrap()),
                    Err(e) => AcpResponse::error(id, AcpError::internal(e.to_string())),
                },
                Err(e) => AcpResponse::error(id, AcpError::invalid_params(e.to_string())),
            },
            "session/prompt" => match serde_json::from_value::<PromptRequest>(params) {
                Ok(req) => match self.handle_session_prompt(req).await {
                    Ok(resp) => AcpResponse::success(id, serde_json::to_value(resp).unwrap()),
                    Err(e) => AcpResponse::error(id, AcpError::internal(e.to_string())),
                },
                Err(e) => AcpResponse::error(id, AcpError::invalid_params(e.to_string())),
            },
            "session/cancel" => match serde_json::from_value::<CancelRequest>(params) {
                Ok(req) => match self.handle_session_cancel(req).await {
                    Ok(resp) => AcpResponse::success(id, serde_json::to_value(resp).unwrap()),
                    Err(e) => AcpResponse::error(id, AcpError::session_not_found(&e.to_string())),
                },
                Err(e) => AcpResponse::error(id, AcpError::invalid_params(e.to_string())),
            },
            "models/list" => match self.handle_models_list().await {
                Ok(resp) => AcpResponse::success(id, serde_json::to_value(resp).unwrap()),
                Err(e) => AcpResponse::error(id, AcpError::internal(e.to_string())),
            },
            "agents/list" => match self.handle_agents_list().await {
                Ok(resp) => AcpResponse::success(id, serde_json::to_value(resp).unwrap()),
                Err(e) => AcpResponse::error(id, AcpError::internal(e.to_string())),
            },
            _ => AcpResponse::error(id, AcpError::method_not_found(method)),
        }
    }
}

/// Convert an event message to a notification.
fn event_to_notification(session_id: &str, msg: EventMsg) -> Option<AcpNotificationEvent> {
    let update = match msg {
        EventMsg::AgentMessageDelta(delta) => SessionUpdate::AgentMessageChunk {
            content: MessageContent::Text { text: delta.delta },
        },
        EventMsg::AgentReasoningDelta(delta) => SessionUpdate::AgentThoughtChunk {
            content: MessageContent::Text { text: delta.delta },
        },
        EventMsg::ExecCommandBegin(begin) => SessionUpdate::ToolCall {
            tool_call_id: begin.call_id,
            title: begin.command.join(" "),
            kind: ToolKind::Execute,
            status: ToolStatus::InProgress,
            locations: vec![],
            raw_input: Value::Null,
        },
        EventMsg::ExecCommandEnd(end) => SessionUpdate::ToolCallUpdate {
            tool_call_id: end.call_id,
            status: if end.exit_code == 0 {
                ToolStatus::Completed
            } else {
                ToolStatus::Failed
            },
            content: None,
            raw_output: None,
        },
        EventMsg::McpToolCallBegin(begin) => SessionUpdate::ToolCall {
            tool_call_id: begin.call_id,
            title: format!("{}:{}", begin.invocation.server, begin.invocation.tool),
            kind: ToolKind::Other,
            status: ToolStatus::InProgress,
            locations: vec![],
            raw_input: begin.invocation.arguments.unwrap_or(Value::Null),
        },
        EventMsg::McpToolCallEnd(end) => SessionUpdate::ToolCallUpdate {
            tool_call_id: end.call_id,
            status: if end.result.is_ok() {
                ToolStatus::Completed
            } else {
                ToolStatus::Failed
            },
            content: None,
            raw_output: Some(serde_json::to_value(&end.result).unwrap_or(Value::Null)),
        },
        _ => return None,
    };

    let notification = SessionNotification {
        session_id: session_id.to_string(),
        update,
    };

    Some(AcpNotificationEvent {
        method: "session/update".to_string(),
        params: serde_json::to_value(notification).unwrap_or(Value::Null),
    })
}

// Additional request/response types for extended protocol

/// Load session request.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadSessionRequest {
    pub session_id: String,
}

/// Load session response.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadSessionResponse {
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub models: Option<SessionModels>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modes: Option<SessionModes>,
}

/// List sessions request.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListSessionsRequest {
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
}

/// List sessions response.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListSessionsResponse {
    pub sessions: Vec<SessionListInfo>,
}

/// Session list info.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionListInfo {
    pub session_id: String,
    pub title: Option<String>,
    pub cwd: String,
    pub created_at: String,
    pub message_count: usize,
}

/// Cancel request.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelRequest {
    pub session_id: String,
}

/// Cancel response.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelResponse {
    pub cancelled: bool,
}

/// Models list response.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelsListResponse {
    pub models: Vec<ModelInfo>,
}

/// Agents list response.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentsListResponse {
    pub agents: Vec<AgentInfo>,
}
