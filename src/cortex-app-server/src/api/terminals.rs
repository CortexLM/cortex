//! Terminal management endpoints.
//!
//! NOTE: Terminal functionality has been removed. These endpoints are stubs
//! for API compatibility.

use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, Query, State},
};

use crate::error::{AppError, AppResult};
use crate::state::AppState;

use super::types::{TerminalLogEntry, TerminalLogsQuery, TerminalResponse};

/// List all terminals.
/// NOTE: Terminal functionality has been removed. Returns empty list.
pub async fn list_terminals(
    State(_state): State<Arc<AppState>>,
) -> AppResult<Json<Vec<TerminalResponse>>> {
    // Terminal functionality removed - return empty list
    Ok(Json(Vec::new()))
}

/// Get terminal logs.
/// NOTE: Terminal functionality has been removed. Always returns not found.
pub async fn get_terminal_logs(
    State(_state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(_query): Query<TerminalLogsQuery>,
) -> AppResult<Json<Vec<TerminalLogEntry>>> {
    // Terminal functionality removed
    Err(AppError::NotFound(format!(
        "Terminal not found: {}. Terminal functionality has been removed.",
        id
    )))
}
