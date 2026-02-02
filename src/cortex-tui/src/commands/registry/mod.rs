//! Command registry for cortex-tui slash commands.
//!
//! This module provides the `CommandRegistry` which stores all available
//! commands and provides lookup by name or alias.

mod builtin;
mod core;

// Re-export for backwards compatibility
pub use builtin::register_builtin_commands;
pub use core::CommandRegistry;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::types::CommandCategory;

    #[test]
    fn test_registry_new() {
        let registry = CommandRegistry::new();
        assert!(registry.is_empty());
    }

    #[test]
    fn test_registry_default_has_commands() {
        let registry = CommandRegistry::default();
        assert!(!registry.is_empty());
        assert!(registry.len() >= 49);
    }

    #[test]
    fn test_registry_get_by_name() {
        let registry = CommandRegistry::default();

        let help = registry.get("help").unwrap();
        assert_eq!(help.name, "help");
        assert_eq!(help.category, CommandCategory::General);
    }

    #[test]
    fn test_registry_get_by_alias() {
        let registry = CommandRegistry::default();

        let help = registry.get("h").unwrap();
        assert_eq!(help.name, "help");

        let help = registry.get("?").unwrap();
        assert_eq!(help.name, "help");
    }

    #[test]
    fn test_registry_case_insensitive() {
        let registry = CommandRegistry::default();

        assert!(registry.get("HELP").is_some());
        assert!(registry.get("Help").is_some());
        assert!(registry.get("H").is_some());
    }

    #[test]
    fn test_registry_get_nonexistent() {
        let registry = CommandRegistry::default();
        assert!(registry.get("nonexistent").is_none());
    }

    #[test]
    fn test_registry_exists() {
        let registry = CommandRegistry::default();

        assert!(registry.exists("help"));
        assert!(registry.exists("h"));
        assert!(!registry.exists("nonexistent"));
    }

    #[test]
    fn test_registry_get_by_category() {
        let registry = CommandRegistry::default();

        let general = registry.get_by_category(CommandCategory::General);
        assert!(!general.is_empty());
        assert!(
            general
                .iter()
                .all(|cmd| cmd.category == CommandCategory::General)
        );

        let session = registry.get_by_category(CommandCategory::Session);
        assert!(!session.is_empty());
        assert!(
            session
                .iter()
                .all(|cmd| cmd.category == CommandCategory::Session)
        );
    }

    #[test]
    fn test_registry_all_sorted() {
        let registry = CommandRegistry::default();
        let all = registry.all();

        // Check sorted order
        for i in 1..all.len() {
            assert!(all[i - 1].name <= all[i].name);
        }
    }

    #[test]
    fn test_registry_hidden_commands() {
        let registry = CommandRegistry::default();

        // Hidden commands should exist
        assert!(registry.exists("crash"));

        // But not appear in all()
        let all = registry.all();
        assert!(!all.iter().any(|cmd| cmd.name == "crash"));

        // But appear in all_including_hidden()
        let all_hidden = registry.all_including_hidden();
        assert!(all_hidden.iter().any(|cmd| cmd.name == "crash"));
    }

    #[test]
    fn test_registry_command_names() {
        let registry = CommandRegistry::default();
        let names = registry.command_names();

        assert!(names.contains(&"help"));
        assert!(names.contains(&"quit"));
        assert!(!names.contains(&"h")); // alias, not primary
    }

    #[test]
    fn test_registry_all_names() {
        let registry = CommandRegistry::default();
        let names = registry.all_names();

        assert!(names.contains(&"help"));
        assert!(names.contains(&"h")); // includes aliases
        assert!(names.contains(&"?"));
    }

    #[test]
    fn test_specific_commands_registered() {
        let registry = CommandRegistry::default();

        // General
        assert!(registry.exists("help"));
        assert!(registry.exists("quit"));
        assert!(registry.exists("version"));
        assert!(registry.exists("settings"));
        assert!(registry.exists("theme"));
        assert!(registry.exists("compact"));
        assert!(registry.exists("init"));
        assert!(registry.exists("commands"));
        assert!(registry.exists("agents"));

        // Session
        assert!(registry.exists("clear"));
        assert!(registry.exists("share"));
        assert!(registry.exists("new"));
        assert!(registry.exists("resume"));
        assert!(registry.exists("sessions"));
        assert!(registry.exists("fork"));
        assert!(registry.exists("rename"));
        assert!(registry.exists("favorite"));
        assert!(registry.exists("unfavorite"));
        assert!(registry.exists("export"));
        assert!(registry.exists("timeline"));
        assert!(registry.exists("rewind"));
        assert!(registry.exists("undo"));
        assert!(registry.exists("redo"));

        // Navigation
        assert!(registry.exists("diff"));
        assert!(registry.exists("transcript"));
        assert!(registry.exists("history"));

        // Files
        assert!(registry.exists("add"));
        assert!(registry.exists("remove"));
        assert!(registry.exists("search"));
        assert!(registry.exists("ls"));
        assert!(registry.exists("mention"));
        assert!(registry.exists("images"));

        // Model
        assert!(registry.exists("model"));
        assert!(registry.exists("models"));
        assert!(registry.exists("approval"));
        assert!(registry.exists("sandbox"));
        assert!(registry.exists("auto"));

        // MCP
        assert!(registry.exists("mcp"));
        assert!(registry.exists("mcp-tools"));
        assert!(registry.exists("mcp-auth"));
        assert!(registry.exists("mcp-reload"));

        // Auth
        assert!(registry.exists("login"));
        assert!(registry.exists("logout"));
        assert!(registry.exists("account"));

        // Debug
        assert!(registry.exists("debug"));
        assert!(registry.exists("status"));
        assert!(registry.exists("config"));
        assert!(registry.exists("logs"));
    }

    #[test]
    fn test_auth_commands_registered() {
        let registry = CommandRegistry::default();

        // Login command
        let login = registry.get("login").unwrap();
        assert_eq!(login.name, "login");
        assert_eq!(login.category, CommandCategory::Auth);
        assert!(login.aliases.contains(&"signin"));

        // Logout command
        let logout = registry.get("logout").unwrap();
        assert_eq!(logout.name, "logout");
        assert_eq!(logout.category, CommandCategory::Auth);
        assert!(logout.aliases.contains(&"signout"));

        // Account command
        let account = registry.get("account").unwrap();
        assert_eq!(account.name, "account");
        assert_eq!(account.category, CommandCategory::Auth);
        assert!(account.aliases.contains(&"whoami"));
        assert!(account.aliases.contains(&"me"));
    }

    #[test]
    fn test_auth_aliases() {
        let registry = CommandRegistry::default();

        // Signin alias
        let signin = registry.get("signin").unwrap();
        assert_eq!(signin.name, "login");

        // Signout alias
        let signout = registry.get("signout").unwrap();
        assert_eq!(signout.name, "logout");

        // Whoami alias
        let whoami = registry.get("whoami").unwrap();
        assert_eq!(whoami.name, "account");

        // Me alias
        let me = registry.get("me").unwrap();
        assert_eq!(me.name, "account");
    }

    #[test]
    fn test_auth_category() {
        let registry = CommandRegistry::default();
        let auth_commands = registry.get_by_category(CommandCategory::Auth);

        assert!(!auth_commands.is_empty());
        assert_eq!(auth_commands.len(), 3);

        let names: Vec<_> = auth_commands.iter().map(|c| c.name).collect();
        assert!(names.contains(&"login"));
        assert!(names.contains(&"logout"));
        assert!(names.contains(&"account"));
    }
}
