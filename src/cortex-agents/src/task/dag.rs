//! Task DAG (Directed Acyclic Graph) for dependency management.
//!
//! This module provides a DAG structure for managing task dependencies,
//! enabling proper ordering and parallel execution of independent tasks.
//!
//! # Example
//!
//! ```rust
//! use cortex_agents::task::dag::{TaskDag, Task, TaskStatus};
//!
//! let mut dag = TaskDag::new();
//!
//! // Add tasks
//! let t1 = dag.add_task(Task::new("Setup", "Initialize project"));
//! let t2 = dag.add_task(Task::new("Build", "Compile code"));
//! let t3 = dag.add_task(Task::new("Test", "Run tests"));
//!
//! // Add dependencies: Build depends on Setup, Test depends on Build
//! dag.add_dependency(t2, t1).unwrap();
//! dag.add_dependency(t3, t2).unwrap();
//!
//! // Get ready tasks (initially only Setup)
//! let ready = dag.get_ready_tasks();
//! assert_eq!(ready.len(), 1);
//! assert_eq!(ready[0].name, "Setup");
//! ```

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use thiserror::Error;

/// Unique identifier for a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TaskId(u64);

impl TaskId {
    /// Create a new task ID.
    pub fn new(id: u64) -> Self {
        Self(id)
    }

    /// Get the inner ID value.
    pub fn inner(&self) -> u64 {
        self.0
    }
}

impl std::fmt::Display for TaskId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "task-{}", self.0)
    }
}

/// Status of a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum TaskStatus {
    /// Task is waiting for dependencies.
    #[default]
    Pending,
    /// Task dependencies are satisfied, ready to run.
    Ready,
    /// Task is currently running.
    Running,
    /// Task completed successfully.
    Completed,
    /// Task failed.
    Failed,
    /// Task was skipped (e.g., dependency failed).
    Skipped,
    /// Task was cancelled.
    Cancelled,
}

impl TaskStatus {
    /// Check if task is in a terminal state.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            TaskStatus::Completed
                | TaskStatus::Failed
                | TaskStatus::Skipped
                | TaskStatus::Cancelled
        )
    }

    /// Check if task completed successfully.
    pub fn is_success(&self) -> bool {
        matches!(self, TaskStatus::Completed)
    }

    /// Check if task can be started.
    pub fn can_start(&self) -> bool {
        matches!(self, TaskStatus::Ready)
    }
}

/// A task in the DAG.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    /// Task ID (assigned when added to DAG).
    pub id: Option<TaskId>,
    /// Task name/identifier.
    pub name: String,
    /// Task description.
    pub description: String,
    /// Current status.
    pub status: TaskStatus,
    /// Files this task will modify.
    pub affected_files: Vec<String>,
    /// Agent ID if assigned.
    pub agent_id: Option<String>,
    /// Result/output of the task.
    pub result: Option<String>,
    /// Error message if failed.
    pub error: Option<String>,
    /// Priority (higher = more important).
    pub priority: i32,
    /// Estimated duration in seconds.
    pub estimated_duration: Option<u64>,
    /// Actual duration in seconds.
    pub actual_duration: Option<u64>,
    /// Custom metadata.
    pub metadata: HashMap<String, serde_json::Value>,
}

impl Task {
    /// Create a new task.
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            id: None,
            name: name.into(),
            description: description.into(),
            status: TaskStatus::Pending,
            affected_files: Vec::new(),
            agent_id: None,
            result: None,
            error: None,
            priority: 0,
            estimated_duration: None,
            actual_duration: None,
            metadata: HashMap::new(),
        }
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

    /// Add metadata.
    pub fn with_metadata(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.metadata.insert(key.into(), value);
        self
    }
}

/// Errors for DAG operations.
#[derive(Debug, Error)]
pub enum DagError {
    /// Task not found.
    #[error("Task not found: {0}")]
    TaskNotFound(TaskId),

    /// Adding dependency would create a cycle.
    #[error("Dependency would create a cycle: {0} -> {1}")]
    CycleDetected(TaskId, TaskId),

    /// Task is already in a terminal state.
    #[error("Task {0} is already in terminal state: {1:?}")]
    TaskTerminal(TaskId, TaskStatus),

    /// Invalid state transition.
    #[error("Invalid state transition for task {0}: {1:?} -> {2:?}")]
    InvalidTransition(TaskId, TaskStatus, TaskStatus),

    /// Dependency not satisfied.
    #[error("Dependency {dependency} not satisfied for task {task}")]
    DependencyNotSatisfied { task: TaskId, dependency: TaskId },
}

/// Result type for DAG operations.
pub type DagResult<T> = std::result::Result<T, DagError>;

/// A DAG of tasks with dependency tracking.
#[derive(Debug, Clone, Default)]
pub struct TaskDag {
    /// All tasks.
    tasks: HashMap<TaskId, Task>,
    /// Dependencies: task -> set of tasks it depends on.
    dependencies: HashMap<TaskId, HashSet<TaskId>>,
    /// Reverse dependencies: task -> set of tasks that depend on it.
    dependents: HashMap<TaskId, HashSet<TaskId>>,
    /// Next task ID.
    next_id: u64,
}

impl TaskDag {
    /// Create a new empty DAG.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a task to the DAG.
    pub fn add_task(&mut self, mut task: Task) -> TaskId {
        let id = TaskId::new(self.next_id);
        self.next_id += 1;

        task.id = Some(id);

        // If no dependencies, task is ready
        if !self.dependencies.contains_key(&id) || self.dependencies[&id].is_empty() {
            task.status = TaskStatus::Ready;
        }

        self.tasks.insert(id, task);
        self.dependencies.entry(id).or_default();
        self.dependents.entry(id).or_default();

        id
    }

    /// Add a dependency between tasks.
    ///
    /// `task` depends on `depends_on`, meaning `depends_on` must complete
    /// before `task` can start.
    pub fn add_dependency(&mut self, task: TaskId, depends_on: TaskId) -> DagResult<()> {
        // Verify both tasks exist
        if !self.tasks.contains_key(&task) {
            return Err(DagError::TaskNotFound(task));
        }
        if !self.tasks.contains_key(&depends_on) {
            return Err(DagError::TaskNotFound(depends_on));
        }

        // Check for cycles
        if self.would_create_cycle(task, depends_on) {
            return Err(DagError::CycleDetected(task, depends_on));
        }

        // Add the dependency
        self.dependencies
            .entry(task)
            .or_default()
            .insert(depends_on);
        self.dependents.entry(depends_on).or_default().insert(task);

        // Update task status to pending if it has unsatisfied dependencies
        let should_update = {
            if let Some(t) = self.tasks.get(&task) {
                !self.are_dependencies_satisfied(task) && t.status == TaskStatus::Ready
            } else {
                false
            }
        };
        if should_update {
            if let Some(t) = self.tasks.get_mut(&task) {
                t.status = TaskStatus::Pending;
            }
        }

        Ok(())
    }

    /// Check if adding a dependency would create a cycle.
    fn would_create_cycle(&self, task: TaskId, depends_on: TaskId) -> bool {
        // If task == depends_on, that's a self-cycle
        if task == depends_on {
            return true;
        }

        // BFS from depends_on to see if we can reach task
        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();
        queue.push_back(depends_on);

        while let Some(current) = queue.pop_front() {
            if current == task {
                return true;
            }
            if visited.insert(current) {
                if let Some(deps) = self.dependencies.get(&current) {
                    for &dep in deps {
                        queue.push_back(dep);
                    }
                }
            }
        }

        false
    }

    /// Check if all dependencies of a task are satisfied (completed).
    fn are_dependencies_satisfied(&self, task: TaskId) -> bool {
        if let Some(deps) = self.dependencies.get(&task) {
            for &dep in deps {
                if let Some(dep_task) = self.tasks.get(&dep) {
                    if !dep_task.status.is_success() {
                        return false;
                    }
                } else {
                    return false;
                }
            }
        }
        true
    }

    /// Get all tasks that are ready to run.
    pub fn get_ready_tasks(&self) -> Vec<&Task> {
        self.tasks
            .values()
            .filter(|t| t.status == TaskStatus::Ready)
            .collect()
    }

    /// Get all tasks that are ready to run, sorted by priority.
    pub fn get_ready_tasks_by_priority(&self) -> Vec<&Task> {
        let mut ready: Vec<_> = self.get_ready_tasks();
        ready.sort_by(|a, b| b.priority.cmp(&a.priority));
        ready
    }

    /// Get a task by ID.
    pub fn get_task(&self, id: TaskId) -> Option<&Task> {
        self.tasks.get(&id)
    }

    /// Get a mutable task by ID.
    pub fn get_task_mut(&mut self, id: TaskId) -> Option<&mut Task> {
        self.tasks.get_mut(&id)
    }

    /// Mark a task as running.
    pub fn start_task(&mut self, id: TaskId, agent_id: Option<String>) -> DagResult<()> {
        let task = self.tasks.get_mut(&id).ok_or(DagError::TaskNotFound(id))?;

        if !task.status.can_start() {
            return Err(DagError::InvalidTransition(
                id,
                task.status,
                TaskStatus::Running,
            ));
        }

        task.status = TaskStatus::Running;
        task.agent_id = agent_id;

        Ok(())
    }

    /// Mark a task as completed.
    pub fn complete_task(&mut self, id: TaskId, result: Option<String>) -> DagResult<()> {
        {
            let task = self.tasks.get_mut(&id).ok_or(DagError::TaskNotFound(id))?;

            if task.status.is_terminal() {
                return Err(DagError::TaskTerminal(id, task.status));
            }

            task.status = TaskStatus::Completed;
            task.result = result;
        }

        // Update dependents - they may now be ready
        self.update_dependents(id);

        Ok(())
    }

    /// Mark a task as failed.
    pub fn fail_task(&mut self, id: TaskId, error: String) -> DagResult<()> {
        {
            let task = self.tasks.get_mut(&id).ok_or(DagError::TaskNotFound(id))?;

            if task.status.is_terminal() {
                return Err(DagError::TaskTerminal(id, task.status));
            }

            task.status = TaskStatus::Failed;
            task.error = Some(error);
        }

        // Skip all dependents since this task failed
        self.skip_dependents(id);

        Ok(())
    }

    /// Skip a task.
    pub fn skip_task(&mut self, id: TaskId) -> DagResult<()> {
        {
            let task = self.tasks.get_mut(&id).ok_or(DagError::TaskNotFound(id))?;

            if task.status.is_terminal() {
                return Err(DagError::TaskTerminal(id, task.status));
            }

            task.status = TaskStatus::Skipped;
        }

        self.skip_dependents(id);

        Ok(())
    }

    /// Cancel a task.
    pub fn cancel_task(&mut self, id: TaskId) -> DagResult<()> {
        let task = self.tasks.get_mut(&id).ok_or(DagError::TaskNotFound(id))?;

        if task.status.is_terminal() {
            return Err(DagError::TaskTerminal(id, task.status));
        }

        task.status = TaskStatus::Cancelled;

        Ok(())
    }

    /// Update dependent tasks after a task completes.
    fn update_dependents(&mut self, completed_id: TaskId) {
        if let Some(dependents) = self.dependents.get(&completed_id).cloned() {
            for dep_id in dependents {
                if self.are_dependencies_satisfied(dep_id) {
                    if let Some(task) = self.tasks.get_mut(&dep_id) {
                        if task.status == TaskStatus::Pending {
                            task.status = TaskStatus::Ready;
                        }
                    }
                }
            }
        }
    }

    /// Skip all tasks that depend on a failed/skipped task.
    fn skip_dependents(&mut self, failed_id: TaskId) {
        if let Some(dependents) = self.dependents.get(&failed_id).cloned() {
            for dep_id in dependents {
                if let Some(task) = self.tasks.get_mut(&dep_id) {
                    if !task.status.is_terminal() {
                        task.status = TaskStatus::Skipped;
                        // Recursively skip dependents of this task too
                        self.skip_dependents(dep_id);
                    }
                }
            }
        }
    }

    /// Perform a topological sort of the DAG.
    ///
    /// Returns tasks in an order where dependencies come before dependents.
    pub fn topological_sort(&self) -> DagResult<Vec<TaskId>> {
        let mut result = Vec::new();
        let mut in_degree: HashMap<TaskId, usize> = HashMap::new();
        let mut queue = VecDeque::new();

        // Calculate in-degrees
        for &id in self.tasks.keys() {
            let deps = self.dependencies.get(&id).map(|d| d.len()).unwrap_or(0);
            in_degree.insert(id, deps);
            if deps == 0 {
                queue.push_back(id);
            }
        }

        // Process nodes with zero in-degree
        while let Some(id) = queue.pop_front() {
            result.push(id);

            if let Some(dependents) = self.dependents.get(&id) {
                for &dep_id in dependents {
                    if let Some(degree) = in_degree.get_mut(&dep_id) {
                        *degree -= 1;
                        if *degree == 0 {
                            queue.push_back(dep_id);
                        }
                    }
                }
            }
        }

        // If we didn't process all nodes, there's a cycle
        if result.len() != self.tasks.len() {
            // Find a task involved in the cycle for error reporting
            for (&id, &degree) in &in_degree {
                if degree > 0 {
                    // Find one of its dependencies that's also in a cycle
                    if let Some(deps) = self.dependencies.get(&id) {
                        for &dep in deps {
                            if in_degree.get(&dep).copied().unwrap_or(0) > 0 {
                                return Err(DagError::CycleDetected(id, dep));
                            }
                        }
                    }
                }
            }
        }

        Ok(result)
    }

    /// Get all tasks.
    pub fn all_tasks(&self) -> impl Iterator<Item = &Task> {
        self.tasks.values()
    }

    /// Get task count.
    pub fn len(&self) -> usize {
        self.tasks.len()
    }

    /// Check if DAG is empty.
    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }

    /// Get count of tasks by status.
    pub fn status_counts(&self) -> HashMap<TaskStatus, usize> {
        let mut counts = HashMap::new();
        for task in self.tasks.values() {
            *counts.entry(task.status).or_insert(0) += 1;
        }
        counts
    }

    /// Check if all tasks are complete (in terminal state).
    pub fn is_complete(&self) -> bool {
        self.tasks.values().all(|t| t.status.is_terminal())
    }

    /// Check if all tasks succeeded.
    pub fn all_succeeded(&self) -> bool {
        self.tasks.values().all(|t| t.status.is_success())
    }

    /// Get the dependencies of a task.
    pub fn get_dependencies(&self, id: TaskId) -> Option<&HashSet<TaskId>> {
        self.dependencies.get(&id)
    }

    /// Get the dependents of a task.
    pub fn get_dependents(&self, id: TaskId) -> Option<&HashSet<TaskId>> {
        self.dependents.get(&id)
    }

    /// Remove a task from the DAG.
    pub fn remove_task(&mut self, id: TaskId) -> DagResult<Task> {
        // Check if any tasks depend on this one
        if let Some(dependents) = self.dependents.get(&id) {
            if !dependents.is_empty() {
                // Can't remove a task that has dependents
                return Err(DagError::DependencyNotSatisfied {
                    task: *dependents.iter().next().unwrap(),
                    dependency: id,
                });
            }
        }

        // Remove from dependencies of other tasks
        if let Some(deps) = self.dependencies.remove(&id) {
            for dep in deps {
                if let Some(dep_dependents) = self.dependents.get_mut(&dep) {
                    dep_dependents.remove(&id);
                }
            }
        }

        // Remove dependents entry
        self.dependents.remove(&id);

        // Remove the task
        self.tasks.remove(&id).ok_or(DagError::TaskNotFound(id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_dag() {
        let dag = TaskDag::new();
        assert!(dag.is_empty());
    }

    #[test]
    fn test_add_task() {
        let mut dag = TaskDag::new();
        let id = dag.add_task(Task::new("Test", "A test task"));

        assert_eq!(dag.len(), 1);
        let task = dag.get_task(id).unwrap();
        assert_eq!(task.name, "Test");
        // No dependencies, should be ready
        assert_eq!(task.status, TaskStatus::Ready);
    }

    #[test]
    fn test_add_dependency() {
        let mut dag = TaskDag::new();
        let t1 = dag.add_task(Task::new("First", "First task"));
        let t2 = dag.add_task(Task::new("Second", "Second task"));

        dag.add_dependency(t2, t1).unwrap();

        // t1 should be ready (no dependencies)
        assert_eq!(dag.get_task(t1).unwrap().status, TaskStatus::Ready);
        // t2 should be pending (depends on t1)
        assert_eq!(dag.get_task(t2).unwrap().status, TaskStatus::Pending);
    }

    #[test]
    fn test_cycle_detection() {
        let mut dag = TaskDag::new();
        let t1 = dag.add_task(Task::new("A", "Task A"));
        let t2 = dag.add_task(Task::new("B", "Task B"));
        let t3 = dag.add_task(Task::new("C", "Task C"));

        dag.add_dependency(t2, t1).unwrap();
        dag.add_dependency(t3, t2).unwrap();

        // This would create a cycle: t1 -> t2 -> t3 -> t1
        let result = dag.add_dependency(t1, t3);
        assert!(matches!(result, Err(DagError::CycleDetected(_, _))));
    }

    #[test]
    fn test_self_cycle_detection() {
        let mut dag = TaskDag::new();
        let t1 = dag.add_task(Task::new("Self", "Self-referencing task"));

        let result = dag.add_dependency(t1, t1);
        assert!(matches!(result, Err(DagError::CycleDetected(_, _))));
    }

    #[test]
    fn test_get_ready_tasks() {
        let mut dag = TaskDag::new();
        let t1 = dag.add_task(Task::new("Independent 1", "No deps"));
        let t2 = dag.add_task(Task::new("Independent 2", "No deps"));
        let t3 = dag.add_task(Task::new("Dependent", "Has deps"));

        dag.add_dependency(t3, t1).unwrap();
        dag.add_dependency(t3, t2).unwrap();

        let ready = dag.get_ready_tasks();
        assert_eq!(ready.len(), 2);

        let ready_names: HashSet<_> = ready.iter().map(|t| t.name.as_str()).collect();
        assert!(ready_names.contains("Independent 1"));
        assert!(ready_names.contains("Independent 2"));
    }

    #[test]
    fn test_complete_task_updates_dependents() {
        let mut dag = TaskDag::new();
        let t1 = dag.add_task(Task::new("First", "First"));
        let t2 = dag.add_task(Task::new("Second", "Second"));

        dag.add_dependency(t2, t1).unwrap();

        // t2 should be pending
        assert_eq!(dag.get_task(t2).unwrap().status, TaskStatus::Pending);

        // Start and complete t1
        dag.start_task(t1, None).unwrap();
        dag.complete_task(t1, Some("Done".to_string())).unwrap();

        // t2 should now be ready
        assert_eq!(dag.get_task(t2).unwrap().status, TaskStatus::Ready);
    }

    #[test]
    fn test_fail_task_skips_dependents() {
        let mut dag = TaskDag::new();
        let t1 = dag.add_task(Task::new("First", "First"));
        let t2 = dag.add_task(Task::new("Second", "Second"));
        let t3 = dag.add_task(Task::new("Third", "Third"));

        dag.add_dependency(t2, t1).unwrap();
        dag.add_dependency(t3, t2).unwrap();

        // Fail t1
        dag.start_task(t1, None).unwrap();
        dag.fail_task(t1, "Error".to_string()).unwrap();

        // t2 and t3 should be skipped
        assert_eq!(dag.get_task(t2).unwrap().status, TaskStatus::Skipped);
        assert_eq!(dag.get_task(t3).unwrap().status, TaskStatus::Skipped);
    }

    #[test]
    fn test_topological_sort() {
        let mut dag = TaskDag::new();
        let t1 = dag.add_task(Task::new("A", "First"));
        let t2 = dag.add_task(Task::new("B", "Second"));
        let t3 = dag.add_task(Task::new("C", "Third"));

        dag.add_dependency(t2, t1).unwrap();
        dag.add_dependency(t3, t2).unwrap();

        let sorted = dag.topological_sort().unwrap();

        // t1 must come before t2, t2 must come before t3
        let pos: HashMap<_, _> = sorted.iter().enumerate().map(|(i, &id)| (id, i)).collect();
        assert!(pos[&t1] < pos[&t2]);
        assert!(pos[&t2] < pos[&t3]);
    }

    #[test]
    fn test_priority_ordering() {
        let mut dag = TaskDag::new();
        dag.add_task(Task::new("Low", "Low priority").with_priority(1));
        dag.add_task(Task::new("High", "High priority").with_priority(10));
        dag.add_task(Task::new("Medium", "Medium priority").with_priority(5));

        let ready = dag.get_ready_tasks_by_priority();
        assert_eq!(ready[0].name, "High");
        assert_eq!(ready[1].name, "Medium");
        assert_eq!(ready[2].name, "Low");
    }

    #[test]
    fn test_status_counts() {
        let mut dag = TaskDag::new();
        let t1 = dag.add_task(Task::new("A", "A"));
        let t2 = dag.add_task(Task::new("B", "B"));
        let t3 = dag.add_task(Task::new("C", "C"));

        dag.add_dependency(t3, t1).unwrap();

        dag.start_task(t1, None).unwrap();
        dag.complete_task(t1, None).unwrap();
        dag.start_task(t2, None).unwrap();

        let counts = dag.status_counts();
        assert_eq!(counts.get(&TaskStatus::Completed), Some(&1));
        assert_eq!(counts.get(&TaskStatus::Running), Some(&1));
        assert_eq!(counts.get(&TaskStatus::Ready), Some(&1));
    }

    #[test]
    fn test_is_complete() {
        let mut dag = TaskDag::new();
        let t1 = dag.add_task(Task::new("A", "A"));
        let t2 = dag.add_task(Task::new("B", "B"));

        assert!(!dag.is_complete());

        dag.start_task(t1, None).unwrap();
        dag.complete_task(t1, None).unwrap();

        assert!(!dag.is_complete());

        dag.start_task(t2, None).unwrap();
        dag.complete_task(t2, None).unwrap();

        assert!(dag.is_complete());
        assert!(dag.all_succeeded());
    }

    #[test]
    fn test_task_with_metadata() {
        let task = Task::new("Test", "Test task")
            .with_priority(5)
            .with_affected_files(vec!["file1.rs".to_string()])
            .with_estimated_duration(60)
            .with_metadata("custom", serde_json::json!({"key": "value"}));

        assert_eq!(task.priority, 5);
        assert_eq!(task.affected_files, vec!["file1.rs"]);
        assert_eq!(task.estimated_duration, Some(60));
        assert!(task.metadata.contains_key("custom"));
    }
}
