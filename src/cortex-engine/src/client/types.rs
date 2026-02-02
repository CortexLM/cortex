//! Client types shared across providers.

use serde::{Deserialize, Serialize};

/// Completion request.
#[derive(Debug, Clone, Serialize)]
pub struct CompletionRequest {
    /// Messages in the conversation.
    pub messages: Vec<Message>,
    /// Model to use.
    pub model: String,
    /// Maximum tokens to generate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    /// Temperature for sampling.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    /// Random seed for reproducibility.
    /// When set, the same seed with identical inputs should produce deterministic outputs.
    /// Note: This is applied to all model calls including tool invocations.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
    /// Tools available for the model.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolDefinition>,
    /// Whether to stream the response.
    #[serde(skip)]
    pub stream: bool,
}

impl Default for CompletionRequest {
    fn default() -> Self {
        Self {
            messages: vec![],
            model: String::new(),
            max_tokens: None,
            temperature: None,
            seed: None,
            tools: vec![],
            stream: true,
        }
    }
}

/// A message in the conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    /// Role of the message sender.
    pub role: MessageRole,
    /// Content of the message.
    pub content: MessageContent,
    /// Tool call ID (for tool results).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// Tool calls made by the assistant.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
}

impl Message {
    /// Create a user message.
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::User,
            content: MessageContent::Text(content.into()),
            tool_call_id: None,
            tool_calls: None,
        }
    }

    /// Create an assistant message.
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::Assistant,
            content: MessageContent::Text(content.into()),
            tool_call_id: None,
            tool_calls: None,
        }
    }

    /// Create a system message.
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::System,
            content: MessageContent::Text(content.into()),
            tool_call_id: None,
            tool_calls: None,
        }
    }

    /// Create a tool result message.
    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::Tool,
            content: MessageContent::Text(content.into()),
            tool_call_id: Some(tool_call_id.into()),
            tool_calls: None,
        }
    }
}

/// Role of a message sender.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    System,
    User,
    Assistant,
    Tool,
}

/// Content of a message.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessageContent {
    /// Simple text content.
    Text(String),
    /// Multi-part content (text and images).
    Parts(Vec<ContentPart>),
    /// Tool result content.
    ToolResult {
        tool_call_id: String,
        content: String,
    },
    /// Tool calls made.
    ToolCalls(Vec<super::ToolCallRef>),
}

impl MessageContent {
    /// Get the text content.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text(s) => Some(s),
            Self::Parts(parts) => parts.iter().find_map(|p| match p {
                ContentPart::Text { text, .. } => Some(text.as_str()),
                _ => None,
            }),
            Self::ToolResult { content, .. } => Some(content),
            Self::ToolCalls(_) => None,
        }
    }
}

/// Cache control for prompt caching (OpenRouter/Anthropic/Gemini).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheControl {
    /// Cache type (always "ephemeral" for now).
    #[serde(rename = "type")]
    pub cache_type: String,
    /// Optional TTL ("1h" for extended caching on Anthropic).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl: Option<String>,
}

impl CacheControl {
    /// Create ephemeral cache control (5 min default TTL).
    pub fn ephemeral() -> Self {
        Self {
            cache_type: "ephemeral".to_string(),
            ttl: None,
        }
    }

    /// Create ephemeral cache control with 1 hour TTL (Anthropic only).
    pub fn ephemeral_1h() -> Self {
        Self {
            cache_type: "ephemeral".to_string(),
            ttl: Some("1h".to_string()),
        }
    }
}

/// Part of a multi-part message.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentPart {
    /// Text content with optional cache control.
    Text {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    /// Image URL.
    ImageUrl { image_url: ImageUrl },
    /// Image content (alternative format).
    Image {
        url: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
    /// Document content.
    Document {
        data: String,
        mime_type: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
}

/// Image URL with optional detail level.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageUrl {
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Tool definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// Type of tool (always "function" for now).
    #[serde(rename = "type")]
    pub tool_type: String,
    /// Function definition.
    pub function: FunctionDefinition,
}

impl ToolDefinition {
    /// Create a new function tool definition.
    pub fn function(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: serde_json::Value,
    ) -> Self {
        Self {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: name.into(),
                description: description.into(),
                parameters,
            },
        }
    }

    /// Get the tool name.
    pub fn name(&self) -> &str {
        &self.function.name
    }

    /// Get the tool description.
    pub fn description(&self) -> &str {
        &self.function.description
    }

    /// Get the parameters schema.
    pub fn parameters(&self) -> &serde_json::Value {
        &self.function.parameters
    }
}

/// Function definition for a tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// Tool call from the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub call_type: String,
    pub function: FunctionCall,
}

/// Function call details.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String,
}

/// Response event from streaming.
#[derive(Debug, Clone)]
pub enum ResponseEvent {
    /// Text delta.
    Delta(String),
    /// Tool call.
    ToolCall(ToolCallEvent),
    /// Reasoning/thinking content.
    Reasoning(String),
    /// Completion finished.
    Done(CompletionResponse),
    /// Error occurred.
    Error(String),
}

/// Tool call event.
#[derive(Debug, Clone)]
pub struct ToolCallEvent {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

/// Full completion response.
#[derive(Debug, Clone, Default)]
pub struct CompletionResponse {
    /// Generated message.
    pub message: Option<Message>,
    /// Token usage.
    pub usage: TokenUsage,
    /// Finish reason.
    pub finish_reason: FinishReason,
    /// Tool calls made.
    pub tool_calls: Vec<ToolCall>,
}

/// Token usage statistics.
#[derive(Debug, Clone, Default)]
pub struct TokenUsage {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub total_tokens: i64,
}

impl TokenUsage {
    /// Create from prompt/completion style tokens.
    pub fn from_prompt_completion(prompt: u32, completion: u32) -> Self {
        Self {
            input_tokens: prompt as i64,
            output_tokens: completion as i64,
            total_tokens: (prompt + completion) as i64,
        }
    }
}

/// Reason for completion finishing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum FinishReason {
    #[default]
    Stop,
    Length,
    ToolCalls,
    ContentFilter,
    Error,
}

/// Model capabilities.
#[derive(Debug, Clone, Default)]
pub struct ModelCapabilities {
    /// Whether the model supports vision (images).
    pub vision: bool,
    /// Whether the model supports tool/function calling.
    pub tools: bool,
    /// Whether the model supports reasoning/thinking.
    pub reasoning: bool,
    /// Context window size.
    pub context_window: u32,
    /// Maximum output tokens.
    pub max_output_tokens: Option<u32>,
}
