//! Regular task execution.
//!
//! Handles standard task execution with tool calls and response processing.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::{TaskMeta, TaskResult, TaskType};

/// Regular task for standard execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegularTask {
    /// Task metadata.
    pub meta: TaskMeta,
    /// User input/prompt.
    pub input: String,
    /// Context from previous turns.
    pub context: Option<String>,
    /// Tools available for this task.
    pub available_tools: Vec<String>,
    /// Maximum tool calls allowed.
    pub max_tool_calls: u32,
    /// Timeout for the task.
    pub timeout_secs: u64,
    /// Whether to auto-approve safe commands.
    pub auto_approve: bool,
}

impl RegularTask {
    /// Create a new regular task.
    pub fn new(id: impl Into<String>, input: impl Into<String>) -> Self {
        Self {
            meta: TaskMeta::new(id, TaskType::Regular),
            input: input.into(),
            context: None,
            available_tools: Vec::new(),
            max_tool_calls: 10,
            timeout_secs: 300,
            auto_approve: false,
        }
    }

    /// Set context.
    pub fn with_context(mut self, context: impl Into<String>) -> Self {
        self.context = Some(context.into());
        self
    }

    /// Set available tools.
    pub fn with_tools(mut self, tools: Vec<String>) -> Self {
        self.available_tools = tools;
        self
    }

    /// Set max tool calls.
    pub fn with_max_tool_calls(mut self, max: u32) -> Self {
        self.max_tool_calls = max;
        self
    }

    /// Set timeout.
    pub fn with_timeout(mut self, secs: u64) -> Self {
        self.timeout_secs = secs;
        self
    }

    /// Set auto-approve.
    pub fn with_auto_approve(mut self, auto: bool) -> Self {
        self.auto_approve = auto;
        self
    }

    /// Get the full prompt with context.
    pub fn full_prompt(&self) -> String {
        match &self.context {
            Some(ctx) => format!("{}\n\n{}", ctx, self.input),
            None => self.input.clone(),
        }
    }
}

/// Task execution state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskExecution {
    /// Task being executed.
    pub task: RegularTask,
    /// Current status.
    pub status: ExecutionStatus,
    /// Tool calls made.
    pub tool_calls: Vec<ExecutedToolCall>,
    /// Model responses.
    pub responses: Vec<ModelResponse>,
    /// Start time.
    pub started_at: Option<u64>,
    /// End time.
    pub ended_at: Option<u64>,
    /// Total tokens used.
    pub total_tokens: u64,
}

impl TaskExecution {
    /// Create a new execution.
    pub fn new(task: RegularTask) -> Self {
        Self {
            task,
            status: ExecutionStatus::Pending,
            tool_calls: Vec::new(),
            responses: Vec::new(),
            started_at: None,
            ended_at: None,
            total_tokens: 0,
        }
    }

    /// Start execution.
    pub fn start(&mut self) {
        self.status = ExecutionStatus::Running;
        self.started_at = Some(timestamp_now());
    }

    /// Add a model response.
    pub fn add_response(&mut self, response: ModelResponse) {
        self.total_tokens += response.tokens_used as u64;
        self.responses.push(response);
    }

    /// Add a tool call.
    pub fn add_tool_call(&mut self, call: ExecutedToolCall) {
        self.tool_calls.push(call);
    }

    /// Check if we can make more tool calls.
    pub fn can_call_tool(&self) -> bool {
        (self.tool_calls.len() as u32) < self.task.max_tool_calls
    }

    /// Complete execution.
    pub fn complete(&mut self, output: impl Into<String>) {
        self.status = ExecutionStatus::Completed(output.into());
        self.ended_at = Some(timestamp_now());
    }

    /// Fail execution.
    pub fn fail(&mut self, error: impl Into<String>) {
        self.status = ExecutionStatus::Failed(error.into());
        self.ended_at = Some(timestamp_now());
    }

    /// Check if waiting for approval.
    pub fn is_waiting_approval(&self) -> bool {
        matches!(self.status, ExecutionStatus::WaitingApproval(_))
    }

    /// Set waiting for approval.
    pub fn wait_for_approval(&mut self, tool_call: PendingToolCall) {
        self.status = ExecutionStatus::WaitingApproval(tool_call);
    }

    /// Get duration.
    pub fn duration(&self) -> Option<Duration> {
        match (self.started_at, self.ended_at) {
            (Some(start), Some(end)) if end >= start => Some(Duration::from_secs(end - start)),
            _ => None,
        }
    }

    /// Get result.
    pub fn result(&self) -> TaskResult {
        let mut meta = self.task.meta.clone();

        match &self.status {
            ExecutionStatus::Completed(output) => {
                meta.complete();
                TaskResult::success(
                    meta,
                    Some(serde_json::json!({
                        "output": output,
                        "tool_calls": self.tool_calls.len(),
                        "tokens": self.total_tokens,
                    })),
                )
            }
            ExecutionStatus::Failed(error) => TaskResult::failure(meta, error),
            _ => TaskResult::failure(meta, "Task not finished"),
        }
    }
}

/// Execution status.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum ExecutionStatus {
    /// Pending start.
    Pending,
    /// Currently running.
    Running,
    /// Waiting for user approval.
    WaitingApproval(PendingToolCall),
    /// Completed successfully.
    Completed(String),
    /// Failed with error.
    Failed(String),
    /// Cancelled.
    Cancelled,
}

/// Pending tool call awaiting approval.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingToolCall {
    /// Tool call ID.
    pub id: String,
    /// Tool name.
    pub name: String,
    /// Arguments.
    pub arguments: serde_json::Value,
    /// Risk level.
    pub risk: RiskLevel,
    /// Reason for requiring approval.
    pub reason: String,
}

impl PendingToolCall {
    /// Create a new pending tool call.
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        arguments: serde_json::Value,
        risk: RiskLevel,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            arguments,
            risk,
            reason: reason.into(),
        }
    }
}

/// Risk level for tool calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum RiskLevel {
    /// Safe operation.
    Safe,
    /// Low risk.
    #[default]
    Low,
    /// Medium risk.
    Medium,
    /// High risk.
    High,
    /// Critical risk.
    Critical,
}

/// Executed tool call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutedToolCall {
    /// Call ID.
    pub id: String,
    /// Tool name.
    pub name: String,
    /// Arguments.
    pub arguments: serde_json::Value,
    /// Result.
    pub result: ToolCallResult,
    /// Duration in milliseconds.
    pub duration_ms: u64,
    /// Whether it was auto-approved.
    pub auto_approved: bool,
}

impl ExecutedToolCall {
    /// Create a successful tool call.
    pub fn success(
        id: impl Into<String>,
        name: impl Into<String>,
        arguments: serde_json::Value,
        output: impl Into<String>,
        duration_ms: u64,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            arguments,
            result: ToolCallResult::Success(output.into()),
            duration_ms,
            auto_approved: false,
        }
    }

    /// Create a failed tool call.
    pub fn failure(
        id: impl Into<String>,
        name: impl Into<String>,
        arguments: serde_json::Value,
        error: impl Into<String>,
        duration_ms: u64,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            arguments,
            result: ToolCallResult::Error(error.into()),
            duration_ms,
            auto_approved: false,
        }
    }

    /// Mark as auto-approved.
    pub fn with_auto_approved(mut self, auto: bool) -> Self {
        self.auto_approved = auto;
        self
    }

    /// Check if successful.
    pub fn is_success(&self) -> bool {
        matches!(self.result, ToolCallResult::Success(_))
    }

    /// Get output if successful.
    pub fn output(&self) -> Option<&str> {
        match &self.result {
            ToolCallResult::Success(output) => Some(output),
            _ => None,
        }
    }

    /// Get error if failed.
    pub fn error(&self) -> Option<&str> {
        match &self.result {
            ToolCallResult::Error(err) => Some(err),
            _ => None,
        }
    }
}

/// Tool call result.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum ToolCallResult {
    /// Successful execution.
    Success(String),
    /// Error during execution.
    Error(String),
    /// Call was denied.
    Denied(String),
    /// Call timed out.
    Timeout,
}

/// Model response in a turn.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelResponse {
    /// Response ID.
    pub id: String,
    /// Text content.
    pub content: String,
    /// Tool calls requested.
    pub tool_calls: Vec<RequestedToolCall>,
    /// Finish reason.
    pub finish_reason: FinishReason,
    /// Tokens used.
    pub tokens_used: u32,
    /// Model used.
    pub model: String,
}

/// Requested tool call from model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestedToolCall {
    /// Call ID.
    pub id: String,
    /// Tool name.
    pub name: String,
    /// Arguments as JSON string.
    pub arguments: String,
}

/// Finish reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum FinishReason {
    /// Normal stop.
    #[default]
    Stop,
    /// Length limit.
    Length,
    /// Tool use.
    ToolUse,
    /// Content filter.
    ContentFilter,
}

/// Get current timestamp.
fn timestamp_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_regular_task() {
        let task = RegularTask::new("task-1", "Hello")
            .with_max_tool_calls(5)
            .with_timeout(60);

        assert_eq!(task.meta.id, "task-1");
        assert_eq!(task.input, "Hello");
        assert_eq!(task.max_tool_calls, 5);
        assert_eq!(task.timeout_secs, 60);
    }

    #[test]
    fn test_task_execution() {
        let task = RegularTask::new("task-1", "Test");
        let mut exec = TaskExecution::new(task);

        exec.start();
        assert!(matches!(exec.status, ExecutionStatus::Running));

        exec.complete("Done");
        assert!(matches!(exec.status, ExecutionStatus::Completed(_)));
    }

    #[test]
    fn test_executed_tool_call() {
        let call = ExecutedToolCall::success(
            "call-1",
            "read_file",
            serde_json::json!({"path": "/test"}),
            "content",
            100,
        );

        assert!(call.is_success());
        assert_eq!(call.output(), Some("content"));
    }
}
