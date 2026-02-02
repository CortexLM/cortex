//! Slash commands system for Cortex CLI.
//!
//! Slash commands are user-invoked actions that can be triggered by typing
//! /command in the chat. They provide quick access to common operations
//! and can be extended through plugins.
//!
//! Built-in commands:
//! - /help - Show available commands
//! - /skills - List available skills
//! - /plugins - List installed plugins
//! - /bug - Report a bug
//! - /clear - Clear conversation history
//! - /compact - Compact conversation context
//! - /undo - Undo last action
//! - /config - Show/edit configuration
//! - /model - Change model
//! - /cost - Show token usage and cost

mod configuration;
mod custom;
mod development;
mod information;
mod navigation;
mod registry;
mod types;

// Re-export main types
pub use types::{
    ArgType, CommandArg, CommandContext, CommandHandler, CommandInvocation, CommandMeta,
    CommandResult, TokenUsage,
};

// Re-export registry
pub use registry::CommandRegistry;

// Re-export custom command handler for external use
pub use custom::DynamicCustomCommand;

// Re-export individual commands for direct use if needed
pub use configuration::{
    AutoCommand, CustomCommandsCommand, DelegatesCommand, ExperimentalCommand, HooksCommand,
    InstallGithubAppCommand, SpecCommand,
};
pub use development::{
    BgProcessCommand, BugCommand, DiagnosticsCommand, GhostCommand, IdeCommand, MultiEditCommand,
    ReviewCommand, ShareCommand,
};
pub use information::{
    AgentsCommand, ConfigCommand, CostCommand, ModelCommand, PluginsCommand, RateLimitsCommand,
    SkillsCommand,
};
pub use navigation::{
    ClearCommand, CompactCommand, ExitCommand, FavoriteCommand, HelpCommand, ResumeCommand,
    RewindCommand, SessionsCommand, UndoCommand,
};
