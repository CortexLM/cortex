//! Command executor for cortex-tui slash commands.
//!
//! This module provides the `CommandExecutor` which connects parsed commands
//! to their handlers and produces `CommandResult` values.

mod billing;
mod debug;
mod development;
mod dispatch;
mod files;
mod general;
mod mcp;
mod model;
mod navigation;
mod session;

#[cfg(test)]
mod tests;

use super::parser::CommandParser;
use super::registry::CommandRegistry;
use super::types::{CommandResult, ParsedCommand};

// ============================================================
// COMMAND EXECUTOR
// ============================================================

/// Executes parsed commands and returns results.
///
/// The executor connects the command registry to command handlers,
/// dispatching parsed commands to the appropriate handler functions.
pub struct CommandExecutor {
    registry: CommandRegistry,
}

impl CommandExecutor {
    /// Creates a new command executor with the default registry.
    pub fn new() -> Self {
        Self {
            registry: CommandRegistry::default(),
        }
    }

    /// Creates a command executor with a custom registry.
    pub fn with_registry(registry: CommandRegistry) -> Self {
        Self { registry }
    }

    /// Execute a parsed command.
    ///
    /// Returns a `CommandResult` indicating the action to take.
    pub fn execute(&self, cmd: &ParsedCommand) -> CommandResult {
        // Check if command exists
        if !self.registry.exists(&cmd.name) {
            return CommandResult::NotFound(format!("Unknown command: /{}", cmd.name));
        }

        // Dispatch to handler
        self.dispatch(cmd)
    }

    /// Execute a command from a raw input string.
    ///
    /// Parses the input and executes if valid.
    pub fn execute_str(&self, input: &str) -> CommandResult {
        match CommandParser::parse(input) {
            Some(cmd) => self.execute(&cmd),
            None => CommandResult::Error("Invalid command format".to_string()),
        }
    }

    /// Get the registry for completions and lookups.
    pub fn registry(&self) -> &CommandRegistry {
        &self.registry
    }
}

impl Default for CommandExecutor {
    fn default() -> Self {
        Self::new()
    }
}
