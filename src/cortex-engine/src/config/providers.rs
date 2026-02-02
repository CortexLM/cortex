//! Custom provider configuration types.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Custom provider configuration.
/// Users can define their own providers in config.toml.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomProviderConfig {
    /// Display name for the provider
    pub name: String,
    /// Base URL for the API (e.g., "https://api.example.com/v1")
    pub base_url: String,
    /// API type: "openai", "anthropic", or "openai-compatible"
    #[serde(default = "default_api_type")]
    pub api_type: String,
    /// Environment variable name for API key (e.g., "MY_PROVIDER_API_KEY")
    #[serde(default)]
    pub api_key_env: Option<String>,
    /// Default model for this provider
    #[serde(default)]
    pub default_model: Option<String>,
    /// Available models for this provider (optional list)
    #[serde(default)]
    pub models: Vec<CustomModelConfig>,
    /// Additional headers to include with requests
    #[serde(default)]
    pub headers: HashMap<String, String>,
    /// Request timeout in seconds
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u64,
}

/// Custom model configuration within a provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomModelConfig {
    /// Model identifier (used in API calls)
    pub id: String,
    /// Display name
    #[serde(default)]
    pub name: Option<String>,
    /// Context window size in tokens
    #[serde(default)]
    pub context_window: Option<i64>,
    /// Whether the model supports vision/images
    #[serde(default)]
    pub supports_vision: bool,
    /// Whether the model supports tool/function calling
    #[serde(default = "default_true")]
    pub supports_tools: bool,
    /// Whether the model supports parallel tool calls
    #[serde(default)]
    pub supports_parallel_tools: bool,
}

fn default_api_type() -> String {
    "openai-compatible".to_string()
}

fn default_timeout() -> u64 {
    120
}

fn default_true() -> bool {
    true
}

impl Default for CustomProviderConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            base_url: String::new(),
            api_type: default_api_type(),
            api_key_env: None,
            default_model: None,
            models: Vec::new(),
            headers: HashMap::new(),
            timeout_seconds: default_timeout(),
        }
    }
}
