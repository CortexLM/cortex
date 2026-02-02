//! Plugin loader for discovering and loading plugins.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::config::PluginConfig;
use crate::manifest::PluginManifest;
use crate::runtime::{WasmPlugin, WasmRuntime};
use crate::{MANIFEST_FILE, PluginError, Result, WASM_FILE};

/// Discovered plugin information.
#[derive(Debug, Clone)]
pub struct DiscoveredPlugin {
    /// Plugin manifest
    pub manifest: PluginManifest,
    /// Plugin directory path
    pub path: PathBuf,
    /// Whether a WASM file exists
    pub has_wasm: bool,
}

impl DiscoveredPlugin {
    /// Get the plugin ID.
    pub fn id(&self) -> &str {
        &self.manifest.plugin.id
    }

    /// Get the plugin name.
    pub fn name(&self) -> &str {
        &self.manifest.plugin.name
    }

    /// Get the plugin version.
    pub fn version(&self) -> &str {
        &self.manifest.plugin.version
    }
}

/// Plugin loader for discovering and loading plugins from directories.
pub struct PluginLoader {
    config: PluginConfig,
    runtime: Arc<WasmRuntime>,
}

impl PluginLoader {
    /// Create a new plugin loader.
    pub fn new(config: PluginConfig, runtime: Arc<WasmRuntime>) -> Self {
        Self { config, runtime }
    }

    /// Discover plugins in all search paths.
    pub async fn discover(&self) -> Vec<DiscoveredPlugin> {
        let mut plugins = Vec::new();

        for search_path in &self.config.search_paths {
            if !search_path.exists() {
                tracing::debug!("Plugin search path does not exist: {:?}", search_path);
                continue;
            }

            tracing::debug!("Searching for plugins in: {:?}", search_path);

            match self.discover_in_path(search_path).await {
                Ok(found) => plugins.extend(found),
                Err(e) => {
                    tracing::warn!("Error discovering plugins in {:?}: {}", search_path, e);
                }
            }
        }

        tracing::info!("Discovered {} plugins", plugins.len());
        plugins
    }

    /// Discover plugins in a specific directory.
    async fn discover_in_path(&self, path: &Path) -> Result<Vec<DiscoveredPlugin>> {
        let mut plugins = Vec::new();

        let mut entries = tokio::fs::read_dir(path).await?;

        while let Some(entry) = entries.next_entry().await? {
            let entry_path = entry.path();

            // Check if it's a directory
            if !entry_path.is_dir() {
                continue;
            }

            // Check for manifest file
            let manifest_path = entry_path.join(MANIFEST_FILE);
            if !manifest_path.exists() {
                continue;
            }

            // Try to load the manifest
            match self.load_manifest(&manifest_path).await {
                Ok(manifest) => {
                    // Validate manifest
                    if let Err(e) = manifest.validate() {
                        tracing::warn!("Invalid manifest in {:?}: {}", manifest_path, e);
                        continue;
                    }

                    // Check for WASM file
                    let wasm_path = entry_path.join(WASM_FILE);
                    let has_wasm = wasm_path.exists();

                    plugins.push(DiscoveredPlugin {
                        manifest,
                        path: entry_path,
                        has_wasm,
                    });
                }
                Err(e) => {
                    tracing::warn!("Failed to load manifest {:?}: {}", manifest_path, e);
                }
            }
        }

        Ok(plugins)
    }

    /// Load a manifest from a file.
    async fn load_manifest(&self, path: &Path) -> Result<PluginManifest> {
        let content = tokio::fs::read_to_string(path).await?;
        PluginManifest::parse(&content)
    }

    /// Load a discovered plugin.
    pub fn load(&self, discovered: &DiscoveredPlugin) -> Result<WasmPlugin> {
        if !discovered.has_wasm {
            return Err(PluginError::load_error(
                discovered.id(),
                format!(
                    "No WASM file found at {:?}",
                    discovered.path.join(WASM_FILE)
                ),
            ));
        }

        let mut plugin = WasmPlugin::new(
            discovered.manifest.clone(),
            discovered.path.clone(),
            self.runtime.clone(),
        )?;

        plugin.load()?;

        Ok(plugin)
    }

    /// Load a plugin from a specific path.
    pub async fn load_from_path(&self, path: &Path) -> Result<WasmPlugin> {
        let manifest_path = if path.is_dir() {
            path.join(MANIFEST_FILE)
        } else {
            path.to_path_buf()
        };

        let plugin_dir = manifest_path.parent().unwrap_or(path);
        let manifest = self.load_manifest(&manifest_path).await?;

        manifest.validate()?;

        let mut plugin = WasmPlugin::new(manifest, plugin_dir.to_path_buf(), self.runtime.clone())?;

        plugin.load()?;

        Ok(plugin)
    }

    /// Get the configuration.
    pub fn config(&self) -> &PluginConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_discover_empty_paths() {
        let config = PluginConfig {
            search_paths: vec![PathBuf::from("/nonexistent/path")],
            ..Default::default()
        };
        let runtime = Arc::new(WasmRuntime::new().unwrap());
        let loader = PluginLoader::new(config, runtime);

        let plugins = loader.discover().await;
        assert!(plugins.is_empty());
    }
}
