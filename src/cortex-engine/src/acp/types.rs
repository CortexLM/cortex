use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

/// ACP Protocol Version.
pub const PROTOCOL_VERSION: i32 = 1;

/// ACP Initialize Request.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeRequest {
    pub protocol_version: i32,
    #[serde(default)]
    pub client_capabilities: ClientCapabilities,
    #[serde(default)]
    pub client_info: ClientInfo,
}

/// ACP Initialize Response.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResponse {
    pub protocol_version: i32,
    pub agent_capabilities: AgentCapabilities,
    pub agent_info: AgentInfo,
    #[serde(default)]
    pub auth_methods: Vec<AuthMethod>,
}

/// Client Capabilities.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientCapabilities {
    pub fs: Option<FileSystemCapability>,
    pub terminal: Option<bool>,
    #[serde(rename = "_meta")]
    pub meta: Option<HashMap<String, Value>>,
}

/// File System Capability.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileSystemCapability {
    pub read_text_file: bool,
    pub write_text_file: bool,
}

/// Agent Capabilities.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentCapabilities {
    pub load_session: bool,
    pub mcp_capabilities: Option<McpCapabilities>,
    pub prompt_capabilities: PromptCapabilities,
}

/// MCP Capabilities.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpCapabilities {
    pub http: bool,
    pub sse: bool,
}

/// Prompt Capabilities.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptCapabilities {
    pub embedded_context: bool,
    pub image: bool,
}

/// Client Info.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientInfo {
    pub name: String,
    pub version: String,
}

/// Agent Info.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentInfo {
    pub name: String,
    pub version: String,
}

/// Auth Method.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthMethod {
    pub id: String,
    pub name: String,
    pub description: String,
    #[serde(rename = "_meta")]
    pub meta: Option<HashMap<String, Value>>,
}

/// New Session Request.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewSessionRequest {
    pub cwd: String,
    #[serde(default)]
    pub mcp_servers: Vec<McpServer>,
}

/// MCP Server.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum McpServer {
    Stdio(McpServerStdio),
    Http(McpServerHttp),
}

/// Stdio MCP Server.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerStdio {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: Vec<EnvVariable>,
}

/// HTTP MCP Server.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerHttp {
    pub name: String,
    pub url: String,
    pub headers: Vec<HttpHeader>,
}

/// Environment Variable.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvVariable {
    pub name: String,
    pub value: String,
}

/// HTTP Header.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpHeader {
    pub name: String,
    pub value: String,
}

/// New Session Response.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewSessionResponse {
    pub session_id: String,
    pub models: Option<SessionModels>,
    pub modes: Option<SessionModes>,
}

/// Session Models.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionModels {
    pub current_model_id: String,
    pub available_models: Vec<ModelInfo>,
}

/// Model Info.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub model_id: String,
    pub name: String,
}

/// Session Modes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionModes {
    pub current_mode_id: String,
    pub available_modes: Vec<ModeInfo>,
}

/// Mode Info.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModeInfo {
    pub id: String,
    pub name: String,
    pub description: String,
}

/// Prompt Request.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptRequest {
    pub session_id: String,
    pub prompt: Vec<PromptContent>,
}

/// Prompt Content.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum PromptContent {
    Text {
        text: String,
    },
    Image {
        data: Option<String>,
        uri: Option<String>,
        mime_type: String,
    },
    Resource {
        resource: Resource,
    },
    ResourceLink {
        uri: String,
    },
}

/// Resource.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Resource {
    Text { text: String },
    // Add other resource types as needed
}

/// Prompt Response.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptResponse {
    pub stop_reason: StopReason,
}

/// Stop Reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    EndTurn,
    Cancelled,
}

/// Session Notification.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionNotification {
    pub session_id: String,
    pub update: SessionUpdate,
}

/// Session Update.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "sessionUpdate", rename_all = "snake_case")]
pub enum SessionUpdate {
    AgentMessageChunk {
        content: MessageContent,
    },
    AgentThoughtChunk {
        content: MessageContent,
    },
    ToolCall {
        tool_call_id: String,
        title: String,
        kind: ToolKind,
        status: ToolStatus,
        locations: Vec<Location>,
        raw_input: Value,
    },
    ToolCallUpdate {
        tool_call_id: String,
        status: ToolStatus,
        #[serde(skip_serializing_if = "Option::is_none")]
        content: Option<Vec<ToolCallContent>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        raw_output: Option<Value>,
    },
    AvailableCommandsUpdate {
        available_commands: Vec<CommandInfo>,
    },
}

/// Message Content.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum MessageContent {
    Text { text: String },
}

/// Tool Kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    Read,
    Edit,
    Search,
    Execute,
    Fetch,
    Other,
}

/// Tool Status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
}

/// Location.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Location {
    pub path: String,
}

/// Tool Call Content.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ToolCallContent {
    Content {
        content: MessageContent,
    },
    Diff {
        path: String,
        old_text: String,
        new_text: String,
    },
}

/// Command Info.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandInfo {
    pub name: String,
    pub description: String,
}
