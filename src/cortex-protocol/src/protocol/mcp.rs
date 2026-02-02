//! MCP (Model Context Protocol) related types.

use std::collections::HashMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// MCP tool invocation details.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, PartialEq)]
pub struct McpInvocation {
    pub server: String,
    pub tool: String,
    pub arguments: Option<serde_json::Value>,
}

/// Event when an MCP tool call begins.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, PartialEq)]
pub struct McpToolCallBeginEvent {
    pub call_id: String,
    pub invocation: McpInvocation,
}

/// Event when an MCP tool call ends.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, PartialEq)]
pub struct McpToolCallEndEvent {
    pub call_id: String,
    pub invocation: McpInvocation,
    /// Duration in milliseconds.
    pub duration_ms: u64,
    pub result: Result<McpToolResult, String>,
}

/// Result from an MCP tool call.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, PartialEq)]
pub struct McpToolResult {
    pub content: Vec<McpContent>,
    #[serde(default)]
    pub is_error: Option<bool>,
}

/// MCP content types.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum McpContent {
    Text { text: String },
    Image { data: String, mime_type: String },
    Resource { uri: String, text: Option<String> },
}

/// Event for MCP server startup status.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct McpStartupUpdateEvent {
    pub server: String,
    pub status: McpStartupStatus,
}

/// MCP server startup status.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum McpStartupStatus {
    Starting,
    Ready,
    Failed { error: String },
    Cancelled,
}

/// Event when all MCP servers have completed startup.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, Default)]
pub struct McpStartupCompleteEvent {
    pub ready: Vec<String>,
    pub failed: Vec<McpStartupFailure>,
    pub cancelled: Vec<String>,
}

/// MCP server startup failure details.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct McpStartupFailure {
    pub server: String,
    pub error: String,
}

/// Response to ListMcpTools operation.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct McpListToolsResponseEvent {
    pub tools: HashMap<String, McpToolDefinition>,
    pub resources: HashMap<String, Vec<McpResource>>,
    pub resource_templates: HashMap<String, Vec<McpResourceTemplate>>,
    pub auth_statuses: HashMap<String, McpAuthStatus>,
}

/// MCP tool definition.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct McpToolDefinition {
    pub name: String,
    pub description: Option<String>,
    pub input_schema: serde_json::Value,
}

/// MCP resource definition.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct McpResource {
    pub uri: String,
    pub name: String,
    pub description: Option<String>,
    pub mime_type: Option<String>,
}

/// MCP resource template.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct McpResourceTemplate {
    pub uri_template: String,
    pub name: String,
    pub description: Option<String>,
    pub mime_type: Option<String>,
}

/// MCP authentication status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum McpAuthStatus {
    Unsupported,
    NotLoggedIn,
    BearerToken,
    OAuth,
}
