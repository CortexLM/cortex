//! DAG scheduler for coordinating task execution.

use anyhow::Result;
use cortex_agents::task::{Task, TaskDag, TaskStatus};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock, Semaphore};

use crate::styled_output::{print_error, print_info, print_success};

use super::executor::TaskExecutor;
use super::types::{DagExecutionStats, FailureMode, TaskExecutionResult};

/// DAG scheduler that handles execution ordering and parallelism.
pub struct DagScheduler {
    pub dag: Arc<RwLock<TaskDag>>,
    executor: Arc<TaskExecutor>,
    max_concurrent: usize,
    failure_mode: FailureMode,
    stats: Arc<Mutex<DagExecutionStats>>,
    quiet: bool,
}

impl DagScheduler {
    pub fn new(
        dag: TaskDag,
        max_concurrent: usize,
        timeout_secs: u64,
        failure_mode: FailureMode,
        verbose: bool,
        quiet: bool,
    ) -> Self {
        let total_tasks = dag.len();
        Self {
            dag: Arc::new(RwLock::new(dag)),
            executor: Arc::new(TaskExecutor::new(timeout_secs, verbose)),
            max_concurrent,
            failure_mode,
            stats: Arc::new(Mutex::new(DagExecutionStats {
                total_tasks,
                ..Default::default()
            })),
            quiet,
        }
    }

    /// Execute the DAG with the configured strategy.
    pub async fn execute(&self) -> Result<DagExecutionStats> {
        let start = Instant::now();

        // Validate DAG has no cycles
        {
            let dag = self.dag.read().await;
            dag.topological_sort()
                .map_err(|e| anyhow::anyhow!("DAG validation failed: {}", e))?;
        }

        // Execute tasks
        self.run_parallel().await?;

        // Finalize stats
        let mut stats = self.stats.lock().await;
        stats.total_duration = start.elapsed();
        Ok(stats.clone())
    }

    /// Execute tasks in parallel with dependency awareness.
    async fn run_parallel(&self) -> Result<()> {
        let semaphore = Arc::new(Semaphore::new(self.max_concurrent));
        let mut handles: Vec<tokio::task::JoinHandle<Result<TaskExecutionResult>>> = Vec::new();
        let mut should_stop = false;

        loop {
            // Check if we should stop due to failure
            if should_stop {
                break;
            }

            // Get ready tasks
            let ready_tasks: Vec<Task> = {
                let dag = self.dag.read().await;
                dag.get_ready_tasks_by_priority()
                    .into_iter()
                    .cloned()
                    .collect()
            };

            if ready_tasks.is_empty() {
                // Wait for any running tasks to complete
                if handles.is_empty() {
                    break;
                }

                // Wait for at least one task to complete
                let (completed, _idx, remaining) = futures::future::select_all(handles).await;
                handles = remaining;

                match completed {
                    Ok(Ok(result)) => {
                        should_stop = self.handle_task_result(result).await?;
                    }
                    Ok(Err(e)) => {
                        print_error(&format!("Task execution error: {}", e));
                        if matches!(self.failure_mode, FailureMode::FailFast) {
                            should_stop = true;
                        }
                    }
                    Err(e) => {
                        print_error(&format!("Task panicked: {}", e));
                        if matches!(self.failure_mode, FailureMode::FailFast) {
                            should_stop = true;
                        }
                    }
                }
                continue;
            }

            // Start ready tasks up to concurrency limit
            for task in ready_tasks {
                let task_id = task.id.expect("Task must have ID");

                // Try to start the task
                {
                    let mut dag = self.dag.write().await;
                    if let Err(e) = dag.start_task(task_id, None) {
                        // Task may have been started by another iteration
                        if !self.quiet {
                            tracing::debug!("Could not start task {}: {}", task.name, e);
                        }
                        continue;
                    }
                }

                if !self.quiet {
                    print_info(&format!("⏳ Starting: {}", task.name));
                }

                let permit = semaphore.clone().acquire_owned().await.unwrap();
                let executor = self.executor.clone();
                let dag = self.dag.clone();
                let stats = self.stats.clone();
                let _failure_mode = self.failure_mode;
                let quiet = self.quiet;

                let handle = tokio::spawn(async move {
                    let result = executor.execute(&task).await;
                    drop(permit); // Release semaphore

                    // Update DAG state
                    let mut dag = dag.write().await;
                    match result.status {
                        TaskStatus::Completed => {
                            dag.complete_task(result.task_id, result.output.clone())
                                .ok();
                            if !quiet {
                                print_success(&format!(
                                    "✓ Completed: {} ({:.2}s)",
                                    result.task_name,
                                    result.duration.as_secs_f64()
                                ));
                            }
                        }
                        TaskStatus::Failed => {
                            dag.fail_task(result.task_id, result.error.clone().unwrap_or_default())
                                .ok();
                            print_error(&format!(
                                "✗ Failed: {} - {}",
                                result.task_name,
                                result.error.as_deref().unwrap_or("Unknown error")
                            ));
                        }
                        _ => {}
                    }

                    // Update stats
                    let mut stats = stats.lock().await;
                    match result.status {
                        TaskStatus::Completed => stats.completed_tasks += 1,
                        TaskStatus::Failed => stats.failed_tasks += 1,
                        TaskStatus::Skipped => stats.skipped_tasks += 1,
                        _ => {}
                    }
                    stats.task_results.push(result.clone());

                    Ok(result)
                });

                handles.push(handle);
            }

            // Small delay to prevent busy-waiting
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        // Wait for remaining tasks
        for handle in handles {
            match handle.await {
                Ok(Ok(result)) => {
                    self.handle_task_result(result).await.ok();
                }
                Ok(Err(e)) => {
                    print_error(&format!("Task error: {}", e));
                }
                Err(e) => {
                    print_error(&format!("Task panicked: {}", e));
                }
            }
        }

        Ok(())
    }

    /// Handle a task result and determine if execution should stop.
    async fn handle_task_result(&self, result: TaskExecutionResult) -> Result<bool> {
        match result.status {
            TaskStatus::Failed => match self.failure_mode {
                FailureMode::FailFast => Ok(true),
                FailureMode::SkipDependents => {
                    // Skip dependents is already handled by the DAG's fail_task method
                    Ok(false)
                }
                FailureMode::Continue => Ok(false),
            },
            _ => Ok(false),
        }
    }

    /// Execute tasks sequentially in topological order.
    pub async fn execute_sequential(&self) -> Result<DagExecutionStats> {
        let start = Instant::now();

        // Get topological order
        let order = {
            let dag = self.dag.read().await;
            dag.topological_sort()
                .map_err(|e| anyhow::anyhow!("DAG validation failed: {}", e))?
        };

        for task_id in order {
            let task = {
                let dag = self.dag.read().await;
                dag.get_task(task_id).cloned()
            };

            let Some(task) = task else {
                continue;
            };

            // Skip if not ready (dependencies failed)
            if task.status != TaskStatus::Ready && task.status != TaskStatus::Pending {
                continue;
            }

            // Check if dependencies are satisfied
            {
                let dag = self.dag.read().await;
                let deps = dag.get_dependencies(task_id);
                if let Some(deps) = deps {
                    let any_failed = deps.iter().any(|&dep_id| {
                        dag.get_task(dep_id)
                            .map(|t| matches!(t.status, TaskStatus::Failed | TaskStatus::Skipped))
                            .unwrap_or(false)
                    });

                    if any_failed {
                        let mut dag_mut = self.dag.write().await;
                        dag_mut.skip_task(task_id).ok();
                        continue;
                    }
                }
            }

            // Start task
            {
                let mut dag = self.dag.write().await;
                dag.start_task(task_id, None).ok();
            }

            if !self.quiet {
                print_info(&format!("⏳ Running: {}", task.name));
            }

            // Execute
            let result = self.executor.execute(&task).await;

            // Update DAG
            {
                let mut dag = self.dag.write().await;
                match result.status {
                    TaskStatus::Completed => {
                        dag.complete_task(result.task_id, result.output.clone())
                            .ok();
                        if !self.quiet {
                            print_success(&format!(
                                "✓ Completed: {} ({:.2}s)",
                                result.task_name,
                                result.duration.as_secs_f64()
                            ));
                        }
                    }
                    TaskStatus::Failed => {
                        dag.fail_task(result.task_id, result.error.clone().unwrap_or_default())
                            .ok();
                        print_error(&format!(
                            "✗ Failed: {} - {}",
                            result.task_name,
                            result.error.as_deref().unwrap_or("Unknown error")
                        ));

                        if matches!(self.failure_mode, FailureMode::FailFast) {
                            break;
                        }
                    }
                    _ => {}
                }
            }

            // Update stats
            {
                let mut stats = self.stats.lock().await;
                match result.status {
                    TaskStatus::Completed => stats.completed_tasks += 1,
                    TaskStatus::Failed => stats.failed_tasks += 1,
                    TaskStatus::Skipped => stats.skipped_tasks += 1,
                    _ => {}
                }
                stats.task_results.push(result);
            }
        }

        // Finalize stats
        let mut stats = self.stats.lock().await;
        stats.total_duration = start.elapsed();
        Ok(stats.clone())
    }
}
