//! Core command registry implementation.
//!
//! This module provides the `CommandRegistry` which stores all available
//! commands and provides lookup by name or alias.

use std::collections::HashMap;

use crate::commands::types::{CommandCategory, CommandDef};

/// Registry storing all available slash commands.
///
/// Provides efficient lookup by command name or alias, and supports
/// filtering by category.
pub struct CommandRegistry {
    /// Map from primary command name to definition.
    commands: HashMap<&'static str, CommandDef>,
    /// Map from alias to primary command name.
    aliases: HashMap<&'static str, &'static str>,
}

impl CommandRegistry {
    /// Creates a new empty command registry.
    pub fn new() -> Self {
        Self {
            commands: HashMap::new(),
            aliases: HashMap::new(),
        }
    }

    /// Registers a command in the registry.
    ///
    /// Also registers all aliases to point to the primary command name.
    pub fn register(&mut self, def: CommandDef) {
        // Register aliases
        for alias in def.aliases {
            self.aliases.insert(alias, def.name);
        }

        // Register the command
        self.commands.insert(def.name, def);
    }

    /// Gets a command by name or alias.
    ///
    /// The lookup is case-insensitive.
    pub fn get(&self, name: &str) -> Option<&CommandDef> {
        let name_lower = name.to_lowercase();

        // Try direct lookup first
        if let Some(def) = self.commands.get(name_lower.as_str()) {
            return Some(def);
        }

        // Try alias lookup
        if let Some(&primary) = self.aliases.get(name_lower.as_str()) {
            return self.commands.get(primary);
        }

        // Try case-insensitive scan as fallback
        for (cmd_name, def) in &self.commands {
            if cmd_name.eq_ignore_ascii_case(&name_lower) {
                return Some(def);
            }
        }

        for (alias, primary) in &self.aliases {
            if alias.eq_ignore_ascii_case(&name_lower) {
                return self.commands.get(primary);
            }
        }

        None
    }

    /// Gets all commands in a specific category.
    ///
    /// Results are sorted by command name.
    pub fn get_by_category(&self, category: CommandCategory) -> Vec<&CommandDef> {
        let mut commands: Vec<_> = self
            .commands
            .values()
            .filter(|def| def.category == category && !def.hidden)
            .collect();

        commands.sort_by_key(|def| def.name);
        commands
    }

    /// Gets all visible (non-hidden) commands.
    ///
    /// Results are sorted by command name.
    pub fn all(&self) -> Vec<&CommandDef> {
        let mut commands: Vec<_> = self.commands.values().filter(|def| !def.hidden).collect();

        commands.sort_by_key(|def| def.name);
        commands
    }

    /// Gets all commands including hidden ones.
    pub fn all_including_hidden(&self) -> Vec<&CommandDef> {
        let mut commands: Vec<_> = self.commands.values().collect();
        commands.sort_by_key(|def| def.name);
        commands
    }

    /// Checks if a command exists by name or alias.
    pub fn exists(&self, name: &str) -> bool {
        self.get(name).is_some()
    }

    /// Returns the number of registered commands.
    pub fn len(&self) -> usize {
        self.commands.len()
    }

    /// Returns true if no commands are registered.
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }

    /// Returns all command names (primary names only).
    pub fn command_names(&self) -> Vec<&'static str> {
        let mut names: Vec<_> = self.commands.keys().copied().collect();
        names.sort();
        names
    }

    /// Returns all names including aliases.
    pub fn all_names(&self) -> Vec<&'static str> {
        let mut names: Vec<_> = self
            .commands
            .keys()
            .copied()
            .chain(self.aliases.keys().copied())
            .collect();
        names.sort();
        names.dedup();
        names
    }
}

impl Default for CommandRegistry {
    fn default() -> Self {
        let mut registry = Self::new();
        super::builtin::register_builtin_commands(&mut registry);
        registry
    }
}
