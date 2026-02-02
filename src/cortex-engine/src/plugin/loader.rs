//! Plugin loader module.
//!
//! Handles discovery and loading of plugins from:
//! - `~/.cortex/plugins/` (user plugins)
//! - `.cortex/plugins/` (project plugins)
//!
//! Supports multiple plugin formats:
//! - WASM plugins (.wasm files)
//! - Native plugins (.so, .dylib, .dll)
//! - Directory-based plugins with manifest

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use super::types::{PluginConfig, PluginInfo, PluginInstance, PluginKind, PluginState};
use crate::error::{CortexError, Result};

/// Plugin manifest file name.
pub const PLUGIN_MANIFEST_FILE: &str = "cortex-plugin.json";

/// Alternative manifest locations.
pub const PLUGIN_MANIFEST_ALT: &[&str] = &[
    ".cortex-plugin/plugin.json",
    "plugin.json",
    "cortex-plugin.toml",
];

/// Plugin directory names.
pub const PLUGIN_DIR_USER: &str = "plugins";
pub const PLUGIN_DIR_PROJECT: &str = ".cortex/plugins";

/// Plugin loader that handles discovery and loading of plugins.
pub struct PluginLoader {
    /// Cortex home directory.
    cortex_home: PathBuf,
    /// Project root directory.
    project_root: Option<PathBuf>,
    /// Discovered plugins.
    discovered: Vec<DiscoveredPlugin>,
    /// Plugin configurations from config file.
    plugin_configs: HashMap<String, PluginConfig>,
}

impl PluginLoader {
    /// Create a new plugin loader.
    pub fn new(cortex_home: impl Into<PathBuf>) -> Self {
        Self {
            cortex_home: cortex_home.into(),
            project_root: None,
            discovered: Vec::new(),
            plugin_configs: HashMap::new(),
        }
    }

    /// Set project root for project-specific plugins.
    pub fn with_project_root(mut self, project_root: impl Into<PathBuf>) -> Self {
        self.project_root = Some(project_root.into());
        self
    }

    /// Add plugin configurations from config file.
    pub fn with_configs(mut self, configs: Vec<PluginConfig>) -> Self {
        for config in configs {
            self.plugin_configs.insert(config.name.clone(), config);
        }
        self
    }

    /// Get all plugin directories to search.
    pub fn plugin_dirs(&self) -> Vec<PluginDir> {
        let mut dirs = Vec::new();

        // User plugins: ~/.cortex/plugins/
        let user_dir = self.cortex_home.join(PLUGIN_DIR_USER);
        if user_dir.exists() {
            dirs.push(PluginDir {
                path: user_dir,
                source: PluginSource::User,
            });
        }

        // Project plugins: .cortex/plugins/
        if let Some(ref project_root) = self.project_root {
            let project_dir = project_root.join(PLUGIN_DIR_PROJECT);
            if project_dir.exists() {
                dirs.push(PluginDir {
                    path: project_dir,
                    source: PluginSource::Project,
                });
            }
        }

        dirs
    }

    /// Discover all plugins in search directories.
    pub async fn discover(&mut self) -> Result<Vec<DiscoveredPlugin>> {
        let mut plugins = Vec::new();

        for dir in self.plugin_dirs() {
            debug!("Scanning plugin directory: {}", dir.path.display());

            let entries = match std::fs::read_dir(&dir.path) {
                Ok(entries) => entries,
                Err(e) => {
                    warn!(
                        "Failed to read plugin directory {}: {}",
                        dir.path.display(),
                        e
                    );
                    continue;
                }
            };

            for entry in entries {
                let entry = match entry {
                    Ok(e) => e,
                    Err(e) => {
                        warn!("Failed to read directory entry: {}", e);
                        continue;
                    }
                };

                let path = entry.path();

                // Try to discover plugin
                match self.discover_plugin(&path, dir.source).await {
                    Ok(Some(plugin)) => {
                        info!(
                            "Discovered plugin: {} at {}",
                            plugin.info.name,
                            path.display()
                        );
                        plugins.push(plugin);
                    }
                    Ok(None) => {
                        debug!("Not a plugin: {}", path.display());
                    }
                    Err(e) => {
                        warn!("Failed to discover plugin at {}: {}", path.display(), e);
                    }
                }
            }
        }

        self.discovered = plugins.clone();
        Ok(plugins)
    }

    /// Discover a single plugin from a path.
    async fn discover_plugin(
        &self,
        path: &Path,
        source: PluginSource,
    ) -> Result<Option<DiscoveredPlugin>> {
        // Check for WASM file
        if path.extension().map(|e| e == "wasm").unwrap_or(false) {
            return self.discover_wasm_plugin(path, source).await;
        }

        // Check for native library
        if is_native_library(path) {
            return self.discover_native_plugin(path, source).await;
        }

        // Check for directory-based plugin
        if path.is_dir() {
            return self.discover_directory_plugin(path, source).await;
        }

        Ok(None)
    }

    /// Discover a WASM plugin.
    async fn discover_wasm_plugin(
        &self,
        path: &Path,
        source: PluginSource,
    ) -> Result<Option<DiscoveredPlugin>> {
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| CortexError::InvalidInput("Invalid WASM filename".to_string()))?
            .to_string();

        // Try to find manifest alongside WASM file
        let manifest_path = path.with_extension("json");
        let info = if manifest_path.exists() {
            self.load_manifest(&manifest_path)?
        } else {
            PluginInfo::new(&name, "0.0.0")
                .with_description("WASM plugin")
                .with_type(PluginKind::Wasm)
        };

        Ok(Some(DiscoveredPlugin {
            info,
            path: path.to_path_buf(),
            source,
            format: PluginFormat::Wasm,
        }))
    }

    /// Discover a native plugin (.so, .dylib, .dll).
    async fn discover_native_plugin(
        &self,
        path: &Path,
        source: PluginSource,
    ) -> Result<Option<DiscoveredPlugin>> {
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| CortexError::InvalidInput("Invalid library filename".to_string()))?
            .to_string();

        // Strip lib prefix if present
        let name = name.strip_prefix("lib").unwrap_or(&name).to_string();

        // Try to find manifest alongside native library
        let manifest_path = path.with_extension("json");
        let info = if manifest_path.exists() {
            self.load_manifest(&manifest_path)?
        } else {
            PluginInfo::new(&name, "0.0.0")
                .with_description("Native plugin")
                .with_type(PluginKind::Native)
        };

        Ok(Some(DiscoveredPlugin {
            info,
            path: path.to_path_buf(),
            source,
            format: PluginFormat::Native,
        }))
    }

    /// Discover a directory-based plugin.
    async fn discover_directory_plugin(
        &self,
        path: &Path,
        source: PluginSource,
    ) -> Result<Option<DiscoveredPlugin>> {
        // Try to find manifest
        let manifest_path = self.find_manifest(path)?;
        if manifest_path.is_none() {
            return Ok(None);
        }

        let manifest_path = manifest_path.unwrap();
        let info = self.load_manifest(&manifest_path)?;

        // Determine format based on contents
        let format = self.detect_plugin_format(path, &info);

        Ok(Some(DiscoveredPlugin {
            info,
            path: path.to_path_buf(),
            source,
            format,
        }))
    }

    /// Find manifest file in plugin directory.
    fn find_manifest(&self, plugin_dir: &Path) -> Result<Option<PathBuf>> {
        // Check primary location
        let primary = plugin_dir.join(PLUGIN_MANIFEST_FILE);
        if primary.exists() {
            return Ok(Some(primary));
        }

        // Check alternative locations
        for alt in PLUGIN_MANIFEST_ALT {
            let path = plugin_dir.join(alt);
            if path.exists() {
                return Ok(Some(path));
            }
        }

        Ok(None)
    }

    /// Load plugin manifest from file.
    fn load_manifest(&self, path: &Path) -> Result<PluginInfo> {
        let content = std::fs::read_to_string(path)?;

        // Try JSON first, then TOML
        let info: PluginInfo = if path.extension().map(|e| e == "toml").unwrap_or(false) {
            toml::from_str(&content).map_err(|e| {
                CortexError::InvalidInput(format!("Failed to parse TOML manifest: {e}"))
            })?
        } else {
            serde_json::from_str(&content)?
        };

        Ok(info)
    }

    /// Detect plugin format from directory contents.
    fn detect_plugin_format(&self, plugin_dir: &Path, info: &PluginInfo) -> PluginFormat {
        // Check for explicit type in manifest
        match info.plugin_type {
            PluginKind::Wasm => return PluginFormat::Wasm,
            PluginKind::Native => return PluginFormat::Native,
            _ => {}
        }

        // Check for WASM file
        if plugin_dir.join("plugin.wasm").exists() {
            return PluginFormat::Wasm;
        }

        // Check for native library
        for ext in ["so", "dylib", "dll"] {
            let lib_name = format!("lib{}.{}", info.name, ext);
            if plugin_dir.join(&lib_name).exists() {
                return PluginFormat::Native;
            }
            let lib_name = format!("{}.{}", info.name, ext);
            if plugin_dir.join(&lib_name).exists() {
                return PluginFormat::Native;
            }
        }

        // Default to script-based
        PluginFormat::Script
    }

    /// Load a discovered plugin into an instance.
    pub async fn load_plugin(&self, discovered: &DiscoveredPlugin) -> Result<PluginInstance> {
        let config = self
            .plugin_configs
            .get(&discovered.info.name)
            .cloned()
            .unwrap_or_else(|| PluginConfig::new(&discovered.info.name));

        let mut instance =
            PluginInstance::new(discovered.info.clone(), discovered.path.clone(), config);

        // Set initial state based on config
        if !instance.config.enabled {
            instance.state = PluginState::Disabled;
        } else {
            instance.state = PluginState::Unloaded;
        }

        Ok(instance)
    }

    /// Load all discovered plugins.
    pub async fn load_all(&self) -> Result<PluginLoadResult> {
        let mut result = PluginLoadResult::default();

        for discovered in &self.discovered {
            match self.load_plugin(discovered).await {
                Ok(instance) => {
                    result.loaded.push(LoadedPluginInfo {
                        name: instance.info.name.clone(),
                        version: instance.info.version.clone(),
                        source: discovered.source,
                        format: discovered.format,
                        path: discovered.path.clone(),
                    });
                }
                Err(e) => {
                    result.errors.push(PluginLoadError {
                        path: discovered.path.clone(),
                        error: e.to_string(),
                    });
                }
            }
        }

        Ok(result)
    }

    /// Get discovered plugins.
    pub fn discovered(&self) -> &[DiscoveredPlugin] {
        &self.discovered
    }
}

/// Plugin directory info.
#[derive(Debug, Clone)]
pub struct PluginDir {
    /// Directory path.
    pub path: PathBuf,
    /// Source type.
    pub source: PluginSource,
}

/// Source of a plugin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginSource {
    /// User plugin from ~/.cortex/plugins/
    User,
    /// Project plugin from .cortex/plugins/
    Project,
    /// Built-in plugin.
    Builtin,
    /// Externally loaded.
    External,
}

impl std::fmt::Display for PluginSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::User => write!(f, "user"),
            Self::Project => write!(f, "project"),
            Self::Builtin => write!(f, "builtin"),
            Self::External => write!(f, "external"),
        }
    }
}

/// Plugin format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginFormat {
    /// WASM plugin.
    Wasm,
    /// Native library (dylib).
    Native,
    /// Script-based plugin.
    Script,
    /// Directory-based plugin with hooks.json.
    Directory,
}

impl std::fmt::Display for PluginFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Wasm => write!(f, "wasm"),
            Self::Native => write!(f, "native"),
            Self::Script => write!(f, "script"),
            Self::Directory => write!(f, "directory"),
        }
    }
}

/// A discovered plugin.
#[derive(Debug, Clone)]
pub struct DiscoveredPlugin {
    /// Plugin info from manifest.
    pub info: PluginInfo,
    /// Path to plugin.
    pub path: PathBuf,
    /// Source of plugin.
    pub source: PluginSource,
    /// Plugin format.
    pub format: PluginFormat,
}

/// Result of loading plugins.
#[derive(Debug, Default)]
pub struct PluginLoadResult {
    /// Successfully loaded plugins.
    pub loaded: Vec<LoadedPluginInfo>,
    /// Errors encountered.
    pub errors: Vec<PluginLoadError>,
}

impl PluginLoadResult {
    /// Check if any plugins were loaded.
    pub fn is_empty(&self) -> bool {
        self.loaded.is_empty()
    }

    /// Get count of loaded plugins.
    pub fn count(&self) -> usize {
        self.loaded.len()
    }

    /// Check if there were any errors.
    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }
}

/// Information about a loaded plugin.
#[derive(Debug, Clone)]
pub struct LoadedPluginInfo {
    /// Plugin name.
    pub name: String,
    /// Plugin version.
    pub version: String,
    /// Source.
    pub source: PluginSource,
    /// Format.
    pub format: PluginFormat,
    /// Path.
    pub path: PathBuf,
}

/// Plugin load error.
#[derive(Debug, Clone)]
pub struct PluginLoadError {
    /// Path to plugin.
    pub path: PathBuf,
    /// Error message.
    pub error: String,
}

/// Check if a path is a native library.
fn is_native_library(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }

    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    matches!(ext, "so" | "dylib" | "dll")
}

/// WASM plugin runtime interface.
///
/// This trait defines the interface for executing WASM plugins.
/// The actual implementation requires a WASM runtime like wasmtime or wasmer.
#[cfg(feature = "wasm-plugins")]
pub trait WasmRuntime: Send + Sync {
    /// Load a WASM module from bytes.
    fn load_module(&mut self, bytes: &[u8]) -> Result<WasmModuleId>;

    /// Call a function in the module.
    fn call_function(
        &self,
        module: WasmModuleId,
        function: &str,
        args: &[WasmValue],
    ) -> Result<Vec<WasmValue>>;

    /// Unload a module.
    fn unload_module(&mut self, module: WasmModuleId) -> Result<()>;
}

/// WASM module identifier.
#[cfg(feature = "wasm-plugins")]
pub type WasmModuleId = u64;

/// WASM value type.
#[cfg(feature = "wasm-plugins")]
#[derive(Debug, Clone)]
pub enum WasmValue {
    I32(i32),
    I64(i64),
    F32(f32),
    F64(f64),
    String(String),
    Bytes(Vec<u8>),
}

/// Native plugin interface.
///
/// This defines the C ABI that native plugins must implement.
#[cfg(feature = "native-plugins")]
pub mod native_abi {
    use super::*;

    /// Plugin info function signature.
    pub type PluginInfoFn = unsafe extern "C" fn() -> *const std::ffi::c_char;

    /// Plugin init function signature.
    pub type PluginInitFn = unsafe extern "C" fn(config: *const std::ffi::c_char) -> i32;

    /// Plugin shutdown function signature.
    pub type PluginShutdownFn = unsafe extern "C" fn() -> i32;

    /// Hook handler function signature.
    pub type HookHandlerFn = unsafe extern "C" fn(
        hook: *const std::ffi::c_char,
        context: *const std::ffi::c_char,
    ) -> *const std::ffi::c_char;

    /// Expected function names in native plugins.
    pub const FN_PLUGIN_INFO: &str = "cortex_plugin_info";
    pub const FN_PLUGIN_INIT: &str = "cortex_plugin_init";
    pub const FN_PLUGIN_SHUTDOWN: &str = "cortex_plugin_shutdown";
    pub const FN_HANDLE_HOOK: &str = "cortex_handle_hook";
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_is_native_library() {
        // Create temp files to test extension checking
        let temp_dir = TempDir::new().unwrap();

        // Create test files with different extensions
        let dll_path = temp_dir.path().join("lib.dll");
        let so_path = temp_dir.path().join("lib.so");
        let dylib_path = temp_dir.path().join("lib.dylib");
        let wasm_path = temp_dir.path().join("lib.wasm");
        let js_path = temp_dir.path().join("lib.js");

        std::fs::write(&dll_path, "test").unwrap();
        std::fs::write(&so_path, "test").unwrap();
        std::fs::write(&dylib_path, "test").unwrap();
        std::fs::write(&wasm_path, "test").unwrap();
        std::fs::write(&js_path, "test").unwrap();

        // Native library extensions should match
        assert!(is_native_library(&dll_path));
        assert!(is_native_library(&so_path));
        assert!(is_native_library(&dylib_path));

        // Non-native extensions should not match
        assert!(!is_native_library(&wasm_path));
        assert!(!is_native_library(&js_path));

        // Non-existent paths should return false
        assert!(!is_native_library(Path::new("nonexistent.dll")));
    }

    #[tokio::test]
    async fn test_plugin_loader() {
        let temp_dir = TempDir::new().unwrap();
        let loader = PluginLoader::new(temp_dir.path());

        let dirs = loader.plugin_dirs();
        assert!(dirs.is_empty()); // No plugins dir exists yet
    }

    #[tokio::test]
    async fn test_plugin_discovery() {
        let temp_dir = TempDir::new().unwrap();
        let plugins_dir = temp_dir.path().join("plugins");
        std::fs::create_dir_all(&plugins_dir).unwrap();

        // Create a simple plugin
        let plugin_dir = plugins_dir.join("test-plugin");
        std::fs::create_dir_all(&plugin_dir).unwrap();

        let manifest = serde_json::json!({
            "name": "test-plugin",
            "version": "1.0.0",
            "description": "A test plugin"
        });
        std::fs::write(
            plugin_dir.join("cortex-plugin.json"),
            serde_json::to_string_pretty(&manifest).unwrap(),
        )
        .unwrap();

        let mut loader = PluginLoader::new(temp_dir.path());
        let plugins = loader.discover().await.unwrap();

        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].info.name, "test-plugin");
        assert_eq!(plugins[0].info.version, "1.0.0");
    }

    #[test]
    fn test_plugin_load_result() {
        let result = PluginLoadResult::default();
        assert!(result.is_empty());
        assert!(!result.has_errors());
    }
}
