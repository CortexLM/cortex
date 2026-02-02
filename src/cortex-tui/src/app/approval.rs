use super::types::ApprovalMode;

/// State for pending tool approval
#[derive(Debug, Clone, Default)]
pub struct ApprovalState {
    /// Unique ID for this tool call (from the LLM)
    pub tool_call_id: String,
    pub tool_name: String,
    pub tool_args: String,
    /// Parsed arguments for execution
    pub tool_args_json: Option<serde_json::Value>,
    pub diff_preview: Option<String>,
    pub approval_mode: ApprovalMode,
}

impl ApprovalState {
    /// Creates a new ApprovalState with the given tool name and arguments.
    pub fn new(tool_name: String, tool_args: serde_json::Value) -> Self {
        Self {
            tool_call_id: String::new(),
            tool_name,
            tool_args: serde_json::to_string_pretty(&tool_args).unwrap_or_default(),
            tool_args_json: Some(tool_args),
            diff_preview: None,
            approval_mode: ApprovalMode::default(),
        }
    }

    /// Creates an ApprovalState with a specific tool call ID.
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.tool_call_id = id.into();
        self
    }

    /// Adds a diff preview to the approval state.
    pub fn with_diff(mut self, diff: String) -> Self {
        self.diff_preview = Some(diff);
        self
    }
}

/// State for a pending tool execution waiting for continuation
#[derive(Debug, Clone)]
pub struct PendingToolResult {
    /// Tool call ID from the LLM
    pub tool_call_id: String,
    /// Tool name
    pub tool_name: String,
    /// Tool output/result
    pub output: String,
    /// Whether the tool succeeded
    pub success: bool,
}
