//! Command registry for managing and executing commands.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;

use crate::error::Result;

use super::configuration::{
    AutoCommand, CustomCommandsCommand, DelegatesCommand, ExperimentalCommand, HooksCommand,
    InstallGithubAppCommand, SpecCommand,
};
use super::custom::DynamicCustomCommand;
use super::development::{
    BgProcessCommand, BugCommand, DiagnosticsCommand, GhostCommand, IdeCommand, MultiEditCommand,
    ReviewCommand, ShareCommand,
};
use super::information::{
    AgentsCommand, ConfigCommand, CostCommand, ModelCommand, PluginsCommand, RateLimitsCommand,
    SkillsCommand,
};
use super::navigation::{
    ClearCommand, CompactCommand, ExitCommand, FavoriteCommand, HelpCommand, ResumeCommand,
    RewindCommand, SessionsCommand, UndoCommand,
};
use super::types::{CommandContext, CommandHandler, CommandInvocation, CommandMeta, CommandResult};

/// Command registry.
pub struct CommandRegistry {
    /// Registered commands.
    commands: RwLock<HashMap<String, Arc<dyn CommandHandler>>>,
    /// Alias mappings.
    aliases: RwLock<HashMap<String, String>>,
}

impl CommandRegistry {
    /// Create a new command registry.
    pub fn new() -> Self {
        Self {
            commands: RwLock::new(HashMap::new()),
            aliases: RwLock::new(HashMap::new()),
        }
    }

    /// Create a registry with built-in commands.
    pub fn with_builtins() -> Self {
        let registry = Self::new();

        // Register built-in commands synchronously during construction
        // We'll use a blocking approach since this is initialization
        let commands = registry.commands.blocking_write();
        let aliases = registry.aliases.blocking_write();

        let builtins: Vec<Arc<dyn CommandHandler>> = vec![
            Arc::new(HelpCommand),
            Arc::new(SkillsCommand),
            Arc::new(PluginsCommand),
            Arc::new(DelegatesCommand),
            Arc::new(AgentsCommand),
            Arc::new(AutoCommand),
            Arc::new(SpecCommand),
            Arc::new(ClearCommand),
            Arc::new(CompactCommand),
            Arc::new(UndoCommand),
            Arc::new(ConfigCommand),
            Arc::new(ModelCommand),
            Arc::new(CostCommand),
            Arc::new(BugCommand),
            Arc::new(ExitCommand),
            Arc::new(SessionsCommand),
            Arc::new(ResumeCommand),
            Arc::new(BgProcessCommand),
            Arc::new(IdeCommand),
            Arc::new(RewindCommand),
            Arc::new(InstallGithubAppCommand),
            Arc::new(FavoriteCommand),
            // Phase 2 commands
            Arc::new(ReviewCommand),
            Arc::new(ShareCommand),
            Arc::new(ExperimentalCommand),
            Arc::new(RateLimitsCommand),
            Arc::new(GhostCommand),
            Arc::new(MultiEditCommand),
            Arc::new(DiagnosticsCommand),
            Arc::new(HooksCommand),
            // Custom commands
            Arc::new(CustomCommandsCommand),
        ];

        drop(commands);
        drop(aliases);

        // Re-acquire locks properly for mutation
        for handler in builtins {
            let meta = handler.metadata();
            let name = meta.name.clone();

            registry
                .commands
                .blocking_write()
                .insert(name.clone(), handler.clone());

            for alias in &meta.aliases {
                registry
                    .aliases
                    .blocking_write()
                    .insert(alias.clone(), name.clone());
            }
        }

        registry
    }

    /// Register a command handler.
    pub async fn register(&self, handler: Arc<dyn CommandHandler>) {
        let meta = handler.metadata().clone();
        let name = meta.name.clone();
        let aliases_to_register: Vec<String> = meta.aliases.clone();

        self.commands.write().await.insert(name.clone(), handler);

        for alias in aliases_to_register {
            self.aliases.write().await.insert(alias, name.clone());
        }
    }

    /// Load and register custom commands from the CustomCommandRegistry.
    pub async fn load_custom_commands(
        &self,
        custom_registry: &crate::custom_command::CustomCommandRegistry,
    ) {
        let commands = custom_registry.list().await;

        for cmd in commands {
            let handler: Arc<dyn CommandHandler> = Arc::new(DynamicCustomCommand::new(cmd));
            self.register(handler).await;
        }
    }

    /// Execute a command.
    pub async fn execute(
        &self,
        invocation: &CommandInvocation,
        ctx: &CommandContext,
    ) -> Result<CommandResult> {
        // Resolve alias
        let name = {
            let aliases = self.aliases.read().await;
            aliases
                .get(&invocation.name)
                .cloned()
                .unwrap_or_else(|| invocation.name.clone())
        };

        // Get handler
        let handler = {
            let commands = self.commands.read().await;
            commands.get(&name).cloned()
        };

        match handler {
            Some(h) => h.execute(invocation, ctx).await,
            None => Ok(CommandResult::error(format!(
                "Unknown command: /{}",
                invocation.name
            ))),
        }
    }

    /// Check if input is a command.
    pub fn is_command(input: &str) -> bool {
        input.trim().starts_with('/')
    }

    /// List all commands.
    pub async fn list(&self) -> Vec<CommandMeta> {
        self.commands
            .read()
            .await
            .values()
            .map(|h| h.metadata().clone())
            .filter(|m| !m.hidden)
            .collect()
    }

    /// Get command by name.
    pub async fn get(&self, name: &str) -> Option<Arc<dyn CommandHandler>> {
        let name = {
            let aliases = self.aliases.read().await;
            aliases
                .get(name)
                .cloned()
                .unwrap_or_else(|| name.to_string())
        };

        self.commands.read().await.get(&name).cloned()
    }
}

impl Default for CommandRegistry {
    fn default() -> Self {
        Self::with_builtins()
    }
}
