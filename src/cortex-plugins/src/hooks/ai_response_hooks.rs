//! AI response hooks (before, stream, and after).

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::types::{HookPriority, HookResult};
use crate::Result;

// ============================================================================
// AI Response Before Hook
// ============================================================================

/// Input for ai.response.before hook - before AI starts generating.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiResponseBeforeInput {
    /// Session ID
    pub session_id: String,
    /// Request ID
    pub request_id: String,
    /// Model being used
    pub model: String,
    /// Temperature setting
    pub temperature: Option<f32>,
    /// Max tokens
    pub max_tokens: Option<u32>,
    /// Whether streaming is enabled
    pub streaming: bool,
}

/// Output for ai.response.before hook (mutable).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiResponseBeforeOutput {
    /// Modified model (can switch models)
    pub model: String,
    /// Modified temperature
    pub temperature: Option<f32>,
    /// Modified max tokens
    pub max_tokens: Option<u32>,
    /// Additional parameters to pass to the model
    pub extra_params: HashMap<String, serde_json::Value>,
    /// Hook result
    pub result: HookResult,
}

impl AiResponseBeforeOutput {
    /// Create a new output with the original settings.
    pub fn new(model: String, temperature: Option<f32>, max_tokens: Option<u32>) -> Self {
        Self {
            model,
            temperature,
            max_tokens,
            extra_params: HashMap::new(),
            result: HookResult::Continue,
        }
    }
}

/// Handler for ai.response.before hook.
#[async_trait]
pub trait AiResponseBeforeHook: Send + Sync {
    fn priority(&self) -> HookPriority {
        HookPriority::default()
    }

    async fn execute(
        &self,
        input: &AiResponseBeforeInput,
        output: &mut AiResponseBeforeOutput,
    ) -> Result<()>;
}

// ============================================================================
// AI Response Stream Hook
// ============================================================================

/// Input for ai.response.stream hook - called for each streaming chunk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiResponseStreamInput {
    /// Session ID
    pub session_id: String,
    /// Request ID
    pub request_id: String,
    /// Chunk index
    pub chunk_index: usize,
    /// Whether this is the final chunk
    pub is_final: bool,
}

/// Output for ai.response.stream hook (mutable).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiResponseStreamOutput {
    /// Chunk content
    pub content: String,
    /// Hook result
    pub result: HookResult,
}

impl AiResponseStreamOutput {
    pub fn new(content: String) -> Self {
        Self {
            content,
            result: HookResult::Continue,
        }
    }
}

/// Handler for ai.response.stream hook.
#[async_trait]
pub trait AiResponseStreamHook: Send + Sync {
    fn priority(&self) -> HookPriority {
        HookPriority::default()
    }

    async fn execute(
        &self,
        input: &AiResponseStreamInput,
        output: &mut AiResponseStreamOutput,
    ) -> Result<()>;
}

// ============================================================================
// AI Response After Hook
// ============================================================================

/// Input for ai.response.after hook - after AI finishes generating.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiResponseAfterInput {
    /// Session ID
    pub session_id: String,
    /// Request ID
    pub request_id: String,
    /// Model used
    pub model: String,
    /// Token usage
    pub usage: Option<TokenUsage>,
    /// Duration in milliseconds
    pub duration_ms: u64,
    /// Whether the response was successful
    pub success: bool,
}

/// Token usage statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenUsage {
    /// Prompt tokens
    pub prompt_tokens: u32,
    /// Completion tokens
    pub completion_tokens: u32,
    /// Total tokens
    pub total_tokens: u32,
    /// Cached tokens (if any)
    pub cached_tokens: Option<u32>,
}

/// Output for ai.response.after hook (mutable).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiResponseAfterOutput {
    /// Response content
    pub content: String,
    /// Whether to save to history
    pub save_to_history: bool,
    /// Additional metadata
    pub metadata: HashMap<String, serde_json::Value>,
    /// Hook result
    pub result: HookResult,
}

impl AiResponseAfterOutput {
    pub fn new(content: String) -> Self {
        Self {
            content,
            save_to_history: true,
            metadata: HashMap::new(),
            result: HookResult::Continue,
        }
    }
}

/// Handler for ai.response.after hook.
#[async_trait]
pub trait AiResponseAfterHook: Send + Sync {
    fn priority(&self) -> HookPriority {
        HookPriority::default()
    }

    async fn execute(
        &self,
        input: &AiResponseAfterInput,
        output: &mut AiResponseAfterOutput,
    ) -> Result<()>;
}
