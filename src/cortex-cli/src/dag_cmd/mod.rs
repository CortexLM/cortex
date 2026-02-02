//! Task DAG CLI command for dependency-aware task execution.
//!
//! This module provides a comprehensive CLI interface for managing and executing
//! task DAGs (Directed Acyclic Graphs) with proper dependency resolution,
//! parallel execution, and failure propagation.
//!
//! # Features
//!
//! - **DAG Creation**: Create task graphs from YAML/JSON specs
//! - **Dependency Resolution**: Automatic topological sorting
//! - **Parallel Execution**: Run independent tasks concurrently
//! - **Failure Propagation**: Skip dependent tasks on failure
//! - **Progress Tracking**: Real-time status updates
//! - **Persistence**: Save and resume DAG execution
//!
//! # Usage
//!
//! ```bash
//! # Create a DAG from a spec file
//! cortex dag create --file tasks.yaml
//!
//! # Execute a DAG
//! cortex dag run --file tasks.yaml
//!
//! # Show DAG status
//! cortex dag status --id <dag-id>
//!
//! # List all DAGs
//! cortex dag list
//! ```

mod args;
mod commands;
mod executor;
mod helpers;
mod scheduler;
mod types;

#[cfg(test)]
mod tests;

use anyhow::Result;

// Re-export public types
pub use args::{
    DEFAULT_MAX_CONCURRENT, DEFAULT_TASK_TIMEOUT_SECS, DagCli, DagCommands, DagCreateArgs,
    DagDeleteArgs, DagGraphArgs, DagListArgs, DagResumeArgs, DagRunArgs, DagStatusArgs,
    DagValidateArgs,
};
pub use types::{
    DagExecutionStats, DagOutputFormat, DagSpecInput, ExecutionStrategy, FailureMode,
    TaskExecutionResult, TaskSpecInput,
};

// Re-export for internal use
pub use executor::TaskExecutor;
pub use helpers::{convert_specs, get_dag_store_path, load_spec};
pub use scheduler::DagScheduler;

/// Run the DAG CLI command.
pub async fn run(cli: DagCli) -> Result<()> {
    match cli.command {
        DagCommands::Create(args) => commands::run_create(args).await,
        DagCommands::Run(args) => commands::run_execute(args).await,
        DagCommands::Status(args) => commands::run_status(args).await,
        DagCommands::List(args) => commands::run_list(args).await,
        DagCommands::Validate(args) => commands::run_validate(args).await,
        DagCommands::Graph(args) => commands::run_graph(args).await,
        DagCommands::Delete(args) => commands::run_delete(args).await,
        DagCommands::Resume(args) => commands::run_resume(args).await,
    }
}
