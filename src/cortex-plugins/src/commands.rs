//! Plugin command system.
//!
//! Allows plugins to register custom slash commands that integrate
//! with the Cortex TUI command system.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::manifest::PluginCommandManifest;
use crate::{PluginContext, PluginError, Result};

/// A command provided by a plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginCommand {
    /// Plugin that provides this command
    pub plugin_id: String,

    /// Command name (without leading /)
    pub name: String,

    /// Command aliases
    pub aliases: Vec<String>,

    /// Command description
    pub description: String,

    /// Usage example
    pub usage: Option<String>,

    /// Command arguments
    pub args: Vec<PluginCommandArg>,

    /// Whether the command is hidden
    pub hidden: bool,

    /// Category for grouping
    pub category: Option<String>,
}

impl PluginCommand {
    /// Create from manifest definition.
    pub fn from_manifest(plugin_id: &str, manifest: &PluginCommandManifest) -> Self {
        Self {
            plugin_id: plugin_id.to_string(),
            name: manifest.name.clone(),
            aliases: manifest.aliases.clone(),
            description: manifest.description.clone(),
            usage: manifest.usage.clone(),
            args: manifest
                .args
                .iter()
                .map(|a| PluginCommandArg {
                    name: a.name.clone(),
                    description: a.description.clone(),
                    required: a.required,
                    default: a.default.clone(),
                    arg_type: a.arg_type.clone(),
                })
                .collect(),
            hidden: manifest.hidden,
            category: manifest.category.clone(),
        }
    }

    /// Check if this command matches a name or alias.
    pub fn matches(&self, name: &str) -> bool {
        self.name.eq_ignore_ascii_case(name)
            || self.aliases.iter().any(|a| a.eq_ignore_ascii_case(name))
    }

    /// Get all names for this command (primary + aliases).
    pub fn all_names(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.name.as_str()).chain(self.aliases.iter().map(|s| s.as_str()))
    }
}

/// Command argument definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginCommandArg {
    /// Argument name
    pub name: String,

    /// Argument description
    pub description: String,

    /// Whether the argument is required
    pub required: bool,

    /// Default value
    pub default: Option<String>,

    /// Argument type
    pub arg_type: String,
}

/// Result of a plugin command execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginCommandResult {
    /// Whether the command succeeded
    pub success: bool,

    /// Output message
    pub message: Option<String>,

    /// Error message if failed
    pub error: Option<String>,

    /// Additional data
    pub data: Option<serde_json::Value>,

    /// Action to take in the UI
    pub action: Option<CommandAction>,
}

impl PluginCommandResult {
    /// Create a success result with a message.
    pub fn success(message: impl Into<String>) -> Self {
        Self {
            success: true,
            message: Some(message.into()),
            error: None,
            data: None,
            action: None,
        }
    }

    /// Create an error result.
    pub fn error(error: impl Into<String>) -> Self {
        Self {
            success: false,
            message: None,
            error: Some(error.into()),
            data: None,
            action: None,
        }
    }

    /// Add an action to the result.
    pub fn with_action(mut self, action: CommandAction) -> Self {
        self.action = Some(action);
        self
    }

    /// Add data to the result.
    pub fn with_data(mut self, data: serde_json::Value) -> Self {
        self.data = Some(data);
        self
    }
}

/// Actions that a plugin command can request.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CommandAction {
    /// Display a message
    Message { content: String },

    /// Open a modal
    OpenModal {
        modal_type: String,
        data: Option<serde_json::Value>,
    },

    /// Set a configuration value
    SetValue { key: String, value: String },

    /// Toggle a setting
    Toggle { key: String },

    /// Start an async operation
    Async { operation: String },

    /// Navigate to a view
    Navigate { view: String },

    /// Copy to clipboard
    Clipboard { content: String },

    /// Open external URL
    OpenUrl { url: String },
}

/// Command executor function type.
pub type CommandExecutor = Arc<
    dyn Fn(
            Vec<String>,
            &PluginContext,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<PluginCommandResult>> + Send>,
        > + Send
        + Sync,
>;

/// Registry for plugin commands.
pub struct PluginCommandRegistry {
    /// Registered commands by name
    commands: RwLock<HashMap<String, PluginCommand>>,

    /// Command executors by plugin_id::command_name
    executors: RwLock<HashMap<String, CommandExecutor>>,

    /// Alias to command name mapping
    aliases: RwLock<HashMap<String, String>>,
}

impl PluginCommandRegistry {
    /// Create a new command registry.
    pub fn new() -> Self {
        Self {
            commands: RwLock::new(HashMap::new()),
            executors: RwLock::new(HashMap::new()),
            aliases: RwLock::new(HashMap::new()),
        }
    }

    /// Register a command.
    pub async fn register(&self, command: PluginCommand, executor: CommandExecutor) -> Result<()> {
        let name = command.name.clone();
        let plugin_id = command.plugin_id.clone();

        // Check for conflicts
        {
            let commands = self.commands.read().await;
            if commands.contains_key(&name) {
                return Err(PluginError::CommandError(format!(
                    "Command '{}' is already registered",
                    name
                )));
            }
        }

        // Register aliases
        {
            let mut aliases = self.aliases.write().await;
            for alias in &command.aliases {
                if aliases.contains_key(alias) {
                    return Err(PluginError::CommandError(format!(
                        "Alias '{}' is already registered",
                        alias
                    )));
                }
                aliases.insert(alias.clone(), name.clone());
            }
        }

        // Register command
        {
            let mut commands = self.commands.write().await;
            commands.insert(name.clone(), command);
        }

        // Register executor
        {
            let mut executors = self.executors.write().await;
            let key = format!("{}::{}", plugin_id, name);
            executors.insert(key, executor);
        }

        tracing::debug!("Registered command: /{} from plugin {}", name, plugin_id);
        Ok(())
    }

    /// Unregister all commands for a plugin.
    pub async fn unregister_plugin(&self, plugin_id: &str) {
        // Find commands to remove
        let to_remove: Vec<String> = {
            let commands = self.commands.read().await;
            commands
                .values()
                .filter(|c| c.plugin_id == plugin_id)
                .map(|c| c.name.clone())
                .collect()
        };

        // Remove aliases
        {
            let mut aliases = self.aliases.write().await;
            aliases.retain(|_, name| !to_remove.contains(name));
        }

        // Remove commands
        {
            let mut commands = self.commands.write().await;
            for name in &to_remove {
                commands.remove(name);
            }
        }

        // Remove executors
        {
            let mut executors = self.executors.write().await;
            executors.retain(|key, _| !key.starts_with(&format!("{}::", plugin_id)));
        }

        tracing::debug!(
            "Unregistered {} commands from plugin {}",
            to_remove.len(),
            plugin_id
        );
    }

    /// Get a command by name or alias.
    pub async fn get(&self, name: &str) -> Option<PluginCommand> {
        let name_lower = name.to_lowercase();

        // Try direct lookup
        {
            let commands = self.commands.read().await;
            if let Some(cmd) = commands.get(&name_lower) {
                return Some(cmd.clone());
            }
        }

        // Try alias lookup
        let resolved_name = {
            let aliases = self.aliases.read().await;
            aliases.get(&name_lower).cloned()
        };

        if let Some(resolved) = resolved_name {
            let commands = self.commands.read().await;
            return commands.get(&resolved).cloned();
        }

        // Try case-insensitive scan
        {
            let commands = self.commands.read().await;
            for (cmd_name, cmd) in commands.iter() {
                if cmd_name.eq_ignore_ascii_case(&name_lower) {
                    return Some(cmd.clone());
                }
            }
        }

        None
    }

    /// Execute a command.
    pub async fn execute(
        &self,
        name: &str,
        args: Vec<String>,
        ctx: &PluginContext,
    ) -> Result<PluginCommandResult> {
        let command = self
            .get(name)
            .await
            .ok_or_else(|| PluginError::CommandError(format!("Command '{}' not found", name)))?;

        let executor_key = format!("{}::{}", command.plugin_id, command.name);

        let executor = {
            let executors = self.executors.read().await;
            executors.get(&executor_key).cloned().ok_or_else(|| {
                PluginError::CommandError(format!("No executor found for command '{}'", name))
            })?
        };

        executor(args, ctx).await
    }

    /// Check if a command exists.
    pub async fn exists(&self, name: &str) -> bool {
        self.get(name).await.is_some()
    }

    /// List all registered commands.
    pub async fn list(&self) -> Vec<PluginCommand> {
        let commands = self.commands.read().await;
        let mut cmds: Vec<_> = commands.values().cloned().collect();
        cmds.sort_by(|a, b| a.name.cmp(&b.name));
        cmds
    }

    /// List visible (non-hidden) commands.
    pub async fn list_visible(&self) -> Vec<PluginCommand> {
        self.list()
            .await
            .into_iter()
            .filter(|c| !c.hidden)
            .collect()
    }

    /// List commands for a specific plugin.
    pub async fn list_for_plugin(&self, plugin_id: &str) -> Vec<PluginCommand> {
        self.list()
            .await
            .into_iter()
            .filter(|c| c.plugin_id == plugin_id)
            .collect()
    }

    /// Get all command names including aliases.
    pub async fn all_names(&self) -> Vec<String> {
        let commands = self.commands.read().await;
        let aliases = self.aliases.read().await;

        let mut names: Vec<_> = commands
            .keys()
            .cloned()
            .chain(aliases.keys().cloned())
            .collect();
        names.sort();
        names.dedup();
        names
    }
}

impl Default for PluginCommandRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_command() -> PluginCommand {
        PluginCommand {
            plugin_id: "test-plugin".to_string(),
            name: "test".to_string(),
            aliases: vec!["t".to_string()],
            description: "A test command".to_string(),
            usage: Some("/test [arg]".to_string()),
            args: vec![PluginCommandArg {
                name: "arg".to_string(),
                description: "Test argument".to_string(),
                required: false,
                default: None,
                arg_type: "string".to_string(),
            }],
            hidden: false,
            category: None,
        }
    }

    fn create_test_executor() -> CommandExecutor {
        Arc::new(|_args, _ctx| {
            Box::pin(async { Ok(PluginCommandResult::success("Test executed")) })
        })
    }

    #[tokio::test]
    async fn test_register_command() {
        let registry = PluginCommandRegistry::new();
        let command = create_test_command();
        let executor = create_test_executor();

        registry.register(command, executor).await.unwrap();

        assert!(registry.exists("test").await);
        assert!(registry.exists("t").await); // alias
    }

    #[tokio::test]
    async fn test_execute_command() {
        let registry = PluginCommandRegistry::new();
        let command = create_test_command();
        let executor = create_test_executor();

        registry.register(command, executor).await.unwrap();

        let ctx = PluginContext::default();
        let result = registry.execute("test", vec![], &ctx).await.unwrap();

        assert!(result.success);
        assert_eq!(result.message, Some("Test executed".to_string()));
    }

    #[tokio::test]
    async fn test_unregister_plugin() {
        let registry = PluginCommandRegistry::new();
        let command = create_test_command();
        let executor = create_test_executor();

        registry.register(command, executor).await.unwrap();
        assert!(registry.exists("test").await);

        registry.unregister_plugin("test-plugin").await;
        assert!(!registry.exists("test").await);
    }

    #[test]
    fn test_command_result() {
        let result = PluginCommandResult::success("OK")
            .with_action(CommandAction::Message {
                content: "Hello".to_string(),
            })
            .with_data(serde_json::json!({"key": "value"}));

        assert!(result.success);
        assert!(result.action.is_some());
        assert!(result.data.is_some());
    }
}
