//! Plugin manifest definitions.
//!
//! The manifest file (`plugin.toml`) contains all metadata about a plugin
//! including its capabilities, commands, hooks, and permissions.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

use crate::{PluginError, Result};

/// Plugin manifest - the main configuration file for a plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    /// Plugin metadata
    pub plugin: PluginMetadata,

    /// Plugin capabilities
    #[serde(default)]
    pub capabilities: Vec<PluginCapability>,

    /// Permissions required by the plugin
    #[serde(default)]
    pub permissions: Vec<PluginPermission>,

    /// Plugin dependencies
    #[serde(default)]
    pub dependencies: Vec<PluginDependency>,

    /// Commands provided by the plugin
    #[serde(default)]
    pub commands: Vec<PluginCommandManifest>,

    /// Hooks registered by the plugin
    #[serde(default)]
    pub hooks: Vec<PluginHookManifest>,

    /// Plugin configuration schema
    #[serde(default)]
    pub config: HashMap<String, ConfigField>,

    /// WASM module settings
    #[serde(default)]
    pub wasm: WasmSettings,
}

impl PluginManifest {
    /// Load manifest from a TOML file.
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self> {
        let content = std::fs::read_to_string(path.as_ref())?;
        Self::parse(&content)
    }

    /// Parse manifest from a TOML string.
    pub fn parse(content: &str) -> Result<Self> {
        toml::from_str(content).map_err(|e| {
            PluginError::invalid_manifest("unknown", format!("Failed to parse TOML: {}", e))
        })
    }

    /// Validate the manifest.
    pub fn validate(&self) -> Result<()> {
        // Validate plugin ID
        if self.plugin.id.is_empty() {
            return Err(PluginError::invalid_manifest(
                &self.plugin.id,
                "Plugin ID cannot be empty",
            ));
        }

        // Validate plugin ID format (alphanumeric, hyphens, underscores)
        if !self
            .plugin
            .id
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
        {
            return Err(PluginError::invalid_manifest(
                &self.plugin.id,
                "Plugin ID can only contain alphanumeric characters, hyphens, and underscores",
            ));
        }

        // Validate version
        if semver::Version::parse(&self.plugin.version).is_err() {
            return Err(PluginError::invalid_manifest(
                &self.plugin.id,
                format!("Invalid semver version: {}", self.plugin.version),
            ));
        }

        // Validate command names
        for cmd in &self.commands {
            if cmd.name.is_empty() {
                return Err(PluginError::invalid_manifest(
                    &self.plugin.id,
                    "Command name cannot be empty",
                ));
            }
        }

        Ok(())
    }

    /// Check if the plugin has a specific capability.
    pub fn has_capability(&self, cap: PluginCapability) -> bool {
        self.capabilities.contains(&cap)
    }

    /// Check if the plugin has a specific permission.
    pub fn has_permission(&self, perm: &PluginPermission) -> bool {
        self.permissions.contains(perm)
    }
}

/// Plugin metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginMetadata {
    /// Unique plugin identifier (e.g., "my-awesome-plugin")
    pub id: String,

    /// Human-readable plugin name
    pub name: String,

    /// Plugin version (semver)
    pub version: String,

    /// Plugin description
    #[serde(default)]
    pub description: String,

    /// Plugin author(s)
    #[serde(default)]
    pub authors: Vec<String>,

    /// Plugin homepage/repository URL
    #[serde(default)]
    pub homepage: Option<String>,

    /// Plugin license
    #[serde(default)]
    pub license: Option<String>,

    /// Minimum Cortex version required
    #[serde(default)]
    pub min_cortex_version: Option<String>,

    /// Keywords for discovery
    #[serde(default)]
    pub keywords: Vec<String>,

    /// Plugin icon (base64 or URL)
    #[serde(default)]
    pub icon: Option<String>,
}

/// Plugin capabilities - what a plugin can do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginCapability {
    /// Plugin can provide custom commands
    Commands,
    /// Plugin can register hooks
    Hooks,
    /// Plugin can handle events
    Events,
    /// Plugin can provide tools (MCP-style)
    Tools,
    /// Plugin can provide formatters
    Formatters,
    /// Plugin can provide custom themes
    Themes,
    /// Plugin can access configuration
    Config,
    /// Plugin can access file system (with permissions)
    FileSystem,
    /// Plugin can execute shell commands (with permissions)
    Shell,
    /// Plugin can make network requests (with permissions)
    Network,
}

impl std::fmt::Display for PluginCapability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Commands => write!(f, "commands"),
            Self::Hooks => write!(f, "hooks"),
            Self::Events => write!(f, "events"),
            Self::Tools => write!(f, "tools"),
            Self::Formatters => write!(f, "formatters"),
            Self::Themes => write!(f, "themes"),
            Self::Config => write!(f, "config"),
            Self::FileSystem => write!(f, "filesystem"),
            Self::Shell => write!(f, "shell"),
            Self::Network => write!(f, "network"),
        }
    }
}

/// Plugin permissions.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginPermission {
    /// Read files from specific paths
    ReadFile { paths: Vec<String> },
    /// Write files to specific paths
    WriteFile { paths: Vec<String> },
    /// Execute specific shell commands
    Execute { commands: Vec<String> },
    /// Access network with optional domain restrictions
    Network { domains: Option<Vec<String>> },
    /// Access environment variables
    Environment { vars: Option<Vec<String>> },
    /// Access Cortex configuration
    Config { keys: Option<Vec<String>> },
    /// Access clipboard
    Clipboard,
    /// Show notifications
    Notifications,
}

/// Plugin dependency.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginDependency {
    /// Dependency plugin ID
    pub id: String,
    /// Required version (semver range)
    pub version: String,
    /// Whether the dependency is optional
    #[serde(default)]
    pub optional: bool,
}

/// Command definition in manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginCommandManifest {
    /// Command name (without leading /)
    pub name: String,

    /// Command aliases
    #[serde(default)]
    pub aliases: Vec<String>,

    /// Command description
    pub description: String,

    /// Usage example
    #[serde(default)]
    pub usage: Option<String>,

    /// Command arguments
    #[serde(default)]
    pub args: Vec<CommandArgManifest>,

    /// Whether command is hidden
    #[serde(default)]
    pub hidden: bool,

    /// Category for grouping
    #[serde(default)]
    pub category: Option<String>,
}

/// Command argument definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandArgManifest {
    /// Argument name
    pub name: String,

    /// Argument description
    #[serde(default)]
    pub description: String,

    /// Whether the argument is required
    #[serde(default)]
    pub required: bool,

    /// Default value
    #[serde(default)]
    pub default: Option<String>,

    /// Argument type (string, number, boolean, etc.)
    #[serde(default = "default_arg_type")]
    pub arg_type: String,
}

fn default_arg_type() -> String {
    "string".to_string()
}

/// Hook definition in manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginHookManifest {
    /// Hook type
    pub hook_type: HookType,

    /// Hook priority (lower runs first)
    #[serde(default = "default_priority")]
    pub priority: i32,

    /// Pattern for filtering (e.g., file pattern for tool hooks)
    #[serde(default)]
    pub pattern: Option<String>,

    /// WASM function name to call
    #[serde(default)]
    pub function: Option<String>,
}

fn default_priority() -> i32 {
    100
}

/// Hook types supported by the plugin system.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookType {
    // ========== Tool Hooks ==========
    /// Before tool execution
    ToolExecuteBefore,
    /// After tool execution
    ToolExecuteAfter,

    // ========== Chat/Message Hooks ==========
    /// Chat message processing
    ChatMessage,

    // ========== Permission Hooks ==========
    /// Permission request
    PermissionAsk,

    // ========== Prompt/AI Hooks ==========
    /// Prompt injection - modify prompts before AI processing
    PromptInject,
    /// Before AI response starts
    AiResponseBefore,
    /// During AI streaming response
    AiResponseStream,
    /// After AI response completes
    AiResponseAfter,

    // ========== Session Hooks ==========
    /// Session start
    SessionStart,
    /// Session end
    SessionEnd,

    // ========== File Operation Hooks ==========
    /// Before file operation
    FileOperationBefore,
    /// After file operation
    FileOperationAfter,
    /// File edited (legacy, kept for compatibility)
    FileEdited,

    // ========== Command Hooks ==========
    /// Before command execution
    CommandExecuteBefore,
    /// After command execution
    CommandExecuteAfter,

    // ========== Input Hooks ==========
    /// Intercept user input
    InputIntercept,

    // ========== Error Hooks ==========
    /// Handle errors
    ErrorHandle,

    // ========== Config Hooks ==========
    /// Configuration changed
    ConfigChanged,
    /// Model changed
    ModelChanged,

    // ========== Workspace Hooks ==========
    /// Workspace/working directory changed
    WorkspaceChanged,

    // ========== Clipboard Hooks ==========
    /// Before clipboard copy
    ClipboardCopy,
    /// Before clipboard paste
    ClipboardPaste,

    // ========== UI Hooks ==========
    /// UI render customization
    UiRender,
    /// Widget registration
    WidgetRegister,
    /// Keyboard binding registration
    KeyBinding,
    /// Theme override
    ThemeOverride,
    /// Layout customization
    LayoutCustomize,
    /// Modal injection
    ModalInject,
    /// Toast notification
    ToastShow,

    // ========== TUI Event Hooks ==========
    /// TUI event subscription
    TuiEventSubscribe,
    /// TUI event dispatch
    TuiEventDispatch,
    /// Custom event emission
    CustomEventEmit,
    /// Event interception
    EventIntercept,
    /// Animation frame callback
    AnimationFrame,

    // ========== Focus Hooks ==========
    /// Focus change (gained/lost)
    FocusChange,
}

impl std::fmt::Display for HookType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // Tool hooks
            Self::ToolExecuteBefore => write!(f, "tool.execute.before"),
            Self::ToolExecuteAfter => write!(f, "tool.execute.after"),
            // Chat hooks
            Self::ChatMessage => write!(f, "chat.message"),
            // Permission hooks
            Self::PermissionAsk => write!(f, "permission.ask"),
            // Prompt/AI hooks
            Self::PromptInject => write!(f, "prompt.inject"),
            Self::AiResponseBefore => write!(f, "ai.response.before"),
            Self::AiResponseStream => write!(f, "ai.response.stream"),
            Self::AiResponseAfter => write!(f, "ai.response.after"),
            // Session hooks
            Self::SessionStart => write!(f, "session.start"),
            Self::SessionEnd => write!(f, "session.end"),
            // File operation hooks
            Self::FileOperationBefore => write!(f, "file.operation.before"),
            Self::FileOperationAfter => write!(f, "file.operation.after"),
            Self::FileEdited => write!(f, "file.edited"),
            // Command hooks
            Self::CommandExecuteBefore => write!(f, "command.execute.before"),
            Self::CommandExecuteAfter => write!(f, "command.execute.after"),
            // Input hooks
            Self::InputIntercept => write!(f, "input.intercept"),
            // Error hooks
            Self::ErrorHandle => write!(f, "error.handle"),
            // Config hooks
            Self::ConfigChanged => write!(f, "config.changed"),
            Self::ModelChanged => write!(f, "model.changed"),
            // Workspace hooks
            Self::WorkspaceChanged => write!(f, "workspace.changed"),
            // Clipboard hooks
            Self::ClipboardCopy => write!(f, "clipboard.copy"),
            Self::ClipboardPaste => write!(f, "clipboard.paste"),
            // UI hooks
            Self::UiRender => write!(f, "ui.render"),
            Self::WidgetRegister => write!(f, "ui.widget.register"),
            Self::KeyBinding => write!(f, "ui.key.binding"),
            Self::ThemeOverride => write!(f, "ui.theme.override"),
            Self::LayoutCustomize => write!(f, "ui.layout.customize"),
            Self::ModalInject => write!(f, "ui.modal.inject"),
            Self::ToastShow => write!(f, "ui.toast.show"),
            // TUI event hooks
            Self::TuiEventSubscribe => write!(f, "tui.event.subscribe"),
            Self::TuiEventDispatch => write!(f, "tui.event.dispatch"),
            Self::CustomEventEmit => write!(f, "tui.event.custom"),
            Self::EventIntercept => write!(f, "tui.event.intercept"),
            Self::AnimationFrame => write!(f, "tui.animation.frame"),
            // Focus hooks
            Self::FocusChange => write!(f, "focus.change"),
        }
    }
}

/// Configuration field definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigField {
    /// Field description
    pub description: String,

    /// Field type
    #[serde(rename = "type")]
    pub field_type: ConfigFieldType,

    /// Default value
    #[serde(default)]
    pub default: Option<serde_json::Value>,

    /// Whether the field is required
    #[serde(default)]
    pub required: bool,

    /// Validation constraints
    #[serde(default)]
    pub validation: Option<ConfigValidation>,
}

/// Configuration field types.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigFieldType {
    String,
    Number,
    Boolean,
    Array,
    Object,
}

/// Configuration validation constraints.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigValidation {
    /// Minimum value (for numbers) or length (for strings/arrays)
    #[serde(default)]
    pub min: Option<f64>,

    /// Maximum value (for numbers) or length (for strings/arrays)
    #[serde(default)]
    pub max: Option<f64>,

    /// Regex pattern (for strings)
    #[serde(default)]
    pub pattern: Option<String>,

    /// Enum of allowed values
    #[serde(default)]
    pub enum_values: Option<Vec<serde_json::Value>>,
}

/// WASM module settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasmSettings {
    /// Memory limit in pages (64KB per page)
    #[serde(default = "default_memory_pages")]
    pub memory_pages: u32,

    /// Execution timeout in milliseconds
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,

    /// Enable WASI preview1 compatibility
    #[serde(default = "default_wasi_enabled")]
    pub wasi_enabled: bool,

    /// Allowed WASI capabilities
    #[serde(default)]
    pub wasi_caps: Vec<WasiCapability>,
}

impl Default for WasmSettings {
    fn default() -> Self {
        Self {
            memory_pages: default_memory_pages(),
            timeout_ms: default_timeout_ms(),
            wasi_enabled: default_wasi_enabled(),
            wasi_caps: Vec::new(),
        }
    }
}

fn default_memory_pages() -> u32 {
    256 // 16 MB
}

fn default_timeout_ms() -> u64 {
    30000 // 30 seconds
}

fn default_wasi_enabled() -> bool {
    true
}

/// WASI capabilities that can be granted to plugins.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WasiCapability {
    /// Access to standard input
    Stdin,
    /// Access to standard output
    Stdout,
    /// Access to standard error
    Stderr,
    /// Access to environment variables
    Env,
    /// Access to preopened directories
    PreopenedDirs,
    /// Access to random number generator
    Random,
    /// Access to clocks
    Clocks,
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE_MANIFEST: &str = r#"
# Top-level capabilities array (before sections)
capabilities = ["commands", "hooks"]

# Top-level permissions array (before sections)
permissions = [
    { read_file = { paths = ["**/*.rs"] } },
]

[plugin]
id = "example-plugin"
name = "Example Plugin"
version = "1.0.0"
description = "An example plugin"
authors = ["Test Author"]

[[commands]]
name = "example"
aliases = ["ex"]
description = "An example command"
usage = "/example [arg]"

[[commands.args]]
name = "arg"
description = "An argument"
required = false

[[hooks]]
hook_type = "tool_execute_before"
priority = 50
pattern = "*.rs"

[config]
api_key = { description = "API key", type = "string", required = true }
max_items = { description = "Maximum items", type = "number", default = 10 }

[wasm]
memory_pages = 128
timeout_ms = 5000
"#;

    #[test]
    fn test_parse_manifest() {
        let manifest = PluginManifest::parse(EXAMPLE_MANIFEST).unwrap();

        assert_eq!(manifest.plugin.id, "example-plugin");
        assert_eq!(manifest.plugin.name, "Example Plugin");
        assert_eq!(manifest.plugin.version, "1.0.0");
        assert_eq!(manifest.capabilities.len(), 2);
        assert_eq!(manifest.commands.len(), 1);
        assert_eq!(manifest.hooks.len(), 1);
    }

    #[test]
    fn test_validate_manifest() {
        let manifest = PluginManifest::parse(EXAMPLE_MANIFEST).unwrap();
        assert!(manifest.validate().is_ok());
    }

    #[test]
    fn test_validate_empty_id() {
        let manifest_str = r#"
[plugin]
id = ""
name = "Test"
version = "1.0.0"
"#;
        let manifest = PluginManifest::parse(manifest_str).unwrap();
        assert!(manifest.validate().is_err());
    }

    #[test]
    fn test_validate_invalid_version() {
        let manifest_str = r#"
[plugin]
id = "test"
name = "Test"
version = "invalid"
"#;
        let manifest = PluginManifest::parse(manifest_str).unwrap();
        assert!(manifest.validate().is_err());
    }

    #[test]
    fn test_has_capability() {
        let manifest = PluginManifest::parse(EXAMPLE_MANIFEST).unwrap();
        assert!(manifest.has_capability(PluginCapability::Commands));
        assert!(manifest.has_capability(PluginCapability::Hooks));
        assert!(!manifest.has_capability(PluginCapability::Network));
    }
}
