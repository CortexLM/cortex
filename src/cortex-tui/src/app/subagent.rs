use std::time::{Duration, Instant};

/// A simple todo item for display in the subagent task view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentTodoItem {
    /// Todo item content/description.
    pub content: String,
    /// Status: pending, in_progress, or completed.
    pub status: SubagentTodoStatus,
}

/// Status of a subagent todo item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubagentTodoStatus {
    /// Not started yet.
    Pending,
    /// Currently being worked on.
    InProgress,
    /// Completed.
    Completed,
}

impl SubagentTodoItem {
    /// Create a new todo item.
    pub fn new(content: impl Into<String>, status: SubagentTodoStatus) -> Self {
        Self {
            content: content.into(),
            status,
        }
    }
}

/// Display state for an active subagent task.
#[derive(Debug, Clone)]
pub struct SubagentTaskDisplay {
    /// Subagent session ID.
    pub session_id: String,
    /// Original tool call ID (for matching response).
    pub tool_call_id: String,
    /// Task description.
    pub description: String,
    /// Subagent type (code, research, etc.).
    pub agent_type: String,
    /// Current status.
    pub status: SubagentDisplayStatus,
    /// Spinner frame (0-3 for animation).
    pub spinner_frame: usize,
    /// Current activity description.
    pub current_activity: String,
    /// Tool calls made by this subagent: (name, success).
    pub tool_calls: Vec<(String, bool)>,
    /// Last output preview (first 200 chars).
    pub output_preview: String,
    /// Start time.
    pub started_at: Instant,
    /// Current todo items from the subagent (if any).
    pub todos: Vec<SubagentTodoItem>,
    /// Error message if the subagent failed.
    /// Stored separately so it can be displayed even when the task is removed.
    pub error_message: Option<String>,
}

impl SubagentTaskDisplay {
    /// Create a new subagent task display.
    pub fn new(
        session_id: impl Into<String>,
        tool_call_id: impl Into<String>,
        description: impl Into<String>,
        agent_type: impl Into<String>,
    ) -> Self {
        Self {
            session_id: session_id.into(),
            tool_call_id: tool_call_id.into(),
            description: description.into(),
            agent_type: agent_type.into(),
            status: SubagentDisplayStatus::Starting,
            spinner_frame: 0,
            current_activity: "Initializing...".to_string(),
            tool_calls: Vec::new(),
            output_preview: String::new(),
            started_at: Instant::now(),
            todos: Vec::new(),
            error_message: None,
        }
    }

    /// Get elapsed time since start.
    pub fn elapsed(&self) -> Duration {
        self.started_at.elapsed()
    }
}

/// Status of a subagent task for display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubagentDisplayStatus {
    /// Subagent is initializing.
    Starting,
    /// Subagent is thinking/generating.
    Thinking,
    /// Subagent is executing a tool.
    ExecutingTool(String),
    /// Subagent completed successfully.
    Completed,
    /// Subagent failed.
    Failed,
}

impl SubagentDisplayStatus {
    /// Get a short description of the status.
    pub fn description(&self) -> String {
        match self {
            Self::Starting => "Processing request...".to_string(),
            Self::Thinking => "Thinking...".to_string(),
            Self::ExecutingTool(name) => format!("Running {}", name),
            Self::Completed => "Completed".to_string(),
            Self::Failed => "Failed".to_string(),
        }
    }

    /// Check if this is a terminal status.
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Completed | Self::Failed)
    }
}
