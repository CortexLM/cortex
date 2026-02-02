//! Cortex configuration management.
//!
//! Handles loading and saving configuration for the Cortex CLI.
//! All model access goes through the Cortex backend API.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

// ============================================================
// CONSTANTS
// ============================================================

/// Default Cortex API URL
pub const DEFAULT_API_URL: &str = "https://api.cortex.foundation";

/// Default provider (always "cortex" now)
pub const DEFAULT_PROVIDER: &str = "cortex";

/// Default model
pub const DEFAULT_MODEL: &str = "anthropic/claude-opus-4.5";

/// Configuration file name
pub const CONFIG_FILE: &str = "config.json";

/// Sessions directory name
pub const SESSIONS_DIR: &str = "sessions";

/// Single provider - Cortex Backend (for compatibility with old code)
pub const PROVIDERS: &[ProviderInfo] = &[ProviderInfo {
    id: "cortex",
    name: "Cortex",
    env_var: "CORTEX_AUTH_TOKEN",
    base_url: "https://api.cortex.foundation",
    requires_key: true,
}];

// ============================================================
// TYPES
// ============================================================

/// Provider information (for compatibility - only Cortex now).
#[derive(Debug, Clone, Copy)]
pub struct ProviderInfo {
    /// Provider identifier.
    pub id: &'static str,
    /// Display name.
    pub name: &'static str,
    /// Environment variable for auth token.
    pub env_var: &'static str,
    /// Default base URL.
    pub base_url: &'static str,
    /// Whether authentication is required.
    pub requires_key: bool,
}

impl ProviderInfo {
    /// Gets the auth token from environment.
    pub fn get_api_key(&self, _config_key: Option<&str>) -> Option<String> {
        std::env::var(self.env_var).ok()
    }

    /// Checks if this provider is available (has auth token).
    pub fn is_available(&self, _config_key: Option<&str>) -> bool {
        std::env::var(self.env_var).is_ok()
    }
}

/// Provider-specific configuration (for compatibility).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProviderConfig {
    /// API key / auth token.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,

    /// Custom base URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,

    /// Whether this provider is enabled.
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// Default model for this provider.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
}

fn default_enabled() -> bool {
    true
}

/// Main configuration structure for Cortex CLI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CortexConfig {
    /// Cortex API base URL (default: https://api.cortex.foundation)
    #[serde(default = "default_api_url")]
    pub api_url: String,

    /// Default provider (always "cortex").
    #[serde(default = "default_provider")]
    pub default_provider: String,

    /// Default model to use.
    #[serde(default = "default_model")]
    pub default_model: String,

    /// Provider configurations (for compatibility).
    #[serde(default)]
    pub providers: HashMap<String, ProviderConfig>,

    /// Model aliases (e.g., "claude" -> "anthropic/claude-opus-4.5").
    #[serde(default)]
    pub aliases: HashMap<String, String>,

    /// Maximum tokens for responses.
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,

    /// Default temperature.
    #[serde(default = "default_temperature")]
    pub temperature: f32,

    /// Auto-save sessions.
    #[serde(default = "default_auto_save")]
    pub auto_save: bool,

    /// Last used model (for persistence across sessions)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_model: Option<String>,

    /// Last used provider (for compatibility)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_provider: Option<String>,

    /// Last used theme (for persistence across sessions)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_theme: Option<String>,
}

fn default_api_url() -> String {
    std::env::var("CORTEX_API_URL").unwrap_or_else(|_| DEFAULT_API_URL.to_string())
}

fn default_provider() -> String {
    DEFAULT_PROVIDER.to_string()
}

fn default_model() -> String {
    DEFAULT_MODEL.to_string()
}

fn default_max_tokens() -> u32 {
    8192
}

fn default_temperature() -> f32 {
    0.7
}

fn default_auto_save() -> bool {
    true
}

impl Default for CortexConfig {
    fn default() -> Self {
        let mut aliases = HashMap::new();
        aliases.insert(
            "claude".to_string(),
            "anthropic/claude-opus-4.5".to_string(),
        );
        aliases.insert("opus".to_string(), "anthropic/claude-opus-4.5".to_string());
        aliases.insert(
            "haiku".to_string(),
            "anthropic/claude-haiku-4.5".to_string(),
        );
        aliases.insert(
            "sonnet".to_string(),
            "anthropic/claude-sonnet-4-20250514".to_string(),
        );
        aliases.insert("gpt4".to_string(), "openai/gpt-4o".to_string());
        aliases.insert("gpt".to_string(), "openai/gpt-4o".to_string());
        aliases.insert("o1".to_string(), "openai/o1".to_string());
        aliases.insert("o3".to_string(), "openai/o3".to_string());
        aliases.insert(
            "gemini".to_string(),
            "google/gemini-2.5-pro-preview-06-05".to_string(),
        );
        aliases.insert("deepseek".to_string(), "deepseek/deepseek-chat".to_string());
        aliases.insert("r1".to_string(), "deepseek/deepseek-r1".to_string());
        aliases.insert(
            "llama".to_string(),
            "meta-llama/llama-3.3-70b-instruct".to_string(),
        );

        Self {
            api_url: default_api_url(),
            default_provider: default_provider(),
            default_model: default_model(),
            providers: HashMap::new(),
            aliases,
            max_tokens: default_max_tokens(),
            temperature: default_temperature(),
            auto_save: default_auto_save(),
            last_model: None,
            last_provider: None,
            last_theme: None,
        }
    }
}

impl CortexConfig {
    /// Gets the Cortex configuration directory.
    ///
    /// Uses platform-specific conventions:
    /// - Linux/macOS: `~/.cortex`
    /// - Windows: `%APPDATA%\cortex`
    ///
    /// Can be overridden with the `CORTEX_HOME` environment variable.
    pub fn config_dir() -> Result<PathBuf> {
        // Check for CORTEX_HOME override first
        if let Ok(home) = std::env::var("CORTEX_HOME") {
            return Ok(PathBuf::from(home));
        }

        let config_dir = if cfg!(windows) {
            // Windows: %APPDATA%\cortex
            dirs::config_dir()
                .context("Could not find config directory")?
                .join("cortex")
        } else {
            // Linux/macOS: ~/.cortex
            dirs::home_dir()
                .context("Could not find home directory")?
                .join(".cortex")
        };
        Ok(config_dir)
    }

    /// Gets the configuration file path.
    pub fn config_path() -> Result<PathBuf> {
        Ok(Self::config_dir()?.join(CONFIG_FILE))
    }

    /// Gets the sessions directory path.
    pub fn sessions_dir() -> Result<PathBuf> {
        Ok(Self::config_dir()?.join(SESSIONS_DIR))
    }

    /// Loads configuration from file, creating default if not exists.
    pub fn load() -> Result<Self> {
        let config_path = Self::config_path()?;
        if config_path.exists() {
            let content = std::fs::read_to_string(&config_path)
                .with_context(|| format!("Failed to read config from {:?}", config_path))?;
            let config: Self =
                serde_json::from_str(&content).with_context(|| "Failed to parse config file")?;
            Ok(config)
        } else {
            let config = Self::default();
            config.save()?;
            Ok(config)
        }
    }

    /// Saves configuration to file.
    pub fn save(&self) -> Result<()> {
        let config_dir = Self::config_dir()?;
        std::fs::create_dir_all(&config_dir)
            .with_context(|| format!("Failed to create config directory {:?}", config_dir))?;
        let config_path = Self::config_path()?;
        let content = serde_json::to_string_pretty(self).context("Failed to serialize config")?;
        std::fs::write(&config_path, content)
            .with_context(|| format!("Failed to write config to {:?}", config_path))?;
        Ok(())
    }

    /// Resolves a model alias to actual model name.
    pub fn resolve_alias(&self, model: &str) -> String {
        self.aliases
            .get(model)
            .cloned()
            .unwrap_or_else(|| model.to_string())
    }

    /// Gets the API key/token (for compatibility).
    ///
    /// Uses the centralized auth module to check:
    /// 1. CORTEX_AUTH_TOKEN environment variable
    /// 2. System keyring (via cortex_login)
    pub fn get_api_key(&self, _provider_id: &str) -> Option<String> {
        cortex_engine::auth_token::get_auth_token(None).ok()
    }

    /// Gets the base URL (for compatibility).
    pub fn get_base_url(&self, _provider_id: &str) -> Option<String> {
        Some(self.api_url.clone())
    }

    /// Lists available providers (for compatibility - always returns Cortex if authenticated).
    pub fn available_providers(&self) -> Vec<&'static ProviderInfo> {
        if cortex_engine::auth_token::is_authenticated(None) {
            PROVIDERS.iter().collect()
        } else {
            vec![]
        }
    }

    /// Checks if a provider is available (for compatibility).
    pub fn is_provider_available(&self, _provider_id: &str) -> bool {
        cortex_engine::auth_token::is_authenticated(None)
    }

    /// Saves the last used model.
    pub fn save_last_model(&mut self, provider: &str, model: &str) -> Result<()> {
        self.last_provider = Some(provider.to_string());
        self.last_model = Some(model.to_string());
        self.save()
    }

    /// Gets the last used model if available.
    pub fn get_last_model(&self) -> Option<(&str, &str)> {
        match (&self.last_provider, &self.last_model) {
            (Some(p), Some(m)) => Some((p.as_str(), m.as_str())),
            _ => None,
        }
    }

    /// Saves the last used theme.
    pub fn save_last_theme(&mut self, theme: &str) -> Result<()> {
        self.last_theme = Some(theme.to_string());
        self.save()
    }

    /// Gets the last used theme if available.
    pub fn get_last_theme(&self) -> Option<&str> {
        self.last_theme.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = CortexConfig::default();
        assert_eq!(config.api_url, DEFAULT_API_URL);
        assert_eq!(config.default_provider, "cortex");
        assert_eq!(config.default_model, "anthropic/claude-opus-4.5");
    }

    #[test]
    fn test_resolve_alias() {
        let config = CortexConfig::default();
        assert_eq!(config.resolve_alias("claude"), "anthropic/claude-opus-4.5");
        assert_eq!(config.resolve_alias("unknown"), "unknown");
    }
}
