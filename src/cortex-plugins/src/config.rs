//! Plugin system configuration.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Plugin system configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginConfig {
    /// Plugin search paths
    #[serde(default = "default_search_paths")]
    pub search_paths: Vec<PathBuf>,

    /// Whether to enable hot-reload in development mode
    #[serde(default)]
    pub hot_reload: bool,

    /// Whether to enable plugin sandboxing
    #[serde(default = "default_true")]
    pub sandbox_enabled: bool,

    /// Default memory limit for plugins (in pages, 64KB each)
    #[serde(default = "default_memory_pages")]
    pub default_memory_pages: u32,

    /// Default execution timeout (in milliseconds)
    #[serde(default = "default_timeout_ms")]
    pub default_timeout_ms: u64,

    /// Plugins that are explicitly disabled
    #[serde(default)]
    pub disabled_plugins: Vec<String>,

    /// Plugins that are explicitly enabled (if empty, all are enabled)
    #[serde(default)]
    pub enabled_plugins: Vec<String>,

    /// Plugin-specific configurations
    #[serde(default)]
    pub plugin_configs: std::collections::HashMap<String, serde_json::Value>,

    /// Whether to load built-in plugins
    #[serde(default = "default_true")]
    pub load_builtin_plugins: bool,

    /// WASM cache directory
    #[serde(default = "default_cache_dir")]
    pub cache_dir: Option<PathBuf>,

    /// Maximum number of concurrent plugin operations
    #[serde(default = "default_max_concurrent")]
    pub max_concurrent: usize,
}

impl Default for PluginConfig {
    fn default() -> Self {
        Self {
            search_paths: default_search_paths(),
            hot_reload: false,
            sandbox_enabled: true,
            default_memory_pages: default_memory_pages(),
            default_timeout_ms: default_timeout_ms(),
            disabled_plugins: Vec::new(),
            enabled_plugins: Vec::new(),
            plugin_configs: std::collections::HashMap::new(),
            load_builtin_plugins: true,
            cache_dir: default_cache_dir(),
            max_concurrent: default_max_concurrent(),
        }
    }
}

impl PluginConfig {
    /// Create a new configuration with custom search paths.
    pub fn with_search_paths(paths: Vec<PathBuf>) -> Self {
        Self {
            search_paths: paths,
            ..Default::default()
        }
    }

    /// Add a search path.
    pub fn add_search_path(&mut self, path: PathBuf) {
        if !self.search_paths.contains(&path) {
            self.search_paths.push(path);
        }
    }

    /// Check if a plugin is enabled.
    pub fn is_plugin_enabled(&self, plugin_id: &str) -> bool {
        // If explicitly disabled, return false
        if self.disabled_plugins.contains(&plugin_id.to_string()) {
            return false;
        }

        // If enabled_plugins is not empty, check if plugin is in the list
        if !self.enabled_plugins.is_empty() {
            return self.enabled_plugins.contains(&plugin_id.to_string());
        }

        // By default, all plugins are enabled
        true
    }

    /// Get configuration for a specific plugin.
    pub fn get_plugin_config(&self, plugin_id: &str) -> Option<&serde_json::Value> {
        self.plugin_configs.get(plugin_id)
    }

    /// Set configuration for a specific plugin.
    pub fn set_plugin_config(&mut self, plugin_id: &str, config: serde_json::Value) {
        self.plugin_configs.insert(plugin_id.to_string(), config);
    }

    /// Enable a plugin.
    pub fn enable_plugin(&mut self, plugin_id: &str) {
        self.disabled_plugins.retain(|id| id != plugin_id);
        if !self.enabled_plugins.is_empty()
            && !self.enabled_plugins.contains(&plugin_id.to_string())
        {
            self.enabled_plugins.push(plugin_id.to_string());
        }
    }

    /// Disable a plugin.
    pub fn disable_plugin(&mut self, plugin_id: &str) {
        if !self.disabled_plugins.contains(&plugin_id.to_string()) {
            self.disabled_plugins.push(plugin_id.to_string());
        }
        self.enabled_plugins.retain(|id| id != plugin_id);
    }
}

fn default_search_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    // Global plugins directory
    if let Some(home) = dirs::home_dir() {
        paths.push(home.join(".cortex").join("plugins"));
    }

    // Config directory plugins
    if let Some(config) = dirs::config_dir() {
        paths.push(config.join("cortex").join("plugins"));
    }

    // Local project plugins
    paths.push(PathBuf::from(".cortex").join("plugins"));

    paths
}

fn default_cache_dir() -> Option<PathBuf> {
    dirs::cache_dir().map(|p| p.join("cortex").join("plugins"))
}

fn default_memory_pages() -> u32 {
    256 // 16 MB
}

fn default_timeout_ms() -> u64 {
    30000 // 30 seconds
}

fn default_true() -> bool {
    true
}

fn default_max_concurrent() -> usize {
    4
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = PluginConfig::default();
        assert!(!config.search_paths.is_empty());
        assert!(config.sandbox_enabled);
        assert_eq!(config.default_memory_pages, 256);
    }

    #[test]
    fn test_plugin_enabled() {
        let mut config = PluginConfig::default();

        // By default, all plugins are enabled
        assert!(config.is_plugin_enabled("test-plugin"));

        // Disable a plugin
        config.disable_plugin("test-plugin");
        assert!(!config.is_plugin_enabled("test-plugin"));

        // Re-enable
        config.enable_plugin("test-plugin");
        assert!(config.is_plugin_enabled("test-plugin"));
    }

    #[test]
    fn test_plugin_config() {
        let mut config = PluginConfig::default();

        config.set_plugin_config("test", serde_json::json!({ "key": "value" }));

        let plugin_config = config.get_plugin_config("test").unwrap();
        assert_eq!(plugin_config["key"], "value");
    }
}
