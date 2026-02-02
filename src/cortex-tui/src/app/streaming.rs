use std::time::Instant;

/// State for streaming responses
#[derive(Debug, Clone, Default)]
pub struct StreamingState {
    pub is_streaming: bool,
    pub current_tool: Option<String>,
    pub tool_status: Option<String>,
    pub thinking: bool,
    /// When the current task started (for elapsed time display)
    pub task_started_at: Option<Instant>,
    /// Name of the tool currently executing in background (for visual indicator)
    pub executing_tool: Option<String>,
    /// When the tool started executing (for elapsed time display)
    pub tool_started_at: Option<Instant>,
    /// When the last user prompt was sent (for total elapsed time from user's perspective)
    /// This persists across streaming restarts (e.g., after tool execution)
    pub prompt_started_at: Option<Instant>,
    /// Whether a subagent (Task) is currently running
    pub is_delegating: bool,
}

impl StreamingState {
    pub fn start(&mut self, tool: Option<String>) {
        self.is_streaming = true;
        self.thinking = true;
        self.current_tool = tool;
        self.task_started_at = Some(Instant::now());
        // Only set prompt_started_at if not already set (first call in a turn)
        if self.prompt_started_at.is_none() {
            self.prompt_started_at = Some(Instant::now());
        }
    }

    /// Get the elapsed seconds since the task started
    pub fn elapsed_seconds(&self) -> u64 {
        self.task_started_at
            .map(|t| t.elapsed().as_secs())
            .unwrap_or(0)
    }

    /// Get the elapsed seconds since the original prompt was sent
    /// This is the total time from the user's perspective
    pub fn prompt_elapsed_seconds(&self) -> u64 {
        self.prompt_started_at
            .map(|t| t.elapsed().as_secs())
            .unwrap_or(0)
    }

    /// Reset streaming state when task completes
    pub fn stop(&mut self) {
        self.is_streaming = false;
        self.thinking = false;
        self.current_tool = None;
        self.tool_status = None;
        self.task_started_at = None;
        // Note: prompt_started_at is NOT reset here - it persists until full_reset()
    }

    /// Full reset when the entire conversation turn is complete
    /// (no more tool executions or continuations expected)
    pub fn full_reset(&mut self) {
        self.stop();
        self.prompt_started_at = None;
        self.is_delegating = false;
    }

    /// Start delegation mode (subagent is running)
    pub fn start_delegation(&mut self) {
        self.is_delegating = true;
    }

    /// Stop delegation mode
    pub fn stop_delegation(&mut self) {
        self.is_delegating = false;
    }

    /// Start tool execution in background
    pub fn start_tool_execution(&mut self, tool_name: String) {
        self.executing_tool = Some(tool_name);
        self.tool_started_at = Some(Instant::now());
    }

    /// Clear tool execution state
    pub fn stop_tool_execution(&mut self) {
        self.executing_tool = None;
        self.tool_started_at = None;
    }

    /// Check if a tool is currently executing
    pub fn is_tool_executing(&self) -> bool {
        self.executing_tool.is_some()
    }

    /// Get the elapsed seconds since tool started executing
    pub fn tool_elapsed_seconds(&self) -> u64 {
        self.tool_started_at
            .map(|t| t.elapsed().as_secs())
            .unwrap_or(0)
    }
}
