//! Plugin system for Cortex CLI.
//!
//! This module provides a comprehensive plugin architecture for extending Cortex
//! with custom tools, hooks, providers, and functionality.
//!
//! # Plugin Types
//!
//! - **WASM Plugins**: WebAssembly-based plugins for cross-platform compatibility
//! - **Native Plugins**: Dynamic libraries (.so, .dylib, .dll) for maximum performance
//! - **Script Plugins**: Directory-based plugins with hooks.json and scripts
//!
//! # Plugin Locations
//!
//! Plugins are discovered from:
//! - `~/.cortex/plugins/` - User plugins
//! - `.cortex/plugins/` - Project-specific plugins
//!
//! # Configuration
//!
//! Plugins can be configured in the main config file:
//!
//! ```toml
//! [[plugins]]
//! name = "my-plugin"
//! path = "~/.cortex/plugins/my-plugin.wasm"
//! enabled = true
//! priority = 0
//! granted_permissions = ["read_files", "network"]
//!
//! [plugins.config]
//! key = "value"
//! ```
//!
//! # Plugin Lifecycle
//!
//! 1. **Discovery**: Plugins are discovered from plugin directories
//! 2. **Loading**: Plugin manifest is loaded and validated
//! 3. **Registration**: Plugin is registered with the manager
//! 4. **Initialization**: Plugin's `initialize()` method is called
//! 5. **Active**: Plugin receives hook calls
//! 6. **Shutdown**: Plugin's `shutdown()` method is called on exit
//!
//! # Hook System
//!
//! Plugins can register for hooks at various points:
//!
//! - `session.starting` / `session.ended` - Session lifecycle
//! - `tool.before_call` / `tool.after_call` - Tool execution
//! - `message.before_send` / `message.received` - Message handling
//! - `compaction.before` / `compaction.after` - Compaction events
//! - `permission.check` - Permission requests
//! - `on_error` / `on_retry` - Error handling
//!
//! # Example
//!
//! ```rust,ignore
//! use cortex_engine::plugin::{PluginManager, PluginConfig};
//!
//! // Create plugin manager
//! let manager = PluginManager::new("~/.cortex")
//!     .with_project_root(".");
//!
//! // Load plugins
//! let result = manager.discover_and_load().await?;
//! println!("Loaded {} plugins", result.count());
//!
//! // Call a hook
//! let context = HookContext::new("session-id", "/cwd");
//! let response = manager.call_hook(PluginHook::SessionStarting, &context).await?;
//! ```

pub mod config;
pub mod hooks;
pub mod loader;
pub mod manager;
pub mod types;

// Re-exports for convenience
pub use config::{PluginConfigBuilder, PluginConfigEntry, PluginSettings, PluginsConfig};
pub use hooks::{
    CombinedHookResult, CompactionHookContext, ErrorHookContext, HookDispatcher, HookRegistration,
    MessageHookContext, PermissionHookContext, SessionHookContext, ToolHookContext,
};
pub use loader::{
    DiscoveredPlugin, LoadedPluginInfo, PluginFormat, PluginLoadError, PluginLoadResult,
    PluginLoader, PluginSource,
};
pub use manager::{
    PluginEvent, PluginLoadedInfo, PluginManager, PluginManagerLoadResult, PluginStats,
    global_manager, init_global_manager,
};
pub use types::{
    HookContext, HookResponse, Plugin, PluginConfig, PluginHook, PluginInfo, PluginInstance,
    PluginKind, PluginPermission, PluginState, RiskLevel,
};

/// Plugin system version.
pub const VERSION: &str = "1.0.0";

/// Type alias for backwards compatibility with old PluginRegistry.
pub type PluginRegistry = PluginManager;

/// Initialize the plugin system with default settings.
///
/// This creates and registers a global plugin manager.
pub async fn init(cortex_home: impl Into<std::path::PathBuf>) -> crate::error::Result<()> {
    let manager = std::sync::Arc::new(PluginManager::new(cortex_home));
    init_global_manager(manager)?;
    Ok(())
}

/// Initialize the plugin system with project root.
pub async fn init_with_project(
    cortex_home: impl Into<std::path::PathBuf>,
    project_root: impl Into<std::path::PathBuf>,
) -> crate::error::Result<()> {
    let manager =
        std::sync::Arc::new(PluginManager::new(cortex_home).with_project_root(project_root));
    init_global_manager(manager)?;
    Ok(())
}

/// Discover and load all plugins using the global manager.
pub async fn discover_and_load() -> crate::error::Result<PluginManagerLoadResult> {
    let manager = global_manager().ok_or_else(|| {
        crate::error::CortexError::Internal("Plugin manager not initialized".into())
    })?;
    manager.discover_and_load().await
}

/// Call a hook using the global manager.
pub async fn call_hook(
    hook: PluginHook,
    context: &HookContext,
) -> crate::error::Result<HookResponse> {
    let manager = global_manager().ok_or_else(|| {
        crate::error::CortexError::Internal("Plugin manager not initialized".into())
    })?;
    manager.call_hook(hook, context).await
}

/// List all plugins using the global manager.
pub async fn list_plugins() -> crate::error::Result<Vec<PluginInfo>> {
    let manager = global_manager().ok_or_else(|| {
        crate::error::CortexError::Internal("Plugin manager not initialized".into())
    })?;
    Ok(manager.list_plugins().await)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version() {
        assert_eq!(VERSION, "1.0.0");
    }
}
