//! Plugin configuration module.
//!
//! Handles plugin configuration from TOML files and provides
//! configuration management utilities.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tracing::warn;

use super::types::{PluginConfig, PluginPermission};
use crate::error::{CortexError, Result};

/// Plugin configuration section in main config file.
///
/// Example TOML:
/// ```toml
/// [[plugins]]
/// name = "my-plugin"
/// path = "~/.cortex/plugins/my-plugin.wasm"
/// enabled = true
/// priority = 0
/// granted_permissions = ["read_files", "network"]
///
/// [plugins.config]
/// key = "value"
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PluginsConfig {
    /// List of plugin configurations.
    #[serde(default)]
    pub plugins: Vec<PluginConfigEntry>,
    /// Plugin directories to scan.
    #[serde(default)]
    pub plugin_dirs: Vec<PathBuf>,
    /// Whether to auto-load discovered plugins.
    #[serde(default = "default_auto_load")]
    pub auto_load: bool,
    /// Global plugin settings.
    #[serde(default)]
    pub settings: PluginSettings,
}

fn default_auto_load() -> bool {
    true
}

/// Plugin configuration entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginConfigEntry {
    /// Plugin name.
    pub name: String,
    /// Path to plugin (optional, can be discovered).
    #[serde(default)]
    pub path: Option<PathBuf>,
    /// Whether plugin is enabled.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// Priority (lower = runs first).
    #[serde(default)]
    pub priority: i32,
    /// Granted permissions.
    #[serde(default)]
    pub granted_permissions: Vec<String>,
    /// Custom configuration.
    #[serde(default)]
    pub config: HashMap<String, toml::Value>,
}

fn default_enabled() -> bool {
    true
}

impl PluginConfigEntry {
    /// Create new plugin configuration entry.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            path: None,
            enabled: true,
            priority: 0,
            granted_permissions: Vec::new(),
            config: HashMap::new(),
        }
    }

    /// Set path.
    pub fn with_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.path = Some(path.into());
        self
    }

    /// Set enabled.
    pub fn with_enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Add permission.
    pub fn with_permission(mut self, permission: impl Into<String>) -> Self {
        self.granted_permissions.push(permission.into());
        self
    }

    /// Add config value.
    pub fn with_config(mut self, key: impl Into<String>, value: toml::Value) -> Self {
        self.config.insert(key.into(), value);
        self
    }

    /// Convert to PluginConfig.
    pub fn to_plugin_config(&self) -> PluginConfig {
        let granted_permissions: Vec<PluginPermission> = self
            .granted_permissions
            .iter()
            .filter_map(|s| parse_permission(s))
            .collect();

        PluginConfig {
            name: self.name.clone(),
            path: self.path.clone(),
            enabled: self.enabled,
            config: self.config.clone(),
            priority: self.priority,
            granted_permissions,
        }
    }
}

/// Global plugin settings.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PluginSettings {
    /// Default timeout for plugin operations (milliseconds).
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
    /// Maximum number of concurrent plugin operations.
    #[serde(default = "default_max_concurrent")]
    pub max_concurrent: usize,
    /// Whether to sandbox WASM plugins.
    #[serde(default = "default_sandbox_wasm")]
    pub sandbox_wasm: bool,
    /// Whether to verify plugin signatures.
    #[serde(default)]
    pub verify_signatures: bool,
    /// Allowed plugin sources.
    #[serde(default)]
    pub allowed_sources: Vec<String>,
}

fn default_timeout() -> u64 {
    30000 // 30 seconds
}

fn default_max_concurrent() -> usize {
    4
}

fn default_sandbox_wasm() -> bool {
    true
}

impl PluginsConfig {
    /// Load plugins configuration from a TOML file.
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }

        let content = std::fs::read_to_string(path)?;
        Self::from_toml(&content)
    }

    /// Parse plugins configuration from TOML string.
    pub fn from_toml(content: &str) -> Result<Self> {
        // Try to parse as PluginsConfig directly
        if let Ok(config) = toml::from_str::<Self>(content) {
            return Ok(config);
        }

        // Try to parse as a parent config and extract plugins section
        #[derive(Deserialize)]
        struct ParentConfig {
            #[serde(default)]
            plugins: Vec<PluginConfigEntry>,
            #[serde(default)]
            plugin_dirs: Vec<PathBuf>,
            #[serde(default)]
            plugin_settings: Option<PluginSettings>,
        }

        let parent: ParentConfig = toml::from_str(content).map_err(|e| {
            CortexError::InvalidInput(format!("Failed to parse plugins config: {e}"))
        })?;

        Ok(Self {
            plugins: parent.plugins,
            plugin_dirs: parent.plugin_dirs,
            auto_load: true,
            settings: parent.plugin_settings.unwrap_or_default(),
        })
    }

    /// Convert all entries to PluginConfig.
    pub fn to_plugin_configs(&self) -> Vec<PluginConfig> {
        self.plugins.iter().map(|e| e.to_plugin_config()).collect()
    }

    /// Get a specific plugin configuration by name.
    pub fn get(&self, name: &str) -> Option<&PluginConfigEntry> {
        self.plugins.iter().find(|p| p.name == name)
    }

    /// Check if a plugin is enabled.
    pub fn is_enabled(&self, name: &str) -> bool {
        self.get(name).map(|p| p.enabled).unwrap_or(true)
    }

    /// Get all plugin directories.
    pub fn all_plugin_dirs(&self, cortex_home: &Path, project_root: Option<&Path>) -> Vec<PathBuf> {
        let mut dirs = Vec::new();

        // Standard directories
        dirs.push(cortex_home.join("plugins"));
        if let Some(project_root) = project_root {
            dirs.push(project_root.join(".cortex").join("plugins"));
        }

        // Custom directories from config
        for dir in &self.plugin_dirs {
            let expanded = expand_path(dir);
            if !dirs.contains(&expanded) {
                dirs.push(expanded);
            }
        }

        dirs
    }

    /// Merge with another plugins configuration.
    pub fn merge(mut self, other: Self) -> Self {
        // Merge plugins (other takes precedence for same name)
        let mut plugins_map: HashMap<String, PluginConfigEntry> = self
            .plugins
            .into_iter()
            .map(|p| (p.name.clone(), p))
            .collect();

        for plugin in other.plugins {
            plugins_map.insert(plugin.name.clone(), plugin);
        }

        self.plugins = plugins_map.into_values().collect();

        // Merge directories (unique)
        for dir in other.plugin_dirs {
            if !self.plugin_dirs.contains(&dir) {
                self.plugin_dirs.push(dir);
            }
        }

        // Use other's auto_load setting
        self.auto_load = other.auto_load;

        // Merge settings (other takes precedence)
        self.settings = other.settings;

        self
    }
}

/// Parse permission string to PluginPermission.
fn parse_permission(s: &str) -> Option<PluginPermission> {
    match s.to_lowercase().replace(['-', '.'], "_").as_str() {
        "read_files" => Some(PluginPermission::ReadFiles),
        "write_files" => Some(PluginPermission::WriteFiles),
        "execute_commands" => Some(PluginPermission::ExecuteCommands),
        "network" => Some(PluginPermission::Network),
        "environment" => Some(PluginPermission::Environment),
        "system_info" => Some(PluginPermission::SystemInfo),
        "clipboard" => Some(PluginPermission::Clipboard),
        "notifications" => Some(PluginPermission::Notifications),
        "modify_session" => Some(PluginPermission::ModifySession),
        "access_tools" => Some(PluginPermission::AccessTools),
        _ => {
            warn!("Unknown permission: {}", s);
            None
        }
    }
}

/// Expand ~ and environment variables in path.
fn expand_path(path: &Path) -> PathBuf {
    let path_str = path.to_string_lossy();

    // Expand ~
    let expanded = if path_str.starts_with("~/") {
        if let Some(home) = dirs::home_dir() {
            home.join(&path_str[2..])
        } else {
            path.to_path_buf()
        }
    } else {
        path.to_path_buf()
    };

    // Expand environment variables
    let expanded_str = expanded.to_string_lossy();
    let mut result = expanded_str.to_string();

    // Simple env var expansion for $VAR or ${VAR}
    for (key, value) in std::env::vars() {
        result = result.replace(&format!("${{{}}}", key), &value);
        result = result.replace(&format!("${}", key), &value);
    }

    PathBuf::from(result)
}

/// Plugin configuration builder.
pub struct PluginConfigBuilder {
    entries: Vec<PluginConfigEntry>,
    dirs: Vec<PathBuf>,
    auto_load: bool,
    settings: PluginSettings,
}

impl PluginConfigBuilder {
    /// Create a new builder.
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            dirs: Vec::new(),
            auto_load: true,
            settings: PluginSettings::default(),
        }
    }

    /// Add a plugin entry.
    pub fn plugin(mut self, entry: PluginConfigEntry) -> Self {
        self.entries.push(entry);
        self
    }

    /// Add a plugin directory.
    pub fn plugin_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.dirs.push(dir.into());
        self
    }

    /// Set auto-load.
    pub fn auto_load(mut self, auto_load: bool) -> Self {
        self.auto_load = auto_load;
        self
    }

    /// Set settings.
    pub fn settings(mut self, settings: PluginSettings) -> Self {
        self.settings = settings;
        self
    }

    /// Build the configuration.
    pub fn build(self) -> PluginsConfig {
        PluginsConfig {
            plugins: self.entries,
            plugin_dirs: self.dirs,
            auto_load: self.auto_load,
            settings: self.settings,
        }
    }
}

impl Default for PluginConfigBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Example TOML configuration.
pub const EXAMPLE_CONFIG: &str = r#"# Plugin Configuration

# Plugin directories to scan (in addition to defaults)
# plugin_dirs = ["~/.cortex/custom-plugins"]

# Auto-load discovered plugins
auto_load = true

# Global plugin settings
[plugin_settings]
timeout_ms = 30000
max_concurrent = 4
sandbox_wasm = true
verify_signatures = false

# Example plugin configurations
[[plugins]]
name = "my-custom-tool"
path = "~/.cortex/plugins/my-custom-tool.wasm"
enabled = true
priority = 0
granted_permissions = ["read_files", "network"]

[plugins.config]
api_key = "your-api-key"
max_retries = 3

[[plugins]]
name = "logging-hook"
enabled = true
priority = -10  # Run early

[plugins.config]
log_level = "debug"
output_file = "~/.cortex/logs/plugin.log"
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_permission() {
        assert_eq!(
            parse_permission("read_files"),
            Some(PluginPermission::ReadFiles)
        );
        assert_eq!(
            parse_permission("write-files"),
            Some(PluginPermission::WriteFiles)
        );
        assert_eq!(parse_permission("NETWORK"), Some(PluginPermission::Network));
        assert!(parse_permission("unknown").is_none());
    }

    #[test]
    fn test_expand_path() {
        let path = Path::new("/absolute/path");
        assert_eq!(expand_path(path), PathBuf::from("/absolute/path"));

        // Test ~ expansion (may fail if HOME not set)
        if dirs::home_dir().is_some() {
            let path = Path::new("~/test");
            let expanded = expand_path(path);
            assert!(!expanded.to_string_lossy().contains("~"));
        }
    }

    #[test]
    fn test_plugin_config_entry() {
        let entry = PluginConfigEntry::new("test-plugin")
            .with_path("/path/to/plugin")
            .with_enabled(true)
            .with_permission("read_files")
            .with_config("key", toml::Value::String("value".to_string()));

        assert_eq!(entry.name, "test-plugin");
        assert!(entry.enabled);
        assert_eq!(entry.granted_permissions.len(), 1);

        let config = entry.to_plugin_config();
        assert_eq!(config.name, "test-plugin");
        assert!(
            config
                .granted_permissions
                .contains(&PluginPermission::ReadFiles)
        );
    }

    #[test]
    fn test_plugins_config_from_toml() {
        let toml = r#"
auto_load = true

[[plugins]]
name = "test-plugin"
enabled = true
priority = 0
granted_permissions = ["read_files"]

[plugins.config]
key = "value"
"#;

        let config = PluginsConfig::from_toml(toml).unwrap();
        assert_eq!(config.plugins.len(), 1);
        assert_eq!(config.plugins[0].name, "test-plugin");
        assert!(config.plugins[0].enabled);
    }

    #[test]
    fn test_plugin_config_builder() {
        let config = PluginConfigBuilder::new()
            .plugin(PluginConfigEntry::new("plugin-a"))
            .plugin(PluginConfigEntry::new("plugin-b"))
            .plugin_dir("~/.cortex/custom-plugins")
            .auto_load(true)
            .build();

        assert_eq!(config.plugins.len(), 2);
        assert_eq!(config.plugin_dirs.len(), 1);
        assert!(config.auto_load);
    }

    #[test]
    fn test_plugins_config_merge() {
        let config1 = PluginsConfig {
            plugins: vec![PluginConfigEntry::new("plugin-a")],
            plugin_dirs: vec![PathBuf::from("/dir1")],
            auto_load: true,
            settings: PluginSettings::default(),
        };

        let config2 = PluginsConfig {
            plugins: vec![
                PluginConfigEntry::new("plugin-a").with_enabled(false),
                PluginConfigEntry::new("plugin-b"),
            ],
            plugin_dirs: vec![PathBuf::from("/dir2")],
            auto_load: false,
            settings: PluginSettings::default(),
        };

        let merged = config1.merge(config2);

        assert_eq!(merged.plugins.len(), 2);
        assert_eq!(merged.plugin_dirs.len(), 2);
        assert!(!merged.auto_load);

        // plugin-a should have been overwritten
        let plugin_a = merged
            .plugins
            .iter()
            .find(|p| p.name == "plugin-a")
            .unwrap();
        assert!(!plugin_a.enabled);
    }
}
