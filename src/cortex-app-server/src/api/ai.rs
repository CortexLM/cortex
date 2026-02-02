//! AI transformation and prediction endpoints.

use std::sync::Arc;

use axum::{Json, extract::State};

use crate::error::{AppError, AppResult};
use crate::state::AppState;

use super::types::{AiInlineRequest, AiPredictRequest};

/// Handle AI inline code transformation requests.
///
/// This endpoint is a placeholder for future AI-powered code transformation.
/// Currently returns an error indicating the feature is not yet available.
/// Full implementation requires LLM provider integration from cortex-engine.
pub async fn ai_inline(
    State(_state): State<Arc<AppState>>,
    Json(_req): Json<AiInlineRequest>,
) -> AppResult<Json<serde_json::Value>> {
    // AI inline transformation requires LLM provider integration.
    // This feature is available through the TUI (cargo run --bin cortex).
    Err(AppError::NotImplemented(
        "AI inline transformation is not yet available via API. Please use the CLI/TUI for AI-powered code transformations.".to_string(),
    ))
}

/// Handle AI code prediction/completion requests.
///
/// This endpoint is a placeholder for future AI-powered code prediction.
/// Currently returns an error indicating the feature is not yet available.
/// Full implementation requires LLM provider integration from cortex-engine.
pub async fn ai_predict(
    State(_state): State<Arc<AppState>>,
    Json(_req): Json<AiPredictRequest>,
) -> AppResult<Json<serde_json::Value>> {
    // AI prediction requires LLM provider integration.
    // This feature is available through the TUI (cargo run --bin cortex).
    Err(AppError::NotImplemented(
        "AI prediction is not yet available via API. Please use the CLI/TUI for AI-powered completions.".to_string(),
    ))
}
