//! Core types and enums for DAG command.

use clap::ValueEnum;
use cortex_agents::task::{TaskId, TaskStatus};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;

/// Output format for DAG commands.
#[derive(Debug, Clone, Copy, Default, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DagOutputFormat {
    /// Human-readable formatted output with colors.
    #[default]
    Text,
    /// JSON output for machine processing.
    Json,
    /// Compact single-line format for scripting.
    Compact,
}

/// Execution strategy for DAG tasks.
#[derive(Debug, Clone, Copy, Default, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExecutionStrategy {
    /// Execute tasks in parallel where possible.
    #[default]
    Parallel,
    /// Execute tasks sequentially in topological order.
    Sequential,
    /// Dry run - validate DAG without executing.
    DryRun,
}

/// Failure handling mode for task execution.
#[derive(Debug, Clone, Copy, Default, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FailureMode {
    /// Stop execution on first failure (default).
    #[default]
    FailFast,
    /// Skip failed task's dependents but continue others.
    SkipDependents,
    /// Ignore failures and continue all tasks.
    Continue,
}

/// Task specification for YAML/JSON input.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskSpecInput {
    /// Task name/identifier.
    pub name: String,
    /// Task description.
    #[serde(default)]
    pub description: String,
    /// Command to execute (optional).
    #[serde(default)]
    pub command: Option<String>,
    /// Task dependencies (names of tasks).
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// Files this task affects.
    #[serde(default)]
    pub affected_files: Vec<String>,
    /// Task priority (higher = runs first when parallel).
    #[serde(default)]
    pub priority: i32,
    /// Estimated duration in seconds.
    #[serde(default)]
    pub estimated_duration: Option<u64>,
    /// Custom metadata.
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

/// DAG specification from file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DagSpecInput {
    /// DAG name.
    #[serde(default)]
    pub name: Option<String>,
    /// DAG description.
    #[serde(default)]
    pub description: Option<String>,
    /// List of tasks.
    pub tasks: Vec<TaskSpecInput>,
}

/// Result of a task execution.
#[derive(Debug, Clone)]
pub struct TaskExecutionResult {
    pub task_id: TaskId,
    pub task_name: String,
    pub status: TaskStatus,
    pub duration: Duration,
    pub output: Option<String>,
    pub error: Option<String>,
}

/// DAG execution statistics.
#[derive(Debug, Clone, Default)]
pub struct DagExecutionStats {
    pub total_tasks: usize,
    pub completed_tasks: usize,
    pub failed_tasks: usize,
    pub skipped_tasks: usize,
    pub total_duration: Duration,
    pub task_results: Vec<TaskExecutionResult>,
}
