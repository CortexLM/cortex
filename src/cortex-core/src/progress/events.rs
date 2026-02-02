//! Progress event system for real-time task tracking.
//!
//! This module provides a comprehensive event system for tracking the progress
//! of agent tasks, tool calls, and todo list updates. Events are emitted during
//! task execution and can be consumed by UI components for live progress display.
//!
//! ## Event Types
//!
//! - [`TaskStarted`](ProgressEvent::TaskStarted) - When a new task begins
//! - [`ToolCallStarted`](ProgressEvent::ToolCallStarted) - When a tool call starts
//! - [`ToolCallCompleted`](ProgressEvent::ToolCallCompleted) - When a tool call finishes
//! - [`TodoUpdated`](ProgressEvent::TodoUpdated) - When the todo list changes
//! - [`TokenGenerated`](ProgressEvent::TokenGenerated) - When streaming tokens arrive
//! - [`ThinkingStarted`](ProgressEvent::ThinkingStarted) - When AI starts thinking
//! - [`TaskCompleted`](ProgressEvent::TaskCompleted) - When a task finishes
//! - [`TaskError`](ProgressEvent::TaskError) - When a task encounters an error
//!
//! ## Example
//!
//! ```ignore
//! use cortex_core::progress::{ProgressEmitter, ProgressEvent, ProgressSubscriber};
//!
//! // Create emitter and subscriber
//! let (emitter, mut subscriber) = ProgressEmitter::new();
//!
//! // Emit events from agent loop
//! emitter.emit(ProgressEvent::TaskStarted {
//!     task_id: "task-1".to_string(),
//!     description: "Processing files...".to_string(),
//! });
//!
//! // Receive events in UI
//! if let Some(event) = subscriber.try_recv() {
//!     println!("Got event: {:?}", event);
//! }
//! ```

use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio::sync::broadcast;

/// Result of a tool execution.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolResult {
    /// Whether the tool execution was successful.
    pub success: bool,
    /// Output from the tool if successful.
    pub output: Option<String>,
    /// Error message if the tool failed.
    pub error: Option<String>,
}

impl ToolResult {
    /// Creates a successful tool result.
    pub fn success(output: impl Into<String>) -> Self {
        Self {
            success: true,
            output: Some(output.into()),
            error: None,
        }
    }

    /// Creates a failed tool result.
    pub fn failure(error: impl Into<String>) -> Self {
        Self {
            success: false,
            output: None,
            error: Some(error.into()),
        }
    }
}

/// A single todo item with its status.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TodoItem {
    /// The text content of the todo item.
    pub text: String,
    /// Current status of the todo item.
    pub status: TodoStatus,
}

impl TodoItem {
    /// Creates a new todo item with the given text and status.
    pub fn new(text: impl Into<String>, status: TodoStatus) -> Self {
        Self {
            text: text.into(),
            status,
        }
    }
}

/// Status of a todo item.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TodoStatus {
    /// Todo item has not been started.
    Pending,
    /// Todo item is currently being worked on.
    InProgress,
    /// Todo item has been completed.
    Completed,
}

impl TodoStatus {
    /// Returns the display icon for this status.
    pub fn icon(&self) -> &'static str {
        match self {
            Self::Pending => "○",
            Self::InProgress => "◐",
            Self::Completed => "●",
        }
    }

    /// Returns the status name as a string.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InProgress => "in_progress",
            Self::Completed => "completed",
        }
    }
}

impl std::fmt::Display for TodoStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl std::str::FromStr for TodoStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "pending" => Ok(Self::Pending),
            "in_progress" | "in-progress" | "inprogress" => Ok(Self::InProgress),
            "completed" | "done" | "complete" => Ok(Self::Completed),
            _ => Err(format!("Unknown todo status: {}", s)),
        }
    }
}

/// Final result of a completed task.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TaskResult {
    /// Whether the task completed successfully.
    pub success: bool,
    /// Summary of what was accomplished.
    pub summary: Option<String>,
    /// Any files that were modified.
    pub files_modified: Vec<String>,
    /// Total tool calls made during the task.
    pub tool_calls: u32,
    /// Total tokens used.
    pub tokens_used: u64,
}

impl Default for TaskResult {
    fn default() -> Self {
        Self {
            success: true,
            summary: None,
            files_modified: Vec::new(),
            tool_calls: 0,
            tokens_used: 0,
        }
    }
}

/// Progress events emitted during task execution.
///
/// These events provide real-time updates about the state of running tasks,
/// tool executions, and todo list changes. UI components can subscribe to
/// these events to display live progress information.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProgressEvent {
    /// A new task has started.
    TaskStarted {
        /// Unique identifier for the task.
        task_id: String,
        /// Human-readable description of the task.
        description: String,
    },

    /// A tool call has started executing.
    ToolCallStarted {
        /// ID of the parent task.
        task_id: String,
        /// Name of the tool being called.
        tool_name: String,
        /// Arguments passed to the tool.
        arguments: serde_json::Value,
    },

    /// A tool call has completed.
    ToolCallCompleted {
        /// ID of the parent task.
        task_id: String,
        /// Name of the tool that was called.
        tool_name: String,
        /// Result of the tool execution.
        result: ToolResult,
        /// How long the tool took to execute.
        duration_ms: u64,
    },

    /// The todo list has been updated.
    TodoUpdated {
        /// ID of the parent task.
        task_id: String,
        /// Current list of todo items.
        todos: Vec<TodoItem>,
    },

    /// A streaming token was generated.
    TokenGenerated {
        /// ID of the parent task.
        task_id: String,
        /// The token that was generated.
        token: String,
    },

    /// The AI started thinking/processing.
    ThinkingStarted {
        /// ID of the parent task.
        task_id: String,
    },

    /// The task has completed successfully.
    TaskCompleted {
        /// ID of the completed task.
        task_id: String,
        /// Result of the task.
        result: TaskResult,
        /// Total duration of the task in milliseconds.
        duration_ms: u64,
    },

    /// The task encountered an error.
    TaskError {
        /// ID of the failed task.
        task_id: String,
        /// Error message describing what went wrong.
        error: String,
    },

    /// Progress percentage update.
    ProgressUpdate {
        /// ID of the parent task.
        task_id: String,
        /// Current progress (0-100).
        percent: u8,
        /// Optional message about current step.
        message: Option<String>,
    },
}

impl ProgressEvent {
    /// Gets the task ID associated with this event.
    pub fn task_id(&self) -> &str {
        match self {
            Self::TaskStarted { task_id, .. } => task_id,
            Self::ToolCallStarted { task_id, .. } => task_id,
            Self::ToolCallCompleted { task_id, .. } => task_id,
            Self::TodoUpdated { task_id, .. } => task_id,
            Self::TokenGenerated { task_id, .. } => task_id,
            Self::ThinkingStarted { task_id } => task_id,
            Self::TaskCompleted { task_id, .. } => task_id,
            Self::TaskError { task_id, .. } => task_id,
            Self::ProgressUpdate { task_id, .. } => task_id,
        }
    }

    /// Returns true if this is a terminal event (task completed or failed).
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::TaskCompleted { .. } | Self::TaskError { .. })
    }

    /// Converts the event to a human-readable message.
    pub fn to_message(&self) -> String {
        match self {
            Self::TaskStarted { description, .. } => {
                format!("Started: {}", description)
            }
            Self::ToolCallStarted { tool_name, .. } => {
                format!("Calling: {}", tool_name)
            }
            Self::ToolCallCompleted {
                tool_name,
                result,
                duration_ms,
                ..
            } => {
                let status = if result.success { "✓" } else { "✗" };
                format!("{} {} ({}ms)", status, tool_name, duration_ms)
            }
            Self::TodoUpdated { todos, .. } => {
                let in_progress = todos
                    .iter()
                    .filter(|t| t.status == TodoStatus::InProgress)
                    .count();
                let completed = todos
                    .iter()
                    .filter(|t| t.status == TodoStatus::Completed)
                    .count();
                format!(
                    "Todos: {} items ({} in progress, {} completed)",
                    todos.len(),
                    in_progress,
                    completed
                )
            }
            Self::TokenGenerated { token, .. } => token.clone(),
            Self::ThinkingStarted { .. } => "Thinking...".to_string(),
            Self::TaskCompleted { result, .. } => {
                format!(
                    "Completed ({} tool calls, {} files)",
                    result.tool_calls,
                    result.files_modified.len()
                )
            }
            Self::TaskError { error, .. } => {
                format!("Error: {}", error)
            }
            Self::ProgressUpdate {
                percent, message, ..
            } => {
                if let Some(msg) = message {
                    format!("{}% - {}", percent, msg)
                } else {
                    format!("{}%", percent)
                }
            }
        }
    }
}

/// Default channel buffer size for progress events.
pub const DEFAULT_PROGRESS_CHANNEL_SIZE: usize = 1000;

/// Emitter for progress events.
///
/// Use this to send progress events from the agent loop or tool executors.
/// Multiple subscribers can receive events from a single emitter.
#[derive(Debug, Clone)]
pub struct ProgressEmitter {
    tx: broadcast::Sender<ProgressEvent>,
}

impl ProgressEmitter {
    /// Creates a new progress emitter and initial subscriber.
    ///
    /// # Returns
    ///
    /// A tuple of `(ProgressEmitter, ProgressSubscriber)` where the emitter
    /// can be used to send events and the subscriber can be used to receive them.
    pub fn new() -> (Self, ProgressSubscriber) {
        Self::with_capacity(DEFAULT_PROGRESS_CHANNEL_SIZE)
    }

    /// Creates a new progress emitter with a custom channel capacity.
    pub fn with_capacity(capacity: usize) -> (Self, ProgressSubscriber) {
        let (tx, rx) = broadcast::channel(capacity);
        (Self { tx }, ProgressSubscriber { rx })
    }

    /// Emits a progress event to all subscribers.
    ///
    /// If there are no active subscribers, the event is silently dropped.
    pub fn emit(&self, event: ProgressEvent) {
        // Ignore send errors (no subscribers)
        let _ = self.tx.send(event);
    }

    /// Creates a new subscriber for this emitter.
    ///
    /// Multiple subscribers can be created to receive events from the same emitter.
    pub fn subscribe(&self) -> ProgressSubscriber {
        ProgressSubscriber {
            rx: self.tx.subscribe(),
        }
    }

    /// Returns the number of active subscribers.
    pub fn subscriber_count(&self) -> usize {
        self.tx.receiver_count()
    }
}

impl Default for ProgressEmitter {
    fn default() -> Self {
        Self::new().0
    }
}

/// Subscriber for progress events.
///
/// Use this to receive progress events from a [`ProgressEmitter`].
pub struct ProgressSubscriber {
    rx: broadcast::Receiver<ProgressEvent>,
}

impl ProgressSubscriber {
    /// Receives the next event asynchronously.
    ///
    /// # Returns
    ///
    /// The next progress event, or `None` if the channel is closed.
    pub async fn recv(&mut self) -> Option<ProgressEvent> {
        match self.rx.recv().await {
            Ok(event) => Some(event),
            Err(broadcast::error::RecvError::Lagged(n)) => {
                // Log lag but continue receiving
                tracing::warn!("Progress subscriber lagged by {} events", n);
                self.rx.recv().await.ok()
            }
            Err(broadcast::error::RecvError::Closed) => None,
        }
    }

    /// Tries to receive the next event without blocking.
    ///
    /// # Returns
    ///
    /// The next progress event if one is available, or `None` otherwise.
    pub fn try_recv(&mut self) -> Option<ProgressEvent> {
        match self.rx.try_recv() {
            Ok(event) => Some(event),
            Err(broadcast::error::TryRecvError::Lagged(n)) => {
                tracing::warn!("Progress subscriber lagged by {} events", n);
                self.rx.try_recv().ok()
            }
            Err(_) => None,
        }
    }
}

// Clone implementation for ProgressSubscriber
impl Clone for ProgressSubscriber {
    fn clone(&self) -> Self {
        Self {
            rx: self.rx.resubscribe(),
        }
    }
}

// ============================================================
// TESTS
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_todo_status_icon() {
        assert_eq!(TodoStatus::Pending.icon(), "○");
        assert_eq!(TodoStatus::InProgress.icon(), "◐");
        assert_eq!(TodoStatus::Completed.icon(), "●");
    }

    #[test]
    fn test_todo_status_from_str() {
        assert_eq!(
            "pending".parse::<TodoStatus>().unwrap(),
            TodoStatus::Pending
        );
        assert_eq!(
            "in_progress".parse::<TodoStatus>().unwrap(),
            TodoStatus::InProgress
        );
        assert_eq!(
            "completed".parse::<TodoStatus>().unwrap(),
            TodoStatus::Completed
        );
        assert_eq!("done".parse::<TodoStatus>().unwrap(), TodoStatus::Completed);
    }

    #[test]
    fn test_tool_result_success() {
        let result = ToolResult::success("output");
        assert!(result.success);
        assert_eq!(result.output, Some("output".to_string()));
        assert!(result.error.is_none());
    }

    #[test]
    fn test_tool_result_failure() {
        let result = ToolResult::failure("error");
        assert!(!result.success);
        assert!(result.output.is_none());
        assert_eq!(result.error, Some("error".to_string()));
    }

    #[test]
    fn test_progress_event_task_id() {
        let event = ProgressEvent::TaskStarted {
            task_id: "task-1".to_string(),
            description: "Test".to_string(),
        };
        assert_eq!(event.task_id(), "task-1");
    }

    #[test]
    fn test_progress_event_is_terminal() {
        let started = ProgressEvent::TaskStarted {
            task_id: "1".to_string(),
            description: "Test".to_string(),
        };
        assert!(!started.is_terminal());

        let completed = ProgressEvent::TaskCompleted {
            task_id: "1".to_string(),
            result: TaskResult::default(),
            duration_ms: 100,
        };
        assert!(completed.is_terminal());

        let error = ProgressEvent::TaskError {
            task_id: "1".to_string(),
            error: "Failed".to_string(),
        };
        assert!(error.is_terminal());
    }

    #[tokio::test]
    async fn test_progress_emitter_subscriber() {
        let (emitter, mut subscriber) = ProgressEmitter::new();

        emitter.emit(ProgressEvent::TaskStarted {
            task_id: "1".to_string(),
            description: "Test task".to_string(),
        });

        let event = subscriber.try_recv();
        assert!(event.is_some());
        assert!(matches!(event.unwrap(), ProgressEvent::TaskStarted { .. }));
    }

    #[tokio::test]
    async fn test_multiple_subscribers() {
        let (emitter, mut sub1) = ProgressEmitter::new();
        let mut sub2 = emitter.subscribe();

        emitter.emit(ProgressEvent::ThinkingStarted {
            task_id: "1".to_string(),
        });

        let e1 = sub1.try_recv();
        let e2 = sub2.try_recv();

        assert!(e1.is_some());
        assert!(e2.is_some());
    }

    #[test]
    fn test_progress_event_to_message() {
        let event = ProgressEvent::ToolCallCompleted {
            task_id: "1".to_string(),
            tool_name: "Read".to_string(),
            result: ToolResult::success("file content"),
            duration_ms: 50,
        };
        assert_eq!(event.to_message(), "✓ Read (50ms)");
    }
}
