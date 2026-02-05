use super::types::ApprovalMode;

/// State for inline approval selection in input zone
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum InlineApprovalSelection {
    #[default]
    AcceptOnce, // 'y' - accept this time
    Reject,       // 'n' - reject
    AcceptAndSet, // 'a' - accept and set risk level
}

impl InlineApprovalSelection {
    /// Move selection to the next item
    pub fn next(self) -> Self {
        match self {
            Self::Reject => Self::AcceptOnce,
            Self::AcceptOnce => Self::AcceptAndSet,
            Self::AcceptAndSet => Self::Reject,
        }
    }

    /// Move selection to the previous item
    pub fn prev(self) -> Self {
        match self {
            Self::Reject => Self::AcceptAndSet,
            Self::AcceptOnce => Self::Reject,
            Self::AcceptAndSet => Self::AcceptOnce,
        }
    }
}

/// State for risk level submenu
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RiskLevelSelection {
    #[default]
    Low,
    Medium,
    High,
}

impl RiskLevelSelection {
    /// Move selection to the next item
    pub fn next(self) -> Self {
        match self {
            Self::Low => Self::Medium,
            Self::Medium => Self::High,
            Self::High => Self::Low,
        }
    }

    /// Move selection to the previous item
    pub fn prev(self) -> Self {
        match self {
            Self::Low => Self::High,
            Self::Medium => Self::Low,
            Self::High => Self::Medium,
        }
    }
}

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
    /// Currently selected action in inline approval UI
    pub selected_action: InlineApprovalSelection,
    /// Whether the risk level submenu is visible
    pub show_risk_submenu: bool,
    /// Selected risk level in submenu
    pub selected_risk_level: RiskLevelSelection,
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
            selected_action: InlineApprovalSelection::default(),
            show_risk_submenu: false,
            selected_risk_level: RiskLevelSelection::default(),
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
