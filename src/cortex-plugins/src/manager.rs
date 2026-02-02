//! Plugin manager - the main interface for the plugin system.

use std::path::Path;
use std::sync::Arc;

use crate::commands::PluginCommandRegistry;
use crate::config::PluginConfig;
use crate::events::EventBus;
use crate::hooks::HookRegistry;
use crate::loader::{DiscoveredPlugin, PluginLoader};
use crate::plugin::{Plugin, PluginInfo, PluginStatus};
use crate::registry::PluginRegistry;
use crate::runtime::WasmRuntime;
use crate::{PluginContext, PluginError, Result};

/// Plugin manager - the main entry point for the plugin system.
///
/// The manager handles:
/// - Plugin discovery and loading
/// - Plugin lifecycle (init, shutdown)
/// - Command and hook registration
/// - Event distribution
pub struct PluginManager {
    /// Configuration
    config: PluginConfig,

    /// WASM runtime (kept for future use)
    #[allow(dead_code)]
    runtime: Arc<WasmRuntime>,

    /// Plugin loader
    loader: PluginLoader,

    /// Plugin registry
    registry: Arc<PluginRegistry>,

    /// Command registry
    commands: Arc<PluginCommandRegistry>,

    /// Hook registry
    hooks: Arc<HookRegistry>,

    /// Event bus
    events: Arc<EventBus>,
}

impl PluginManager {
    /// Create a new plugin manager.
    pub async fn new(config: PluginConfig) -> Result<Self> {
        let runtime = Arc::new(WasmRuntime::new()?);
        let loader = PluginLoader::new(config.clone(), runtime.clone());
        let registry = Arc::new(PluginRegistry::new());
        let commands = Arc::new(PluginCommandRegistry::new());
        let hooks = Arc::new(HookRegistry::new());
        let events = Arc::new(EventBus::new());

        Ok(Self {
            config,
            runtime,
            loader,
            registry,
            commands,
            hooks,
            events,
        })
    }

    /// Create a plugin manager with default configuration.
    pub async fn default_manager() -> Result<Self> {
        Self::new(PluginConfig::default()).await
    }

    // ========== Discovery and Loading ==========

    /// Discover plugins in all search paths.
    pub async fn discover(&self) -> Vec<DiscoveredPlugin> {
        self.loader.discover().await
    }

    /// Discover and load all plugins.
    pub async fn discover_and_load(&self) -> Result<Vec<String>> {
        let discovered = self.discover().await;
        let mut loaded = Vec::new();

        for plugin in discovered {
            // Check if plugin is enabled
            if !self.config.is_plugin_enabled(plugin.id()) {
                tracing::debug!("Plugin {} is disabled, skipping", plugin.id());
                continue;
            }

            // Check if already loaded
            if self.registry.is_registered(plugin.id()).await {
                tracing::debug!("Plugin {} is already loaded", plugin.id());
                continue;
            }

            match self.load_discovered(&plugin).await {
                Ok(()) => {
                    loaded.push(plugin.id().to_string());
                }
                Err(e) => {
                    tracing::error!("Failed to load plugin {}: {}", plugin.id(), e);
                }
            }
        }

        Ok(loaded)
    }

    /// Load a discovered plugin.
    async fn load_discovered(&self, discovered: &DiscoveredPlugin) -> Result<()> {
        // Load the WASM plugin
        let plugin = self.loader.load(discovered)?;
        let plugin_id = plugin.info().id.clone();

        // Register the plugin
        self.registry.register(Box::new(plugin)).await?;

        // Register commands from manifest
        self.register_plugin_commands(&plugin_id, &discovered.manifest)
            .await?;

        // Publish load event
        self.events
            .publish(crate::events::Event::PluginLoaded {
                plugin_id: plugin_id.clone(),
            })
            .await;

        Ok(())
    }

    /// Load a plugin from a path.
    pub async fn load_from_path(&self, path: &Path) -> Result<String> {
        let plugin = self.loader.load_from_path(path).await?;
        let plugin_id = plugin.info().id.clone();
        let manifest = plugin.manifest().clone();

        self.registry.register(Box::new(plugin)).await?;
        self.register_plugin_commands(&plugin_id, &manifest).await?;

        self.events
            .publish(crate::events::Event::PluginLoaded {
                plugin_id: plugin_id.clone(),
            })
            .await;

        Ok(plugin_id)
    }

    /// Register commands from a plugin manifest.
    async fn register_plugin_commands(
        &self,
        plugin_id: &str,
        manifest: &crate::manifest::PluginManifest,
    ) -> Result<()> {
        for cmd_manifest in &manifest.commands {
            let cmd = crate::commands::PluginCommand::from_manifest(plugin_id, cmd_manifest);

            // Create executor that calls the plugin
            let registry = self.registry.clone();
            let pid = plugin_id.to_string();
            let cmd_name = cmd.name.clone();

            let executor: crate::commands::CommandExecutor = Arc::new(move |args, ctx| {
                let registry = registry.clone();
                let pid = pid.clone();
                let cmd_name = cmd_name.clone();
                let ctx = ctx.clone();

                Box::pin(async move {
                    let handle = registry
                        .get(&pid)
                        .await
                        .ok_or_else(|| PluginError::NotFound(pid.clone()))?;

                    let result = handle.execute_command(&cmd_name, args, &ctx).await?;

                    Ok(crate::commands::PluginCommandResult::success(result))
                })
            });

            self.commands.register(cmd, executor).await?;
        }

        Ok(())
    }

    /// Unload a plugin.
    pub async fn unload(&self, plugin_id: &str) -> Result<()> {
        // Unregister commands
        self.commands.unregister_plugin(plugin_id).await;

        // Unregister hooks
        self.hooks.unregister_plugin(plugin_id).await;

        // Unregister event handlers
        self.events.unsubscribe_plugin(plugin_id).await;

        // Unregister plugin
        self.registry.unregister(plugin_id).await?;

        // Publish unload event
        self.events
            .publish(crate::events::Event::PluginUnloaded {
                plugin_id: plugin_id.to_string(),
            })
            .await;

        Ok(())
    }

    // ========== Plugin Lifecycle ==========

    /// Initialize all plugins.
    pub async fn init_all(&self) -> Result<()> {
        let results = self.registry.init_all().await;

        for (plugin_id, result) in results {
            if let Err(e) = result {
                tracing::error!("Failed to initialize plugin {}: {}", plugin_id, e);
                self.events
                    .publish(crate::events::Event::PluginError {
                        plugin_id,
                        error: e.to_string(),
                    })
                    .await;
            }
        }

        Ok(())
    }

    /// Shutdown all plugins.
    pub async fn shutdown_all(&self) -> Result<()> {
        let results = self.registry.shutdown_all().await;

        for (plugin_id, result) in results {
            if let Err(e) = result {
                tracing::error!("Failed to shutdown plugin {}: {}", plugin_id, e);
            }
        }

        Ok(())
    }

    // ========== Plugin Management ==========

    /// List all loaded plugins.
    pub async fn list_plugins(&self) -> Vec<PluginInfo> {
        self.registry.list().await
    }

    /// List plugins with status.
    pub async fn list_plugins_status(&self) -> Vec<PluginStatus> {
        self.registry.list_status().await
    }

    /// Get a plugin by ID.
    pub async fn get_plugin(&self, id: &str) -> Option<PluginInfo> {
        self.registry.get(id).await.map(|h| {
            // This is a bit awkward, but we need to get the info synchronously
            let rt = tokio::runtime::Handle::current();
            rt.block_on(h.info())
        })
    }

    /// Check if a plugin is loaded.
    pub async fn is_loaded(&self, id: &str) -> bool {
        self.registry.is_registered(id).await
    }

    /// Enable a plugin.
    pub async fn enable(&self, plugin_id: &str) -> Result<()> {
        let handle = self
            .registry
            .get(plugin_id)
            .await
            .ok_or_else(|| PluginError::NotFound(plugin_id.to_string()))?;

        let mut plugin = handle.write().await;
        plugin.init().await?;

        Ok(())
    }

    /// Disable a plugin (without unloading).
    pub async fn disable(&self, plugin_id: &str) -> Result<()> {
        let handle = self
            .registry
            .get(plugin_id)
            .await
            .ok_or_else(|| PluginError::NotFound(plugin_id.to_string()))?;

        let mut plugin = handle.write().await;
        plugin.shutdown().await?;

        Ok(())
    }

    // ========== Commands ==========

    /// Execute a plugin command.
    pub async fn execute_command(
        &self,
        name: &str,
        args: Vec<String>,
        ctx: &PluginContext,
    ) -> Result<crate::commands::PluginCommandResult> {
        self.commands.execute(name, args, ctx).await
    }

    /// Check if a command exists.
    pub async fn has_command(&self, name: &str) -> bool {
        self.commands.exists(name).await
    }

    /// List all plugin commands.
    pub async fn list_commands(&self) -> Vec<crate::commands::PluginCommand> {
        self.commands.list_visible().await
    }

    /// Get command registry.
    pub fn command_registry(&self) -> &Arc<PluginCommandRegistry> {
        &self.commands
    }

    // ========== Hooks ==========

    /// Get hook registry.
    pub fn hook_registry(&self) -> &Arc<HookRegistry> {
        &self.hooks
    }

    /// Create a hook dispatcher.
    pub fn hook_dispatcher(&self) -> crate::hooks::HookDispatcher {
        crate::hooks::HookDispatcher::new(self.hooks.clone())
    }

    // ========== Events ==========

    /// Get event bus.
    pub fn event_bus(&self) -> &Arc<EventBus> {
        &self.events
    }

    /// Publish an event.
    pub async fn publish_event(&self, event: crate::events::Event) {
        self.events.publish(event).await;
    }

    // ========== Configuration ==========

    /// Get configuration.
    pub fn config(&self) -> &PluginConfig {
        &self.config
    }

    /// Get plugin configuration.
    pub fn get_plugin_config(&self, plugin_id: &str) -> Option<&serde_json::Value> {
        self.config.get_plugin_config(plugin_id)
    }

    // ========== Statistics ==========

    /// Get plugin count.
    pub async fn plugin_count(&self) -> usize {
        self.registry.count().await
    }

    /// Get command count.
    pub async fn command_count(&self) -> usize {
        self.commands.list().await.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[tokio::test]
    async fn test_create_manager() {
        let config = PluginConfig::default();
        let manager = PluginManager::new(config).await;
        assert!(manager.is_ok());
    }

    #[tokio::test]
    async fn test_discover_empty() {
        let config = PluginConfig {
            search_paths: vec![PathBuf::from("/nonexistent")],
            ..Default::default()
        };
        let manager = PluginManager::new(config).await.unwrap();
        let plugins = manager.discover().await;
        assert!(plugins.is_empty());
    }
}
