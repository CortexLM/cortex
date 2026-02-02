//! Task management system with DAG-based dependency tracking.
//!
//! This module provides a comprehensive task management system for
//! orchestrating multi-agent workflows with proper dependency handling.
//!
//! # Features
//!
//! - **DAG Structure**: Tasks organized in a directed acyclic graph
//! - **Dependency Tracking**: Automatic handling of task dependencies
//! - **Status Management**: Lifecycle tracking for all tasks
//! - **Persistence**: Save and restore DAG state
//! - **Hydration**: Create DAGs from various specification formats
//! - **Parallel Execution**: Execute independent tasks concurrently
//! - **Session Recovery**: Restore and continue from previous sessions
//!
//! # Example
//!
//! ```rust
//! use cortex_agents::task::{TaskDag, Task, TaskStatus, DagBuilder, TaskSpec};
//!
//! // Create a DAG using the builder
//! let dag = DagBuilder::new()
//!     .add_task("setup", "Initialize environment")
//!     .add_task("build", "Build the project")
//!     .depends_on("setup")
//!     .add_task("test", "Run tests")
//!     .depends_on("build")
//!     .build()
//!     .unwrap();
//!
//! // Get ready tasks
//! let ready = dag.get_ready_tasks();
//! assert_eq!(ready[0].name, "setup");
//! ```
//!
//! # Workflow
//!
//! 1. Create tasks with dependencies
//! 2. Get ready tasks (no unsatisfied dependencies)
//! 3. Assign tasks to agents
//! 4. Mark tasks as completed/failed
//! 5. Dependent tasks become ready automatically
//!
//! # Parallel Execution
//!
//! The executor runs independent tasks in parallel:
//!
//! ```rust,ignore
//! use cortex_agents::task::{TaskDag, TaskExecutor, ExecutorConfig, execute_dag};
//!
//! // Simple execution
//! let results = execute_dag(dag, |task| async move {
//!     println!("Executing: {}", task.name);
//!     Ok(Some("Done".to_string()))
//! }).await?;
//!
//! // With custom configuration
//! let config = ExecutorConfig::default()
//!     .with_max_parallel(4)
//!     .with_fail_fast(true);
//! let mut executor = TaskExecutor::new(dag, config);
//! let results = executor.execute_all(|task| async move {
//!     // Execute task
//!     Ok(Some("Done".to_string()))
//! }).await?;
//! ```
//!
//! # Persistence
//!
//! DAGs can be saved and restored for session recovery:
//!
//! ```rust,ignore
//! use cortex_agents::task::{DagStore, TaskDag};
//!
//! let store = DagStore::new("/path/to/store");
//!
//! // Save
//! store.save("session-123", &dag).await?;
//!
//! // Load
//! let restored = store.load("session-123").await?;
//! ```
//!
//! # Session Restoration
//!
//! Resume work from a previous session with stale task detection:
//!
//! ```rust,ignore
//! use cortex_agents::task::{SessionHydrator, StaleTaskChecker};
//!
//! let hydrator = SessionHydrator::new(store)
//!     .with_stale_check("/workspace");
//!
//! // Restore previous session, reset running tasks, check for stale tasks
//! let dag = hydrator.restore_session("old-session", Some("new-session")).await?;
//! ```

pub mod dag;
pub mod executor;
pub mod hydration;
pub mod persistence;

// Re-export commonly used types
pub use dag::{DagError, DagResult, Task, TaskDag, TaskId, TaskStatus};
pub use executor::{
    execute_dag, ExecutionProgress, ExecutorConfig, ExecutorError, ExecutorResult,
    TaskExecutionResult, TaskExecutor, TaskStore,
};
pub use hydration::{
    DagBuilder, DagHydrator, SessionHydrationError, SessionHydrationResult, SessionHydrator,
    SessionRestoreConfig, StaleTaskChecker, StaleTaskInfo, TaskSpec,
};
pub use persistence::{DagStore, InMemoryDagStore, PersistenceError, SerializedDag};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_complete_workflow() {
        // Create a DAG representing a build pipeline
        let mut dag = DagBuilder::new()
            .task(TaskSpec::new("checkout", "Checkout code").with_priority(10))
            .task(TaskSpec::new("install", "Install dependencies").depends_on("checkout"))
            .task(TaskSpec::new("lint", "Run linter").depends_on("install"))
            .task(TaskSpec::new("test", "Run tests").depends_on("install"))
            .task(
                TaskSpec::new("build", "Build project")
                    .depends_on("lint")
                    .depends_on("test"),
            )
            .task(TaskSpec::new("deploy", "Deploy").depends_on("build"))
            .build()
            .unwrap();

        // Initially only checkout is ready
        assert_eq!(dag.get_ready_tasks().len(), 1);
        assert_eq!(dag.get_ready_tasks()[0].name, "checkout");

        // Simulate execution
        let checkout_id = dag
            .all_tasks()
            .find(|t| t.name == "checkout")
            .unwrap()
            .id
            .unwrap();

        dag.start_task(checkout_id, Some("agent-1".to_string()))
            .unwrap();
        dag.complete_task(checkout_id, Some("done".to_string()))
            .unwrap();

        // Now install should be ready
        let ready = dag.get_ready_tasks();
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].name, "install");

        // Complete install
        let install_id = dag
            .all_tasks()
            .find(|t| t.name == "install")
            .unwrap()
            .id
            .unwrap();
        dag.start_task(install_id, None).unwrap();
        dag.complete_task(install_id, None).unwrap();

        // Both lint and test should be ready (parallel)
        let ready = dag.get_ready_tasks();
        assert_eq!(ready.len(), 2);
        let ready_names: Vec<_> = ready.iter().map(|t| t.name.as_str()).collect();
        assert!(ready_names.contains(&"lint"));
        assert!(ready_names.contains(&"test"));
    }

    #[test]
    fn test_failure_propagation() {
        let mut dag = DagBuilder::new()
            .add_task("a", "Task A")
            .add_task("b", "Task B")
            .depends_on("a")
            .add_task("c", "Task C")
            .depends_on("b")
            .build()
            .unwrap();

        let a_id = dag.all_tasks().find(|t| t.name == "a").unwrap().id.unwrap();

        // Fail task A
        dag.start_task(a_id, None).unwrap();
        dag.fail_task(a_id, "Error".to_string()).unwrap();

        // Both B and C should be skipped
        for task in dag.all_tasks() {
            if task.name != "a" {
                assert_eq!(task.status, TaskStatus::Skipped);
            }
        }
    }

    #[tokio::test]
    async fn test_persistence_roundtrip() {
        let store = InMemoryDagStore::new();

        let mut dag = DagBuilder::new()
            .add_task("task1", "First task")
            .add_task("task2", "Second task")
            .depends_on("task1")
            .build()
            .unwrap();

        // Complete first task
        let t1_id = dag
            .all_tasks()
            .find(|t| t.name == "task1")
            .unwrap()
            .id
            .unwrap();
        dag.start_task(t1_id, None).unwrap();
        dag.complete_task(t1_id, Some("result".to_string()))
            .unwrap();

        // Save
        store.save("test-session", &dag).await.unwrap();

        // Load
        let restored = store.load("test-session").await.unwrap();

        // Verify state was preserved
        assert_eq!(restored.len(), 2);

        let restored_t1 = restored.all_tasks().find(|t| t.name == "task1").unwrap();
        assert_eq!(restored_t1.status, TaskStatus::Completed);

        // task2 should be ready now
        let ready = restored.get_ready_tasks();
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].name, "task2");
    }
}
