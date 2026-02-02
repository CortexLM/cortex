//! Command registry for managing loaded commands.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::command::Command;
use crate::loader::{CommandLoader, LoaderError};

/// Registry of custom commands.
///
/// The registry manages a collection of commands and provides
/// methods for looking up, listing, and registering commands.
#[derive(Debug, Default)]
pub struct CommandRegistry {
    /// Map of command names to commands.
    commands: HashMap<String, Command>,
    /// Map of aliases to command names.
    aliases: HashMap<String, String>,
}

impl CommandRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self {
            commands: HashMap::new(),
            aliases: HashMap::new(),
        }
    }

    /// Create a registry and load commands from default directories.
    pub async fn load_default() -> Result<Self, LoaderError> {
        let mut registry = Self::new();
        let loader = CommandLoader::new();
        registry.load_from_loader(&loader).await?;
        Ok(registry)
    }

    /// Load commands from the given directories.
    pub async fn load_from_directories(&mut self, dirs: &[PathBuf]) -> Result<(), LoaderError> {
        let loader = CommandLoader::with_dirs(dirs.iter().cloned());
        self.load_from_loader(&loader).await
    }

    /// Load commands using a loader.
    pub async fn load_from_loader(&mut self, loader: &CommandLoader) -> Result<(), LoaderError> {
        let commands = loader.load_all().await?;

        for cmd in commands {
            self.register(cmd);
        }

        Ok(())
    }

    /// Register a command in the registry.
    ///
    /// If a command with the same name exists, it will be replaced.
    pub fn register(&mut self, command: Command) {
        // Register aliases
        for alias in &command.config.aliases {
            self.aliases.insert(alias.clone(), command.name.clone());
        }

        // Register command
        self.commands.insert(command.name.clone(), command);
    }

    /// Unregister a command by name.
    pub fn unregister(&mut self, name: &str) -> Option<Command> {
        if let Some(cmd) = self.commands.remove(name) {
            // Remove aliases
            for alias in &cmd.config.aliases {
                self.aliases.remove(alias);
            }
            Some(cmd)
        } else {
            None
        }
    }

    /// Get a command by name or alias.
    pub fn get(&self, name: &str) -> Option<&Command> {
        // Try direct lookup first
        if let Some(cmd) = self.commands.get(name) {
            return Some(cmd);
        }

        // Try alias lookup
        if let Some(real_name) = self.aliases.get(name) {
            return self.commands.get(real_name);
        }

        None
    }

    /// Check if a command exists by name or alias.
    pub fn contains(&self, name: &str) -> bool {
        self.commands.contains_key(name) || self.aliases.contains_key(name)
    }

    /// Get all registered commands.
    pub fn list(&self) -> Vec<&Command> {
        self.commands.values().collect()
    }

    /// Get all command names.
    pub fn names(&self) -> Vec<&str> {
        self.commands.keys().map(String::as_str).collect()
    }

    /// Get the number of registered commands.
    pub fn len(&self) -> usize {
        self.commands.len()
    }

    /// Check if the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }

    /// Clear all commands from the registry.
    pub fn clear(&mut self) {
        self.commands.clear();
        self.aliases.clear();
    }

    /// Reload commands from default directories.
    pub async fn reload(&mut self) -> Result<(), LoaderError> {
        self.clear();
        let loader = CommandLoader::new();
        self.load_from_loader(&loader).await
    }

    /// Find commands matching a prefix (for autocompletion).
    pub fn find_by_prefix(&self, prefix: &str) -> Vec<&Command> {
        let prefix_lower = prefix.to_lowercase();

        self.commands
            .values()
            .filter(|cmd| {
                cmd.name.to_lowercase().starts_with(&prefix_lower)
                    || cmd
                        .config
                        .aliases
                        .iter()
                        .any(|a| a.to_lowercase().starts_with(&prefix_lower))
            })
            .collect()
    }

    /// Search commands by name or description.
    pub fn search(&self, query: &str) -> Vec<&Command> {
        let query_lower = query.to_lowercase();

        self.commands
            .values()
            .filter(|cmd| {
                cmd.name.to_lowercase().contains(&query_lower)
                    || cmd.description().to_lowercase().contains(&query_lower)
            })
            .collect()
    }

    /// Get commands grouped by their source directory.
    pub fn by_source(&self) -> HashMap<PathBuf, Vec<&Command>> {
        let mut grouped: HashMap<PathBuf, Vec<&Command>> = HashMap::new();

        for cmd in self.commands.values() {
            if let Some(parent) = cmd.source_path.parent() {
                grouped.entry(parent.to_path_buf()).or_default().push(cmd);
            }
        }

        grouped
    }

    /// Merge another registry into this one.
    ///
    /// Commands from the other registry will overwrite existing commands.
    pub fn merge(&mut self, other: CommandRegistry) {
        for (_, cmd) in other.commands {
            self.register(cmd);
        }
    }

    /// Create an iterator over all commands.
    pub fn iter(&self) -> impl Iterator<Item = &Command> {
        self.commands.values()
    }
}

impl IntoIterator for CommandRegistry {
    type Item = Command;
    type IntoIter = std::collections::hash_map::IntoValues<String, Command>;

    fn into_iter(self) -> Self::IntoIter {
        self.commands.into_values()
    }
}

impl<'a> IntoIterator for &'a CommandRegistry {
    type Item = &'a Command;
    type IntoIter = std::collections::hash_map::Values<'a, String, Command>;

    fn into_iter(self) -> Self::IntoIter {
        self.commands.values()
    }
}

impl FromIterator<Command> for CommandRegistry {
    fn from_iter<I: IntoIterator<Item = Command>>(iter: I) -> Self {
        let mut registry = Self::new();
        for cmd in iter {
            registry.register(cmd);
        }
        registry
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::CommandConfig;

    fn make_command(name: &str, description: &str) -> Command {
        Command::new(
            name,
            CommandConfig {
                description: Some(description.to_string()),
                ..Default::default()
            },
            format!("Template for {name}"),
            PathBuf::from(format!("/test/{name}.md")),
        )
    }

    #[test]
    fn test_register_and_get() {
        let mut registry = CommandRegistry::new();

        let cmd = make_command("test", "A test command");
        registry.register(cmd);

        assert!(registry.contains("test"));
        assert_eq!(
            registry.get("test").map(|c| &c.name),
            Some(&"test".to_string())
        );
    }

    #[test]
    fn test_alias() {
        let mut registry = CommandRegistry::new();

        let cmd = Command::new(
            "build",
            CommandConfig {
                description: Some("Build the project".to_string()),
                aliases: vec!["b".to_string(), "compile".to_string()],
                ..Default::default()
            },
            "Build template",
            PathBuf::from("/test/build.md"),
        );

        registry.register(cmd);

        assert!(registry.contains("build"));
        assert!(registry.contains("b"));
        assert!(registry.contains("compile"));

        assert_eq!(
            registry.get("b").map(|c| &c.name),
            Some(&"build".to_string())
        );
    }

    #[test]
    fn test_unregister() {
        let mut registry = CommandRegistry::new();

        let cmd = Command::new(
            "temp",
            CommandConfig {
                aliases: vec!["t".to_string()],
                ..Default::default()
            },
            "Temp template",
            PathBuf::from("/test/temp.md"),
        );

        registry.register(cmd);
        assert!(registry.contains("temp"));
        assert!(registry.contains("t"));

        registry.unregister("temp");
        assert!(!registry.contains("temp"));
        assert!(!registry.contains("t"));
    }

    #[test]
    fn test_list_and_names() {
        let mut registry = CommandRegistry::new();

        registry.register(make_command("cmd1", "First"));
        registry.register(make_command("cmd2", "Second"));

        assert_eq!(registry.len(), 2);

        let names: Vec<_> = registry.names();
        assert!(names.contains(&"cmd1"));
        assert!(names.contains(&"cmd2"));
    }

    #[test]
    fn test_find_by_prefix() {
        let mut registry = CommandRegistry::new();

        registry.register(make_command("build", "Build"));
        registry.register(make_command("bump", "Bump"));
        registry.register(make_command("test", "Test"));

        let found = registry.find_by_prefix("bu");
        assert_eq!(found.len(), 2);

        let found = registry.find_by_prefix("te");
        assert_eq!(found.len(), 1);
    }

    #[test]
    fn test_search() {
        let mut registry = CommandRegistry::new();

        registry.register(make_command("build", "Build the project"));
        registry.register(make_command("test", "Run tests"));
        registry.register(make_command("deploy", "Deploy to production"));

        let found = registry.search("project");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "build");

        let found = registry.search("build");
        assert_eq!(found.len(), 1);
    }

    #[test]
    fn test_merge() {
        let mut registry1 = CommandRegistry::new();
        registry1.register(make_command("cmd1", "First"));

        let mut registry2 = CommandRegistry::new();
        registry2.register(make_command("cmd2", "Second"));

        registry1.merge(registry2);

        assert_eq!(registry1.len(), 2);
        assert!(registry1.contains("cmd1"));
        assert!(registry1.contains("cmd2"));
    }

    #[test]
    fn test_from_iterator() {
        let commands = vec![make_command("a", "A"), make_command("b", "B")];

        let registry: CommandRegistry = commands.into_iter().collect();

        assert_eq!(registry.len(), 2);
    }
}
