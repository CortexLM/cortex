//! Model response types.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Items that can appear in a model response.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseItem {
    /// A message from the model or user.
    Message {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        parent_id: Option<String>,
        role: String,
        content: Vec<ContentItem>,
    },

    /// A function/tool call from the model.
    FunctionCall {
        id: String,
        call_id: String,
        name: String,
        arguments: String,
    },

    /// Output from a function/tool call.
    FunctionCallOutput {
        id: String,
        call_id: String,
        output: String,
    },

    /// Reasoning output from the model.
    Reasoning {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        content: Vec<ReasoningContent>,
    },

    /// File citation.
    FileCitation {
        file_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        quote: Option<String>,
    },

    /// Web search result.
    WebSearchResult {
        id: String,
        url: String,
        title: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        snippet: Option<String>,
    },
}

/// Content items within a message.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentItem {
    /// Text input from user.
    InputText { text: String },

    /// Text output from model.
    OutputText { text: String },

    /// Image input.
    InputImage {
        image_url: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },

    /// Base64 encoded image input.
    InputImageBase64 {
        data: String,
        media_type: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },

    /// Model refusal.
    Refusal { refusal: String },

    /// Tool use request.
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },

    /// Tool result.
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(default)]
        is_error: bool,
    },
}

/// Reasoning content from the model.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ReasoningContent {
    /// Summary of reasoning.
    Summary { text: String },

    /// Raw thinking content.
    Thinking { text: String },
}

/// Local shell action for tool calls.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LocalShellAction {
    /// Execute a command.
    Exec(LocalShellExecAction),

    /// Read a file.
    ReadFile { path: String },

    /// Write a file.
    WriteFile { path: String, content: String },

    /// List directory.
    ListDir { path: String },
}

/// Shell execution action.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LocalShellExecAction {
    pub command: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workdir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

/// Status of a local shell operation.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LocalShellStatus {
    /// Command completed successfully.
    Success {
        exit_code: i32,
        stdout: String,
        stderr: String,
    },

    /// Command failed.
    Error {
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        exit_code: Option<i32>,
    },

    /// Command timed out.
    Timeout { stdout: String, stderr: String },

    /// Waiting for approval.
    PendingApproval,

    /// Denied by user.
    Denied,
}

impl ContentItem {
    /// Create a text input content item.
    pub fn input_text(text: impl Into<String>) -> Self {
        Self::InputText { text: text.into() }
    }

    /// Create a text output content item.
    pub fn output_text(text: impl Into<String>) -> Self {
        Self::OutputText { text: text.into() }
    }

    /// Extract text content if this is a text item.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::InputText { text } | Self::OutputText { text } => Some(text),
            _ => None,
        }
    }
}

impl ResponseItem {
    /// Check if this is an assistant message.
    pub fn is_assistant_message(&self) -> bool {
        matches!(self, Self::Message { role, .. } if role == "assistant")
    }

    /// Check if this is a user message.
    pub fn is_user_message(&self) -> bool {
        matches!(self, Self::Message { role, .. } if role == "user")
    }

    /// Check if this is a function call.
    pub fn is_function_call(&self) -> bool {
        matches!(self, Self::FunctionCall { .. })
    }

    /// Get the text content of a message.
    pub fn get_text_content(&self) -> Option<String> {
        match self {
            Self::Message { content, .. } => {
                let texts: Vec<&str> = content.iter().filter_map(|c| c.as_text()).collect();
                if texts.is_empty() {
                    None
                } else {
                    Some(texts.join(""))
                }
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_response_item_message() {
        let item = ResponseItem::Message {
            id: Some("msg_1".to_string()),
            parent_id: None,
            role: "assistant".to_string(),
            content: vec![ContentItem::output_text("Hello!")],
        };

        assert!(item.is_assistant_message());
        assert!(!item.is_user_message());
        assert_eq!(item.get_text_content(), Some("Hello!".to_string()));
    }

    #[test]
    fn test_content_item_serde() {
        let item = ContentItem::InputImage {
            image_url: "https://example.com/image.png".to_string(),
            detail: Some("high".to_string()),
        };

        let json = serde_json::to_string(&item).expect("serialize");
        assert!(json.contains("input_image"));
        assert!(json.contains("image_url"));
    }
}
