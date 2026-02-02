//! WASM runtime for executing plugins.
//!
//! This module provides the WebAssembly runtime using wasmtime
//! for executing plugin code in a sandboxed environment.
//!
//! # Security
//!
//! The runtime includes resource limits to prevent DoS attacks:
//! - CPU: Fuel-based limiting and epoch interruption
//! - Memory: Maximum 16MB per plugin instance

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use wasmtime::*;

use crate::api::{PluginContext, PluginHostFunctions};
use crate::manifest::PluginManifest;
use crate::plugin::{Plugin, PluginInfo, PluginState};
use crate::{PluginError, Result};

/// Default fuel limit for WASM execution (CPU operations limit).
/// This value allows approximately 10 million operations before exhaustion.
const DEFAULT_FUEL_LIMIT: u64 = 10_000_000;

/// Maximum memory size for a plugin instance (16MB).
const MAX_MEMORY_SIZE: usize = 16 * 1024 * 1024;

/// Maximum number of memory pages (256 pages = 16MB, each page is 64KB).
#[allow(dead_code)]
const MAX_MEMORY_PAGES: u64 = 256;

/// Maximum number of table elements.
const MAX_TABLE_ELEMENTS: u64 = 10_000;

/// Maximum number of instances per plugin.
const MAX_INSTANCES: u32 = 10;

/// Maximum number of tables per instance.
const MAX_TABLES: u32 = 10;

/// Maximum number of memories per instance.
const MAX_MEMORIES: u32 = 1;

/// WASM runtime for executing plugins.
pub struct WasmRuntime {
    engine: Engine,
}

impl WasmRuntime {
    /// Create a new WASM runtime with security limits.
    ///
    /// # Security
    ///
    /// The runtime is configured with:
    /// - Fuel consumption for CPU limiting
    /// - Epoch-based interruption for timeout handling
    pub fn new() -> Result<Self> {
        let mut config = Config::new();
        config.async_support(true);

        // SECURITY: Enable fuel consumption for CPU limiting
        // This prevents infinite loops and excessive CPU usage
        config.consume_fuel(true);

        // SECURITY: Enable epoch-based interruption for timeout handling
        // This allows external timeout enforcement
        config.epoch_interruption(true);

        let engine = Engine::new(&config)?;

        Ok(Self { engine })
    }

    /// Compile a WASM module from bytes.
    pub fn compile(&self, wasm_bytes: &[u8]) -> Result<Module> {
        Module::new(&self.engine, wasm_bytes)
            .map_err(|e| PluginError::compilation_error("unknown", e.to_string()))
    }

    /// Compile a WASM module from a file.
    pub fn compile_file(&self, path: &Path) -> Result<Module> {
        Module::from_file(&self.engine, path)
            .map_err(|e| PluginError::compilation_error(path.display().to_string(), e.to_string()))
    }

    /// Get the engine reference.
    pub fn engine(&self) -> &Engine {
        &self.engine
    }
}

// NOTE: Default impl intentionally removed for WasmRuntime.
// SECURITY: WasmRuntime::new() can fail, and using expect() in Default::default()
// would cause a panic. Callers should explicitly handle the Result from new().

/// A WASM-based plugin implementation.
pub struct WasmPlugin {
    info: PluginInfo,
    manifest: PluginManifest,
    state: PluginState,
    wasm_path: PathBuf,
    module: Option<Module>,
    #[allow(dead_code)]
    host: Arc<PluginHostFunctions>,
    config: RwLock<HashMap<String, serde_json::Value>>,
    runtime: Arc<WasmRuntime>,
}

impl WasmPlugin {
    /// Create a new WASM plugin.
    pub fn new(manifest: PluginManifest, path: PathBuf, runtime: Arc<WasmRuntime>) -> Result<Self> {
        let info = PluginInfo::from_manifest(&manifest, path.clone());
        let wasm_path = path.join(crate::WASM_FILE);

        let host = Arc::new(PluginHostFunctions::new(&info.id, path.clone()));

        Ok(Self {
            info,
            manifest,
            state: PluginState::Discovered,
            wasm_path,
            module: None,
            host,
            config: RwLock::new(HashMap::new()),
            runtime,
        })
    }

    /// Load and compile the WASM module.
    pub fn load(&mut self) -> Result<()> {
        self.state = PluginState::Loading;

        if !self.wasm_path.exists() {
            self.state = PluginState::Error;
            return Err(PluginError::load_error(
                &self.info.id,
                format!("WASM file not found: {}", self.wasm_path.display()),
            ));
        }

        match self.runtime.compile_file(&self.wasm_path) {
            Ok(module) => {
                self.module = Some(module);
                self.state = PluginState::Loaded;
                tracing::info!(
                    "Loaded WASM plugin: {} v{}",
                    self.info.name,
                    self.info.version
                );
                Ok(())
            }
            Err(e) => {
                self.state = PluginState::Error;
                Err(e)
            }
        }
    }

    /// Call a WASM function with no arguments.
    ///
    /// # Security
    ///
    /// The store is configured with resource limits:
    /// - Fuel limit: Prevents excessive CPU usage (default: 10M operations)
    /// - Memory limit: Maximum 16MB per instance
    /// - Table/instance limits for additional sandboxing
    pub async fn call_function(&self, name: &str) -> Result<i32> {
        let module = self
            .module
            .as_ref()
            .ok_or_else(|| PluginError::execution_error(&self.info.id, "Plugin not loaded"))?;

        // SECURITY: Create store with resource limiter
        let mut store = Store::new(self.runtime.engine(), PluginStoreLimits::default());

        // SECURITY: Set fuel limit to prevent infinite loops and excessive CPU usage
        store.set_fuel(DEFAULT_FUEL_LIMIT).map_err(|e| {
            PluginError::execution_error(&self.info.id, format!("Failed to set fuel: {}", e))
        })?;

        // SECURITY: Configure the store's resource limiter
        store.limiter(|limits| limits);

        let instance = Instance::new(&mut store, module, &[])
            .map_err(|e| PluginError::execution_error(&self.info.id, e.to_string()))?;

        let func = instance
            .get_typed_func::<(), i32>(&mut store, name)
            .map_err(|e| {
                PluginError::execution_error(
                    &self.info.id,
                    format!("Function '{}' not found or wrong signature: {}", name, e),
                )
            })?;

        func.call(&mut store, ())
            .map_err(|e| PluginError::execution_error(&self.info.id, e.to_string()))
    }
}

/// Store limits for WASM plugin execution.
///
/// SECURITY: Implements wasmtime's ResourceLimiter trait to enforce
/// memory and resource constraints on plugin execution.
#[derive(Debug, Clone)]
struct PluginStoreLimits {
    /// Current memory allocated by this store.
    memory_used: usize,
}

impl Default for PluginStoreLimits {
    fn default() -> Self {
        Self { memory_used: 0 }
    }
}

impl ResourceLimiter for PluginStoreLimits {
    /// Called when memory is being grown.
    ///
    /// # Security
    ///
    /// Enforces maximum memory limit of 16MB per plugin instance.
    fn memory_growing(
        &mut self,
        current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> anyhow::Result<bool> {
        // SECURITY: Check if the desired memory exceeds our limit
        if desired > MAX_MEMORY_SIZE {
            tracing::warn!(
                current_bytes = current,
                desired_bytes = desired,
                max_bytes = MAX_MEMORY_SIZE,
                "Plugin memory request denied: exceeds maximum allowed"
            );
            return Ok(false);
        }

        self.memory_used = desired;
        Ok(true)
    }

    /// Called when a table is being grown.
    ///
    /// # Security
    ///
    /// Enforces maximum table elements limit.
    fn table_growing(
        &mut self,
        _current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> anyhow::Result<bool> {
        // SECURITY: Limit table size to prevent excessive memory usage
        if desired as u64 > MAX_TABLE_ELEMENTS {
            tracing::warn!(
                desired_elements = desired,
                max_elements = MAX_TABLE_ELEMENTS,
                "Plugin table growth denied: exceeds maximum allowed"
            );
            return Ok(false);
        }
        Ok(true)
    }

    /// Returns the maximum number of instances.
    fn instances(&self) -> usize {
        MAX_INSTANCES as usize
    }

    /// Returns the maximum number of tables.
    fn tables(&self) -> usize {
        MAX_TABLES as usize
    }

    /// Returns the maximum number of memories.
    fn memories(&self) -> usize {
        MAX_MEMORIES as usize
    }
}

#[async_trait::async_trait]
impl Plugin for WasmPlugin {
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
        if self.state != PluginState::Loaded {
            return Err(PluginError::InvalidState {
                expected: "loaded".to_string(),
                actual: self.state.to_string(),
            });
        }

        self.state = PluginState::Initializing;

        // Call the plugin's init function if it exists
        if let Ok(result) = self.call_function("init").await {
            tracing::debug!(
                "Called init function for plugin {}: {}",
                self.info.id,
                result
            );
        }

        self.state = PluginState::Active;
        Ok(())
    }

    async fn shutdown(&mut self) -> Result<()> {
        self.state = PluginState::Unloading;

        // Call the plugin's shutdown function if it exists
        if let Ok(result) = self.call_function("shutdown").await {
            tracing::debug!(
                "Called shutdown function for plugin {}: {}",
                self.info.id,
                result
            );
        }

        self.state = PluginState::Unloaded;
        Ok(())
    }

    async fn execute_command(
        &self,
        name: &str,
        _args: Vec<String>,
        _ctx: &PluginContext,
    ) -> Result<String> {
        // Find the command in the manifest
        let cmd = self
            .manifest
            .commands
            .iter()
            .find(|c| c.name == name || c.aliases.contains(&name.to_string()))
            .ok_or_else(|| PluginError::CommandError(format!("Command '{}' not found", name)))?;

        // Determine the function name to call
        let func_name = format!("cmd_{}", cmd.name.replace('-', "_"));

        // Call the function
        let result = self.call_function(&func_name).await?;

        Ok(format!("Command {} executed with result: {}", name, result))
    }

    fn get_config(&self, key: &str) -> Option<serde_json::Value> {
        let config = self.config.blocking_read();
        config.get(key).cloned()
    }

    fn set_config(&mut self, key: &str, value: serde_json::Value) -> Result<()> {
        let mut config = self.config.blocking_write();
        config.insert(key.to_string(), value);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wasm_runtime_creation() {
        let runtime = WasmRuntime::new();
        assert!(runtime.is_ok());
    }
}
