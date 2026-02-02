//! CLI command definitions for debug subcommands.

use clap::Parser;
use std::path::PathBuf;

/// Debug CLI for Cortex.
#[derive(Debug, Parser)]
pub struct DebugCli {
    #[command(subcommand)]
    pub subcommand: DebugSubcommand,
}

/// Debug subcommands.
#[derive(Debug, clap::Subcommand)]
pub enum DebugSubcommand {
    /// Show resolved configuration and config file locations.
    Config(ConfigArgs),

    /// Show file metadata, MIME type, and encoding.
    File(FileArgs),

    /// List and test LSP servers.
    Lsp(LspArgs),

    /// Check ripgrep availability and test search.
    Ripgrep(RipgrepArgs),

    /// Parse and validate a skill file.
    Skill(SkillArgs),

    /// Show snapshot status and diffs.
    Snapshot(SnapshotArgs),

    /// Show all Cortex paths.
    Paths(PathsArgs),

    /// Show system information (OS, architecture, shell, etc.) for bug reports.
    System(SystemArgs),

    /// Wait for a condition (useful for scripts).
    Wait(WaitArgs),
}

// =============================================================================
// Config subcommand args
// =============================================================================

/// Arguments for config subcommand.
#[derive(Debug, Parser)]
pub struct ConfigArgs {
    /// Output as JSON.
    #[arg(long)]
    pub json: bool,

    /// Show environment variables related to Cortex.
    #[arg(long)]
    pub env: bool,

    /// Show diff between local project config and global config.
    #[arg(long)]
    pub diff: bool,
}

// =============================================================================
// File subcommand args
// =============================================================================

/// Arguments for file subcommand.
#[derive(Debug, Parser)]
pub struct FileArgs {
    /// Path to the file to inspect.
    pub path: PathBuf,

    /// Output as JSON.
    #[arg(long)]
    pub json: bool,
}

// =============================================================================
// LSP subcommand args
// =============================================================================

/// Arguments for lsp subcommand.
#[derive(Debug, Parser)]
pub struct LspArgs {
    /// Test a specific LSP server.
    #[arg(long)]
    pub server: Option<String>,

    /// Filter by programming language (e.g., python, rust, go).
    #[arg(long, short = 'l')]
    pub language: Option<String>,

    /// Test LSP connection for a specific file.
    #[arg(long)]
    pub file: Option<PathBuf>,

    /// Output as JSON.
    #[arg(long)]
    pub json: bool,
}

// =============================================================================
// Ripgrep subcommand args
// =============================================================================

/// Arguments for ripgrep subcommand.
#[derive(Debug, Parser)]
pub struct RipgrepArgs {
    /// Test search with a pattern.
    #[arg(long)]
    pub test: Option<String>,

    /// Directory to search in for test.
    #[arg(long)]
    pub dir: Option<PathBuf>,

    /// Offer to install ripgrep if not found.
    #[arg(long)]
    pub install: bool,

    /// Output as JSON.
    #[arg(long)]
    pub json: bool,
}

// =============================================================================
// Skill subcommand args
// =============================================================================

/// Arguments for skill subcommand.
#[derive(Debug, Parser)]
pub struct SkillArgs {
    /// Name or path of the skill to validate.
    pub name: String,

    /// Output as JSON.
    #[arg(long)]
    pub json: bool,
}

// =============================================================================
// Snapshot subcommand args
// =============================================================================

/// Arguments for snapshot subcommand.
#[derive(Debug, Parser)]
pub struct SnapshotArgs {
    /// Session ID to inspect snapshots for.
    #[arg(long)]
    pub session: Option<String>,

    /// Show diff between snapshots.
    #[arg(long)]
    pub diff: bool,

    /// Create a new snapshot of the current workspace state.
    #[arg(long, conflicts_with_all = ["restore", "delete"])]
    pub create: bool,

    /// Restore workspace to a specific snapshot state.
    /// Requires --snapshot-id to specify which snapshot to restore.
    #[arg(long, conflicts_with_all = ["create", "delete"])]
    pub restore: bool,

    /// Delete a specific snapshot.
    /// Requires --snapshot-id to specify which snapshot to delete.
    #[arg(long, conflicts_with_all = ["create", "restore"])]
    pub delete: bool,

    /// Snapshot ID for restore/delete operations.
    #[arg(long, value_name = "ID")]
    pub snapshot_id: Option<String>,

    /// Description for the snapshot (used with --create).
    #[arg(long, value_name = "DESC")]
    pub description: Option<String>,

    /// Output as JSON.
    #[arg(long)]
    pub json: bool,
}

// =============================================================================
// Paths subcommand args
// =============================================================================

/// Arguments for paths subcommand.
#[derive(Debug, Parser)]
pub struct PathsArgs {
    /// Output as JSON.
    #[arg(long)]
    pub json: bool,

    /// Check if write locations are accessible (useful for Docker read-only containers).
    #[arg(long)]
    pub check_writable: bool,
}

// =============================================================================
// Wait subcommand args
// =============================================================================

/// Arguments for wait subcommand.
#[derive(Debug, Parser)]
pub struct WaitArgs {
    /// Wait for LSP to be ready.
    #[arg(long)]
    pub lsp_ready: bool,

    /// Wait for server to be ready.
    #[arg(long)]
    pub server_ready: bool,

    /// Server URL to check (default: http://127.0.0.1:3000).
    #[arg(long, default_value = "http://127.0.0.1:3000")]
    pub server_url: String,

    /// Wait for a TCP port to be available.
    #[arg(long)]
    pub port: Option<u16>,

    /// Host to check when using --port (default: 127.0.0.1).
    #[arg(long, default_value = "127.0.0.1")]
    pub host: String,

    /// Timeout in seconds.
    #[arg(long, default_value = "30")]
    pub timeout: u64,

    /// Check interval in milliseconds.
    #[arg(long, default_value = "500")]
    pub interval: u64,

    /// Output as JSON.
    #[arg(long)]
    pub json: bool,
}

// =============================================================================
// System subcommand args
// =============================================================================

/// Arguments for system subcommand.
#[derive(Debug, Parser)]
pub struct SystemArgs {
    /// Output as JSON.
    #[arg(long)]
    pub json: bool,
}
