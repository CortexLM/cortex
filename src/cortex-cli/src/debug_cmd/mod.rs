//! Debug commands for Cortex CLI.
//!
//! Provides diagnostic and debugging functionality:
//! - Configuration inspection
//! - File metadata and MIME detection
//! - LSP server status
//! - Ripgrep availability
//! - Skill validation
//! - Snapshot inspection
//! - Path inspection
//! - System information (OS, arch, shell, etc.)
//! - Wait for conditions

mod commands;
mod handlers;
mod types;
pub mod utils;

pub use commands::{
    ConfigArgs, DebugCli, DebugSubcommand, FileArgs, LspArgs, PathsArgs, RipgrepArgs, SkillArgs,
    SnapshotArgs, SystemArgs, WaitArgs,
};
pub use types::*;

use anyhow::Result;

impl DebugCli {
    /// Run the debug command.
    pub async fn run(self) -> Result<()> {
        match self.subcommand {
            DebugSubcommand::Config(args) => handlers::run_config(args).await,
            DebugSubcommand::File(args) => handlers::run_file(args).await,
            DebugSubcommand::Lsp(args) => handlers::run_lsp(args).await,
            DebugSubcommand::Ripgrep(args) => handlers::run_ripgrep(args).await,
            DebugSubcommand::Skill(args) => handlers::run_skill(args).await,
            DebugSubcommand::Snapshot(args) => handlers::run_snapshot(args).await,
            DebugSubcommand::Paths(args) => handlers::run_paths(args).await,
            DebugSubcommand::System(args) => handlers::run_system(args).await,
            DebugSubcommand::Wait(args) => handlers::run_wait(args).await,
        }
    }
}
