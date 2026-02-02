//! Model information types.
//!
//! This module provides model metadata types for the Cortex CLI.
//! Models are fetched from the Cortex backend API.

use serde::{Deserialize, Serialize};

/// Model information from the backend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    /// Model identifier (e.g., "anthropic/claude-opus-4.5").
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Provider name (e.g., "Anthropic").
    pub provider: String,
    /// Context window size.
    pub context_length: u32,
    /// Alias for context_length (for compatibility).
    pub context_window: u32,
    /// Maximum output tokens.
    pub max_output_tokens: u32,
    /// Brief description.
    pub description: String,
    /// Whether this model supports vision/images.
    pub vision: bool,
    /// Whether this model supports tool calling.
    pub tools: bool,
    /// Credit multiplier for input tokens (from API).
    pub credit_multiplier_input: Option<String>,
    /// Credit multiplier for output tokens (from API).
    pub credit_multiplier_output: Option<String>,
    /// Price version for price verification (from API).
    pub price_version: Option<i32>,
}

impl ModelInfo {
    /// Create a new model info.
    pub fn new(id: &str, name: &str, provider: &str) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
            provider: provider.to_string(),
            context_length: 200_000,
            context_window: 200_000,
            max_output_tokens: 8192,
            description: String::new(),
            vision: true,
            tools: true,
            credit_multiplier_input: None,
            credit_multiplier_output: None,
            price_version: None,
        }
    }

    /// Create with context window.
    pub fn with_context(mut self, context_window: u32) -> Self {
        self.context_length = context_window;
        self.context_window = context_window;
        self
    }

    /// Create with vision capability.
    pub fn with_vision(mut self, vision: bool) -> Self {
        self.vision = vision;
        self
    }

    /// Create with tools capability.
    pub fn with_tools(mut self, tools: bool) -> Self {
        self.tools = tools;
        self
    }
}

/// Get models - returns empty vec. Models must come from Cortex API.
pub fn get_popular_models() -> Vec<ModelInfo> {
    Vec::new()
}

/// Get models for a provider - returns empty vec. Models must come from Cortex API.
pub fn get_models_for_provider(_provider: &str) -> Vec<ModelInfo> {
    Vec::new()
}
