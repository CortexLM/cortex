//! Session management endpoints.

use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, Query, State},
};
use uuid::Uuid;

use crate::error::AppResult;
use crate::state::{AppState, CreateSessionOptions, SessionMessage, SessionStatus};

use super::types::{
    CreateSessionRequest, ListSessionsQuery, MessageResponse, SendMessageRequest, SessionListItem,
    SessionResponse, ToolCallResponse,
};

/// Format session status for API response.
pub fn format_status(status: &SessionStatus) -> String {
    match status {
        SessionStatus::Active => "active".to_string(),
        SessionStatus::Processing => "processing".to_string(),
        SessionStatus::Paused => "paused".to_string(),
        SessionStatus::Completed => "completed".to_string(),
        SessionStatus::Error(msg) => format!("error: {msg}"),
    }
}

/// Create a new session.
pub async fn create_session(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateSessionRequest>,
) -> AppResult<Json<SessionResponse>> {
    let session = state
        .create_session(CreateSessionOptions {
            user_id: None,
            model: req.model,
            system_prompt: req.system_prompt,
            metadata: req.metadata,
        })
        .await?;

    Ok(Json(SessionResponse {
        id: session.id,
        model: session.model,
        status: format_status(&session.status),
        message_count: session.messages.len(),
        total_tokens: session.total_tokens,
        system_prompt: session.system_prompt,
        metadata: session.metadata,
    }))
}

/// List all sessions.
pub async fn list_sessions(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ListSessionsQuery>,
) -> Json<Vec<SessionListItem>> {
    let sessions = state.list_sessions(query.limit, query.offset).await;
    Json(
        sessions
            .into_iter()
            .map(|s| SessionListItem {
                id: s.id,
                model: s.model,
                status: format_status(&s.status),
                message_count: s.message_count,
                total_tokens: s.total_tokens,
            })
            .collect(),
    )
}

/// Get a session by ID.
pub async fn get_session(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> AppResult<Json<SessionResponse>> {
    let session = state.get_session(&id).await?;
    Ok(Json(SessionResponse {
        id: session.id,
        model: session.model,
        status: format_status(&session.status),
        message_count: session.messages.len(),
        total_tokens: session.total_tokens,
        system_prompt: session.system_prompt,
        metadata: session.metadata,
    }))
}

/// Delete a session.
pub async fn delete_session(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> AppResult<Json<serde_json::Value>> {
    state.delete_session(&id).await?;
    Ok(Json(serde_json::json!({ "deleted": true })))
}

/// Send a message to a session.
pub async fn send_message(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<SendMessageRequest>,
) -> AppResult<Json<MessageResponse>> {
    let message = SessionMessage {
        id: Uuid::new_v4().to_string(),
        role: req.role.unwrap_or_else(|| "user".to_string()),
        content: req.content,
        tokens: 0, // Will be calculated
        tool_calls: None,
        timestamp: std::time::Instant::now(),
    };

    let msg_clone = message.clone();
    state
        .update_session(&id, |session| {
            session.add_message(msg_clone);
        })
        .await?;

    Ok(Json(MessageResponse {
        id: message.id,
        role: message.role,
        content: message.content,
        tokens: message.tokens,
        tool_calls: None,
    }))
}

/// List messages in a session.
pub async fn list_messages(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> AppResult<Json<Vec<MessageResponse>>> {
    let session = state.get_session(&id).await?;
    Ok(Json(
        session
            .messages
            .into_iter()
            .map(|m| MessageResponse {
                id: m.id,
                role: m.role,
                content: m.content,
                tokens: m.tokens,
                tool_calls: m.tool_calls.map(|calls| {
                    calls
                        .into_iter()
                        .map(|c| ToolCallResponse {
                            id: c.id,
                            name: c.name,
                            arguments: serde_json::from_str(&c.arguments).unwrap_or_default(),
                            result: c.result,
                        })
                        .collect()
                }),
            })
            .collect(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_status() {
        assert_eq!(format_status(&SessionStatus::Active), "active");
        assert_eq!(format_status(&SessionStatus::Processing), "processing");
        assert_eq!(
            format_status(&SessionStatus::Error("test".into())),
            "error: test"
        );
    }
}
