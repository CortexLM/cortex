//! API request and response types.

use serde::{Deserialize, Serialize};

// ============================================================================
// Health and Metrics
// ============================================================================

/// Health check response.
#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
    pub uptime_seconds: u64,
}

// ============================================================================
// mDNS Discovery
// ============================================================================

/// Query parameters for service discovery.
#[derive(Debug, Deserialize)]
pub struct DiscoverQuery {
    /// Timeout in seconds for discovery (default: 3 seconds, max: 30 seconds).
    #[serde(default = "default_discover_timeout")]
    pub timeout: u64,
}

fn default_discover_timeout() -> u64 {
    3
}

/// Discovery response.
#[derive(Debug, Serialize)]
pub struct DiscoverResponse {
    /// List of discovered Cortex servers on the network.
    pub servers: Vec<crate::mdns::DiscoveredServer>,
    /// Number of servers found.
    pub count: usize,
    /// Time taken for discovery in milliseconds.
    pub duration_ms: u64,
}

// ============================================================================
// Sessions
// ============================================================================

/// Create session request.
#[derive(Debug, Deserialize)]
pub struct CreateSessionRequest {
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub system_prompt: Option<String>,
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
}

/// Session response.
#[derive(Debug, Serialize)]
pub struct SessionResponse {
    pub id: String,
    pub model: String,
    pub status: String,
    pub message_count: usize,
    pub total_tokens: u64,
    pub system_prompt: Option<String>,
    pub metadata: Option<serde_json::Value>,
}

/// List sessions query parameters.
#[derive(Debug, Deserialize)]
pub struct ListSessionsQuery {
    #[serde(default = "default_limit")]
    pub limit: usize,
    #[serde(default)]
    pub offset: usize,
}

fn default_limit() -> usize {
    20
}

/// Session list item.
#[derive(Debug, Serialize)]
pub struct SessionListItem {
    pub id: String,
    pub model: String,
    pub status: String,
    pub message_count: usize,
    pub total_tokens: u64,
}

// ============================================================================
// Messages
// ============================================================================

/// Send message request.
#[derive(Debug, Deserialize)]
pub struct SendMessageRequest {
    pub content: String,
    #[serde(default)]
    pub role: Option<String>,
}

/// Message response.
#[derive(Debug, Serialize)]
pub struct MessageResponse {
    pub id: String,
    pub role: String,
    pub content: String,
    pub tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCallResponse>>,
}

/// Tool call response.
#[derive(Debug, Serialize)]
pub struct ToolCallResponse {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
}

// ============================================================================
// Models
// ============================================================================

/// Model information.
#[derive(Debug, Serialize)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub provider: String,
    pub context_window: u32,
    pub max_output_tokens: Option<u32>,
    pub supports_vision: bool,
    pub supports_tools: bool,
    pub supports_streaming: bool,
}

// ============================================================================
// Chat Completions (OpenAI-compatible)
// ============================================================================

/// Chat completion request (OpenAI-compatible).
#[derive(Debug, Deserialize)]
pub struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub stream: bool,
    #[serde(default)]
    pub tools: Vec<ChatToolDefinition>,
}

/// Chat message.
#[derive(Debug, Deserialize, Serialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<serde_json::Value>>,
}

/// OpenAI-compatible tool definition for chat completions.
#[derive(Debug, Deserialize)]
pub struct ChatToolDefinition {
    pub r#type: String,
    pub function: FunctionDefinition,
}

/// Function definition.
#[derive(Debug, Deserialize)]
pub struct FunctionDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// Chat completion response.
#[derive(Debug, Serialize)]
pub struct ChatCompletionResponse {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub choices: Vec<ChatChoice>,
    pub usage: ChatUsage,
}

/// Chat choice.
#[derive(Debug, Serialize)]
pub struct ChatChoice {
    pub index: u32,
    pub message: ChatMessage,
    pub finish_reason: String,
}

/// Token usage.
#[derive(Debug, Serialize)]
pub struct ChatUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

// ============================================================================
// Tools
// ============================================================================

/// Execute tool request.
#[derive(Debug, Deserialize)]
pub struct ExecuteToolRequest {
    pub arguments: serde_json::Value,
}

/// Execute tool response.
#[derive(Debug, Serialize)]
pub struct ExecuteToolResponse {
    pub success: bool,
    pub output: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

// ============================================================================
// File Explorer
// ============================================================================

/// List files request.
#[derive(Debug, Deserialize)]
pub struct ListFilesRequest {
    #[serde(default = "default_path")]
    pub path: String,
}

fn default_path() -> String {
    "/workspace".to_string()
}

/// File entry.
#[derive(Debug, Serialize)]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    #[serde(rename = "type")]
    pub file_type: String,
    pub size: u64,
    pub modified: Option<String>,
}

/// List files response.
#[derive(Debug, Serialize)]
pub struct ListFilesResponse {
    pub path: String,
    pub entries: Vec<FileEntry>,
}

/// File tree query params.
#[derive(Debug, Deserialize)]
pub struct FileTreeQuery {
    pub path: String,
    #[serde(default = "default_depth")]
    pub depth: usize,
}

fn default_depth() -> usize {
    3
}

/// File tree node.
#[derive(Debug, Serialize)]
pub struct FileTreeNode {
    pub name: String,
    pub path: String,
    #[serde(rename = "isDir")]
    pub is_dir: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub children: Option<Vec<FileTreeNode>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
}

/// Read file request.
#[derive(Debug, Deserialize)]
pub struct ReadFileRequest {
    pub path: String,
}

/// Read file response.
#[derive(Debug, Serialize)]
pub struct ReadFileResponse {
    pub path: String,
    pub content: String,
    pub size: u64,
}

/// Write file request.
#[derive(Debug, Deserialize)]
pub struct WriteFileRequest {
    pub path: String,
    pub content: String,
}

/// Write file response.
#[derive(Debug, Serialize)]
pub struct WriteFileResponse {
    pub path: String,
    pub size: u64,
    pub success: bool,
}

/// Delete file request.
#[derive(Debug, Deserialize)]
pub struct DeleteFileRequest {
    pub path: String,
}

/// Delete file response.
#[derive(Debug, Serialize)]
pub struct DeleteFileResponse {
    pub path: String,
    pub success: bool,
}

/// Create directory request.
#[derive(Debug, Deserialize)]
pub struct CreateDirRequest {
    pub path: String,
}

/// Rename request.
#[derive(Debug, Deserialize)]
pub struct RenameRequest {
    pub old_path: String,
    pub new_path: String,
}

// ============================================================================
// Agents
// ============================================================================

/// Agent definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDefinition {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default = "default_permission_mode")]
    pub permission_mode: String,
    pub prompt: String,
    #[serde(default)]
    pub scope: String,
}

fn default_model() -> String {
    "inherit".to_string()
}

fn default_permission_mode() -> String {
    "default".to_string()
}

/// Create agent request.
#[derive(Debug, Deserialize)]
pub struct CreateAgentRequest {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default = "default_permission_mode")]
    pub permission_mode: String,
    pub prompt: String,
    #[serde(default)]
    pub scope: String,
}

/// Update agent request.
#[derive(Debug, Deserialize)]
pub struct UpdateAgentRequest {
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub tools: Option<Vec<String>>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub permission_mode: Option<String>,
    #[serde(default)]
    pub prompt: Option<String>,
}

/// Import agent request.
#[derive(Debug, Deserialize)]
pub struct ImportAgentRequest {
    /// Content of the agent file (markdown).
    pub content: String,
    /// Format of the content.
    #[serde(default = "default_format")]
    pub format: String,
    /// Where to save (project or user).
    #[serde(default = "default_scope")]
    pub scope: String,
}

fn default_format() -> String {
    "auto".to_string()
}

fn default_scope() -> String {
    "user".to_string()
}

/// Generate prompt request.
#[derive(Debug, Deserialize)]
pub struct GeneratePromptRequest {
    pub description: String,
    #[serde(default)]
    pub tools: Vec<String>,
    pub name: Option<String>,
}

/// Generate prompt response.
#[derive(Debug, Serialize)]
pub struct GeneratePromptResponse {
    pub name: String,
    pub description: String,
    pub prompt: String,
    pub tools: Vec<String>,
    pub model: String,
    pub permission_mode: String,
}

// ============================================================================
// Terminals
// ============================================================================

/// Terminal info response.
#[derive(Debug, Serialize)]
pub struct TerminalResponse {
    pub id: String,
    pub name: String,
    pub cwd: String,
    pub status: String,
    pub created_at: u64,
    pub exit_code: Option<i32>,
}

/// Terminal log entry.
#[derive(Debug, Serialize)]
pub struct TerminalLogEntry {
    pub timestamp: u64,
    pub content: String,
    pub stream: String,
}

/// Query params for terminal logs.
#[derive(Debug, Deserialize)]
pub struct TerminalLogsQuery {
    #[serde(default = "default_tail")]
    pub tail: usize,
}

fn default_tail() -> usize {
    1000
}

// ============================================================================
// Search
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    pub path: String,
    pub query: String,
    #[serde(default)]
    pub case_sensitive: bool,
    #[serde(default)]
    pub regex: bool,
    #[serde(default)]
    pub whole_word: bool,
    pub include: Option<String>,
    pub exclude: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SearchMatch {
    pub line: usize,
    pub column: usize,
    pub text: String,
    pub match_start: usize,
    pub match_end: usize,
}

#[derive(Debug, Serialize)]
pub struct SearchResult {
    pub file: String,
    pub matches: Vec<SearchMatch>,
}

#[derive(Debug, Serialize)]
pub struct SearchResponse {
    pub results: Vec<SearchResult>,
}

// ============================================================================
// Git
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct GitPathQuery {
    pub path: String,
}

#[derive(Debug, Serialize)]
pub struct GitStatusFile {
    pub path: String,
    pub status: String,
    pub staged: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conflict_type: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct GitStatusResponse {
    pub branch: Option<String>,
    pub staged: Vec<GitStatusFile>,
    pub unstaged: Vec<GitStatusFile>,
    pub conflicts: Vec<GitStatusFile>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ahead: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub behind: Option<u32>,
}

#[derive(Debug, Serialize)]
pub struct GitBranch {
    pub name: String,
    pub current: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ahead: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub behind: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_commit: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct GitDiffQuery {
    pub path: String,
    pub file: String,
    #[serde(default)]
    pub staged: bool,
}

#[derive(Debug, Deserialize)]
pub struct GitBlameQuery {
    pub path: String,
    pub file: String,
}

#[derive(Debug, Deserialize)]
pub struct GitLogQuery {
    pub path: String,
    #[serde(default = "default_log_limit")]
    pub limit: usize,
}

fn default_log_limit() -> usize {
    100
}

#[derive(Debug, Deserialize)]
pub struct GitStageRequest {
    pub path: String,
    pub files: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct GitPathRequest {
    pub path: String,
}

#[derive(Debug, Deserialize)]
pub struct GitCommitRequest {
    pub path: String,
    pub message: String,
}

#[derive(Debug, Deserialize)]
pub struct GitCheckoutRequest {
    pub path: String,
    pub branch: String,
}

#[derive(Debug, Deserialize)]
pub struct GitCreateBranchRequest {
    pub path: String,
    pub name: String,
    #[serde(default)]
    pub start_point: Option<String>,
    #[serde(default)]
    pub checkout: bool,
}

#[derive(Debug, Deserialize)]
pub struct GitDeleteBranchRequest {
    pub path: String,
    pub name: String,
    #[serde(default)]
    pub force: bool,
}

#[derive(Debug, Deserialize)]
pub struct GitMergeRequest {
    pub path: String,
    pub branch: String,
    #[serde(default)]
    pub no_ff: bool,
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct GitStashCreateRequest {
    pub path: String,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default = "default_true")]
    pub include_untracked: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize)]
pub struct GitStashIndexRequest {
    pub path: String,
    #[serde(default)]
    pub index: usize,
}

// ============================================================================
// AI
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct AiInlineRequest {
    pub prompt: String,
    pub code: String,
    pub action: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AiPredictRequest {
    pub content: String,
    pub language: String,
    pub cursor: serde_json::Value,
    pub file_path: Option<String>,
}
