//! Model management endpoints.

use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
};

use crate::error::{AppError, AppResult};
use crate::state::AppState;

use super::types::ModelInfo;

/// List available models.
pub async fn list_models(_state: State<Arc<AppState>>) -> Json<Vec<ModelInfo>> {
    Json(vec![
        ModelInfo {
            id: "gpt-4o".to_string(),
            name: "GPT-4o".to_string(),
            provider: "openai".to_string(),
            context_window: 128000,
            max_output_tokens: Some(16384),
            supports_vision: true,
            supports_tools: true,
            supports_streaming: true,
        },
        ModelInfo {
            id: "gpt-4o-mini".to_string(),
            name: "GPT-4o Mini".to_string(),
            provider: "openai".to_string(),
            context_window: 128000,
            max_output_tokens: Some(16384),
            supports_vision: true,
            supports_tools: true,
            supports_streaming: true,
        },
        ModelInfo {
            id: "o1".to_string(),
            name: "o1".to_string(),
            provider: "openai".to_string(),
            context_window: 200000,
            max_output_tokens: Some(100000),
            supports_vision: true,
            supports_tools: false,
            supports_streaming: false,
        },
        ModelInfo {
            id: "o3-mini".to_string(),
            name: "o3-mini".to_string(),
            provider: "openai".to_string(),
            context_window: 200000,
            max_output_tokens: Some(100000),
            supports_vision: true,
            supports_tools: false,
            supports_streaming: false,
        },
        ModelInfo {
            id: "claude-3-5-sonnet-20241022".to_string(),
            name: "Claude 3.5 Sonnet".to_string(),
            provider: "anthropic".to_string(),
            context_window: 200000,
            max_output_tokens: Some(8192),
            supports_vision: true,
            supports_tools: true,
            supports_streaming: true,
        },
        ModelInfo {
            id: "claude-3-5-haiku-20241022".to_string(),
            name: "Claude 3.5 Haiku".to_string(),
            provider: "anthropic".to_string(),
            context_window: 200000,
            max_output_tokens: Some(8192),
            supports_vision: true,
            supports_tools: true,
            supports_streaming: true,
        },
    ])
}

/// Get a specific model.
pub async fn get_model(Path(id): Path<String>) -> AppResult<Json<ModelInfo>> {
    let models = vec![ModelInfo {
        id: "gpt-4o".to_string(),
        name: "GPT-4o".to_string(),
        provider: "openai".to_string(),
        context_window: 128000,
        max_output_tokens: Some(16384),
        supports_vision: true,
        supports_tools: true,
        supports_streaming: true,
    }];

    models
        .into_iter()
        .find(|m| m.id == id)
        .map(Json)
        .ok_or_else(|| AppError::NotFound(format!("Model not found: {id}")))
}

/// Parse model string to determine provider and model ID.
/// Supports formats like "gpt-4o", "openai/gpt-4o", "anthropic/claude-3-5-sonnet"
#[allow(dead_code)]
pub fn parse_model_provider(model: &str) -> (String, String) {
    if model.contains('/') {
        let parts: Vec<&str> = model.splitn(2, '/').collect();
        (parts[0].to_string(), parts[1].to_string())
    } else {
        // Infer provider from model name
        let provider =
            if model.starts_with("gpt-") || model.starts_with("o1") || model.starts_with("o3") {
                "openai"
            } else if model.starts_with("claude") {
                "anthropic"
            } else {
                "cortex" // Default to Cortex for unknown models
            };
        (provider.to_string(), model.to_string())
    }
}
