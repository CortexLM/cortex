//! Task hydration for restoring DAG state from partial data.
//!
//! This module provides functionality to "hydrate" task DAGs from
//! various sources, including partial state, logs, and external systems.
//!
//! # Example
//!
//! ```rust,ignore
//! use cortex_agents::task::hydration::DagHydrator;
//!
//! let hydrator = DagHydrator::new();
//!
//! // Hydrate from task list
//! let tasks = vec![
//!     TaskSpec::new("build", "Build the project"),
//!     TaskSpec::new("test", "Run tests").depends_on("build"),
//! ];
//! let dag = hydrator.hydrate_from_specs(&tasks)?;
//! ```
//!
//! # Session Restoration
//!
//! The module also provides session restoration for resuming work:
//!
//! ```rust,ignore
//! use cortex_agents::task::hydration::{SessionHydrator, StaleTaskChecker};
//! use cortex_agents::task::persistence::DagStore;
//!
//! let store = DagStore::new("/path/to/store");
//! let hydrator = SessionHydrator::new(store);
//!
//! // Restore a previous session, resetting in-progress tasks
//! let dag = hydrator.restore_session("old-session", "new-session").await?;
//!
//! // Check for stale tasks (files modified since completion)
//! let stale = StaleTaskChecker::check_stale_tasks(&dag).await;
//! ```

use super::dag::{DagError, DagResult, Task, TaskDag, TaskId, TaskStatus};
use super::persistence::{DagStore, PersistenceError};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use thiserror::Error;

/// A task specification for hydration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskSpec {
    /// Task name (used as identifier).
    pub name: String,
    /// Task description.
    pub description: String,
    /// Names of tasks this task depends on.
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// Files this task affects.
    #[serde(default)]
    pub affected_files: Vec<String>,
    /// Priority.
    #[serde(default)]
    pub priority: i32,
    /// Estimated duration in seconds.
    pub estimated_duration: Option<u64>,
    /// Initial status (default: pending).
    #[serde(default)]
    pub status: Option<TaskStatus>,
    /// Custom metadata.
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

impl TaskSpec {
    /// Create a new task spec.
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            depends_on: Vec::new(),
            affected_files: Vec::new(),
            priority: 0,
            estimated_duration: None,
            status: None,
            metadata: HashMap::new(),
        }
    }

    /// Add a dependency.
    pub fn depends_on(mut self, name: impl Into<String>) -> Self {
        self.depends_on.push(name.into());
        self
    }

    /// Add multiple dependencies.
    pub fn depends_on_all(mut self, names: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.depends_on.extend(names.into_iter().map(Into::into));
        self
    }

    /// Set affected files.
    pub fn with_affected_files(mut self, files: Vec<String>) -> Self {
        self.affected_files = files;
        self
    }

    /// Set priority.
    pub fn with_priority(mut self, priority: i32) -> Self {
        self.priority = priority;
        self
    }

    /// Set estimated duration.
    pub fn with_estimated_duration(mut self, seconds: u64) -> Self {
        self.estimated_duration = Some(seconds);
        self
    }

    /// Set initial status.
    pub fn with_status(mut self, status: TaskStatus) -> Self {
        self.status = Some(status);
        self
    }

    /// Add metadata.
    pub fn with_metadata(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.metadata.insert(key.into(), value);
        self
    }
}

/// Hydrator for creating DAGs from various sources.
#[derive(Debug, Default)]
pub struct DagHydrator {
    /// Whether to infer dependencies from affected files.
    pub infer_file_dependencies: bool,
    /// Whether to validate the DAG after hydration.
    pub validate: bool,
}

impl DagHydrator {
    /// Create a new hydrator with default settings.
    pub fn new() -> Self {
        Self {
            infer_file_dependencies: false,
            validate: true,
        }
    }

    /// Enable file-based dependency inference.
    pub fn with_file_inference(mut self) -> Self {
        self.infer_file_dependencies = true;
        self
    }

    /// Disable post-hydration validation.
    pub fn without_validation(mut self) -> Self {
        self.validate = false;
        self
    }

    /// Hydrate a DAG from task specifications.
    pub fn hydrate_from_specs(&self, specs: &[TaskSpec]) -> DagResult<TaskDag> {
        let mut dag = TaskDag::new();
        let mut name_to_id: HashMap<String, TaskId> = HashMap::new();

        // First pass: create all tasks
        for spec in specs {
            let mut task = Task::new(&spec.name, &spec.description)
                .with_priority(spec.priority)
                .with_affected_files(spec.affected_files.clone());

            if let Some(duration) = spec.estimated_duration {
                task = task.with_estimated_duration(duration);
            }

            for (key, value) in &spec.metadata {
                task.metadata.insert(key.clone(), value.clone());
            }

            let id = dag.add_task(task);
            name_to_id.insert(spec.name.clone(), id);
        }

        // Second pass: add dependencies
        for spec in specs {
            if let Some(&task_id) = name_to_id.get(&spec.name) {
                for dep_name in &spec.depends_on {
                    if let Some(&dep_id) = name_to_id.get(dep_name) {
                        dag.add_dependency(task_id, dep_id)?;
                    }
                    // Skip missing dependencies (might be optional)
                }
            }
        }

        // Optional: infer dependencies from affected files
        if self.infer_file_dependencies {
            self.infer_dependencies_from_files(&mut dag, specs, &name_to_id)?;
        }

        // Restore statuses
        for spec in specs {
            if let Some(status) = spec.status {
                if let Some(&task_id) = name_to_id.get(&spec.name) {
                    if let Some(task) = dag.get_task_mut(task_id) {
                        task.status = status;
                    }
                }
            }
        }

        // Validate if enabled
        if self.validate {
            // Check for cycles by doing topological sort
            dag.topological_sort()?;
        }

        Ok(dag)
    }

    /// Infer dependencies based on overlapping affected files.
    fn infer_dependencies_from_files(
        &self,
        dag: &mut TaskDag,
        specs: &[TaskSpec],
        name_to_id: &HashMap<String, TaskId>,
    ) -> DagResult<()> {
        // Build a map of file -> tasks that affect it
        let mut file_to_tasks: HashMap<&str, Vec<&str>> = HashMap::new();

        for spec in specs {
            for file in &spec.affected_files {
                file_to_tasks
                    .entry(file.as_str())
                    .or_default()
                    .push(&spec.name);
            }
        }

        // For each file affected by multiple tasks, add dependencies
        // based on priority (higher priority tasks first)
        for (_file, task_names) in file_to_tasks {
            if task_names.len() < 2 {
                continue;
            }

            // Get tasks with their priorities
            let mut tasks_with_priority: Vec<_> = task_names
                .iter()
                .filter_map(|&name| {
                    let id = name_to_id.get(name)?;
                    let task = dag.get_task(*id)?;
                    Some((name, *id, task.priority))
                })
                .collect();

            // Sort by priority (descending)
            tasks_with_priority.sort_by(|a, b| b.2.cmp(&a.2));

            // Add dependencies: lower priority depends on higher priority
            for i in 1..tasks_with_priority.len() {
                let (_higher_name, higher_id, _) = tasks_with_priority[i - 1];
                let (_lower_name, lower_id, _) = tasks_with_priority[i];

                // Only add if not already a dependency (to avoid redundant edges)
                if let Some(deps) = dag.get_dependencies(lower_id) {
                    if !deps.contains(&higher_id) {
                        // Check if this would create a cycle before adding
                        let result = dag.add_dependency(lower_id, higher_id);
                        if result.is_err() {
                            // Skip if it would create a cycle
                            continue;
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Hydrate a DAG from JSON task specifications.
    pub fn hydrate_from_json(&self, json: &str) -> DagResult<TaskDag> {
        let specs: Vec<TaskSpec> =
            serde_json::from_str(json).map_err(|_e| DagError::TaskNotFound(TaskId::new(0)))?;
        self.hydrate_from_specs(&specs)
    }
}

/// Builder for creating complex DAGs programmatically.
#[derive(Debug, Default)]
pub struct DagBuilder {
    specs: Vec<TaskSpec>,
}

impl DagBuilder {
    /// Create a new builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a task.
    pub fn task(mut self, spec: TaskSpec) -> Self {
        self.specs.push(spec);
        self
    }

    /// Add a simple task by name and description.
    pub fn add_task(mut self, name: impl Into<String>, description: impl Into<String>) -> Self {
        self.specs.push(TaskSpec::new(name, description));
        self
    }

    /// Add a dependency to the last added task.
    pub fn depends_on(mut self, name: impl Into<String>) -> Self {
        if let Some(last) = self.specs.last_mut() {
            last.depends_on.push(name.into());
        }
        self
    }

    /// Build the DAG.
    pub fn build(self) -> DagResult<TaskDag> {
        DagHydrator::new().hydrate_from_specs(&self.specs)
    }

    /// Build the DAG with custom hydrator settings.
    pub fn build_with(self, hydrator: &DagHydrator) -> DagResult<TaskDag> {
        hydrator.hydrate_from_specs(&self.specs)
    }
}

/// Errors for session hydration.
#[derive(Debug, Error)]
pub enum SessionHydrationError {
    /// Persistence error.
    #[error("Persistence error: {0}")]
    Persistence(#[from] PersistenceError),

    /// DAG error.
    #[error("DAG error: {0}")]
    Dag(#[from] DagError),

    /// Session not found.
    #[error("Session not found: {0}")]
    SessionNotFound(String),

    /// IO error.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Result type for session hydration.
pub type SessionHydrationResult<T> = std::result::Result<T, SessionHydrationError>;

/// Hydrator for restoring DAG state from previous sessions.
///
/// This handles session continuity, including:
/// - Restoring DAG state from storage
/// - Resetting in-progress tasks to pending (since they weren't completed)
/// - Optionally checking for stale tasks
///
/// # Example
///
/// ```rust,ignore
/// use cortex_agents::task::hydration::SessionHydrator;
/// use cortex_agents::task::persistence::DagStore;
///
/// let store = DagStore::new("/path/to/store");
/// let hydrator = SessionHydrator::new(store);
///
/// // Restore previous session
/// let dag = hydrator.restore_session("old-session", "new-session").await?;
/// ```
pub struct SessionHydrator {
    /// Storage backend.
    store: DagStore,
    /// Whether to reset running tasks to pending.
    reset_running: bool,
    /// Whether to check for stale tasks.
    check_stale: bool,
    /// Base path for stale task checking.
    workspace_path: Option<std::path::PathBuf>,
}

impl SessionHydrator {
    /// Create a new session hydrator.
    pub fn new(store: DagStore) -> Self {
        Self {
            store,
            reset_running: true,
            check_stale: false,
            workspace_path: None,
        }
    }

    /// Set whether to reset running tasks to pending.
    pub fn with_reset_running(mut self, reset: bool) -> Self {
        self.reset_running = reset;
        self
    }

    /// Enable stale task checking.
    pub fn with_stale_check(mut self, workspace_path: impl Into<std::path::PathBuf>) -> Self {
        self.check_stale = true;
        self.workspace_path = Some(workspace_path.into());
        self
    }

    /// Restore a DAG from a previous session.
    ///
    /// This loads the DAG, resets in-progress tasks to pending,
    /// and optionally saves it with a new session ID.
    pub async fn restore_session(
        &self,
        old_session_id: &str,
        new_session_id: Option<&str>,
    ) -> SessionHydrationResult<TaskDag> {
        // Load the existing DAG
        let mut dag = self.store.load(old_session_id).await.map_err(|e| match e {
            PersistenceError::NotFound(id) => SessionHydrationError::SessionNotFound(id),
            other => SessionHydrationError::Persistence(other),
        })?;

        // Reset running tasks to pending
        if self.reset_running {
            self.reset_running_tasks(&mut dag);
        }

        // Check for stale tasks if enabled
        if self.check_stale {
            if let Some(ref workspace) = self.workspace_path {
                let stale =
                    StaleTaskChecker::check_stale_tasks_with_workspace(&dag, workspace).await;
                for task_id in stale {
                    self.mark_stale(&mut dag, task_id);
                }
            }
        }

        // Save with new session ID if provided
        if let Some(new_id) = new_session_id {
            self.store.save(new_id, &dag).await?;
        }

        Ok(dag)
    }

    /// Reset all running tasks to pending.
    fn reset_running_tasks(&self, dag: &mut TaskDag) {
        let running_ids: Vec<TaskId> = dag
            .all_tasks()
            .filter(|t| t.status == TaskStatus::Running)
            .filter_map(|t| t.id)
            .collect();

        for id in running_ids {
            if let Some(task) = dag.get_task_mut(id) {
                task.status = TaskStatus::Ready;
                task.agent_id = None;
            }
        }
    }

    /// Mark a task as stale by resetting it to pending.
    fn mark_stale(&self, dag: &mut TaskDag, task_id: TaskId) {
        if let Some(task) = dag.get_task_mut(task_id) {
            if task.status == TaskStatus::Completed {
                task.status = TaskStatus::Ready;
                task.result = None;
                task.metadata
                    .insert("was_stale".to_string(), serde_json::json!(true));
            }
        }
    }

    /// List available sessions.
    pub async fn list_sessions(&self) -> SessionHydrationResult<Vec<String>> {
        Ok(self.store.list().await?)
    }

    /// Check if a session exists.
    pub fn session_exists(&self, session_id: &str) -> bool {
        self.store.exists(session_id)
    }
}

/// Information about a stale task.
#[derive(Debug, Clone)]
pub struct StaleTaskInfo {
    /// Task ID.
    pub task_id: TaskId,
    /// Task name.
    pub task_name: String,
    /// Files that were modified.
    pub modified_files: Vec<String>,
}

/// Checker for detecting stale tasks.
///
/// A task is considered stale if:
/// - It was completed, but
/// - One of its affected files has been modified since completion
///
/// This is useful for detecting when external changes have invalidated
/// previous work and tasks need to be re-run.
pub struct StaleTaskChecker;

impl StaleTaskChecker {
    /// Check for stale tasks in a DAG.
    ///
    /// Returns a list of task IDs that are potentially stale.
    /// This uses the affected_files metadata to check file modification times.
    pub async fn check_stale_tasks(dag: &TaskDag) -> Vec<TaskId> {
        Self::check_stale_tasks_with_workspace(dag, Path::new(".")).await
    }

    /// Check for stale tasks with a specific workspace path.
    pub async fn check_stale_tasks_with_workspace(dag: &TaskDag, workspace: &Path) -> Vec<TaskId> {
        let mut stale = Vec::new();

        for task in dag.all_tasks() {
            // Only check completed tasks
            if task.status != TaskStatus::Completed {
                continue;
            }

            let task_id = match task.id {
                Some(id) => id,
                None => continue,
            };

            // Get completion time from metadata if available
            let completion_time = task
                .metadata
                .get("completed_at")
                .and_then(|v| v.as_str())
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.with_timezone(&chrono::Utc));

            // Check each affected file
            for file in &task.affected_files {
                let file_path = workspace.join(file);

                if let Ok(metadata) = tokio::fs::metadata(&file_path).await {
                    if let Ok(modified) = metadata.modified() {
                        // If we have a completion time, compare
                        if let Some(completed) = completion_time {
                            let modified_time: chrono::DateTime<chrono::Utc> = modified.into();
                            if modified_time > completed {
                                stale.push(task_id);
                                break;
                            }
                        } else {
                            // No completion time, can't determine staleness
                            // For now, assume not stale
                        }
                    }
                }
            }
        }

        stale
    }

    /// Get detailed information about stale tasks.
    pub async fn get_stale_task_info(dag: &TaskDag, workspace: &Path) -> Vec<StaleTaskInfo> {
        let mut results = Vec::new();

        for task in dag.all_tasks() {
            if task.status != TaskStatus::Completed {
                continue;
            }

            let task_id = match task.id {
                Some(id) => id,
                None => continue,
            };

            let completion_time = task
                .metadata
                .get("completed_at")
                .and_then(|v| v.as_str())
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.with_timezone(&chrono::Utc));

            let mut modified_files = Vec::new();

            for file in &task.affected_files {
                let file_path = workspace.join(file);

                if let Ok(metadata) = tokio::fs::metadata(&file_path).await {
                    if let Ok(modified) = metadata.modified() {
                        if let Some(completed) = completion_time {
                            let modified_time: chrono::DateTime<chrono::Utc> = modified.into();
                            if modified_time > completed {
                                modified_files.push(file.clone());
                            }
                        }
                    }
                }
            }

            if !modified_files.is_empty() {
                results.push(StaleTaskInfo {
                    task_id,
                    task_name: task.name.clone(),
                    modified_files,
                });
            }
        }

        results
    }
}

/// Configuration for session restoration behavior.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRestoreConfig {
    /// Whether to reset running tasks to pending.
    pub reset_running: bool,
    /// Whether to check for stale tasks.
    pub check_stale: bool,
    /// Whether to invalidate stale tasks.
    pub invalidate_stale: bool,
    /// Maximum age of a session to consider for restoration (seconds).
    pub max_session_age: Option<u64>,
}

impl Default for SessionRestoreConfig {
    fn default() -> Self {
        Self {
            reset_running: true,
            check_stale: true,
            invalidate_stale: false,
            max_session_age: Some(86400), // 24 hours
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hydrate_simple() {
        let specs = vec![
            TaskSpec::new("build", "Build the project"),
            TaskSpec::new("test", "Run tests").depends_on("build"),
            TaskSpec::new("deploy", "Deploy").depends_on("test"),
        ];

        let dag = DagHydrator::new().hydrate_from_specs(&specs).unwrap();

        assert_eq!(dag.len(), 3);

        // Only build should be ready initially
        let ready = dag.get_ready_tasks();
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].name, "build");
    }

    #[test]
    fn test_hydrate_parallel() {
        let specs = vec![
            TaskSpec::new("lint", "Run linter"),
            TaskSpec::new("test", "Run tests"),
            TaskSpec::new("build", "Build").depends_on_all(["lint", "test"]),
        ];

        let dag = DagHydrator::new().hydrate_from_specs(&specs).unwrap();

        // Both lint and test should be ready
        let ready = dag.get_ready_tasks();
        assert_eq!(ready.len(), 2);
    }

    #[test]
    fn test_hydrate_with_status() {
        let specs = vec![
            TaskSpec::new("completed", "Done task").with_status(TaskStatus::Completed),
            TaskSpec::new("pending", "Pending task").depends_on("completed"),
        ];

        let dag = DagHydrator::new().hydrate_from_specs(&specs).unwrap();

        let tasks: Vec<_> = dag.all_tasks().collect();
        let completed = tasks.iter().find(|t| t.name == "completed").unwrap();
        assert_eq!(completed.status, TaskStatus::Completed);
    }

    #[test]
    fn test_dag_builder() {
        let dag = DagBuilder::new()
            .add_task("a", "Task A")
            .add_task("b", "Task B")
            .depends_on("a")
            .add_task("c", "Task C")
            .depends_on("b")
            .build()
            .unwrap();

        let sorted = dag.topological_sort().unwrap();
        assert_eq!(sorted.len(), 3);
    }

    #[test]
    fn test_builder_with_full_spec() {
        let dag = DagBuilder::new()
            .task(
                TaskSpec::new("complex", "Complex task")
                    .with_priority(10)
                    .with_affected_files(vec!["file.rs".to_string()])
                    .with_metadata("custom", serde_json::json!(true)),
            )
            .build()
            .unwrap();

        let task = dag.get_ready_tasks()[0];
        assert_eq!(task.name, "complex");
        assert_eq!(task.priority, 10);
    }

    #[test]
    fn test_hydrate_detects_cycle() {
        let specs = vec![
            TaskSpec::new("a", "A").depends_on("c"),
            TaskSpec::new("b", "B").depends_on("a"),
            TaskSpec::new("c", "C").depends_on("b"),
        ];

        let result = DagHydrator::new().hydrate_from_specs(&specs);
        assert!(result.is_err());
    }

    #[test]
    fn test_file_dependency_inference() {
        let specs = vec![
            TaskSpec::new("write_config", "Write config")
                .with_affected_files(vec!["config.json".to_string()])
                .with_priority(10),
            TaskSpec::new("read_config", "Read config")
                .with_affected_files(vec!["config.json".to_string()])
                .with_priority(5),
        ];

        let dag = DagHydrator::new()
            .with_file_inference()
            .hydrate_from_specs(&specs)
            .unwrap();

        // read_config should depend on write_config due to file overlap
        // and priority ordering
        let ready = dag.get_ready_tasks();
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].name, "write_config");
    }

    #[test]
    fn test_hydrate_missing_dependency() {
        // Missing dependency should be silently ignored
        let specs = vec![TaskSpec::new("task", "Task").depends_on("nonexistent")];

        let dag = DagHydrator::new().hydrate_from_specs(&specs).unwrap();

        // Task should be ready since the dependency doesn't exist
        let ready = dag.get_ready_tasks();
        assert_eq!(ready.len(), 1);
    }

    #[test]
    fn test_task_spec_builder() {
        let spec = TaskSpec::new("test", "Test task")
            .depends_on("a")
            .depends_on("b")
            .with_priority(5)
            .with_affected_files(vec!["test.rs".to_string()])
            .with_estimated_duration(60)
            .with_metadata("key", serde_json::json!("value"));

        assert_eq!(spec.name, "test");
        assert_eq!(spec.depends_on, vec!["a", "b"]);
        assert_eq!(spec.priority, 5);
        assert_eq!(spec.affected_files, vec!["test.rs"]);
        assert_eq!(spec.estimated_duration, Some(60));
        assert!(spec.metadata.contains_key("key"));
    }
}
