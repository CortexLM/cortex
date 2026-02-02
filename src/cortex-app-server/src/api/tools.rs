//! Tool management and execution endpoints.

use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
};

use crate::error::AppResult;
use crate::state::AppState;
use crate::tools::{ToolDefinition, ToolExecutor, get_tool_definitions};

use super::types::{ExecuteToolRequest, ExecuteToolResponse};

/// List available tools.
pub async fn list_tools(_state: State<Arc<AppState>>) -> Json<Vec<ToolDefinition>> {
    Json(get_tool_definitions())
}

/// Execute a tool.
pub async fn execute_tool(
    State(_state): State<Arc<AppState>>,
    Path(name): Path<String>,
    Json(req): Json<ExecuteToolRequest>,
) -> AppResult<Json<ExecuteToolResponse>> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("/"));
    let executor = ToolExecutor::new(cwd);

    let result = executor.execute(&name, req.arguments).await;

    Ok(Json(ExecuteToolResponse {
        success: result.success,
        output: result.output,
        error: result.error,
        metadata: result.metadata,
    }))
}
