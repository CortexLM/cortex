//! Plugin manager module.
//!
//! Provides a centralized manager for plugin lifecycle, registration,
//! and hook dispatch.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use once_cell::sync::OnceCell;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use super::hooks::{HookDispatcher, HookRegistration};
use super::loader::{PluginFormat, PluginLoader, PluginSource};
use super::types::{
    HookContext, HookResponse, Plugin, PluginConfig, PluginHook, PluginInfo, PluginInstance,
    PluginPermission, PluginState,
};
use crate::error::{CortexError, Result};

/// Global plugin manager instance.
static GLOBAL_MANAGER: OnceCell<Arc<PluginManager>> = OnceCell::new();

/// Get the global plugin manager.
pub fn global_manager() -> Option<Arc<PluginManager>> {
    GLOBAL_MANAGER.get().cloned()
}

/// Initialize the global plugin manager.
pub fn init_global_manager(manager: Arc<PluginManager>) -> Result<()> {
    GLOBAL_MANAGER
        .set(manager)
        .map_err(|_| CortexError::Internal("Global plugin manager already initialized".into()))
}

/// Plugin manager that handles plugin lifecycle and hook dispatch.
pub struct PluginManager {
    /// Registered plugin instances.
    plugins: RwLock<HashMap<String, PluginInstance>>,
    /// Active plugin implementations (for plugins that implement Plugin trait).
    active_plugins: RwLock<HashMap<String, Arc<dyn Plugin>>>,
    /// Hook dispatcher.
    hook_dispatcher: RwLock<HookDispatcher>,
    /// Granted permissions per plugin.
    granted_permissions: RwLock<HashMap<String, Vec<PluginPermission>>>,
    /// Plugin configurations.
    configs: RwLock<HashMap<String, PluginConfig>>,
    /// Cortex home directory.
    cortex_home: PathBuf,
    /// Project root directory.
    project_root: Option<PathBuf>,
}

impl PluginManager {
    /// Create a new plugin manager.
    pub fn new(cortex_home: impl Into<PathBuf>) -> Self {
        Self {
            plugins: RwLock::new(HashMap::new()),
            active_plugins: RwLock::new(HashMap::new()),
            hook_dispatcher: RwLock::new(HookDispatcher::new()),
            granted_permissions: RwLock::new(HashMap::new()),
            configs: RwLock::new(HashMap::new()),
            cortex_home: cortex_home.into(),
            project_root: None,
        }
    }

    /// Set project root for project-specific plugins.
    pub fn with_project_root(mut self, project_root: impl Into<PathBuf>) -> Self {
        self.project_root = Some(project_root.into());
        self
    }

    /// Add plugin configurations.
    pub async fn add_configs(&self, configs: Vec<PluginConfig>) {
        let mut cfg = self.configs.write().await;
        for config in configs {
            cfg.insert(config.name.clone(), config);
        }
    }

    /// Register a plugin instance.
    pub async fn register_plugin(&self, instance: PluginInstance) -> Result<()> {
        let name = instance.info.name.clone();
        info!("Registering plugin: {}", name);

        // Register hook handlers if plugin has hooks
        let hooks = match instance.info.plugin_type {
            super::types::PluginKind::Hook => {
                // Hook plugins register for specific events
                vec![
                    PluginHook::SessionStarting,
                    PluginHook::SessionEnded,
                    PluginHook::ToolBeforeCall,
                    PluginHook::ToolAfterCall,
                ]
            }
            _ => Vec::new(),
        };

        // Register with hook dispatcher
        if !hooks.is_empty() {
            let mut dispatcher = self.hook_dispatcher.write().await;
            for hook in hooks {
                dispatcher.register(HookRegistration {
                    plugin_name: name.clone(),
                    hook,
                    priority: instance.config.priority,
                    enabled: instance.config.enabled,
                });
            }
        }

        self.plugins.write().await.insert(name, instance);
        Ok(())
    }

    /// Register a plugin implementation (for plugins that implement Plugin trait).
    pub async fn register_plugin_impl(&self, plugin: Arc<dyn Plugin>) -> Result<()> {
        let info = plugin.info().clone();
        let name = info.name.clone();
        info!("Registering plugin implementation: {}", name);

        // Get or create config
        let config = self
            .configs
            .read()
            .await
            .get(&name)
            .cloned()
            .unwrap_or_else(|| PluginConfig::new(&name));

        // Create instance
        let instance = PluginInstance::new(info.clone(), PathBuf::new(), config.clone());

        // Register hooks from plugin
        let hooks = plugin.hooks();
        if !hooks.is_empty() {
            let mut dispatcher = self.hook_dispatcher.write().await;
            for hook in hooks {
                dispatcher.register(HookRegistration {
                    plugin_name: name.clone(),
                    hook,
                    priority: config.priority,
                    enabled: config.enabled,
                });
            }
        }

        // Store plugin implementation and instance
        self.plugins.write().await.insert(name.clone(), instance);
        self.active_plugins.write().await.insert(name, plugin);

        Ok(())
    }

    /// Unregister a plugin.
    pub async fn unregister_plugin(&self, name: &str) -> Result<Option<PluginInstance>> {
        info!("Unregistering plugin: {}", name);

        // Remove from hook dispatcher
        self.hook_dispatcher.write().await.unregister_all(name);

        // Remove active plugin if exists
        if let Some(plugin) = self.active_plugins.write().await.remove(name) {
            // Note: Plugin trait doesn't have shutdown in this simplified version
            // In production, you'd want to call plugin.shutdown() here
            let _ = plugin;
        }

        // Remove and return instance
        Ok(self.plugins.write().await.remove(name))
    }

    /// Get a plugin by name.
    pub async fn get_plugin(&self, name: &str) -> Option<PluginInstance> {
        self.plugins.read().await.get(name).cloned()
    }

    /// List all registered plugins.
    pub async fn list_plugins(&self) -> Vec<PluginInfo> {
        self.plugins
            .read()
            .await
            .values()
            .map(|p| p.info.clone())
            .collect()
    }

    /// List all registered plugin instances (alias for backwards compatibility).
    pub async fn list(&self) -> Vec<PluginInstance> {
        self.plugins.read().await.values().cloned().collect()
    }

    /// List active plugins.
    pub async fn active_plugins(&self) -> Vec<PluginInfo> {
        self.plugins
            .read()
            .await
            .values()
            .filter(|p| p.is_active())
            .map(|p| p.info.clone())
            .collect()
    }

    /// Enable a plugin.
    pub async fn enable_plugin(&self, name: &str) -> Result<()> {
        let mut plugins = self.plugins.write().await;
        let plugin = plugins
            .get_mut(name)
            .ok_or_else(|| CortexError::NotFound(format!("Plugin not found: {name}")))?;

        if !plugin.can_load() && plugin.state != PluginState::Disabled {
            return Err(CortexError::InvalidInput(format!(
                "Plugin {} cannot be enabled in state {:?}",
                name, plugin.state
            )));
        }

        plugin.mark_active();
        info!("Enabled plugin: {}", name);

        // Enable in hook dispatcher
        self.hook_dispatcher.write().await.set_enabled(name, true);

        Ok(())
    }

    /// Disable a plugin.
    pub async fn disable_plugin(&self, name: &str) -> Result<()> {
        let mut plugins = self.plugins.write().await;
        let plugin = plugins
            .get_mut(name)
            .ok_or_else(|| CortexError::NotFound(format!("Plugin not found: {name}")))?;

        plugin.mark_disabled();
        info!("Disabled plugin: {}", name);

        // Disable in hook dispatcher
        self.hook_dispatcher.write().await.set_enabled(name, false);

        Ok(())
    }

    /// Call a hook and collect responses from all registered handlers.
    pub async fn call_hook(&self, hook: PluginHook, context: &HookContext) -> Result<HookResponse> {
        let dispatcher = self.hook_dispatcher.read().await;
        let registrations = dispatcher.get_handlers(hook);

        if registrations.is_empty() {
            return Ok(HookResponse::Continue);
        }

        debug!(
            "Calling hook {:?} with {} handlers",
            hook,
            registrations.len()
        );

        let mut final_response = HookResponse::Continue;
        let mut modified_data = None;

        for registration in registrations {
            if !registration.enabled {
                continue;
            }

            // Try to get active plugin implementation
            let active = self.active_plugins.read().await;
            if let Some(plugin) = active.get(&registration.plugin_name) {
                let response = plugin.handle_hook(hook, context).await?;

                match &response {
                    HookResponse::Continue => {}
                    HookResponse::ContinueWith { data } => {
                        modified_data = Some(data.clone());
                    }
                    HookResponse::Stop { .. } => {
                        return Ok(response);
                    }
                    HookResponse::Skip => {}
                    HookResponse::InjectMessage { .. } => {
                        final_response = response.clone();
                    }
                    HookResponse::Error { .. } => {
                        return Ok(response);
                    }
                }
            }
        }

        // Return modified data if any
        if let Some(data) = modified_data {
            return Ok(HookResponse::ContinueWith { data });
        }

        Ok(final_response)
    }

    /// Grant a permission to a plugin.
    pub async fn grant_permission(&self, plugin_name: &str, permission: PluginPermission) {
        let mut granted = self.granted_permissions.write().await;
        granted
            .entry(plugin_name.to_string())
            .or_insert_with(Vec::new)
            .push(permission);

        info!(
            "Granted permission {:?} to plugin {}",
            permission, plugin_name
        );
    }

    /// Check if plugin has a granted permission.
    pub async fn has_permission(&self, plugin_name: &str, permission: PluginPermission) -> bool {
        // Check granted permissions
        let granted = self.granted_permissions.read().await;
        if let Some(perms) = granted.get(plugin_name) {
            if perms.contains(&permission) {
                return true;
            }
        }

        // Check plugin's declared permissions
        let plugins = self.plugins.read().await;
        if let Some(plugin) = plugins.get(plugin_name) {
            return plugin.has_permission(permission);
        }

        false
    }

    /// Discover and load all plugins.
    pub async fn discover_and_load(&self) -> Result<PluginManagerLoadResult> {
        let configs: Vec<_> = self.configs.read().await.values().cloned().collect();

        let mut loader = PluginLoader::new(&self.cortex_home).with_configs(configs);

        if let Some(ref project_root) = self.project_root {
            loader = loader.with_project_root(project_root);
        }

        // Discover plugins
        let discovered = loader.discover().await?;
        info!("Discovered {} plugins", discovered.len());

        let mut result = PluginManagerLoadResult::default();

        // Load and register each plugin
        for plugin in discovered {
            match loader.load_plugin(&plugin).await {
                Ok(instance) => {
                    let name = instance.info.name.clone();
                    if let Err(e) = self.register_plugin(instance).await {
                        warn!("Failed to register plugin {}: {}", name, e);
                        result.errors.push(format!("{}: {}", name, e));
                    } else {
                        result.loaded.push(PluginLoadedInfo {
                            name,
                            version: plugin.info.version.clone(),
                            source: plugin.source,
                            format: plugin.format,
                        });
                    }
                }
                Err(e) => {
                    warn!("Failed to load plugin at {}: {}", plugin.path.display(), e);
                    result
                        .errors
                        .push(format!("{}: {}", plugin.path.display(), e));
                }
            }
        }

        Ok(result)
    }

    /// Get plugin statistics.
    pub async fn stats(&self) -> PluginStats {
        let plugins = self.plugins.read().await;

        let mut by_state = HashMap::new();
        let mut by_type = HashMap::new();

        for plugin in plugins.values() {
            *by_state.entry(plugin.state).or_insert(0) += 1;
            *by_type.entry(plugin.info.plugin_type).or_insert(0) += 1;
        }

        PluginStats {
            total: plugins.len(),
            active: plugins.values().filter(|p| p.is_active()).count(),
            disabled: plugins
                .values()
                .filter(|p| p.state == PluginState::Disabled)
                .count(),
            by_state,
            by_type,
        }
    }

    /// Shutdown all plugins.
    pub async fn shutdown(&self) -> Result<()> {
        info!("Shutting down plugin manager");

        // Shutdown active plugins
        let mut active = self.active_plugins.write().await;
        for (name, _plugin) in active.drain() {
            // In production, call plugin.shutdown() here
            debug!("Shutdown plugin: {}", name);
        }

        // Clear registrations
        self.hook_dispatcher.write().await.clear();
        self.plugins.write().await.clear();

        Ok(())
    }
}

impl Default for PluginManager {
    fn default() -> Self {
        Self::new(
            dirs::home_dir()
                .map(|h| h.join(".cortex"))
                .unwrap_or_else(|| PathBuf::from(".cortex")),
        )
    }
}

/// Result of plugin discovery and loading.
#[derive(Debug, Default)]
pub struct PluginManagerLoadResult {
    /// Successfully loaded plugins.
    pub loaded: Vec<PluginLoadedInfo>,
    /// Errors encountered.
    pub errors: Vec<String>,
}

impl PluginManagerLoadResult {
    /// Check if loading was successful.
    pub fn is_successful(&self) -> bool {
        self.errors.is_empty()
    }

    /// Get count of loaded plugins.
    pub fn count(&self) -> usize {
        self.loaded.len()
    }
}

/// Information about a loaded plugin.
#[derive(Debug, Clone)]
pub struct PluginLoadedInfo {
    /// Plugin name.
    pub name: String,
    /// Plugin version.
    pub version: String,
    /// Source.
    pub source: PluginSource,
    /// Format.
    pub format: PluginFormat,
}

/// Plugin statistics.
#[derive(Debug, Clone)]
pub struct PluginStats {
    /// Total number of plugins.
    pub total: usize,
    /// Number of active plugins.
    pub active: usize,
    /// Number of disabled plugins.
    pub disabled: usize,
    /// Count by state.
    pub by_state: HashMap<PluginState, usize>,
    /// Count by type.
    pub by_type: HashMap<super::types::PluginKind, usize>,
}

/// Plugin event for lifecycle tracking.
#[derive(Debug, Clone)]
pub enum PluginEvent {
    /// Plugin registered.
    Registered { name: String },
    /// Plugin enabled.
    Enabled { name: String },
    /// Plugin disabled.
    Disabled { name: String },
    /// Plugin unregistered.
    Unregistered { name: String },
    /// Plugin error.
    Error { name: String, error: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_plugin_manager_creation() {
        let manager = PluginManager::new("/tmp/.cortex");
        let stats = manager.stats().await;
        assert_eq!(stats.total, 0);
    }

    #[tokio::test]
    async fn test_plugin_registration() {
        let manager = PluginManager::new("/tmp/.cortex");

        let info = PluginInfo::new("test-plugin", "1.0.0");
        let config = PluginConfig::new("test-plugin");
        let instance = PluginInstance::new(info, "/path/to/plugin", config);

        manager.register_plugin(instance).await.unwrap();

        let plugins = manager.list_plugins().await;
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].name, "test-plugin");
    }

    #[tokio::test]
    async fn test_plugin_enable_disable() {
        let manager = PluginManager::new("/tmp/.cortex");

        let info = PluginInfo::new("test-plugin", "1.0.0");
        let config = PluginConfig::new("test-plugin");
        let instance = PluginInstance::new(info, "/path/to/plugin", config);

        manager.register_plugin(instance).await.unwrap();

        // Enable
        manager.enable_plugin("test-plugin").await.unwrap();
        let plugin = manager.get_plugin("test-plugin").await.unwrap();
        assert!(plugin.is_active());

        // Disable
        manager.disable_plugin("test-plugin").await.unwrap();
        let plugin = manager.get_plugin("test-plugin").await.unwrap();
        assert!(!plugin.is_active());
    }

    #[tokio::test]
    async fn test_permission_granting() {
        let manager = PluginManager::new("/tmp/.cortex");

        let info = PluginInfo::new("test-plugin", "1.0.0");
        let config = PluginConfig::new("test-plugin");
        let instance = PluginInstance::new(info, "/path/to/plugin", config);

        manager.register_plugin(instance).await.unwrap();

        // Grant permission
        manager
            .grant_permission("test-plugin", PluginPermission::ReadFiles)
            .await;

        // Check permission
        assert!(
            manager
                .has_permission("test-plugin", PluginPermission::ReadFiles)
                .await
        );
        assert!(
            !manager
                .has_permission("test-plugin", PluginPermission::WriteFiles)
                .await
        );
    }

    #[tokio::test]
    async fn test_hook_calling() {
        let manager = PluginManager::new("/tmp/.cortex");

        let context = HookContext::new("test-session", "/tmp");
        let response = manager
            .call_hook(PluginHook::SessionStarting, &context)
            .await
            .unwrap();

        assert!(response.should_continue());
    }
}
