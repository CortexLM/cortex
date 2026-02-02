//! Chat completions endpoint (OpenAI-compatible).

use std::sync::Arc;

use axum::{Json, extract::State};
use uuid::Uuid;

use crate::error::AppResult;
use crate::state::AppState;

use super::types::{
    ChatChoice, ChatCompletionRequest, ChatCompletionResponse, ChatMessage, ChatUsage,
};

/// Handle chat completions by calling the actual LLM provider.
/// NOTE: This endpoint is temporarily disabled as the providers module was removed.
/// Use the TUI (cargo run --bin cortex) for LLM completions instead.
pub async fn chat_completions(
    State(_state): State<Arc<AppState>>,
    Json(req): Json<ChatCompletionRequest>,
) -> AppResult<Json<ChatCompletionResponse>> {
    // Return a stub response indicating the endpoint is not implemented
    // The providers module was removed as dead code - use the TUI instead
    Ok(Json(ChatCompletionResponse {
        id: format!("chatcmpl-{}", Uuid::new_v4()),
        object: "chat.completion".to_string(),
        created: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs(),
        model: req.model,
        choices: vec![ChatChoice {
            index: 0,
            message: ChatMessage {
                role: "assistant".to_string(),
                content: "Chat completions API endpoint is temporarily disabled. The providers module was removed during dead code cleanup. Please use the TUI (cargo run --bin cortex) for LLM interactions.".to_string(),
                tool_call_id: None,
                tool_calls: None,
            },
            finish_reason: "stop".to_string(),
        }],
        usage: ChatUsage {
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
        },
    }))
}
