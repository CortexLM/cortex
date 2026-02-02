//! Custom command system for Cortex CLI.
//!
//! This crate provides a system for loading and executing custom slash commands
//! defined in markdown files with YAML frontmatter, as well as built-in commands.
//!
//! # Command File Format
//!
//! Commands are defined in `.md` files with YAML frontmatter:
//!
//! ```markdown
//! ---
//! description: "Description of the command"
//! agent: "build"  # optional: specific agent
//! model: "gpt-4o" # optional: specific model
//! subtask: false  # optional: execute as subtask
//! ---
//!
//! Your command template here.
//! $ARGUMENTS will be replaced with all arguments.
//! $1 will be replaced with the first argument.
//! $2 will be replaced with the second argument.
//! ```
//!
//! # Search Paths
//!
//! Commands are loaded from:
//! 1. `.cortex/commands/` (project-local)
//! 2. `~/.config/cortex/commands/` (global)
//!
//! # Built-in Commands
//!
//! The following built-in commands are always available:
//!
//! - `/init` - Initialize a project with AGENTS.md
//! - `/commands` - List all available custom commands
//! - `/agents` - List all available custom agents (subagents)
//!
//! ```rust,ignore
//! use cortex_commands::builtin::{InitCommand, InitResult};
//! use std::path::Path;
//!
//! let cmd = InitCommand::new();
//! let result = cmd.execute(Path::new("."))?;
//! println!("{}", result.message());
//! ```
//!
//! # Command Manager
//!
//! The `CommandManager` provides TUI integration with hot reloading:
//!
//! ```rust,ignore
//! use cortex_commands::CommandManager;
//!
//! let mut manager = CommandManager::new();
//! manager.set_project_path("/path/to/project");
//! manager.load_all().await?;
//! ```
//!
//! # Import from Claude Code
//!
//! Commands can be imported from Claude Code format:
//!
//! ```rust,ignore
//! use cortex_commands::import::{ClaudeImporter, ImportType};
//!
//! let importer = ClaudeImporter::new();
//! let scan = importer.scan();
//! importer.import(&scan.commands, target_dir, ImportType::Command).await?;
//! ```

pub mod builtin;
mod command;
mod executor;
pub mod import;
mod loader;
mod manager;
mod registry;

pub use command::{Command, CommandConfig, CommandError, hints, substitute_placeholders};
pub use executor::{ExecutionContext, ExecutionResult, Executor, format_command, parse_invocation};
pub use loader::{CommandLoader, LoaderError};
pub use manager::{CommandManager, ManagerError, ReloadEvent};
pub use registry::CommandRegistry;

/// Synchronous loader utilities.
pub mod sync {
    pub use crate::loader::sync::*;
}

/// Re-export common types for convenience.
pub mod prelude {
    pub use crate::builtin::{
        InitCommand, InitError, InitOptions, InitResult, ProjectInfo, ProjectType,
    };
    pub use crate::{
        Command, CommandConfig, CommandError, CommandLoader, CommandRegistry, ExecutionContext,
        ExecutionResult, Executor, LoaderError,
    };
}
