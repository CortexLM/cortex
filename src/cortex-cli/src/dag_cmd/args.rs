//! CLI argument definitions for DAG commands.

use clap::{Parser, Subcommand};
use std::path::PathBuf;

use super::types::{DagOutputFormat, ExecutionStrategy, FailureMode};

/// Maximum concurrent task execution limit.
pub const DEFAULT_MAX_CONCURRENT: usize = 4;

/// Default task timeout in seconds.
pub const DEFAULT_TASK_TIMEOUT_SECS: u64 = 300;

/// DAG CLI command group.
#[derive(Debug, Parser)]
#[command(
    name = "dag",
    about = "Task DAG execution and management",
    long_about = "Manage and execute task DAGs (Directed Acyclic Graphs) with dependency resolution."
)]
pub struct DagCli {
    #[command(subcommand)]
    pub command: DagCommands,
}

/// DAG subcommands.
#[derive(Debug, Subcommand)]
pub enum DagCommands {
    /// Create a new task DAG from a specification file.
    #[command(visible_alias = "new")]
    Create(DagCreateArgs),

    /// Execute a task DAG.
    #[command(visible_alias = "exec")]
    Run(DagRunArgs),

    /// Show the status of a DAG.
    #[command(visible_alias = "info")]
    Status(DagStatusArgs),

    /// List all DAGs.
    #[command(visible_alias = "ls")]
    List(DagListArgs),

    /// Validate a DAG specification.
    #[command(visible_alias = "check")]
    Validate(DagValidateArgs),

    /// Visualize a DAG structure.
    #[command(visible_alias = "viz")]
    Graph(DagGraphArgs),

    /// Delete a DAG.
    #[command(visible_alias = "rm")]
    Delete(DagDeleteArgs),

    /// Resume a partially executed DAG.
    Resume(DagResumeArgs),
}

/// Arguments for DAG creation.
#[derive(Debug, Parser)]
pub struct DagCreateArgs {
    /// Path to task specification file (YAML or JSON).
    #[arg(short = 'f', long = "file", required = true)]
    pub file: PathBuf,

    /// Custom DAG ID (generated if not provided).
    #[arg(short = 'i', long = "id")]
    pub id: Option<String>,

    /// Validate only, don't save.
    #[arg(long = "dry-run")]
    pub dry_run: bool,

    /// Output format.
    #[arg(long = "format", value_enum, default_value_t = DagOutputFormat::Text)]
    pub format: DagOutputFormat,
}

/// Arguments for DAG execution.
#[derive(Debug, Parser)]
pub struct DagRunArgs {
    /// Path to task specification file (YAML or JSON).
    #[arg(short = 'f', long = "file", required = true)]
    pub file: PathBuf,

    /// Maximum concurrent tasks.
    #[arg(short = 'j', long = "jobs", default_value_t = DEFAULT_MAX_CONCURRENT)]
    pub max_concurrent: usize,

    /// Task timeout in seconds.
    #[arg(short = 't', long = "timeout", default_value_t = DEFAULT_TASK_TIMEOUT_SECS)]
    pub timeout: u64,

    /// Execution strategy.
    #[arg(short = 's', long = "strategy", value_enum, default_value_t = ExecutionStrategy::Parallel)]
    pub strategy: ExecutionStrategy,

    /// Failure handling mode.
    #[arg(long = "on-failure", value_enum, default_value_t = FailureMode::FailFast)]
    pub failure_mode: FailureMode,

    /// Output format.
    #[arg(long = "format", value_enum, default_value_t = DagOutputFormat::Text)]
    pub format: DagOutputFormat,

    /// Quiet mode - minimal output.
    #[arg(short = 'q', long = "quiet")]
    pub quiet: bool,

    /// Verbose mode - show task details.
    #[arg(short = 'v', long = "verbose")]
    pub verbose: bool,

    /// Save DAG state for later resume.
    #[arg(long = "save")]
    pub save: bool,

    /// Custom DAG ID for saving.
    #[arg(long = "id")]
    pub id: Option<String>,

    /// Infer task dependencies from affected files.
    #[arg(long = "infer-deps")]
    pub infer_deps: bool,
}

/// Arguments for DAG status.
#[derive(Debug, Parser)]
pub struct DagStatusArgs {
    /// DAG ID to check.
    #[arg(required = true)]
    pub id: String,

    /// Output format.
    #[arg(long = "format", value_enum, default_value_t = DagOutputFormat::Text)]
    pub format: DagOutputFormat,

    /// Show detailed task information.
    #[arg(short = 'v', long = "verbose")]
    pub verbose: bool,
}

/// Arguments for DAG listing.
#[derive(Debug, Parser)]
pub struct DagListArgs {
    /// Filter by status (pending, running, completed, failed).
    #[arg(long = "status")]
    pub status: Option<String>,

    /// Output format.
    #[arg(long = "format", value_enum, default_value_t = DagOutputFormat::Text)]
    pub format: DagOutputFormat,

    /// Maximum number of DAGs to show.
    #[arg(short = 'n', long = "limit")]
    pub limit: Option<usize>,
}

/// Arguments for DAG validation.
#[derive(Debug, Parser)]
pub struct DagValidateArgs {
    /// Path to task specification file.
    #[arg(short = 'f', long = "file", required = true)]
    pub file: PathBuf,

    /// Show validation details.
    #[arg(short = 'v', long = "verbose")]
    pub verbose: bool,
}

/// Arguments for DAG visualization.
#[derive(Debug, Parser)]
pub struct DagGraphArgs {
    /// Path to task specification file.
    #[arg(short = 'f', long = "file", required = true)]
    pub file: PathBuf,

    /// Output format (ascii, dot, mermaid).
    #[arg(long = "output", default_value = "ascii")]
    pub output: String,
}

/// Arguments for DAG deletion.
#[derive(Debug, Parser)]
pub struct DagDeleteArgs {
    /// DAG ID to delete.
    #[arg(required = true)]
    pub id: String,

    /// Skip confirmation prompt.
    #[arg(short = 'y', long = "yes")]
    pub yes: bool,
}

/// Arguments for DAG resume.
#[derive(Debug, Parser)]
pub struct DagResumeArgs {
    /// DAG ID to resume.
    #[arg(required = true)]
    pub id: String,

    /// Maximum concurrent tasks.
    #[arg(short = 'j', long = "jobs", default_value_t = DEFAULT_MAX_CONCURRENT)]
    pub max_concurrent: usize,

    /// Task timeout in seconds.
    #[arg(short = 't', long = "timeout", default_value_t = DEFAULT_TASK_TIMEOUT_SECS)]
    pub timeout: u64,

    /// Failure handling mode.
    #[arg(long = "on-failure", value_enum, default_value_t = FailureMode::FailFast)]
    pub failure_mode: FailureMode,

    /// Output format.
    #[arg(long = "format", value_enum, default_value_t = DagOutputFormat::Text)]
    pub format: DagOutputFormat,
}
