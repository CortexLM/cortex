//! Plugin system error types.

use thiserror::Error;

/// Plugin system errors.
#[derive(Error, Debug)]
pub enum PluginError {
    /// Plugin not found.
    #[error("Plugin not found: {0}")]
    NotFound(String),

    /// Plugin already exists.
    #[error("Plugin already exists: {0}")]
    AlreadyExists(String),

    /// Plugin load error.
    #[error("Failed to load plugin '{plugin}': {message}")]
    LoadError { plugin: String, message: String },

    /// Plugin initialization error.
    #[error("Failed to initialize plugin '{plugin}': {message}")]
    InitError { plugin: String, message: String },

    /// WASM runtime error.
    #[error("WASM runtime error: {0}")]
    WasmError(String),

    /// WASM compilation error.
    #[error("WASM compilation error for '{plugin}': {message}")]
    CompilationError { plugin: String, message: String },

    /// Invalid plugin manifest.
    #[error("Invalid manifest for plugin '{plugin}': {message}")]
    InvalidManifest { plugin: String, message: String },

    /// Plugin execution error.
    #[error("Plugin execution error in '{plugin}': {message}")]
    ExecutionError { plugin: String, message: String },

    /// Permission denied.
    #[error("Permission denied: {0}")]
    PermissionDenied(String),

    /// Invalid plugin state.
    #[error("Invalid plugin state: expected {expected}, got {actual}")]
    InvalidState { expected: String, actual: String },

    /// Hook error.
    #[error("Hook error in '{plugin}': {message}")]
    HookError { plugin: String, message: String },

    /// Command error.
    #[error("Command error: {0}")]
    CommandError(String),

    /// Configuration error.
    #[error("Configuration error: {0}")]
    ConfigError(String),

    /// IO error.
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    /// Serialization error.
    #[error("Serialization error: {0}")]
    SerializationError(String),

    /// Dependency error.
    #[error("Dependency error for plugin '{plugin}': {message}")]
    DependencyError { plugin: String, message: String },

    /// Version mismatch.
    #[error("Version mismatch for plugin '{plugin}': required {required}, found {found}")]
    VersionMismatch {
        plugin: String,
        required: String,
        found: String,
    },

    /// Timeout error.
    #[error("Plugin operation timed out: {0}")]
    Timeout(String),

    /// Plugin is disabled.
    #[error("Plugin is disabled: {0}")]
    Disabled(String),
}

impl PluginError {
    /// Create a load error.
    pub fn load_error(plugin: impl Into<String>, message: impl Into<String>) -> Self {
        Self::LoadError {
            plugin: plugin.into(),
            message: message.into(),
        }
    }

    /// Create an init error.
    pub fn init_error(plugin: impl Into<String>, message: impl Into<String>) -> Self {
        Self::InitError {
            plugin: plugin.into(),
            message: message.into(),
        }
    }

    /// Create a compilation error.
    pub fn compilation_error(plugin: impl Into<String>, message: impl Into<String>) -> Self {
        Self::CompilationError {
            plugin: plugin.into(),
            message: message.into(),
        }
    }

    /// Create an invalid manifest error.
    pub fn invalid_manifest(plugin: impl Into<String>, message: impl Into<String>) -> Self {
        Self::InvalidManifest {
            plugin: plugin.into(),
            message: message.into(),
        }
    }

    /// Create an execution error.
    pub fn execution_error(plugin: impl Into<String>, message: impl Into<String>) -> Self {
        Self::ExecutionError {
            plugin: plugin.into(),
            message: message.into(),
        }
    }

    /// Create a hook error.
    pub fn hook_error(plugin: impl Into<String>, message: impl Into<String>) -> Self {
        Self::HookError {
            plugin: plugin.into(),
            message: message.into(),
        }
    }

    /// Create a dependency error.
    pub fn dependency_error(plugin: impl Into<String>, message: impl Into<String>) -> Self {
        Self::DependencyError {
            plugin: plugin.into(),
            message: message.into(),
        }
    }
}

impl From<toml::de::Error> for PluginError {
    fn from(err: toml::de::Error) -> Self {
        Self::SerializationError(err.to_string())
    }
}

impl From<serde_json::Error> for PluginError {
    fn from(err: serde_json::Error) -> Self {
        Self::SerializationError(err.to_string())
    }
}

impl From<wasmtime::Error> for PluginError {
    fn from(err: wasmtime::Error) -> Self {
        Self::WasmError(err.to_string())
    }
}

/// Result type alias for plugin operations.
pub type Result<T> = std::result::Result<T, PluginError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let err = PluginError::NotFound("test-plugin".to_string());
        assert_eq!(err.to_string(), "Plugin not found: test-plugin");
    }

    #[test]
    fn test_load_error() {
        let err = PluginError::load_error("my-plugin", "file not found");
        assert!(err.to_string().contains("my-plugin"));
        assert!(err.to_string().contains("file not found"));
    }

    #[test]
    fn test_from_io_error() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let plugin_err: PluginError = io_err.into();
        assert!(matches!(plugin_err, PluginError::IoError(_)));
    }
}
