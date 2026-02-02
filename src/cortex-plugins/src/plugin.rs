//! Core plugin types and traits.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;

use crate::Result;
use crate::manifest::PluginManifest;

/// Plugin information extracted from manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginInfo {
    /// Plugin unique identifier
    pub id: String,

    /// Human-readable name
    pub name: String,

    /// Version string (semver)
    pub version: String,

    /// Description
    pub description: String,

    /// Authors
    pub authors: Vec<String>,

    /// Homepage URL
    pub homepage: Option<String>,

    /// License
    pub license: Option<String>,

    /// Installation path
    pub path: PathBuf,

    /// Whether the plugin is enabled
    pub enabled: bool,
}

impl PluginInfo {
    /// Create plugin info from manifest and path.
    pub fn from_manifest(manifest: &PluginManifest, path: PathBuf) -> Self {
        Self {
            id: manifest.plugin.id.clone(),
            name: manifest.plugin.name.clone(),
            version: manifest.plugin.version.clone(),
            description: manifest.plugin.description.clone(),
            authors: manifest.plugin.authors.clone(),
            homepage: manifest.plugin.homepage.clone(),
            license: manifest.plugin.license.clone(),
            path,
            enabled: true,
        }
    }
}

/// Plugin runtime state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginState {
    /// Plugin is discovered but not loaded
    Discovered,
    /// Plugin is loading
    Loading,
    /// Plugin is loaded and ready
    Loaded,
    /// Plugin is initializing
    Initializing,
    /// Plugin is active and running
    Active,
    /// Plugin is being unloaded
    Unloading,
    /// Plugin is unloaded
    Unloaded,
    /// Plugin failed to load or initialize
    Error,
    /// Plugin is disabled
    Disabled,
}

impl std::fmt::Display for PluginState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Discovered => write!(f, "discovered"),
            Self::Loading => write!(f, "loading"),
            Self::Loaded => write!(f, "loaded"),
            Self::Initializing => write!(f, "initializing"),
            Self::Active => write!(f, "active"),
            Self::Unloading => write!(f, "unloading"),
            Self::Unloaded => write!(f, "unloaded"),
            Self::Error => write!(f, "error"),
            Self::Disabled => write!(f, "disabled"),
        }
    }
}

/// Plugin status with detailed information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginStatus {
    /// Plugin info
    pub info: PluginInfo,

    /// Current state
    pub state: PluginState,

    /// Error message if in error state
    pub error: Option<String>,

    /// Last activity timestamp
    pub last_activity: Option<chrono::DateTime<chrono::Utc>>,

    /// Statistics
    pub stats: PluginStats,
}

/// Plugin usage statistics.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PluginStats {
    /// Number of commands executed
    pub commands_executed: u64,

    /// Number of hooks triggered
    pub hooks_triggered: u64,

    /// Number of events handled
    pub events_handled: u64,

    /// Total execution time in milliseconds
    pub total_execution_ms: u64,

    /// Number of errors
    pub errors: u64,
}

/// Trait for plugin implementations.
#[async_trait::async_trait]
pub trait Plugin: Send + Sync {
    /// Get plugin information.
    fn info(&self) -> &PluginInfo;

    /// Get plugin manifest.
    fn manifest(&self) -> &PluginManifest;

    /// Get current state.
    fn state(&self) -> PluginState;

    /// Initialize the plugin.
    async fn init(&mut self) -> Result<()>;

    /// Shutdown the plugin.
    async fn shutdown(&mut self) -> Result<()>;

    /// Execute a command.
    async fn execute_command(
        &self,
        name: &str,
        args: Vec<String>,
        ctx: &crate::PluginContext,
    ) -> Result<String>;

    /// Get plugin configuration.
    fn get_config(&self, key: &str) -> Option<serde_json::Value>;

    /// Set plugin configuration.
    fn set_config(&mut self, key: &str, value: serde_json::Value) -> Result<()>;
}

/// A handle to a loaded plugin.
#[derive(Clone)]
pub struct PluginHandle {
    inner: Arc<tokio::sync::RwLock<Box<dyn Plugin>>>,
}

impl PluginHandle {
    /// Create a new plugin handle.
    pub fn new(plugin: Box<dyn Plugin>) -> Self {
        Self {
            inner: Arc::new(tokio::sync::RwLock::new(plugin)),
        }
    }

    /// Get read access to the plugin.
    pub async fn read(&self) -> tokio::sync::RwLockReadGuard<'_, Box<dyn Plugin>> {
        self.inner.read().await
    }

    /// Get write access to the plugin.
    pub async fn write(&self) -> tokio::sync::RwLockWriteGuard<'_, Box<dyn Plugin>> {
        self.inner.write().await
    }

    /// Get plugin info.
    pub async fn info(&self) -> PluginInfo {
        self.inner.read().await.info().clone()
    }

    /// Get plugin state.
    pub async fn state(&self) -> PluginState {
        self.inner.read().await.state()
    }

    /// Execute a command on the plugin.
    pub async fn execute_command(
        &self,
        name: &str,
        args: Vec<String>,
        ctx: &crate::PluginContext,
    ) -> Result<String> {
        self.inner
            .read()
            .await
            .execute_command(name, args, ctx)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plugin_state_display() {
        assert_eq!(PluginState::Active.to_string(), "active");
        assert_eq!(PluginState::Disabled.to_string(), "disabled");
    }

    #[test]
    fn test_plugin_stats_default() {
        let stats = PluginStats::default();
        assert_eq!(stats.commands_executed, 0);
        assert_eq!(stats.errors, 0);
    }
}
