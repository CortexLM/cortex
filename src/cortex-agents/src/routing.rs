//! Routing decision framework for task dispatch.
//!
//! This module provides logic for deciding how to dispatch tasks
//! to agents - whether to run them in parallel, sequentially, or
//! in the background.
//!
//! # Example
//!
//! ```rust
//! use cortex_agents::routing::{RoutingDecision, DispatchMode, decide_routing, TaskInfo};
//!
//! let tasks = vec![
//!     TaskInfo::new("Search for errors").reading(vec!["src/".into()]),
//!     TaskInfo::new("Search for warnings").reading(vec!["src/".into()]),
//!     TaskInfo::new("Search for TODOs").reading(vec!["src/".into()]),
//! ];
//!
//! let decision = decide_routing(&tasks);
//! assert_eq!(decision.mode, DispatchMode::Parallel);
//! ```

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;

/// Mode for dispatching tasks to agents.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum DispatchMode {
    /// Run tasks in parallel.
    /// Best for: 3+ unrelated tasks, no shared state, read-only operations.
    Parallel,

    /// Run tasks sequentially.
    /// Best for: Tasks with dependencies, shared file modifications.
    Sequential,

    /// Run in background without blocking.
    /// Best for: Research/analysis tasks that don't modify files.
    Background,

    /// Run as a single task.
    /// Best for: Single task or tightly coupled operations.
    #[default]
    Single,
}

impl std::fmt::Display for DispatchMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DispatchMode::Parallel => write!(f, "parallel"),
            DispatchMode::Sequential => write!(f, "sequential"),
            DispatchMode::Background => write!(f, "background"),
            DispatchMode::Single => write!(f, "single"),
        }
    }
}

/// Information about a task for routing decisions.
#[derive(Debug, Clone, Default)]
pub struct TaskInfo {
    /// Task description.
    pub description: String,

    /// Files this task will read.
    pub reads: Vec<PathBuf>,

    /// Files this task will write/modify.
    pub writes: Vec<PathBuf>,

    /// Whether this task executes shell commands.
    pub executes_commands: bool,

    /// Whether this task is read-only (no modifications).
    pub read_only: bool,

    /// Whether this task requires user interaction.
    pub interactive: bool,

    /// Estimated duration in seconds.
    pub estimated_duration: Option<u64>,

    /// Priority (higher = more important).
    pub priority: i32,

    /// Tags for categorization.
    pub tags: HashSet<String>,
}

impl TaskInfo {
    /// Create a new task info.
    pub fn new(description: impl Into<String>) -> Self {
        Self {
            description: description.into(),
            ..Default::default()
        }
    }

    /// Set files being read.
    pub fn reading(mut self, paths: Vec<PathBuf>) -> Self {
        self.reads = paths;
        self
    }

    /// Set files being written.
    pub fn writing(mut self, paths: Vec<PathBuf>) -> Self {
        self.writes = paths;
        self
    }

    /// Mark as read-only.
    pub fn read_only(mut self) -> Self {
        self.read_only = true;
        self
    }

    /// Mark as executing commands.
    pub fn with_commands(mut self) -> Self {
        self.executes_commands = true;
        self
    }

    /// Mark as interactive.
    pub fn interactive(mut self) -> Self {
        self.interactive = true;
        self
    }

    /// Set estimated duration.
    pub fn with_duration(mut self, seconds: u64) -> Self {
        self.estimated_duration = Some(seconds);
        self
    }

    /// Set priority.
    pub fn with_priority(mut self, priority: i32) -> Self {
        self.priority = priority;
        self
    }

    /// Add a tag.
    pub fn with_tag(mut self, tag: impl Into<String>) -> Self {
        self.tags.insert(tag.into());
        self
    }

    /// Check if this task has file conflicts with another.
    pub fn has_file_conflict(&self, other: &TaskInfo) -> bool {
        // Check if any of our writes overlap with their reads or writes
        for write in &self.writes {
            for other_write in &other.writes {
                if paths_overlap(write, other_write) {
                    return true;
                }
            }
            for other_read in &other.reads {
                if paths_overlap(write, other_read) {
                    return true;
                }
            }
        }

        // Check if any of their writes overlap with our reads
        for other_write in &other.writes {
            for read in &self.reads {
                if paths_overlap(other_write, read) {
                    return true;
                }
            }
        }

        false
    }
}

/// Check if two paths overlap (one is a prefix of the other or they're equal).
fn paths_overlap(a: &PathBuf, b: &PathBuf) -> bool {
    a == b || a.starts_with(b) || b.starts_with(a)
}

/// A routing decision for a set of tasks.
#[derive(Debug, Clone)]
pub struct RoutingDecision {
    /// Recommended dispatch mode.
    pub mode: DispatchMode,

    /// Reason for the decision.
    pub reason: String,

    /// Files that may be affected.
    pub affected_files: Vec<PathBuf>,

    /// Tasks that should run first (if sequential).
    pub priority_order: Vec<usize>,

    /// Groups of tasks that can run in parallel.
    pub parallel_groups: Vec<Vec<usize>>,

    /// Confidence in the decision (0.0 - 1.0).
    pub confidence: f64,

    /// Warnings or considerations.
    pub warnings: Vec<String>,
}

impl Default for RoutingDecision {
    fn default() -> Self {
        Self {
            mode: DispatchMode::Single,
            reason: "Default single task execution".to_string(),
            affected_files: Vec::new(),
            priority_order: Vec::new(),
            parallel_groups: Vec::new(),
            confidence: 1.0,
            warnings: Vec::new(),
        }
    }
}

impl RoutingDecision {
    /// Create a new decision.
    pub fn new(mode: DispatchMode, reason: impl Into<String>) -> Self {
        Self {
            mode,
            reason: reason.into(),
            ..Default::default()
        }
    }

    /// Add affected files.
    pub fn with_affected_files(mut self, files: Vec<PathBuf>) -> Self {
        self.affected_files = files;
        self
    }

    /// Add a warning.
    pub fn with_warning(mut self, warning: impl Into<String>) -> Self {
        self.warnings.push(warning.into());
        self
    }

    /// Set confidence.
    pub fn with_confidence(mut self, confidence: f64) -> Self {
        self.confidence = confidence.clamp(0.0, 1.0);
        self
    }

    /// Set parallel groups.
    pub fn with_parallel_groups(mut self, groups: Vec<Vec<usize>>) -> Self {
        self.parallel_groups = groups;
        self
    }

    /// Set priority order.
    pub fn with_priority_order(mut self, order: Vec<usize>) -> Self {
        self.priority_order = order;
        self
    }
}

/// Decide on routing for a set of tasks.
///
/// Analyzes the tasks and their interactions to recommend
/// the best dispatch mode.
pub fn decide_routing(tasks: &[TaskInfo]) -> RoutingDecision {
    // Empty or single task - just run it
    if tasks.is_empty() {
        return RoutingDecision::new(DispatchMode::Single, "No tasks to route");
    }

    if tasks.len() == 1 {
        let task = &tasks[0];
        if task.read_only && !task.interactive {
            return RoutingDecision::new(
                DispatchMode::Background,
                "Single read-only task can run in background",
            );
        }
        return RoutingDecision::new(DispatchMode::Single, "Single task");
    }

    // Check for interactive tasks
    if tasks.iter().any(|t| t.interactive) {
        return RoutingDecision::new(
            DispatchMode::Sequential,
            "Interactive tasks require sequential execution",
        )
        .with_warning("Some tasks require user interaction");
    }

    // Analyze file conflicts
    let conflicts = analyze_file_conflicts(tasks);

    // All tasks are read-only with no conflicts
    if tasks.iter().all(|t| t.read_only) && conflicts.is_empty() {
        return RoutingDecision::new(
            DispatchMode::Parallel,
            "All tasks are read-only with no conflicts",
        )
        .with_confidence(0.95)
        .with_parallel_groups(vec![(0..tasks.len()).collect()]);
    }

    // No file conflicts at all
    if conflicts.is_empty() {
        if tasks.len() >= 3 {
            return RoutingDecision::new(
                DispatchMode::Parallel,
                "No file conflicts detected, 3+ tasks can run in parallel",
            )
            .with_confidence(0.9)
            .with_parallel_groups(vec![(0..tasks.len()).collect()]);
        } else {
            return RoutingDecision::new(
                DispatchMode::Parallel,
                "No file conflicts detected, tasks can run in parallel",
            )
            .with_confidence(0.85)
            .with_parallel_groups(vec![(0..tasks.len()).collect()]);
        }
    }

    // Has conflicts - need to be more careful
    let (groups, order) = compute_execution_plan(tasks, &conflicts);

    if groups.len() == 1 && groups[0].len() == tasks.len() {
        // All tasks can still run in parallel despite some reads
        return RoutingDecision::new(
            DispatchMode::Parallel,
            "Tasks have shared reads but no conflicting writes",
        )
        .with_confidence(0.7)
        .with_parallel_groups(groups)
        .with_warning("Tasks read from shared files - verify no race conditions");
    }

    if groups.len() == tasks.len() {
        // Each task is its own group - fully sequential
        return RoutingDecision::new(
            DispatchMode::Sequential,
            "File conflicts require sequential execution",
        )
        .with_confidence(0.9)
        .with_priority_order(order)
        .with_affected_files(collect_affected_files(tasks));
    }

    // Mixed - some parallel groups
    RoutingDecision::new(
        DispatchMode::Sequential,
        "Some tasks have conflicts, using sequential with parallel subgroups",
    )
    .with_confidence(0.8)
    .with_parallel_groups(groups)
    .with_priority_order(order)
    .with_affected_files(collect_affected_files(tasks))
}

/// Analyze file conflicts between tasks.
/// Returns pairs of task indices that have conflicts.
fn analyze_file_conflicts(tasks: &[TaskInfo]) -> Vec<(usize, usize)> {
    let mut conflicts = Vec::new();

    for i in 0..tasks.len() {
        for j in (i + 1)..tasks.len() {
            if tasks[i].has_file_conflict(&tasks[j]) {
                conflicts.push((i, j));
            }
        }
    }

    conflicts
}

/// Compute an execution plan based on conflicts.
/// Returns (parallel groups, execution order).
fn compute_execution_plan(
    tasks: &[TaskInfo],
    conflicts: &[(usize, usize)],
) -> (Vec<Vec<usize>>, Vec<usize>) {
    let n = tasks.len();

    // Build conflict graph
    let mut has_conflict = vec![vec![false; n]; n];
    for &(i, j) in conflicts {
        has_conflict[i][j] = true;
        has_conflict[j][i] = true;
    }

    // Greedy coloring to find parallel groups
    let mut colors = vec![None; n];
    let mut groups: Vec<Vec<usize>> = Vec::new();

    // Sort tasks by priority (descending) for better ordering
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| tasks[b].priority.cmp(&tasks[a].priority));

    for &task_idx in &order {
        // Find the first color that doesn't conflict
        let mut used_colors: HashSet<usize> = HashSet::new();
        for j in 0..n {
            if has_conflict[task_idx][j] {
                if let Some(c) = colors[j] {
                    used_colors.insert(c);
                }
            }
        }

        let color = (0..).find(|c| !used_colors.contains(c)).unwrap();

        colors[task_idx] = Some(color);

        // Add to group
        while groups.len() <= color {
            groups.push(Vec::new());
        }
        groups[color].push(task_idx);
    }

    (groups, order)
}

/// Collect all affected files from tasks.
fn collect_affected_files(tasks: &[TaskInfo]) -> Vec<PathBuf> {
    let mut files: HashSet<PathBuf> = HashSet::new();

    for task in tasks {
        for path in &task.reads {
            files.insert(path.clone());
        }
        for path in &task.writes {
            files.insert(path.clone());
        }
    }

    files.into_iter().collect()
}

/// Quick check if tasks can run in parallel.
pub fn can_parallelize(tasks: &[TaskInfo]) -> bool {
    if tasks.len() < 2 {
        return false;
    }

    // Check for any conflicts
    for i in 0..tasks.len() {
        for j in (i + 1)..tasks.len() {
            if tasks[i].has_file_conflict(&tasks[j]) {
                return false;
            }
        }
    }

    true
}

/// Estimate total duration for tasks based on dispatch mode.
pub fn estimate_duration(tasks: &[TaskInfo], mode: DispatchMode) -> Option<u64> {
    let durations: Vec<u64> = tasks.iter().filter_map(|t| t.estimated_duration).collect();

    if durations.is_empty() {
        return None;
    }

    match mode {
        DispatchMode::Sequential | DispatchMode::Single => Some(durations.iter().sum()),
        DispatchMode::Parallel | DispatchMode::Background => durations.iter().max().copied(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_single_task() {
        let tasks = vec![TaskInfo::new("Single task")];
        let decision = decide_routing(&tasks);
        assert_eq!(decision.mode, DispatchMode::Single);
    }

    #[test]
    fn test_single_readonly_task() {
        let tasks = vec![TaskInfo::new("Read-only task").read_only()];
        let decision = decide_routing(&tasks);
        assert_eq!(decision.mode, DispatchMode::Background);
    }

    #[test]
    fn test_parallel_readonly_tasks() {
        let tasks = vec![
            TaskInfo::new("Read 1")
                .read_only()
                .reading(vec!["src/".into()]),
            TaskInfo::new("Read 2")
                .read_only()
                .reading(vec!["tests/".into()]),
            TaskInfo::new("Read 3")
                .read_only()
                .reading(vec!["docs/".into()]),
        ];

        let decision = decide_routing(&tasks);
        assert_eq!(decision.mode, DispatchMode::Parallel);
    }

    #[test]
    fn test_sequential_with_conflicts() {
        let tasks = vec![
            TaskInfo::new("Write config").writing(vec!["config.json".into()]),
            TaskInfo::new("Read config").reading(vec!["config.json".into()]),
        ];

        let decision = decide_routing(&tasks);
        assert_eq!(decision.mode, DispatchMode::Sequential);
    }

    #[test]
    fn test_no_conflicts_parallel() {
        let tasks = vec![
            TaskInfo::new("Task A").writing(vec!["a.txt".into()]),
            TaskInfo::new("Task B").writing(vec!["b.txt".into()]),
            TaskInfo::new("Task C").writing(vec!["c.txt".into()]),
        ];

        let decision = decide_routing(&tasks);
        assert_eq!(decision.mode, DispatchMode::Parallel);
    }

    #[test]
    fn test_interactive_sequential() {
        let tasks = vec![
            TaskInfo::new("Interactive").interactive(),
            TaskInfo::new("Normal task"),
        ];

        let decision = decide_routing(&tasks);
        assert_eq!(decision.mode, DispatchMode::Sequential);
    }

    #[test]
    fn test_file_conflict_detection() {
        let task1 = TaskInfo::new("Writer").writing(vec!["shared/file.txt".into()]);
        let task2 = TaskInfo::new("Reader").reading(vec!["shared/file.txt".into()]);
        let task3 = TaskInfo::new("Other").writing(vec!["other/file.txt".into()]);

        assert!(task1.has_file_conflict(&task2));
        assert!(!task1.has_file_conflict(&task3));
        assert!(!task2.has_file_conflict(&task3));
    }

    #[test]
    fn test_path_overlap() {
        assert!(paths_overlap(&"src/".into(), &"src/main.rs".into()));
        assert!(paths_overlap(&"src/main.rs".into(), &"src/".into()));
        assert!(paths_overlap(&"file.txt".into(), &"file.txt".into()));
        assert!(!paths_overlap(&"src/".into(), &"tests/".into()));
    }

    #[test]
    fn test_can_parallelize() {
        let parallel_tasks = vec![
            TaskInfo::new("A").writing(vec!["a.txt".into()]),
            TaskInfo::new("B").writing(vec!["b.txt".into()]),
        ];
        assert!(can_parallelize(&parallel_tasks));

        let conflicting_tasks = vec![
            TaskInfo::new("A").writing(vec!["shared.txt".into()]),
            TaskInfo::new("B").reading(vec!["shared.txt".into()]),
        ];
        assert!(!can_parallelize(&conflicting_tasks));
    }

    #[test]
    fn test_estimate_duration() {
        let tasks = vec![
            TaskInfo::new("A").with_duration(10),
            TaskInfo::new("B").with_duration(20),
            TaskInfo::new("C").with_duration(15),
        ];

        assert_eq!(
            estimate_duration(&tasks, DispatchMode::Sequential),
            Some(45)
        );
        assert_eq!(estimate_duration(&tasks, DispatchMode::Parallel), Some(20));
    }

    #[test]
    fn test_priority_ordering() {
        let tasks = vec![
            TaskInfo::new("Low")
                .with_priority(1)
                .writing(vec!["file.txt".into()]),
            TaskInfo::new("High")
                .with_priority(10)
                .writing(vec!["file.txt".into()]),
        ];

        let decision = decide_routing(&tasks);
        // High priority should come first in the order
        assert!(!decision.priority_order.is_empty());
    }

    #[test]
    fn test_task_info_builder() {
        let task = TaskInfo::new("Complex task")
            .reading(vec!["input.txt".into()])
            .writing(vec!["output.txt".into()])
            .with_commands()
            .with_duration(60)
            .with_priority(5)
            .with_tag("important");

        assert_eq!(task.description, "Complex task");
        assert_eq!(task.reads, vec![PathBuf::from("input.txt")]);
        assert_eq!(task.writes, vec![PathBuf::from("output.txt")]);
        assert!(task.executes_commands);
        assert_eq!(task.estimated_duration, Some(60));
        assert_eq!(task.priority, 5);
        assert!(task.tags.contains("important"));
    }

    #[test]
    fn test_dispatch_mode_display() {
        assert_eq!(format!("{}", DispatchMode::Parallel), "parallel");
        assert_eq!(format!("{}", DispatchMode::Sequential), "sequential");
        assert_eq!(format!("{}", DispatchMode::Background), "background");
        assert_eq!(format!("{}", DispatchMode::Single), "single");
    }
}
