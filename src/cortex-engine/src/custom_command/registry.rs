//! Custom command registry.
//!
//! Manages discovery, loading, and access to custom commands from:
//! - Personal: ~/.cortex/commands/*.md
//! - Project: .cortex/commands/*.md
//! - Config: [[commands]] in config.toml

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::RwLock;

use crate::error::Result;

use super::loader::{
    create_command_file, personal_commands_dir, project_commands_dir, scan_directory,
};
use super::template::{TemplateContext, expand_template};
use super::types::{CommandExecutionResult, CommandSource, CustomCommand, CustomCommandConfig};

/// Registry for custom commands.
pub struct CustomCommandRegistry {
    /// Loaded commands by name.
    commands: RwLock<HashMap<String, CustomCommand>>,
    /// Alias mappings (alias -> command name).
    aliases: RwLock<HashMap<String, String>>,
    /// Personal commands directory.
    personal_dir: PathBuf,
    /// Project commands directory.
    project_dir: Option<PathBuf>,
}

impl CustomCommandRegistry {
    /// Create a new custom command registry.
    pub fn new(cortex_home: &Path, project_root: Option<&Path>) -> Self {
        let personal_dir = personal_commands_dir(cortex_home);
        let project_dir = project_root.map(project_commands_dir);

        Self {
            commands: RwLock::new(HashMap::new()),
            aliases: RwLock::new(HashMap::new()),
            personal_dir,
            project_dir,
        }
    }

    /// Scan and load all custom commands.
    pub async fn scan(&self) -> Result<Vec<CustomCommand>> {
        let mut all_commands = Vec::new();
        let mut loaded_names: HashMap<String, CustomCommand> = HashMap::new();

        // Load project commands first (higher priority)
        if let Some(ref project_dir) = self.project_dir {
            if project_dir.exists() {
                let commands = scan_directory(project_dir, CommandSource::Project)?;
                for cmd in commands {
                    loaded_names.insert(cmd.name.clone(), cmd.clone());
                    all_commands.push(cmd);
                }
            }
        }

        // Load personal commands (lower priority, won't override project)
        if self.personal_dir.exists() {
            let commands = scan_directory(&self.personal_dir, CommandSource::Personal)?;
            for cmd in commands {
                if !loaded_names.contains_key(&cmd.name) {
                    loaded_names.insert(cmd.name.clone(), cmd.clone());
                    all_commands.push(cmd);
                }
            }
        }

        // Register all commands and aliases
        let mut registry = self.commands.write().await;
        let mut aliases = self.aliases.write().await;

        for cmd in &all_commands {
            registry.insert(cmd.name.clone(), cmd.clone());

            // Register aliases
            for alias in &cmd.aliases {
                aliases.insert(alias.clone(), cmd.name.clone());
            }
        }

        Ok(all_commands)
    }

    /// Load commands from config (TOML [[commands]] entries).
    pub async fn load_from_config(&self, configs: Vec<CustomCommandConfig>) -> Result<()> {
        let mut registry = self.commands.write().await;
        let mut aliases = self.aliases.write().await;

        for config in configs {
            let command: CustomCommand = config.into();

            if let Err(e) = command.validate() {
                tracing::warn!("Invalid command config '{}': {}", command.name, e);
                continue;
            }

            // Config commands have lowest priority (don't override file-based)
            if !registry.contains_key(&command.name) {
                // Register aliases
                for alias in &command.aliases {
                    aliases.insert(alias.clone(), command.name.clone());
                }

                registry.insert(command.name.clone(), command);
            }
        }

        Ok(())
    }

    /// Register a single command.
    pub async fn register(&self, command: CustomCommand) -> Result<()> {
        command
            .validate()
            .map_err(|e| crate::error::CortexError::InvalidInput(e))?;

        let mut registry = self.commands.write().await;
        let mut aliases = self.aliases.write().await;

        // Register aliases
        for alias in &command.aliases {
            aliases.insert(alias.clone(), command.name.clone());
        }

        registry.insert(command.name.clone(), command);

        Ok(())
    }

    /// Get a command by name or alias.
    pub async fn get(&self, name: &str) -> Option<CustomCommand> {
        // First check direct name
        let registry = self.commands.read().await;
        if let Some(cmd) = registry.get(name) {
            return Some(cmd.clone());
        }

        // Then check aliases
        let aliases = self.aliases.read().await;
        if let Some(actual_name) = aliases.get(name) {
            return registry.get(actual_name).cloned();
        }

        None
    }

    /// List all commands.
    pub async fn list(&self) -> Vec<CustomCommand> {
        self.commands.read().await.values().cloned().collect()
    }

    /// List commands by source.
    pub async fn list_by_source(&self, source: CommandSource) -> Vec<CustomCommand> {
        self.commands
            .read()
            .await
            .values()
            .filter(|c| c.source == source)
            .cloned()
            .collect()
    }

    /// Check if a command exists.
    pub async fn exists(&self, name: &str) -> bool {
        let registry = self.commands.read().await;
        if registry.contains_key(name) {
            return true;
        }

        let aliases = self.aliases.read().await;
        aliases.contains_key(name)
    }

    /// Execute a custom command with the given input.
    pub async fn execute(
        &self,
        name: &str,
        ctx: &TemplateContext,
    ) -> Option<CommandExecutionResult> {
        let command = self.get(name).await?;

        let prompt = expand_template(&command.template, ctx);

        Some(CommandExecutionResult {
            prompt,
            agent: command.agent,
            model: command.model,
            subtask: command.subtask,
        })
    }

    /// Create a new command file.
    pub async fn create(&self, command: CustomCommand, location: CommandSource) -> Result<PathBuf> {
        let dir = match location {
            CommandSource::Personal => &self.personal_dir,
            CommandSource::Project => self.project_dir.as_ref().ok_or_else(|| {
                crate::error::CortexError::InvalidInput("No project directory".to_string())
            })?,
            _ => {
                return Err(crate::error::CortexError::InvalidInput(
                    "Can only create personal or project commands".to_string(),
                ));
            }
        };

        let path = create_command_file(dir, &command)?;

        // Reload to pick up the new command
        self.scan().await?;

        Ok(path)
    }

    /// Delete a command.
    pub async fn delete(&self, name: &str) -> Result<()> {
        let command = self.get(name).await.ok_or_else(|| {
            crate::error::CortexError::NotFound(format!("Command not found: {name}"))
        })?;

        let path = command.source_path.ok_or_else(|| {
            crate::error::CortexError::InvalidInput(
                "Cannot delete config-defined commands".to_string(),
            )
        })?;

        std::fs::remove_file(&path)?;

        // Reload
        self.scan().await?;

        Ok(())
    }

    /// Reload all commands.
    pub async fn reload(&self) -> Result<Vec<CustomCommand>> {
        self.commands.write().await.clear();
        self.aliases.write().await.clear();
        self.scan().await
    }

    /// Get command names for autocompletion.
    pub async fn get_names(&self) -> Vec<String> {
        let registry = self.commands.read().await;
        let aliases = self.aliases.read().await;

        let mut names: Vec<String> = registry.keys().cloned().collect();
        names.extend(aliases.keys().cloned());
        names.sort();
        names.dedup();
        names
    }
}

/// Global custom command registry.
static GLOBAL_REGISTRY: std::sync::OnceLock<Arc<CustomCommandRegistry>> =
    std::sync::OnceLock::new();

/// Initialize the global registry.
pub fn init_global_registry(
    cortex_home: &Path,
    project_root: Option<&Path>,
) -> Arc<CustomCommandRegistry> {
    GLOBAL_REGISTRY
        .get_or_init(|| Arc::new(CustomCommandRegistry::new(cortex_home, project_root)))
        .clone()
}

/// Get the global registry (panics if not initialized).
pub fn global_registry() -> Arc<CustomCommandRegistry> {
    GLOBAL_REGISTRY
        .get()
        .cloned()
        .expect("Custom command registry not initialized")
}

/// Try to get the global registry.
pub fn try_global_registry() -> Option<Arc<CustomCommandRegistry>> {
    GLOBAL_REGISTRY.get().cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    async fn setup_test_registry() -> (CustomCommandRegistry, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let cortex_home = temp_dir.path().join(".cortex");
        std::fs::create_dir_all(&cortex_home).unwrap();

        let registry = CustomCommandRegistry::new(&cortex_home, None);
        (registry, temp_dir)
    }

    #[tokio::test]
    async fn test_register_and_get() {
        let (registry, _temp) = setup_test_registry().await;

        let cmd = CustomCommand::new("test", "Test command", "Do: {{input}}").with_alias("t");

        registry.register(cmd).await.unwrap();

        // Get by name
        let retrieved = registry.get("test").await.unwrap();
        assert_eq!(retrieved.name, "test");

        // Get by alias
        let by_alias = registry.get("t").await.unwrap();
        assert_eq!(by_alias.name, "test");
    }

    #[tokio::test]
    async fn test_execute_command() {
        let (registry, _temp) = setup_test_registry().await;

        let cmd = CustomCommand::new("greet", "Greet someone", "Hello, {{input}}!");
        registry.register(cmd).await.unwrap();

        let ctx = TemplateContext::new("World");
        let result = registry.execute("greet", &ctx).await.unwrap();

        assert_eq!(result.prompt, "Hello, World!");
    }

    #[tokio::test]
    async fn test_list_commands() {
        let (registry, _temp) = setup_test_registry().await;

        registry
            .register(CustomCommand::new("a", "A", "{{input}}"))
            .await
            .unwrap();
        registry
            .register(CustomCommand::new("b", "B", "{{input}}"))
            .await
            .unwrap();

        let commands = registry.list().await;
        assert_eq!(commands.len(), 2);
    }

    #[tokio::test]
    async fn test_exists() {
        let (registry, _temp) = setup_test_registry().await;

        let cmd = CustomCommand::new("exists", "Test", "{{input}}").with_alias("e");
        registry.register(cmd).await.unwrap();

        assert!(registry.exists("exists").await);
        assert!(registry.exists("e").await);
        assert!(!registry.exists("nonexistent").await);
    }

    #[tokio::test]
    async fn test_load_from_config() {
        let (registry, _temp) = setup_test_registry().await;

        let configs = vec![CustomCommandConfig {
            name: "config-cmd".to_string(),
            description: "From config".to_string(),
            template: "Config: {{input}}".to_string(),
            ..Default::default()
        }];

        registry.load_from_config(configs).await.unwrap();

        let cmd = registry.get("config-cmd").await.unwrap();
        assert_eq!(cmd.source, CommandSource::Config);
    }
}
