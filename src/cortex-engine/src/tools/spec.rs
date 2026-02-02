//! Tool specifications and types.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::Result;

/// A tool call from the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    /// Unique ID for this call.
    pub id: String,
    /// Tool name.
    pub name: String,
    /// Arguments as JSON value.
    pub arguments: Value,
}

/// Tool definition for the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// Tool name.
    pub name: String,
    /// Tool description.
    pub description: String,
    /// JSON Schema for parameters.
    pub parameters: Value,
}

impl ToolDefinition {
    /// Create a new tool definition.
    pub fn new(name: impl Into<String>, description: impl Into<String>, parameters: Value) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            parameters,
        }
    }
}

/// Result of a tool execution.
#[derive(Debug, Clone)]
pub struct ToolResult {
    /// Output content.
    pub output: String,
    /// Whether execution was successful.
    pub success: bool,
    /// Error message if failed.
    pub error: Option<String>,
    /// Additional metadata.
    pub metadata: Option<ToolMetadata>,
}

impl ToolResult {
    /// Create a successful result.
    pub fn success(output: impl Into<String>) -> Self {
        Self {
            output: output.into(),
            success: true,
            error: None,
            metadata: None,
        }
    }

    /// Create an error result.
    pub fn error(message: impl Into<String>) -> Self {
        let msg = message.into();
        Self {
            output: msg.clone(),
            success: false,
            error: Some(msg),
            metadata: None,
        }
    }

    /// Add metadata to the result.
    pub fn with_metadata(mut self, metadata: ToolMetadata) -> Self {
        self.metadata = Some(metadata);
        self
    }

    /// Get output reference (for backwards compat).
    pub fn content(&self) -> &str {
        &self.output
    }

    /// Check if error (for backwards compat).
    pub fn is_error(&self) -> bool {
        !self.success
    }
}

/// Metadata for tool execution.
#[derive(Debug, Clone)]
pub struct ToolMetadata {
    /// Duration in milliseconds.
    pub duration_ms: u64,
    /// Exit code (for shell commands).
    pub exit_code: Option<i32>,
    /// Files modified.
    pub files_modified: Vec<String>,
    /// Structured JSON data (file info, entries, etc.)
    pub data: Option<serde_json::Value>,
}

/// Trait for tool handlers.
#[async_trait]
pub trait ToolHandler: Send + Sync {
    /// Get the tool name.
    fn name(&self) -> &str;

    /// Execute the tool.
    async fn execute(
        &self,
        arguments: Value,
        context: &super::context::ToolContext,
    ) -> Result<ToolResult>;
}

/// Standard tool names.
pub mod tools {
    pub const LOCAL_SHELL: &str = "local_shell";
    pub const APPLY_PATCH: &str = "apply_patch";
    pub const READ_FILE: &str = "read_file";
    pub const LIST_DIR: &str = "list_dir";
    pub const WRITE_FILE: &str = "write_file";
    pub const SEARCH_FILES: &str = "search_files";
    pub const WEB_SEARCH: &str = "web_search";
    pub const VIEW_IMAGE: &str = "view_image";
    pub const EDIT_FILE: &str = "edit_file";
    pub const GREP: &str = "grep";
    pub const GLOB: &str = "glob";
    pub const FETCH_URL: &str = "fetch_url";
    pub const TODO_WRITE: &str = "todo_write";
    pub const TODO_READ: &str = "todo_read";
}
