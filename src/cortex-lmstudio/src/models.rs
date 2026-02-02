//! Model types for LM Studio API

use serde::{Deserialize, Serialize};

/// Model information returned by LM Studio
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Model {
    /// Model identifier
    pub id: String,
    /// Object type (usually "model")
    #[serde(default)]
    pub object: String,
    /// Owner/creator of the model
    #[serde(default)]
    pub owned_by: String,
    /// Creation timestamp
    #[serde(default)]
    pub created: i64,
}

/// Extended model information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    /// Model identifier
    pub id: String,
    /// Human-readable name
    #[serde(default)]
    pub name: String,
    /// Model description
    #[serde(default)]
    pub description: String,
    /// Context window size
    #[serde(default)]
    pub context_length: usize,
    /// Whether the model is currently loaded
    #[serde(default)]
    pub loaded: bool,
    /// Model file path
    #[serde(default)]
    pub path: String,
}

/// Chat message for completions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    /// Role of the message author (system, user, assistant)
    pub role: String,
    /// Message content
    pub content: String,
}

impl ChatMessage {
    /// Create a system message
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".to_string(),
            content: content.into(),
        }
    }

    /// Create a user message
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".to_string(),
            content: content.into(),
        }
    }

    /// Create an assistant message
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: "assistant".to_string(),
            content: content.into(),
        }
    }
}

/// Chat completion request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatRequest {
    /// Model to use for completion
    pub model: String,
    /// Messages in the conversation
    pub messages: Vec<ChatMessage>,
    /// Maximum tokens to generate
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    /// Sampling temperature (0.0 to 2.0)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    /// Top-p sampling (0.0 to 1.0)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    /// Whether to stream the response
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    /// Stop sequences
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop: Option<Vec<String>>,
}

impl ChatRequest {
    /// Create a new chat request
    pub fn new(model: impl Into<String>, messages: Vec<ChatMessage>) -> Self {
        Self {
            model: model.into(),
            messages,
            max_tokens: None,
            temperature: None,
            top_p: None,
            stream: None,
            stop: None,
        }
    }

    /// Set max tokens
    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = Some(max_tokens);
        self
    }

    /// Set temperature
    pub fn with_temperature(mut self, temperature: f32) -> Self {
        self.temperature = Some(temperature);
        self
    }

    /// Set top_p
    pub fn with_top_p(mut self, top_p: f32) -> Self {
        self.top_p = Some(top_p);
        self
    }

    /// Enable streaming
    pub fn with_stream(mut self, stream: bool) -> Self {
        self.stream = Some(stream);
        self
    }

    /// Set stop sequences
    pub fn with_stop(mut self, stop: Vec<String>) -> Self {
        self.stop = Some(stop);
        self
    }
}

/// Choice in a chat completion response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatChoice {
    /// Index of this choice
    pub index: usize,
    /// The message generated
    pub message: ChatMessage,
    /// Reason for finishing (stop, length, etc.)
    #[serde(default)]
    pub finish_reason: Option<String>,
}

/// Usage statistics for a completion
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Usage {
    /// Tokens in the prompt
    pub prompt_tokens: u32,
    /// Tokens in the completion
    pub completion_tokens: u32,
    /// Total tokens used
    pub total_tokens: u32,
}

/// Chat completion response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatResponse {
    /// Unique identifier for this completion
    pub id: String,
    /// Object type (usually "chat.completion")
    pub object: String,
    /// Creation timestamp
    pub created: i64,
    /// Model used
    pub model: String,
    /// Generated choices
    pub choices: Vec<ChatChoice>,
    /// Usage statistics
    #[serde(default)]
    pub usage: Option<Usage>,
}

impl ChatResponse {
    /// Get the first choice's message content, if any
    pub fn content(&self) -> Option<&str> {
        self.choices.first().map(|c| c.message.content.as_str())
    }
}

/// Models list response from LM Studio API
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelsResponse {
    /// Object type
    pub object: String,
    /// List of models
    pub data: Vec<Model>,
}
