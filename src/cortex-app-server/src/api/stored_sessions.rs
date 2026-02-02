//! Stored sessions (persistent) endpoints.

use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
};

use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::storage::{StoredMessage, StoredSession};

/// List all stored sessions.
pub async fn list_stored_sessions(
    State(state): State<Arc<AppState>>,
) -> AppResult<Json<Vec<StoredSession>>> {
    let sessions = state
        .cli_sessions
        .storage()
        .list_sessions()
        .map_err(|e| AppError::Internal(format!("Failed to list sessions: {}", e)))?;
    Ok(Json(sessions))
}

/// Get a stored session by ID.
pub async fn get_stored_session(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> AppResult<Json<StoredSession>> {
    let session = state
        .cli_sessions
        .storage()
        .load_session(&id)
        .map_err(|e| AppError::NotFound(format!("Session not found: {}", e)))?;
    Ok(Json(session))
}

/// Delete a stored session.
pub async fn delete_stored_session(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> AppResult<Json<serde_json::Value>> {
    state
        .cli_sessions
        .storage()
        .delete_session(&id)
        .map_err(|e| AppError::Internal(format!("Failed to delete session: {}", e)))?;
    Ok(Json(serde_json::json!({"deleted": true})))
}

/// Get session history (all messages).
pub async fn get_session_history(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> AppResult<Json<Vec<StoredMessage>>> {
    let messages = state
        .cli_sessions
        .storage()
        .read_history(&id)
        .map_err(|e| AppError::Internal(format!("Failed to read history: {}", e)))?;
    Ok(Json(messages))
}
