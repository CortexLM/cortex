//! Task executor for parallel DAG execution.
//!
//! This module provides the `TaskExecutor` which orchestrates task execution
//! respecting dependencies and enabling parallel execution of independent tasks.
//!
//! # Example
//!
//! ```rust,ignore
//! use cortex_agents::task::{TaskDag, Task, TaskExecutor, ExecutorConfig};
//!
//! let mut dag = TaskDag::new();
//! let t1 = dag.add_task(Task::new("Setup", "Initialize"));
//! let t2 = dag.add_task(Task::new("Build", "Build project"));
//! dag.add_dependency(t2, t1).unwrap();
//!
//! let config = ExecutorConfig::default();
//! let mut executor = TaskExecutor::new(dag, config);
//!
//! executor.execute_all(|task| async move {
//!     println!("Executing: {}", task.name);
//!     Ok(Some("Done".to_string()))
//! }).await?;
//! ```

use super::dag::{DagError, Task, TaskDag, TaskId, TaskStatus};
use super::persistence::{DagStore, InMemoryDagStore, PersistenceError, PersistenceResult};
use futures::stream::{FuturesUnordered, StreamExt};
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};
use thiserror::Error;
use tokio::sync::RwLock;

/// Errors for executor operations.
#[derive(Debug, Error)]
pub enum ExecutorError {
    /// DAG error.
    #[error("DAG error: {0}")]
    Dag(#[from] DagError),

    /// Persistence error.
    #[error("Persistence error: {0}")]
    Persistence(#[from] PersistenceError),

    /// Task execution failed.
    #[error("Task {task_id} failed: {message}")]
    TaskFailed { task_id: TaskId, message: String },

    /// Deadlock detected - no tasks can make progress.
    #[error("Deadlock detected: no ready tasks but {pending} tasks pending")]
    DeadlockDetected { pending: usize },

    /// Execution was cancelled.
    #[error("Execution cancelled")]
    Cancelled,

    /// Timeout exceeded.
    #[error("Execution timeout exceeded: {0:?}")]
    Timeout(Duration),

    /// File conflict detected between parallel tasks.
    #[error("File conflict between tasks {task1} and {task2}: {file}")]
    FileConflict {
        task1: TaskId,
        task2: TaskId,
        file: String,
    },

    /// IO error.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Result type for executor operations.
pub type ExecutorResult<T> = std::result::Result<T, ExecutorError>;

/// Configuration for the task executor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutorConfig {
    /// Maximum number of tasks to execute in parallel.
    pub max_parallel: usize,
    /// Whether to stop on first failure.
    pub fail_fast: bool,
    /// Overall timeout for all tasks.
    pub timeout: Option<Duration>,
    /// Per-task timeout.
    pub task_timeout: Option<Duration>,
    /// Whether to check for file conflicts before parallel execution.
    pub check_file_conflicts: bool,
    /// Interval for persisting DAG state.
    pub persist_interval: Option<Duration>,
    /// Session ID for persistence.
    pub session_id: Option<String>,
}

impl Default for ExecutorConfig {
    fn default() -> Self {
        Self {
            max_parallel: 4,
            fail_fast: true,
            timeout: None,
            task_timeout: None,
            check_file_conflicts: true,
            persist_interval: Some(Duration::from_secs(5)),
            session_id: None,
        }
    }
}

impl ExecutorConfig {
    /// Create a new config with default settings.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set max parallel tasks.
    pub fn with_max_parallel(mut self, max: usize) -> Self {
        self.max_parallel = max.max(1);
        self
    }

    /// Enable fail-fast mode.
    pub fn with_fail_fast(mut self, fail_fast: bool) -> Self {
        self.fail_fast = fail_fast;
        self
    }

    /// Set overall timeout.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Set per-task timeout.
    pub fn with_task_timeout(mut self, timeout: Duration) -> Self {
        self.task_timeout = Some(timeout);
        self
    }

    /// Enable file conflict checking.
    pub fn with_file_conflict_check(mut self, check: bool) -> Self {
        self.check_file_conflicts = check;
        self
    }

    /// Set session ID for persistence.
    pub fn with_session_id(mut self, id: impl Into<String>) -> Self {
        self.session_id = Some(id.into());
        self
    }
}

/// Result of a single task execution.
#[derive(Debug, Clone)]
pub struct TaskExecutionResult {
    /// Task ID.
    pub task_id: TaskId,
    /// Whether the task succeeded.
    pub success: bool,
    /// Result value if successful.
    pub result: Option<String>,
    /// Error message if failed.
    pub error: Option<String>,
    /// Execution duration.
    pub duration: Duration,
    /// Agent ID that executed the task.
    pub agent_id: Option<String>,
}

/// Progress callback data.
#[derive(Debug, Clone)]
pub struct ExecutionProgress {
    /// Total number of tasks.
    pub total: usize,
    /// Number of completed tasks.
    pub completed: usize,
    /// Number of failed tasks.
    pub failed: usize,
    /// Number of skipped tasks.
    pub skipped: usize,
    /// Number of running tasks.
    pub running: usize,
    /// Number of pending tasks.
    pub pending: usize,
    /// Current running task names.
    pub running_tasks: Vec<String>,
}

impl ExecutionProgress {
    /// Calculate completion percentage.
    pub fn percentage(&self) -> f64 {
        if self.total == 0 {
            100.0
        } else {
            ((self.completed + self.failed + self.skipped) as f64 / self.total as f64) * 100.0
        }
    }

    /// Check if execution is complete.
    pub fn is_complete(&self) -> bool {
        self.completed + self.failed + self.skipped >= self.total
    }
}

/// Task executor for parallel DAG execution.
pub struct TaskExecutor<S = InMemoryDagStore> {
    /// The task DAG.
    dag: Arc<RwLock<TaskDag>>,
    /// Executor configuration.
    config: ExecutorConfig,
    /// Optional persistence store.
    store: Option<S>,
    /// Whether execution was cancelled.
    cancelled: Arc<std::sync::atomic::AtomicBool>,
    /// Execution start time.
    start_time: Option<Instant>,
    /// Last persist time.
    last_persist: Arc<RwLock<Option<Instant>>>,
}

impl TaskExecutor<InMemoryDagStore> {
    /// Create a new executor with an in-memory store.
    pub fn new(dag: TaskDag, config: ExecutorConfig) -> Self {
        Self {
            dag: Arc::new(RwLock::new(dag)),
            config,
            store: None,
            cancelled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            start_time: None,
            last_persist: Arc::new(RwLock::new(None)),
        }
    }
}

impl<S> TaskExecutor<S>
where
    S: TaskStore + Send + Sync,
{
    /// Create a new executor with a custom store.
    pub fn with_store(dag: TaskDag, config: ExecutorConfig, store: S) -> Self {
        Self {
            dag: Arc::new(RwLock::new(dag)),
            config,
            store: Some(store),
            cancelled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            start_time: None,
            last_persist: Arc::new(RwLock::new(None)),
        }
    }

    /// Cancel the execution.
    pub fn cancel(&self) {
        self.cancelled
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// Check if execution was cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Get current progress.
    pub async fn progress(&self) -> ExecutionProgress {
        let dag = self.dag.read().await;
        let counts = dag.status_counts();

        let running_tasks: Vec<String> = dag
            .all_tasks()
            .filter(|t| t.status == TaskStatus::Running)
            .map(|t| t.name.clone())
            .collect();

        ExecutionProgress {
            total: dag.len(),
            completed: *counts.get(&TaskStatus::Completed).unwrap_or(&0),
            failed: *counts.get(&TaskStatus::Failed).unwrap_or(&0),
            skipped: *counts.get(&TaskStatus::Skipped).unwrap_or(&0),
            running: *counts.get(&TaskStatus::Running).unwrap_or(&0),
            pending: *counts.get(&TaskStatus::Pending).unwrap_or(&0)
                + *counts.get(&TaskStatus::Ready).unwrap_or(&0),
            running_tasks,
        }
    }

    /// Execute all tasks in the DAG.
    ///
    /// The executor will:
    /// 1. Get all ready tasks (dependencies satisfied)
    /// 2. Check for file conflicts if enabled
    /// 3. Execute up to `max_parallel` tasks concurrently
    /// 4. Mark completed/failed tasks and update dependents
    /// 5. Persist state periodically
    /// 6. Repeat until all tasks are done or error occurs
    pub async fn execute_all<F, Fut>(
        &mut self,
        execute_fn: F,
    ) -> ExecutorResult<Vec<TaskExecutionResult>>
    where
        F: Fn(Task) -> Fut + Clone + Send + Sync + 'static,
        Fut: Future<Output = Result<Option<String>, String>> + Send + 'static,
    {
        self.start_time = Some(Instant::now());
        let mut results = Vec::new();

        loop {
            // Check for cancellation
            if self.is_cancelled() {
                return Err(ExecutorError::Cancelled);
            }

            // Check for timeout
            if let Some(timeout) = self.config.timeout {
                if let Some(start) = self.start_time {
                    if start.elapsed() > timeout {
                        return Err(ExecutorError::Timeout(timeout));
                    }
                }
            }

            // Get ready tasks
            let ready_tasks = {
                let dag = self.dag.read().await;
                let ready: Vec<Task> = dag
                    .get_ready_tasks_by_priority()
                    .into_iter()
                    .cloned()
                    .collect();
                ready
            };

            if ready_tasks.is_empty() {
                // Check if we're done or deadlocked
                let dag = self.dag.read().await;
                let counts = dag.status_counts();
                let pending = *counts.get(&TaskStatus::Pending).unwrap_or(&0);
                let running = *counts.get(&TaskStatus::Running).unwrap_or(&0);

                if pending == 0 && running == 0 {
                    // All tasks are complete
                    break;
                } else if running == 0 {
                    // No running tasks but still pending - deadlock
                    return Err(ExecutorError::DeadlockDetected { pending });
                }

                // Wait for running tasks to complete
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }

            // Select batch of tasks to execute
            let batch_size = self.config.max_parallel.min(ready_tasks.len());
            let batch: Vec<Task> = ready_tasks.into_iter().take(batch_size).collect();

            // Check for file conflicts in the batch
            if self.config.check_file_conflicts {
                self.check_batch_conflicts(&batch)?;
            }

            // Mark tasks as running
            {
                let mut dag = self.dag.write().await;
                for task in &batch {
                    if let Some(id) = task.id {
                        let _ = dag.start_task(id, None);
                    }
                }
            }

            // Persist state
            self.maybe_persist().await?;

            // Execute batch in parallel
            let batch_results = self.execute_batch(&batch, execute_fn.clone()).await?;

            // Process results
            {
                let mut dag = self.dag.write().await;
                for result in &batch_results {
                    if result.success {
                        let _ = dag.complete_task(result.task_id, result.result.clone());
                    } else {
                        let error = result
                            .error
                            .clone()
                            .unwrap_or_else(|| "Unknown error".to_string());
                        let _ = dag.fail_task(result.task_id, error);

                        // Fail-fast check
                        if self.config.fail_fast {
                            // Persist final state
                            self.maybe_persist().await?;
                            return Err(ExecutorError::TaskFailed {
                                task_id: result.task_id,
                                message: result.error.clone().unwrap_or_default(),
                            });
                        }
                    }
                }
            }

            results.extend(batch_results);

            // Persist state after batch
            self.maybe_persist().await?;
        }

        // Final persist
        self.force_persist().await?;

        Ok(results)
    }

    /// Execute a batch of tasks in parallel.
    async fn execute_batch<F, Fut>(
        &self,
        batch: &[Task],
        execute_fn: F,
    ) -> ExecutorResult<Vec<TaskExecutionResult>>
    where
        F: Fn(Task) -> Fut + Clone + Send + Sync + 'static,
        Fut: Future<Output = Result<Option<String>, String>> + Send + 'static,
    {
        let mut futures = FuturesUnordered::new();

        for task in batch {
            let task_clone = task.clone();
            let task_id = task.id.expect("Task must have an ID");
            let execute = execute_fn.clone();
            let task_timeout = self.config.task_timeout;

            futures.push(async move {
                let start = Instant::now();

                let result = if let Some(timeout) = task_timeout {
                    match tokio::time::timeout(timeout, execute(task_clone)).await {
                        Ok(r) => r,
                        Err(_) => Err(format!("Task timed out after {:?}", timeout)),
                    }
                } else {
                    execute(task_clone).await
                };

                let duration = start.elapsed();

                match result {
                    Ok(output) => TaskExecutionResult {
                        task_id,
                        success: true,
                        result: output,
                        error: None,
                        duration,
                        agent_id: None,
                    },
                    Err(e) => TaskExecutionResult {
                        task_id,
                        success: false,
                        result: None,
                        error: Some(e),
                        duration,
                        agent_id: None,
                    },
                }
            });
        }

        let mut results = Vec::new();
        while let Some(result) = futures.next().await {
            results.push(result);
        }

        Ok(results)
    }

    /// Check for file conflicts in a batch of tasks.
    fn check_batch_conflicts(&self, batch: &[Task]) -> ExecutorResult<()> {
        for i in 0..batch.len() {
            for j in (i + 1)..batch.len() {
                let t1 = &batch[i];
                let t2 = &batch[j];

                // Check if any affected files overlap
                for file1 in &t1.affected_files {
                    if t2.affected_files.contains(file1) {
                        return Err(ExecutorError::FileConflict {
                            task1: t1.id.unwrap_or(TaskId::new(0)),
                            task2: t2.id.unwrap_or(TaskId::new(0)),
                            file: file1.clone(),
                        });
                    }
                }
            }
        }
        Ok(())
    }

    /// Persist state if interval has elapsed.
    async fn maybe_persist(&self) -> ExecutorResult<()> {
        if self.store.is_none() {
            return Ok(());
        }

        let should_persist = if let Some(interval) = self.config.persist_interval {
            let last = self.last_persist.read().await;
            match *last {
                None => true,
                Some(t) => t.elapsed() >= interval,
            }
        } else {
            false
        };

        if should_persist {
            self.force_persist().await?;
        }

        Ok(())
    }

    /// Force persist state.
    async fn force_persist(&self) -> ExecutorResult<()> {
        if let Some(ref store) = self.store {
            let session_id = self.config.session_id.as_deref().unwrap_or("default");
            let dag = self.dag.read().await;
            store.save(session_id, &dag).await?;
            *self.last_persist.write().await = Some(Instant::now());
        }
        Ok(())
    }

    /// Get the current DAG state.
    pub async fn dag(&self) -> TaskDag {
        self.dag.read().await.clone()
    }
}

/// Trait for task storage backends.
#[async_trait::async_trait]
pub trait TaskStore {
    /// Save the DAG state.
    async fn save(&self, id: &str, dag: &TaskDag) -> PersistenceResult<()>;
    /// Load the DAG state.
    async fn load(&self, id: &str) -> PersistenceResult<TaskDag>;
    /// Check if a DAG exists.
    async fn exists(&self, id: &str) -> bool;
}

#[async_trait::async_trait]
impl TaskStore for InMemoryDagStore {
    async fn save(&self, id: &str, dag: &TaskDag) -> PersistenceResult<()> {
        InMemoryDagStore::save(self, id, dag).await
    }

    async fn load(&self, id: &str) -> PersistenceResult<TaskDag> {
        InMemoryDagStore::load(self, id).await
    }

    async fn exists(&self, id: &str) -> bool {
        InMemoryDagStore::exists(self, id).await
    }
}

#[async_trait::async_trait]
impl TaskStore for DagStore {
    async fn save(&self, id: &str, dag: &TaskDag) -> PersistenceResult<()> {
        DagStore::save(self, id, dag).await
    }

    async fn load(&self, id: &str) -> PersistenceResult<TaskDag> {
        DagStore::load(self, id).await
    }

    async fn exists(&self, id: &str) -> bool {
        DagStore::exists(self, id)
    }
}

/// Execute tasks with a simple function.
///
/// Convenience function for simple use cases.
///
/// # Example
///
/// ```rust,ignore
/// use cortex_agents::task::{TaskDag, Task, execute_dag};
///
/// let mut dag = TaskDag::new();
/// dag.add_task(Task::new("task1", "Do something"));
///
/// let results = execute_dag(dag, |task| async move {
///     println!("Running {}", task.name);
///     Ok(Some("Done".to_string()))
/// }).await?;
/// ```
pub async fn execute_dag<F, Fut>(
    dag: TaskDag,
    execute_fn: F,
) -> ExecutorResult<Vec<TaskExecutionResult>>
where
    F: Fn(Task) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = Result<Option<String>, String>> + Send + 'static,
{
    let mut executor = TaskExecutor::new(dag, ExecutorConfig::default());
    executor.execute_all(execute_fn).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::DagBuilder;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn test_execute_simple_dag() {
        let dag = DagBuilder::new()
            .add_task("task1", "First task")
            .add_task("task2", "Second task")
            .depends_on("task1")
            .build()
            .unwrap();

        let executed = Arc::new(AtomicUsize::new(0));
        let executed_clone = executed.clone();

        let results = execute_dag(dag, move |_task| {
            let executed = executed_clone.clone();
            async move {
                executed.fetch_add(1, Ordering::SeqCst);
                Ok(Some("Done".to_string()))
            }
        })
        .await
        .unwrap();

        assert_eq!(results.len(), 2);
        assert_eq!(executed.load(Ordering::SeqCst), 2);
        assert!(results.iter().all(|r| r.success));
    }

    #[tokio::test]
    async fn test_parallel_execution() {
        let dag = DagBuilder::new()
            .add_task("a", "Task A")
            .add_task("b", "Task B")
            .add_task("c", "Task C")
            .build()
            .unwrap();

        let config = ExecutorConfig::default().with_max_parallel(3);
        let mut executor = TaskExecutor::new(dag, config);

        let max_concurrent = Arc::new(AtomicUsize::new(0));
        let current_concurrent = Arc::new(AtomicUsize::new(0));
        let max_clone = max_concurrent.clone();
        let current_clone = current_concurrent.clone();

        let results = executor
            .execute_all(move |_task| {
                let max = max_clone.clone();
                let current = current_clone.clone();
                async move {
                    let c = current.fetch_add(1, Ordering::SeqCst) + 1;
                    max.fetch_max(c, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    current.fetch_sub(1, Ordering::SeqCst);
                    Ok(Some("Done".to_string()))
                }
            })
            .await
            .unwrap();

        assert_eq!(results.len(), 3);
        // With 3 independent tasks and max_parallel=3, all should run concurrently
        assert!(max_concurrent.load(Ordering::SeqCst) >= 2);
    }

    #[tokio::test]
    async fn test_dependency_ordering() {
        let dag = DagBuilder::new()
            .add_task("first", "First")
            .add_task("second", "Second")
            .depends_on("first")
            .add_task("third", "Third")
            .depends_on("second")
            .build()
            .unwrap();

        let execution_order = Arc::new(RwLock::new(Vec::new()));
        let order_clone = execution_order.clone();

        let config = ExecutorConfig::default().with_max_parallel(1);
        let mut executor = TaskExecutor::new(dag, config);

        executor
            .execute_all(move |task| {
                let order = order_clone.clone();
                async move {
                    order.write().await.push(task.name.clone());
                    Ok(Some("Done".to_string()))
                }
            })
            .await
            .unwrap();

        let order = execution_order.read().await;
        assert_eq!(*order, vec!["first", "second", "third"]);
    }

    #[tokio::test]
    async fn test_fail_fast() {
        let dag = DagBuilder::new()
            .add_task("fail", "Will fail")
            .add_task("success", "Will succeed")
            .build()
            .unwrap();

        let config = ExecutorConfig::default()
            .with_fail_fast(true)
            .with_max_parallel(1);
        let mut executor = TaskExecutor::new(dag, config);

        let result = executor
            .execute_all(move |task| async move {
                if task.name == "fail" {
                    Err("Intentional failure".to_string())
                } else {
                    Ok(Some("Done".to_string()))
                }
            })
            .await;

        assert!(matches!(result, Err(ExecutorError::TaskFailed { .. })));
    }

    #[tokio::test]
    async fn test_continue_on_failure() {
        let dag = DagBuilder::new()
            .add_task("fail", "Will fail")
            .add_task("success", "Will succeed")
            .build()
            .unwrap();

        let config = ExecutorConfig::default()
            .with_fail_fast(false)
            .with_max_parallel(1);
        let mut executor = TaskExecutor::new(dag, config);

        let executed = Arc::new(AtomicUsize::new(0));
        let executed_clone = executed.clone();

        let results = executor
            .execute_all(move |task| {
                let executed = executed_clone.clone();
                async move {
                    executed.fetch_add(1, Ordering::SeqCst);
                    if task.name == "fail" {
                        Err("Intentional failure".to_string())
                    } else {
                        Ok(Some("Done".to_string()))
                    }
                }
            })
            .await
            .unwrap();

        // Both tasks should have been attempted
        assert_eq!(executed.load(Ordering::SeqCst), 2);
        assert_eq!(results.len(), 2);
        assert!(results.iter().filter(|r| r.success).count() == 1);
        assert!(results.iter().filter(|r| !r.success).count() == 1);
    }

    #[tokio::test]
    async fn test_task_timeout() {
        let dag = DagBuilder::new()
            .add_task("slow", "Slow task")
            .build()
            .unwrap();

        let config = ExecutorConfig::default().with_task_timeout(Duration::from_millis(50));
        let mut executor = TaskExecutor::new(dag, config);

        let result = executor
            .execute_all(|_task| async {
                tokio::time::sleep(Duration::from_secs(10)).await;
                Ok(Some("Done".to_string()))
            })
            .await;

        assert!(matches!(result, Err(ExecutorError::TaskFailed { .. })));
    }

    #[tokio::test]
    async fn test_file_conflict_detection() {
        let dag = {
            let mut dag = TaskDag::new();
            dag.add_task(
                Task::new("writer1", "Writer 1")
                    .with_affected_files(vec!["shared.txt".to_string()]),
            );
            dag.add_task(
                Task::new("writer2", "Writer 2")
                    .with_affected_files(vec!["shared.txt".to_string()]),
            );
            dag
        };

        let config = ExecutorConfig::default()
            .with_max_parallel(2)
            .with_file_conflict_check(true);
        let mut executor = TaskExecutor::new(dag, config);

        let result = executor
            .execute_all(|_task| async { Ok(Some("Done".to_string())) })
            .await;

        assert!(matches!(result, Err(ExecutorError::FileConflict { .. })));
    }

    #[tokio::test]
    async fn test_cancellation() {
        let dag = DagBuilder::new()
            .add_task("task", "Long task")
            .build()
            .unwrap();

        let config = ExecutorConfig::default();
        let mut executor = TaskExecutor::new(dag, config);

        // Cancel before starting
        executor.cancel();

        let result = executor
            .execute_all(|_task| async { Ok(Some("Done".to_string())) })
            .await;

        assert!(matches!(result, Err(ExecutorError::Cancelled)));
    }

    #[tokio::test]
    async fn test_progress_tracking() {
        let dag = DagBuilder::new()
            .add_task("task1", "Task 1")
            .add_task("task2", "Task 2")
            .build()
            .unwrap();

        let config = ExecutorConfig::default();
        let executor = TaskExecutor::new(dag, config);

        let progress = executor.progress().await;
        assert_eq!(progress.total, 2);
        assert_eq!(progress.pending, 2);
        assert_eq!(progress.percentage(), 0.0);
    }

    #[tokio::test]
    async fn test_with_persistence() {
        let store = InMemoryDagStore::new();

        let dag = DagBuilder::new()
            .add_task("task1", "Task 1")
            .build()
            .unwrap();

        let config = ExecutorConfig::default()
            .with_session_id("test-session")
            .with_max_parallel(1);

        let mut executor = TaskExecutor::with_store(dag, config, store.clone());

        executor
            .execute_all(|_task| async { Ok(Some("Done".to_string())) })
            .await
            .unwrap();

        // Verify state was persisted
        assert!(store.exists("test-session").await);
        let loaded = store.load("test-session").await.unwrap();
        assert!(loaded.all_succeeded());
    }
}
