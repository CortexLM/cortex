//! Configuration types.

use std::collections::HashMap;
use std::path::PathBuf;

use cortex_protocol::AskForApproval;
use serde::{Deserialize, Serialize};

use super::execution::ExecutionConfig;
use super::providers::CustomProviderConfig;
use crate::custom_command::CustomCommandConfig;
use crate::plugin::{PluginConfigEntry, PluginSettings};

/// Permission level for granular permission control.
/// Supports three-tier allow/ask/deny permission model.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PermissionLevel {
    /// Automatically allow the operation.
    Allow,
    /// Ask the user for confirmation (default).
    #[default]
    Ask,
    /// Automatically deny the operation.
    Deny,
}

impl std::fmt::Display for PermissionLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Allow => write!(f, "allow"),
            Self::Ask => write!(f, "ask"),
            Self::Deny => write!(f, "deny"),
        }
    }
}

/// Permission configuration structure.
/// Supports granular permission control per operation type.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct PermissionConfig {
    /// Default permission level for edit operations.
    #[serde(default)]
    pub edit: PermissionLevel,
    /// Pattern-based bash command permissions.
    /// Example: `"git *" = "allow"`, `"rm -rf *" = "deny"`
    #[serde(default)]
    pub bash: HashMap<String, PermissionLevel>,
    /// Skill-specific permissions.
    /// Example: `"*" = "ask"`, `"trusted-skill" = "allow"`
    #[serde(default)]
    pub skill: HashMap<String, PermissionLevel>,
    /// Default permission for web fetch operations.
    #[serde(default)]
    pub webfetch: PermissionLevel,
    /// Permission for doom loop detection actions.
    #[serde(default)]
    pub doom_loop: PermissionLevel,
    /// Permission for operations outside workspace.
    #[serde(default)]
    pub external_directory: PermissionLevel,
    /// Permission for MCP tool calls.
    #[serde(default)]
    pub mcp: HashMap<String, PermissionLevel>,
}

/// Configuration file structure.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ConfigToml {
    pub model: Option<String>,
    pub model_provider: Option<String>,
    pub model_context_window: Option<i64>,
    pub model_auto_compact_token_limit: Option<i64>,
    pub approval_policy: Option<AskForApproval>,
    pub sandbox_mode: Option<SandboxMode>,
    pub sandbox_workspace_write: Option<SandboxWorkspaceWrite>,
    pub instructions: Option<String>,
    #[serde(default)]
    pub mcp_servers: HashMap<String, McpServerConfig>,
    pub history: Option<HistoryConfig>,
    pub model_reasoning_effort: Option<ReasoningEffort>,
    pub model_reasoning_summary: Option<ReasoningSummary>,
    pub hide_agent_reasoning: Option<bool>,
    pub show_raw_agent_reasoning: Option<bool>,
    pub check_for_update_on_startup: Option<bool>,
    pub disable_paste_burst: Option<bool>,
    pub tui: Option<TuiConfig>,
    /// Current agent profile name.
    pub current_agent: Option<String>,
    /// Named profiles for different use cases.
    #[serde(default)]
    pub profiles: HashMap<String, ProfileConfig>,
    /// Trusted directories (auto-approve in these locations).
    #[serde(default)]
    pub trusted_directories: Vec<PathBuf>,
    /// Granular permission configuration.
    #[serde(default)]
    pub permission: PermissionConfig,
    /// Custom commands defined inline.
    #[serde(default, rename = "commands")]
    pub custom_commands: Vec<CustomCommandConfig>,
    /// Plugin configurations.
    #[serde(default)]
    pub plugins: Vec<PluginConfigEntry>,
    /// Plugin directories to scan (in addition to defaults).
    #[serde(default)]
    pub plugin_dirs: Vec<PathBuf>,
    /// Global plugin settings.
    #[serde(default)]
    pub plugin_settings: Option<PluginSettings>,
    /// Small model for lightweight tasks (e.g., "openai/gpt-4o-mini").
    /// Auto-detected if not specified.
    pub small_model: Option<String>,
    /// User-defined model aliases.
    /// Maps short alias names to full model identifiers.
    /// Example: `fast = "gpt-4-turbo"`, `coding = "claude-sonnet-4"`
    #[serde(default)]
    pub model_aliases: HashMap<String, String>,
    /// Custom providers defined by the user.
    /// Key is the provider ID (e.g., "my-provider"), value is the provider config.
    #[serde(default)]
    pub providers: HashMap<String, CustomProviderConfig>,
    /// Execution configuration for runtime behavior.
    #[serde(default)]
    pub execution: ExecutionConfig,
}

/// Profile configuration - named presets.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ProfileConfig {
    pub model: Option<String>,
    pub model_provider: Option<String>,
    pub approval_policy: Option<AskForApproval>,
    pub sandbox_mode: Option<SandboxMode>,
    pub instructions: Option<String>,
}

/// Sandbox mode.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SandboxMode {
    DangerFullAccess,
    ReadOnly,
    #[default]
    WorkspaceWrite,
}

/// Workspace write sandbox configuration.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SandboxWorkspaceWrite {
    #[serde(default)]
    pub writable_roots: Vec<PathBuf>,
    #[serde(default)]
    pub network_access: bool,
    #[serde(default)]
    pub exclude_tmpdir_env_var: bool,
    #[serde(default)]
    pub exclude_slash_tmp: bool,
}

/// MCP server configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct McpServerConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub timeout_seconds: Option<u64>,
}

/// History configuration.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct HistoryConfig {
    #[serde(default)]
    pub persistence: HistoryPersistence,
    pub max_bytes: Option<usize>,
}

/// History persistence mode.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HistoryPersistence {
    #[default]
    SaveAll,
    None,
}

/// Reasoning effort level.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    Low,
    #[default]
    Medium,
    High,
}

/// Reasoning summary mode.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningSummary {
    #[default]
    None,
    Brief,
    Detailed,
    Auto,
}

/// TUI configuration.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct TuiConfig {
    #[serde(default = "default_animations")]
    pub animations: bool,
    #[serde(default)]
    pub notifications: NotificationsConfig,
    #[serde(default)]
    pub theme: ThemeConfig,
}

fn default_animations() -> bool {
    true
}

/// Theme configuration.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ThemeConfig {
    /// Theme name (e.g., "dark", "light", "ocean_dark", "monokai")
    #[serde(default = "default_theme")]
    pub name: String,
}

fn default_theme() -> String {
    "dark".to_string()
}

/// Notifications configuration.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct NotificationsConfig {
    #[serde(default)]
    pub enabled: bool,
}

/// Model provider information.
#[derive(Debug, Clone)]
pub struct ModelProviderInfo {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub api_type: ApiType,
}

impl Default for ModelProviderInfo {
    fn default() -> Self {
        Self {
            id: "openai".to_string(),
            name: "OpenAI".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            api_type: ApiType::OpenAi,
        }
    }
}

/// API type for providers.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ApiType {
    #[default]
    OpenAi,
    Anthropic,
    OpenAiCompatible,
}

/// Feature flags.
#[derive(Debug, Clone, Default)]
pub struct Features {
    pub apply_patch_freeform: bool,
    pub web_search_request: bool,
    pub unified_exec: bool,
    pub rmcp_client: bool,
    pub sandbox_command_assessment: bool,
    pub windows_sandbox: bool,
}

impl Features {
    /// Check if a feature is enabled.
    pub fn enabled(&self, feature: Feature) -> bool {
        match feature {
            Feature::ApplyPatchFreeform => self.apply_patch_freeform,
            Feature::WebSearchRequest => self.web_search_request,
            Feature::UnifiedExec => self.unified_exec,
            Feature::RmcpClient => self.rmcp_client,
            Feature::SandboxCommandAssessment => self.sandbox_command_assessment,
            Feature::WindowsSandbox => self.windows_sandbox,
        }
    }

    /// Enable a feature.
    pub fn enable(&mut self, feature: Feature) {
        match feature {
            Feature::ApplyPatchFreeform => self.apply_patch_freeform = true,
            Feature::WebSearchRequest => self.web_search_request = true,
            Feature::UnifiedExec => self.unified_exec = true,
            Feature::RmcpClient => self.rmcp_client = true,
            Feature::SandboxCommandAssessment => self.sandbox_command_assessment = true,
            Feature::WindowsSandbox => self.windows_sandbox = true,
        }
    }

    /// Disable a feature.
    pub fn disable(&mut self, feature: Feature) {
        match feature {
            Feature::ApplyPatchFreeform => self.apply_patch_freeform = false,
            Feature::WebSearchRequest => self.web_search_request = false,
            Feature::UnifiedExec => self.unified_exec = false,
            Feature::RmcpClient => self.rmcp_client = false,
            Feature::SandboxCommandAssessment => self.sandbox_command_assessment = false,
            Feature::WindowsSandbox => self.windows_sandbox = false,
        }
    }
}

/// Feature identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feature {
    ApplyPatchFreeform,
    WebSearchRequest,
    UnifiedExec,
    RmcpClient,
    SandboxCommandAssessment,
    WindowsSandbox,
}
