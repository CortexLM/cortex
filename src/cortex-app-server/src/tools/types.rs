//! Core types for tool execution.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Tool result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub success: bool,
    pub output: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

impl ToolResult {
    pub fn success(output: impl Into<String>) -> Self {
        Self {
            success: true,
            output: output.into(),
            error: None,
            metadata: None,
        }
    }

    pub fn error(error: impl Into<String>) -> Self {
        Self {
            success: false,
            output: String::new(),
            error: Some(error.into()),
            metadata: None,
        }
    }
}

/// Tool definition for API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: Value,
    pub category: String,
}
