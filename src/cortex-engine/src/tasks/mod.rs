//! Task management modules.
//!
//! Provides various task types for the agent system including
//! regular execution, compact, review, and undo operations.

pub mod compact;
pub mod regular;
pub mod review;
pub mod snapshot;
pub mod undo;

pub use compact::{CompactTask, CompactionStrategy};
pub use regular::{RegularTask, TaskExecution};
pub use review::{ReviewFormat, ReviewResult, ReviewTask};
pub use snapshot::{Snapshot, SnapshotManager};
pub use undo::{UndoAction, UndoResult, UndoTask};

use std::fmt;

use serde::{Deserialize, Serialize};

/// Task status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum TaskStatus {
    /// Task is pending execution.
    #[default]
    Pending,
    /// Task is currently running.
    Running,
    /// Task completed successfully.
    Completed,
    /// Task failed with an error.
    Failed,
    /// Task was cancelled.
    Cancelled,
    /// Task was skipped.
    Skipped,
}

impl fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::Running => write!(f, "running"),
            Self::Completed => write!(f, "completed"),
            Self::Failed => write!(f, "failed"),
            Self::Cancelled => write!(f, "cancelled"),
            Self::Skipped => write!(f, "skipped"),
        }
    }
}

/// Task priority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum TaskPriority {
    /// Low priority.
    Low = 0,
    /// Normal priority.
    #[default]
    Normal = 1,
    /// High priority.
    High = 2,
    /// Critical priority.
    Critical = 3,
}

/// Task type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum TaskType {
    /// Regular task execution.
    #[default]
    Regular,
    /// Context compaction task.
    Compact,
    /// Code review task.
    Review,
    /// Undo operation.
    Undo,
    /// User shell command.
    UserShell,
    /// Ghost snapshot for recovery.
    GhostSnapshot,
}

/// Task metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskMeta {
    /// Task ID.
    pub id: String,
    /// Task type.
    pub task_type: TaskType,
    /// Task status.
    pub status: TaskStatus,
    /// Priority.
    pub priority: TaskPriority,
    /// Created timestamp.
    pub created_at: u64,
    /// Started timestamp.
    pub started_at: Option<u64>,
    /// Completed timestamp.
    pub completed_at: Option<u64>,
    /// Duration in milliseconds.
    pub duration_ms: Option<u64>,
    /// Error message if failed.
    pub error: Option<String>,
    /// Parent task ID.
    pub parent_id: Option<String>,
    /// Tags for categorization.
    pub tags: Vec<String>,
}

impl TaskMeta {
    /// Create new task metadata.
    pub fn new(id: impl Into<String>, task_type: TaskType) -> Self {
        Self {
            id: id.into(),
            task_type,
            status: TaskStatus::Pending,
            priority: TaskPriority::Normal,
            created_at: timestamp_now(),
            started_at: None,
            completed_at: None,
            duration_ms: None,
            error: None,
            parent_id: None,
            tags: Vec::new(),
        }
    }

    /// Mark as running.
    pub fn start(&mut self) {
        self.status = TaskStatus::Running;
        self.started_at = Some(timestamp_now());
    }

    /// Mark as completed.
    pub fn complete(&mut self) {
        self.status = TaskStatus::Completed;
        self.completed_at = Some(timestamp_now());
        self.calculate_duration();
    }

    /// Mark as failed.
    pub fn fail(&mut self, error: impl Into<String>) {
        self.status = TaskStatus::Failed;
        self.completed_at = Some(timestamp_now());
        self.error = Some(error.into());
        self.calculate_duration();
    }

    /// Mark as cancelled.
    pub fn cancel(&mut self) {
        self.status = TaskStatus::Cancelled;
        self.completed_at = Some(timestamp_now());
        self.calculate_duration();
    }

    /// Calculate duration.
    fn calculate_duration(&mut self) {
        if let (Some(start), Some(end)) = (self.started_at, self.completed_at)
            && end >= start
        {
            self.duration_ms = Some((end - start) * 1000);
        }
    }

    /// Check if task is finished.
    pub fn is_finished(&self) -> bool {
        matches!(
            self.status,
            TaskStatus::Completed
                | TaskStatus::Failed
                | TaskStatus::Cancelled
                | TaskStatus::Skipped
        )
    }

    /// Add a tag.
    pub fn tag(mut self, tag: impl Into<String>) -> Self {
        self.tags.push(tag.into());
        self
    }

    /// Set priority.
    pub fn with_priority(mut self, priority: TaskPriority) -> Self {
        self.priority = priority;
        self
    }

    /// Set parent.
    pub fn with_parent(mut self, parent_id: impl Into<String>) -> Self {
        self.parent_id = Some(parent_id.into());
        self
    }
}

/// Task result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskResult {
    /// Task metadata.
    pub meta: TaskMeta,
    /// Output data.
    pub output: Option<serde_json::Value>,
    /// Artifacts produced.
    pub artifacts: Vec<TaskArtifact>,
}

impl TaskResult {
    /// Create a successful result.
    pub fn success(mut meta: TaskMeta, output: Option<serde_json::Value>) -> Self {
        meta.complete();
        Self {
            meta,
            output,
            artifacts: Vec::new(),
        }
    }

    /// Create a failed result.
    pub fn failure(mut meta: TaskMeta, error: impl Into<String>) -> Self {
        meta.fail(error);
        Self {
            meta,
            output: None,
            artifacts: Vec::new(),
        }
    }

    /// Add an artifact.
    pub fn with_artifact(mut self, artifact: TaskArtifact) -> Self {
        self.artifacts.push(artifact);
        self
    }
}

/// Task artifact.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskArtifact {
    /// Artifact name.
    pub name: String,
    /// Artifact type.
    pub artifact_type: ArtifactType,
    /// Content or path.
    pub content: String,
    /// Size in bytes.
    pub size: Option<usize>,
}

impl TaskArtifact {
    /// Create a file artifact.
    pub fn file(name: impl Into<String>, path: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            artifact_type: ArtifactType::File,
            content: path.into(),
            size: None,
        }
    }

    /// Create a text artifact.
    pub fn text(name: impl Into<String>, content: impl Into<String>) -> Self {
        let content = content.into();
        let size = content.len();
        Self {
            name: name.into(),
            artifact_type: ArtifactType::Text,
            content,
            size: Some(size),
        }
    }

    /// Create a diff artifact.
    pub fn diff(name: impl Into<String>, content: impl Into<String>) -> Self {
        let content = content.into();
        let size = content.len();
        Self {
            name: name.into(),
            artifact_type: ArtifactType::Diff,
            content,
            size: Some(size),
        }
    }
}

/// Artifact type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactType {
    /// File path.
    File,
    /// Text content.
    Text,
    /// Diff content.
    Diff,
    /// Image content.
    Image,
    /// JSON data.
    Json,
    /// Binary data.
    Binary,
}

/// Get current timestamp in seconds.
fn timestamp_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Task queue for managing pending tasks.
#[derive(Debug, Default)]
pub struct TaskQueue {
    tasks: Vec<TaskMeta>,
}

impl TaskQueue {
    /// Create a new task queue.
    pub fn new() -> Self {
        Self { tasks: Vec::new() }
    }

    /// Add a task to the queue.
    pub fn push(&mut self, task: TaskMeta) {
        self.tasks.push(task);
        self.sort();
    }

    /// Get the next task.
    pub fn pop(&mut self) -> Option<TaskMeta> {
        self.tasks.pop()
    }

    /// Peek at the next task.
    pub fn peek(&self) -> Option<&TaskMeta> {
        self.tasks.last()
    }

    /// Get queue length.
    pub fn len(&self) -> usize {
        self.tasks.len()
    }

    /// Check if queue is empty.
    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }

    /// Sort by priority (highest first).
    fn sort(&mut self) {
        self.tasks.sort_by(|a, b| a.priority.cmp(&b.priority));
    }

    /// Get all pending tasks.
    pub fn pending(&self) -> Vec<&TaskMeta> {
        self.tasks
            .iter()
            .filter(|t| t.status == TaskStatus::Pending)
            .collect()
    }

    /// Cancel all pending tasks.
    pub fn cancel_all(&mut self) {
        for task in &mut self.tasks {
            if task.status == TaskStatus::Pending {
                task.cancel();
            }
        }
    }
}
