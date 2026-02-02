//! Plugin type definitions.
//!
//! This module contains the core traits and types for the Cortex plugin system.
//! Plugins can extend Cortex with custom tools, hooks, and functionality.

use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::Result;

/// Plugin trait for implementing custom plugins.
///
/// This trait defines the lifecycle methods and core functionality
/// that all plugins must implement.
#[async_trait]
pub trait Plugin: Send + Sync {
    /// Get plugin information.
    fn info(&self) -> &PluginInfo;

    /// Initialize the plugin.
    ///
    /// Called when the plugin is first loaded. Use this to set up
    /// any necessary resources or state.
    async fn initialize(&mut self, config: &PluginConfig) -> Result<()>;

    /// Shutdown the plugin.
    ///
    /// Called when the plugin is being unloaded. Use this to clean up
    /// any resources or state.
    async fn shutdown(&mut self) -> Result<()>;

    /// Check if the plugin is healthy.
    ///
    /// Called periodically to ensure the plugin is functioning correctly.
    fn is_healthy(&self) -> bool {
        true
    }

    /// Get the hooks this plugin provides.
    fn hooks(&self) -> Vec<PluginHook> {
        Vec::new()
    }

    /// Handle a hook event.
    ///
    /// Called when a hook event occurs that this plugin has registered for.
    async fn handle_hook(&self, hook: PluginHook, context: &HookContext) -> Result<HookResponse> {
        let _ = (hook, context);
        Ok(HookResponse::Continue)
    }
}

/// Information about a plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginInfo {
    /// Unique plugin name.
    pub name: String,
    /// Semantic version string.
    pub version: String,
    /// Human-readable description.
    pub description: String,
    /// Plugin author.
    pub author: Option<String>,
    /// License identifier.
    pub license: Option<String>,
    /// Homepage URL.
    pub homepage: Option<String>,
    /// Repository URL.
    pub repository: Option<String>,
    /// Plugin type.
    #[serde(default)]
    pub plugin_type: PluginKind,
    /// Keywords for discovery.
    #[serde(default)]
    pub keywords: Vec<String>,
    /// Minimum Cortex version required.
    pub min_cortex_version: Option<String>,
    /// Required permissions.
    #[serde(default)]
    pub permissions: Vec<PluginPermission>,
}

impl PluginInfo {
    /// Create new plugin info.
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
            description: String::new(),
            author: None,
            license: None,
            homepage: None,
            repository: None,
            plugin_type: PluginKind::default(),
            keywords: Vec::new(),
            min_cortex_version: None,
            permissions: Vec::new(),
        }
    }

    /// Set description.
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = description.into();
        self
    }

    /// Set author.
    pub fn with_author(mut self, author: impl Into<String>) -> Self {
        self.author = Some(author.into());
        self
    }

    /// Set plugin type.
    pub fn with_type(mut self, plugin_type: PluginKind) -> Self {
        self.plugin_type = plugin_type;
        self
    }

    /// Add permission.
    pub fn with_permission(mut self, permission: PluginPermission) -> Self {
        self.permissions.push(permission);
        self
    }

    /// Get full plugin ID (name@version).
    pub fn id(&self) -> String {
        format!("{}@{}", self.name, self.version)
    }
}

/// Plugin kind/type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginKind {
    /// Tool extension plugin.
    #[default]
    Tool,
    /// Provider plugin.
    Provider,
    /// Hook plugin.
    Hook,
    /// Theme plugin.
    Theme,
    /// Extension plugin.
    Extension,
    /// WASM plugin.
    Wasm,
    /// Native (dylib) plugin.
    Native,
}

impl fmt::Display for PluginKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Tool => write!(f, "tool"),
            Self::Provider => write!(f, "provider"),
            Self::Hook => write!(f, "hook"),
            Self::Theme => write!(f, "theme"),
            Self::Extension => write!(f, "extension"),
            Self::Wasm => write!(f, "wasm"),
            Self::Native => write!(f, "native"),
        }
    }
}

/// Plugin state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginState {
    /// Not loaded.
    #[default]
    Unloaded,
    /// Currently loading.
    Loading,
    /// Loaded and active.
    Active,
    /// Disabled by user.
    Disabled,
    /// Error state.
    Error,
}

impl fmt::Display for PluginState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unloaded => write!(f, "unloaded"),
            Self::Loading => write!(f, "loading"),
            Self::Active => write!(f, "active"),
            Self::Disabled => write!(f, "disabled"),
            Self::Error => write!(f, "error"),
        }
    }
}

/// Plugin permissions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginPermission {
    /// Read files from filesystem.
    ReadFiles,
    /// Write files to filesystem.
    WriteFiles,
    /// Execute shell commands.
    ExecuteCommands,
    /// Make network requests.
    Network,
    /// Access environment variables.
    Environment,
    /// Read system information.
    SystemInfo,
    /// Access clipboard.
    Clipboard,
    /// Show notifications.
    Notifications,
    /// Modify session state.
    ModifySession,
    /// Access tool registry.
    AccessTools,
}

impl PluginPermission {
    /// Get description of this permission.
    pub fn description(&self) -> &'static str {
        match self {
            Self::ReadFiles => "Read files from the filesystem",
            Self::WriteFiles => "Write files to the filesystem",
            Self::ExecuteCommands => "Execute shell commands",
            Self::Network => "Make network requests",
            Self::Environment => "Access environment variables",
            Self::SystemInfo => "Read system information",
            Self::Clipboard => "Access the clipboard",
            Self::Notifications => "Show system notifications",
            Self::ModifySession => "Modify session state",
            Self::AccessTools => "Access tool registry",
        }
    }

    /// Get risk level of this permission.
    pub fn risk_level(&self) -> RiskLevel {
        match self {
            Self::ReadFiles => RiskLevel::Medium,
            Self::WriteFiles => RiskLevel::High,
            Self::ExecuteCommands => RiskLevel::High,
            Self::Network => RiskLevel::Medium,
            Self::Environment => RiskLevel::Low,
            Self::SystemInfo => RiskLevel::Low,
            Self::Clipboard => RiskLevel::Medium,
            Self::Notifications => RiskLevel::Low,
            Self::ModifySession => RiskLevel::High,
            Self::AccessTools => RiskLevel::Medium,
        }
    }
}

/// Risk level for permissions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RiskLevel {
    /// Low risk - safe operations.
    Low,
    /// Medium risk - may affect system.
    Medium,
    /// High risk - can cause significant changes.
    High,
}

/// Plugin configuration from TOML.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PluginConfig {
    /// Plugin name.
    pub name: String,
    /// Path to plugin (WASM file, dylib, or directory).
    #[serde(default)]
    pub path: Option<PathBuf>,
    /// Whether plugin is enabled.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// Custom configuration values.
    #[serde(default)]
    pub config: HashMap<String, toml::Value>,
    /// Priority (lower = runs first).
    #[serde(default)]
    pub priority: i32,
    /// Granted permissions.
    #[serde(default)]
    pub granted_permissions: Vec<PluginPermission>,
}

fn default_enabled() -> bool {
    true
}

impl PluginConfig {
    /// Create new plugin configuration.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            path: None,
            enabled: true,
            config: HashMap::new(),
            priority: 0,
            granted_permissions: Vec::new(),
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

    /// Add config value.
    pub fn with_config(mut self, key: impl Into<String>, value: toml::Value) -> Self {
        self.config.insert(key.into(), value);
        self
    }

    /// Get string config value.
    pub fn get_string(&self, key: &str) -> Option<&str> {
        self.config.get(key).and_then(|v| v.as_str())
    }

    /// Get integer config value.
    pub fn get_int(&self, key: &str) -> Option<i64> {
        self.config.get(key).and_then(|v| v.as_integer())
    }

    /// Get boolean config value.
    pub fn get_bool(&self, key: &str) -> Option<bool> {
        self.config.get(key).and_then(|v| v.as_bool())
    }
}

/// Hook types that plugins can register for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginHook {
    // Session lifecycle
    /// Session is starting.
    SessionStarting,
    /// Session has started.
    SessionStarted,
    /// Session is ending.
    SessionEnding,
    /// Session has ended.
    SessionEnded,

    // Tool lifecycle
    /// Before a tool is called.
    ToolBeforeCall,
    /// After a tool completes.
    ToolAfterCall,
    /// Tool call failed.
    ToolError,

    // Message lifecycle
    /// Before sending message to model.
    MessageBeforeSend,
    /// Message received from model.
    MessageReceived,
    /// User input received.
    UserInput,

    // Compaction
    /// Before compaction runs.
    CompactionBefore,
    /// After compaction completes.
    CompactionAfter,

    // Permission
    /// Permission check requested.
    PermissionCheck,
    /// Permission granted.
    PermissionGranted,
    /// Permission denied.
    PermissionDenied,

    // Error handling
    /// Error occurred.
    OnError,
    /// Retry attempted.
    OnRetry,
}

impl fmt::Display for PluginHook {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SessionStarting => write!(f, "session.starting"),
            Self::SessionStarted => write!(f, "session.started"),
            Self::SessionEnding => write!(f, "session.ending"),
            Self::SessionEnded => write!(f, "session.ended"),
            Self::ToolBeforeCall => write!(f, "tool.before_call"),
            Self::ToolAfterCall => write!(f, "tool.after_call"),
            Self::ToolError => write!(f, "tool.error"),
            Self::MessageBeforeSend => write!(f, "message.before_send"),
            Self::MessageReceived => write!(f, "message.received"),
            Self::UserInput => write!(f, "user.input"),
            Self::CompactionBefore => write!(f, "compaction.before"),
            Self::CompactionAfter => write!(f, "compaction.after"),
            Self::PermissionCheck => write!(f, "permission.check"),
            Self::PermissionGranted => write!(f, "permission.granted"),
            Self::PermissionDenied => write!(f, "permission.denied"),
            Self::OnError => write!(f, "on_error"),
            Self::OnRetry => write!(f, "on_retry"),
        }
    }
}

impl std::str::FromStr for PluginHook {
    type Err = crate::error::CortexError;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().replace(['-', '.'], "_").as_str() {
            "session_starting" => Ok(Self::SessionStarting),
            "session_started" => Ok(Self::SessionStarted),
            "session_ending" => Ok(Self::SessionEnding),
            "session_ended" => Ok(Self::SessionEnded),
            "tool_before_call" => Ok(Self::ToolBeforeCall),
            "tool_after_call" => Ok(Self::ToolAfterCall),
            "tool_error" => Ok(Self::ToolError),
            "message_before_send" => Ok(Self::MessageBeforeSend),
            "message_received" => Ok(Self::MessageReceived),
            "user_input" => Ok(Self::UserInput),
            "compaction_before" => Ok(Self::CompactionBefore),
            "compaction_after" => Ok(Self::CompactionAfter),
            "permission_check" => Ok(Self::PermissionCheck),
            "permission_granted" => Ok(Self::PermissionGranted),
            "permission_denied" => Ok(Self::PermissionDenied),
            "on_error" => Ok(Self::OnError),
            "on_retry" => Ok(Self::OnRetry),
            _ => Err(crate::error::CortexError::InvalidInput(format!(
                "Unknown plugin hook: {s}"
            ))),
        }
    }
}

/// Context passed to hook handlers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookContext {
    /// Session ID.
    pub session_id: String,
    /// Current turn ID.
    pub turn_id: Option<String>,
    /// Working directory.
    pub cwd: PathBuf,
    /// Hook-specific data.
    pub data: serde_json::Value,
    /// Environment variables.
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Timestamp.
    pub timestamp: u64,
}

impl HookContext {
    /// Create new hook context.
    pub fn new(session_id: impl Into<String>, cwd: impl Into<PathBuf>) -> Self {
        Self {
            session_id: session_id.into(),
            turn_id: None,
            cwd: cwd.into(),
            data: serde_json::Value::Null,
            env: HashMap::new(),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        }
    }

    /// Set turn ID.
    pub fn with_turn_id(mut self, turn_id: impl Into<String>) -> Self {
        self.turn_id = Some(turn_id.into());
        self
    }

    /// Set hook data.
    pub fn with_data(mut self, data: serde_json::Value) -> Self {
        self.data = data;
        self
    }

    /// Add environment variable.
    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }

    /// Get tool name from data if this is a tool hook.
    pub fn tool_name(&self) -> Option<&str> {
        self.data.get("tool_name").and_then(|v| v.as_str())
    }

    /// Get tool arguments from data if this is a tool hook.
    pub fn tool_args(&self) -> Option<&serde_json::Value> {
        self.data.get("args")
    }
}

/// Response from a hook handler.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action")]
pub enum HookResponse {
    /// Continue with default behavior.
    Continue,
    /// Continue with modified data.
    ContinueWith {
        /// Modified data.
        data: serde_json::Value,
    },
    /// Stop processing (cancel operation).
    Stop {
        /// Reason for stopping.
        reason: Option<String>,
    },
    /// Skip this plugin's processing.
    Skip,
    /// Inject a message into the conversation.
    InjectMessage {
        /// Message to inject.
        message: String,
    },
    /// Error occurred.
    Error {
        /// Error message.
        message: String,
    },
}

impl HookResponse {
    /// Create continue response.
    pub fn continue_default() -> Self {
        Self::Continue
    }

    /// Create continue with modified data.
    pub fn continue_with(data: serde_json::Value) -> Self {
        Self::ContinueWith { data }
    }

    /// Create stop response.
    pub fn stop(reason: impl Into<String>) -> Self {
        Self::Stop {
            reason: Some(reason.into()),
        }
    }

    /// Create skip response.
    pub fn skip() -> Self {
        Self::Skip
    }

    /// Create inject message response.
    pub fn inject_message(message: impl Into<String>) -> Self {
        Self::InjectMessage {
            message: message.into(),
        }
    }

    /// Create error response.
    pub fn error(message: impl Into<String>) -> Self {
        Self::Error {
            message: message.into(),
        }
    }

    /// Check if this response allows continuation.
    pub fn should_continue(&self) -> bool {
        matches!(
            self,
            Self::Continue | Self::ContinueWith { .. } | Self::Skip | Self::InjectMessage { .. }
        )
    }
}

impl Default for HookResponse {
    fn default() -> Self {
        Self::Continue
    }
}

/// Plugin instance wrapper with state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginInstance {
    /// Plugin info.
    pub info: PluginInfo,
    /// Plugin path.
    pub path: PathBuf,
    /// Current state.
    pub state: PluginState,
    /// Configuration.
    pub config: PluginConfig,
    /// Error message if in error state.
    pub error: Option<String>,
    /// When the plugin was loaded.
    pub loaded_at: Option<u64>,
    /// Last health check time.
    pub last_health_check: Option<u64>,
}

impl PluginInstance {
    /// Create new plugin instance.
    pub fn new(info: PluginInfo, path: impl Into<PathBuf>, config: PluginConfig) -> Self {
        Self {
            info,
            path: path.into(),
            state: PluginState::Unloaded,
            config,
            error: None,
            loaded_at: None,
            last_health_check: None,
        }
    }

    /// Get plugin ID.
    pub fn id(&self) -> String {
        self.info.id()
    }

    /// Check if plugin is active.
    pub fn is_active(&self) -> bool {
        self.state == PluginState::Active
    }

    /// Check if plugin is enabled.
    pub fn is_enabled(&self) -> bool {
        self.config.enabled && self.state != PluginState::Disabled
    }

    /// Check if plugin can be loaded.
    pub fn can_load(&self) -> bool {
        matches!(self.state, PluginState::Unloaded | PluginState::Disabled)
    }

    /// Check if plugin has a permission.
    pub fn has_permission(&self, permission: PluginPermission) -> bool {
        self.info.permissions.contains(&permission)
            || self.config.granted_permissions.contains(&permission)
    }

    /// Set error state.
    pub fn set_error(&mut self, error: impl Into<String>) {
        self.state = PluginState::Error;
        self.error = Some(error.into());
    }

    /// Mark as active.
    pub fn mark_active(&mut self) {
        self.state = PluginState::Active;
        self.loaded_at = Some(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        );
        self.error = None;
    }

    /// Mark as disabled.
    pub fn mark_disabled(&mut self) {
        self.state = PluginState::Disabled;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plugin_info() {
        let info = PluginInfo::new("test-plugin", "1.0.0")
            .with_description("A test plugin")
            .with_author("Test Author")
            .with_type(PluginKind::Tool);

        assert_eq!(info.name, "test-plugin");
        assert_eq!(info.version, "1.0.0");
        assert_eq!(info.id(), "test-plugin@1.0.0");
    }

    #[test]
    fn test_plugin_config() {
        let config = PluginConfig::new("test")
            .with_path("/path/to/plugin.wasm")
            .with_enabled(true)
            .with_config("key", toml::Value::String("value".to_string()));

        assert_eq!(config.name, "test");
        assert!(config.enabled);
        assert_eq!(config.get_string("key"), Some("value"));
    }

    #[test]
    fn test_plugin_hook_parsing() {
        assert_eq!(
            "session.starting".parse::<PluginHook>().unwrap(),
            PluginHook::SessionStarting
        );
        assert_eq!(
            "tool_before_call".parse::<PluginHook>().unwrap(),
            PluginHook::ToolBeforeCall
        );
    }

    #[test]
    fn test_hook_response() {
        let response = HookResponse::continue_default();
        assert!(response.should_continue());

        let response = HookResponse::stop("Test reason");
        assert!(!response.should_continue());
    }

    #[test]
    fn test_plugin_instance() {
        let info = PluginInfo::new("test", "1.0.0");
        let config = PluginConfig::new("test");
        let mut instance = PluginInstance::new(info, "/path", config);

        assert!(!instance.is_active());
        assert!(instance.can_load());

        instance.mark_active();
        assert!(instance.is_active());
        assert!(instance.loaded_at.is_some());
    }

    #[test]
    fn test_permission_risk() {
        assert_eq!(PluginPermission::ReadFiles.risk_level(), RiskLevel::Medium);
        assert_eq!(
            PluginPermission::ExecuteCommands.risk_level(),
            RiskLevel::High
        );
        assert_eq!(PluginPermission::SystemInfo.risk_level(), RiskLevel::Low);
    }
}
