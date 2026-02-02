//! Slash command system for cortex-tui.
//!
//! This module provides a complete slash command infrastructure including:
//! - Command registry with 45+ built-in commands
//! - Command parser supporting quoted arguments
//! - Fuzzy completion engine
//! - Command execution framework
//!
//! # Overview
//!
//! Commands are entered with a leading `/` character:
//! - `/help` - Show help
//! - `/model claude-sonnet-4-20250514` - Switch model
//! - `/search "hello world"` - Search with quoted argument
//!
//! # Example
//!
//! ```rust,ignore
//! use cortex_tui::commands::{CommandRegistry, CommandParser, CompletionEngine};
//!
//! // Get the default registry with all builtins
//! let registry = CommandRegistry::default();
//!
//! // Parse user input
//! if let Some(cmd) = CommandParser::parse("/help topic") {
//!     // Look up the command
//!     if let Some(def) = registry.get(&cmd.name) {
//!         println!("Command: {} - {}", def.name, def.description);
//!     }
//! }
//!
//! // Get completions for partial input
//! let engine = CompletionEngine::new(&registry);
//! let completions = engine.complete("/hel");
//! ```

pub mod completion;
pub mod executor;
pub mod forms;
pub mod parser;
pub mod registry;
pub mod types;

// Re-exports for convenience
pub use completion::{Completion, CompletionEngine};
pub use executor::CommandExecutor;
pub use forms::FormRegistry;
pub use parser::CommandParser;
pub use registry::{CommandRegistry, register_builtin_commands};
pub use types::{CommandCategory, CommandDef, CommandResult, ModalType, ParsedCommand, ViewType};

/// Initialize the command system with all builtins.
///
/// This is a convenience function that creates a `CommandRegistry`
/// with all built-in commands registered.
pub fn init() -> CommandRegistry {
    CommandRegistry::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_init_creates_registry() {
        let registry = init();
        assert!(!registry.is_empty());
        assert!(registry.exists("help"));
    }

    #[test]
    fn test_full_workflow() {
        let registry = init();

        // Parse a command
        let input = "/help topic";
        let cmd = CommandParser::parse(input).unwrap();
        assert_eq!(cmd.name, "help");
        assert_eq!(cmd.args, vec!["topic"]);

        // Look up definition
        let def = registry.get(&cmd.name).unwrap();
        assert_eq!(def.category, CommandCategory::General);

        // Get completions
        let engine = CompletionEngine::new(&registry);
        let completions = engine.complete("/hel");
        assert!(!completions.is_empty());
        assert!(completions.iter().any(|c| c.command == "help"));
    }
}
