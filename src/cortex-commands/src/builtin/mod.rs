//! Built-in commands for Cortex CLI.
//!
//! This module provides built-in slash commands that are always available
//! regardless of custom command configuration.
//!
//! # Available Commands
//!
//! - `/init` - Initialize a project with AGENTS.md
//! - `/commands` - List all available custom commands
//! - `/agents` - List all available custom agents (subagents)
//! - `/favorite` - Toggle favorite status of current session
//! - `/share` - Generate a share link for current session
//!
//! # Hierarchy Support
//!
//! AGENTS.md files are loaded from multiple locations in order of specificity:
//!
//! 1. Global: `~/.config/cortex/AGENTS.md`
//! 2. Project: `<project>/AGENTS.md`
//! 3. Local: `<project>/.cortex/AGENTS.md`
//!
//! # Orchestration State
//!
//! The module provides orchestration state tracking for multi-agent workflows:
//!
//! ```rust,ignore
//! use cortex_commands::builtin::state::{StateManager, AgentState};
//!
//! let mut manager = StateManager::new(project_root, "my-project")?;
//! manager.add_agent(AgentState::new("agent1", "Analysis Agent", 5))?;
//! manager.start()?;
//! manager.start_agent("agent1")?;
//! manager.complete_agent("agent1")?;
//! ```
//!
//! # Atomic File Writing
//!
//! All file operations use atomic writes to prevent corruption:
//!
//! ```rust,ignore
//! use cortex_commands::builtin::atomic::atomic_write_str;
//!
//! atomic_write_str("config.yaml", "key: value")?;
//! ```
//!
//! # Usage
//!
//! ```rust,ignore
//! use cortex_commands::builtin::{InitCommand, InitResult};
//! use std::path::Path;
//!
//! let cmd = InitCommand::new();
//! match cmd.execute(Path::new(".")) {
//!     Ok(InitResult::Created(path)) => println!("Created {}", path.display()),
//!     Ok(InitResult::AlreadyExists(path)) => println!("Already exists at {}", path.display()),
//!     Ok(InitResult::Updated(path)) => println!("Updated {}", path.display()),
//!     Err(e) => eprintln!("Error: {}", e),
//! }
//! ```

mod agents_cmd;
pub mod atomic;
mod commands_cmd;
mod favorite_cmd;
pub mod hierarchy;
mod init;
mod share_cmd;
pub mod state;
mod templates;

pub use agents_cmd::{AgentsCommand, AgentsError, AgentsResult, BuiltinAgentInfo, CustomAgentInfo};
pub use commands_cmd::{CommandInfo, CommandsCommand, CommandsError, CommandsResult};
pub use favorite_cmd::FavoriteResult;
pub use init::{
    InitCommand, InitError, InitOptions, InitResult, KeyFile, ProjectInfo, ProjectType,
};
pub use share_cmd::{DEFAULT_SHARE_DURATION, ShareResult, format_duration, parse_duration};
pub use templates::{
    AGENTS_MD_MINIMAL_TEMPLATE, AGENTS_MD_TEMPLATE, GO_PROJECT_DEFAULTS, NODE_PROJECT_DEFAULTS,
    PYTHON_PROJECT_DEFAULTS, ProjectDefaults, RUST_PROJECT_DEFAULTS,
};

/// Trait for built-in commands.
pub trait BuiltinCommand {
    /// The command name (without leading slash).
    const NAME: &'static str;

    /// Human-readable description.
    const DESCRIPTION: &'static str;

    /// Usage example.
    const USAGE: &'static str;
}

impl BuiltinCommand for InitCommand {
    const NAME: &'static str = "init";
    const DESCRIPTION: &'static str = "Initialize AGENTS.md in the current directory";
    const USAGE: &'static str = "/init [--force]";
}

impl BuiltinCommand for CommandsCommand {
    const NAME: &'static str = "commands";
    const DESCRIPTION: &'static str = "List all available custom commands";
    const USAGE: &'static str = "/commands";
}

impl BuiltinCommand for AgentsCommand {
    const NAME: &'static str = "agents";
    const DESCRIPTION: &'static str = "List all available custom agents (subagents)";
    const USAGE: &'static str = "/agents";
}

/// Favorite command metadata.
pub struct FavoriteCommand;

impl BuiltinCommand for FavoriteCommand {
    const NAME: &'static str = "favorite";
    const DESCRIPTION: &'static str = "Toggle favorite status of the current session";
    const USAGE: &'static str = "/favorite";
}

/// Share command metadata.
pub struct ShareCommand;

impl BuiltinCommand for ShareCommand {
    const NAME: &'static str = "share";
    const DESCRIPTION: &'static str = "Generate a share link for the current session";
    const USAGE: &'static str = "/share [duration]";
}

/// Registry of all built-in commands.
#[derive(Debug, Default)]
pub struct BuiltinRegistry {
    /// Init command handler.
    pub init: InitCommand,
    /// Commands command handler.
    pub commands: CommandsCommand,
    /// Agents command handler.
    pub agents: AgentsCommand,
}

impl BuiltinRegistry {
    /// Create a new builtin registry with default handlers.
    pub fn new() -> Self {
        Self::default()
    }

    /// Get the names of all built-in commands.
    pub fn command_names() -> &'static [&'static str] {
        &["init", "commands", "agents", "favorite", "share"]
    }

    /// Check if a command name is a built-in command.
    pub fn is_builtin(name: &str) -> bool {
        matches!(
            name.to_lowercase().as_str(),
            "init" | "commands" | "agents" | "favorite" | "share"
        )
    }

    /// Get information about all built-in commands.
    pub fn command_info() -> Vec<BuiltinCommandInfo> {
        vec![
            BuiltinCommandInfo {
                name: InitCommand::NAME,
                description: InitCommand::DESCRIPTION,
                usage: InitCommand::USAGE,
            },
            BuiltinCommandInfo {
                name: CommandsCommand::NAME,
                description: CommandsCommand::DESCRIPTION,
                usage: CommandsCommand::USAGE,
            },
            BuiltinCommandInfo {
                name: AgentsCommand::NAME,
                description: AgentsCommand::DESCRIPTION,
                usage: AgentsCommand::USAGE,
            },
            BuiltinCommandInfo {
                name: FavoriteCommand::NAME,
                description: FavoriteCommand::DESCRIPTION,
                usage: FavoriteCommand::USAGE,
            },
            BuiltinCommandInfo {
                name: ShareCommand::NAME,
                description: ShareCommand::DESCRIPTION,
                usage: ShareCommand::USAGE,
            },
        ]
    }
}

/// Information about a built-in command.
#[derive(Debug, Clone)]
pub struct BuiltinCommandInfo {
    /// Command name.
    pub name: &'static str,
    /// Description.
    pub description: &'static str,
    /// Usage example.
    pub usage: &'static str,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_builtin() {
        assert!(BuiltinRegistry::is_builtin("init"));
        assert!(BuiltinRegistry::is_builtin("INIT"));
        assert!(BuiltinRegistry::is_builtin("Init"));
        assert!(BuiltinRegistry::is_builtin("commands"));
        assert!(BuiltinRegistry::is_builtin("COMMANDS"));
        assert!(BuiltinRegistry::is_builtin("agents"));
        assert!(BuiltinRegistry::is_builtin("AGENTS"));
        assert!(BuiltinRegistry::is_builtin("favorite"));
        assert!(BuiltinRegistry::is_builtin("FAVORITE"));
        assert!(BuiltinRegistry::is_builtin("share"));
        assert!(BuiltinRegistry::is_builtin("SHARE"));
        assert!(!BuiltinRegistry::is_builtin("unknown"));
    }

    #[test]
    fn test_command_names() {
        let names = BuiltinRegistry::command_names();
        assert!(names.contains(&"init"));
        assert!(names.contains(&"commands"));
        assert!(names.contains(&"agents"));
        assert!(names.contains(&"favorite"));
        assert!(names.contains(&"share"));
    }

    #[test]
    fn test_command_info() {
        let info = BuiltinRegistry::command_info();
        assert_eq!(info.len(), 5);
        assert!(info.iter().any(|i| i.name == "init"));
        assert!(info.iter().any(|i| i.name == "commands"));
        assert!(info.iter().any(|i| i.name == "agents"));
        assert!(info.iter().any(|i| i.name == "favorite"));
        assert!(info.iter().any(|i| i.name == "share"));
    }

    #[test]
    fn test_init_command_trait() {
        assert_eq!(InitCommand::NAME, "init");
        assert!(!InitCommand::DESCRIPTION.is_empty());
        assert!(InitCommand::USAGE.contains("/init"));
    }

    #[test]
    fn test_commands_command_trait() {
        assert_eq!(CommandsCommand::NAME, "commands");
        assert!(!CommandsCommand::DESCRIPTION.is_empty());
        assert!(CommandsCommand::USAGE.contains("/commands"));
    }

    #[test]
    fn test_agents_command_trait() {
        assert_eq!(AgentsCommand::NAME, "agents");
        assert!(!AgentsCommand::DESCRIPTION.is_empty());
        assert!(AgentsCommand::USAGE.contains("/agents"));
    }

    #[test]
    fn test_favorite_command_trait() {
        assert_eq!(FavoriteCommand::NAME, "favorite");
        assert!(!FavoriteCommand::DESCRIPTION.is_empty());
        assert!(FavoriteCommand::USAGE.contains("/favorite"));
    }

    #[test]
    fn test_share_command_trait() {
        assert_eq!(ShareCommand::NAME, "share");
        assert!(!ShareCommand::DESCRIPTION.is_empty());
        assert!(ShareCommand::USAGE.contains("/share"));
    }
}
