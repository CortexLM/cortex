//! Plugin registry for managing loaded plugins.

use std::collections::HashMap;
use tokio::sync::RwLock;

use crate::plugin::{Plugin, PluginHandle, PluginInfo, PluginState, PluginStats, PluginStatus};
use crate::{PluginError, Result};

/// Registry for loaded plugins.
pub struct PluginRegistry {
    /// Loaded plugins by ID
    plugins: RwLock<HashMap<String, PluginHandle>>,

    /// Plugin statistics
    stats: RwLock<HashMap<String, PluginStats>>,
}

impl PluginRegistry {
    /// Create a new plugin registry.
    pub fn new() -> Self {
        Self {
            plugins: RwLock::new(HashMap::new()),
            stats: RwLock::new(HashMap::new()),
        }
    }

    /// Register a plugin.
    pub async fn register(&self, plugin: Box<dyn Plugin>) -> Result<()> {
        let info = plugin.info().clone();
        let id = info.id.clone();

        {
            let plugins = self.plugins.read().await;
            if plugins.contains_key(&id) {
                return Err(PluginError::AlreadyExists(id));
            }
        }

        let handle = PluginHandle::new(plugin);

        {
            let mut plugins = self.plugins.write().await;
            plugins.insert(id.clone(), handle);
        }

        {
            let mut stats = self.stats.write().await;
            stats.insert(id.clone(), PluginStats::default());
        }

        tracing::info!("Registered plugin: {} v{}", info.name, info.version);
        Ok(())
    }

    /// Unregister a plugin.
    pub async fn unregister(&self, id: &str) -> Result<()> {
        let handle = {
            let mut plugins = self.plugins.write().await;
            plugins.remove(id)
        };

        if let Some(handle) = handle {
            // Shutdown the plugin
            let mut plugin = handle.write().await;
            plugin.shutdown().await?;

            tracing::info!("Unregistered plugin: {}", id);
        }

        {
            let mut stats = self.stats.write().await;
            stats.remove(id);
        }

        Ok(())
    }

    /// Get a plugin by ID.
    pub async fn get(&self, id: &str) -> Option<PluginHandle> {
        self.plugins.read().await.get(id).cloned()
    }

    /// Check if a plugin is registered.
    pub async fn is_registered(&self, id: &str) -> bool {
        self.plugins.read().await.contains_key(id)
    }

    /// List all registered plugins.
    pub async fn list(&self) -> Vec<PluginInfo> {
        let plugins = self.plugins.read().await;
        let mut infos = Vec::new();

        for handle in plugins.values() {
            infos.push(handle.info().await);
        }

        infos.sort_by(|a, b| a.name.cmp(&b.name));
        infos
    }

    /// List plugins with detailed status.
    pub async fn list_status(&self) -> Vec<PluginStatus> {
        let plugins = self.plugins.read().await;
        let stats = self.stats.read().await;
        let mut statuses = Vec::new();

        for (id, handle) in plugins.iter() {
            let info = handle.info().await;
            let state = handle.state().await;
            let plugin_stats = stats.get(id).cloned().unwrap_or_default();

            statuses.push(PluginStatus {
                info,
                state,
                error: None,
                last_activity: None,
                stats: plugin_stats,
            });
        }

        statuses.sort_by(|a, b| a.info.name.cmp(&b.info.name));
        statuses
    }

    /// Get plugin count.
    pub async fn count(&self) -> usize {
        self.plugins.read().await.len()
    }

    /// Initialize all plugins.
    pub async fn init_all(&self) -> Vec<(String, Result<()>)> {
        let plugins = self.plugins.read().await;
        let mut results = Vec::new();

        for (id, handle) in plugins.iter() {
            let mut plugin = handle.write().await;
            let result = plugin.init().await;

            if let Err(ref e) = result {
                tracing::warn!("Failed to initialize plugin {}: {}", id, e);
            }

            results.push((id.clone(), result));
        }

        results
    }

    /// Shutdown all plugins.
    pub async fn shutdown_all(&self) -> Vec<(String, Result<()>)> {
        let plugins = self.plugins.read().await;
        let mut results = Vec::new();

        for (id, handle) in plugins.iter() {
            let mut plugin = handle.write().await;
            let result = plugin.shutdown().await;

            if let Err(ref e) = result {
                tracing::warn!("Failed to shutdown plugin {}: {}", id, e);
            }

            results.push((id.clone(), result));
        }

        results
    }

    /// Update statistics for a plugin.
    pub async fn update_stats<F>(&self, id: &str, f: F)
    where
        F: FnOnce(&mut PluginStats),
    {
        let mut stats = self.stats.write().await;
        if let Some(plugin_stats) = stats.get_mut(id) {
            f(plugin_stats);
        }
    }

    /// Get statistics for a plugin.
    pub async fn get_stats(&self, id: &str) -> Option<PluginStats> {
        self.stats.read().await.get(id).cloned()
    }

    /// Get IDs of all active plugins.
    pub async fn active_plugin_ids(&self) -> Vec<String> {
        let plugins = self.plugins.read().await;
        let mut ids = Vec::new();

        for (id, handle) in plugins.iter() {
            if handle.state().await == PluginState::Active {
                ids.push(id.clone());
            }
        }

        ids
    }
}

impl Default for PluginRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{PluginManifest, PluginMetadata};
    use std::path::PathBuf;

    // Mock plugin for testing
    struct MockPlugin {
        info: PluginInfo,
        manifest: PluginManifest,
        state: PluginState,
    }

    impl MockPlugin {
        fn new(id: &str) -> Self {
            let manifest = PluginManifest {
                plugin: PluginMetadata {
                    id: id.to_string(),
                    name: format!("Test Plugin {}", id),
                    version: "1.0.0".to_string(),
                    description: "A test plugin".to_string(),
                    authors: vec![],
                    homepage: None,
                    license: None,
                    min_cortex_version: None,
                    keywords: vec![],
                    icon: None,
                },
                capabilities: vec![],
                permissions: vec![],
                dependencies: vec![],
                commands: vec![],
                hooks: vec![],
                config: HashMap::new(),
                wasm: Default::default(),
            };

            let info = PluginInfo::from_manifest(&manifest, PathBuf::from("/tmp"));

            Self {
                info,
                manifest,
                state: PluginState::Loaded,
            }
        }
    }

    #[async_trait::async_trait]
    impl Plugin for MockPlugin {
        fn info(&self) -> &PluginInfo {
            &self.info
        }

        fn manifest(&self) -> &PluginManifest {
            &self.manifest
        }

        fn state(&self) -> PluginState {
            self.state
        }

        async fn init(&mut self) -> Result<()> {
            self.state = PluginState::Active;
            Ok(())
        }

        async fn shutdown(&mut self) -> Result<()> {
            self.state = PluginState::Unloaded;
            Ok(())
        }

        async fn execute_command(
            &self,
            name: &str,
            _args: Vec<String>,
            _ctx: &crate::PluginContext,
        ) -> Result<String> {
            Ok(format!("Mock command: {}", name))
        }

        fn get_config(&self, _key: &str) -> Option<serde_json::Value> {
            None
        }

        fn set_config(&mut self, _key: &str, _value: serde_json::Value) -> Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_register_plugin() {
        let registry = PluginRegistry::new();
        let plugin = MockPlugin::new("test");

        registry.register(Box::new(plugin)).await.unwrap();

        assert!(registry.is_registered("test").await);
        assert_eq!(registry.count().await, 1);
    }

    #[tokio::test]
    async fn test_register_duplicate() {
        let registry = PluginRegistry::new();

        registry
            .register(Box::new(MockPlugin::new("test")))
            .await
            .unwrap();

        let result = registry.register(Box::new(MockPlugin::new("test"))).await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_unregister_plugin() {
        let registry = PluginRegistry::new();

        registry
            .register(Box::new(MockPlugin::new("test")))
            .await
            .unwrap();
        assert!(registry.is_registered("test").await);

        registry.unregister("test").await.unwrap();
        assert!(!registry.is_registered("test").await);
    }

    #[tokio::test]
    async fn test_list_plugins() {
        let registry = PluginRegistry::new();

        registry
            .register(Box::new(MockPlugin::new("plugin-a")))
            .await
            .unwrap();
        registry
            .register(Box::new(MockPlugin::new("plugin-b")))
            .await
            .unwrap();

        let plugins = registry.list().await;
        assert_eq!(plugins.len(), 2);
    }

    #[tokio::test]
    async fn test_init_all() {
        let registry = PluginRegistry::new();

        registry
            .register(Box::new(MockPlugin::new("test")))
            .await
            .unwrap();

        let results = registry.init_all().await;
        assert_eq!(results.len(), 1);
        assert!(results[0].1.is_ok());
    }
}
