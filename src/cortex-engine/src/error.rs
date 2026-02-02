//! Error types for Cortex Engine.

use std::path::PathBuf;

use thiserror::Error;

/// Result type alias for Cortex operations.
pub type Result<T> = std::result::Result<T, CortexError>;

/// Main error type for Cortex Engine.
#[derive(Debug, Error)]
pub enum CortexError {
    // Configuration errors
    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Configuration file not found: {path}")]
    ConfigNotFound { path: PathBuf },

    #[error("Invalid configuration: {field} - {message}")]
    InvalidConfig { field: String, message: String },

    // Authentication errors
    #[error("Authentication failed: {0}")]
    Auth(String),

    #[error("API key not found for provider: {provider}")]
    ApiKeyNotFound { provider: String },

    #[error("Token expired")]
    TokenExpired,

    // Network errors
    #[error("Network error: {0}")]
    Network(#[from] reqwest::Error),

    #[error("Connection failed to {endpoint}: {message}")]
    ConnectionFailed { endpoint: String, message: String },

    #[error("Proxy error: Failed to connect via proxy {proxy}: {message}")]
    ProxyError { proxy: String, message: String },

    #[error("Request timeout")]
    Timeout,

    // Provider errors
    #[error("Provider error: {0}")]
    Provider(String),

    #[error("Provider error: {message}")]
    ProviderError { message: String },

    #[error("Backend unavailable: {0}")]
    BackendUnavailable(String),

    #[error("Backend error: {message}")]
    BackendError { message: String },

    #[error("Authentication error: {message}")]
    AuthenticationError { message: String },

    #[error("Rate limit: {0}")]
    RateLimit(String),

    #[error("Rate limit exceeded, retry after {retry_after_secs} seconds: {message}")]
    RateLimitWithRetryAfter {
        message: String,
        retry_after_secs: u64,
    },

    // Model errors
    #[error("Model error: {0}")]
    Model(String),

    #[error("Model not found: {model}")]
    ModelNotFound { model: String },

    #[error("Model deprecated: {model}. {suggestion}")]
    ModelDeprecated { model: String, suggestion: String },

    #[error("Provider not found: {provider}")]
    ProviderNotFound { provider: String },

    #[error("Context window exceeded: {used} / {limit} tokens")]
    ContextWindowExceeded { used: i64, limit: i64 },

    #[error("Rate limit exceeded")]
    RateLimitExceeded,

    // Tool errors
    #[error("Tool execution failed: {tool} - {message}")]
    ToolExecution { tool: String, message: String },

    #[error("Unknown tool: {name}")]
    UnknownTool { name: String },

    #[error("Tool timeout: {tool} after {timeout_ms}ms")]
    ToolTimeout { tool: String, timeout_ms: u64 },

    // Sandbox errors
    #[error("Sandbox error: {0}")]
    Sandbox(String),

    #[error("Sandbox not available on this platform")]
    SandboxNotAvailable,

    #[error("Command denied by sandbox: {command}")]
    SandboxDenied { command: String },

    // File system errors
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("File not found: {path}")]
    FileNotFound { path: PathBuf },

    #[error("Permission denied: {path}")]
    PermissionDenied { path: PathBuf },

    #[error("Permission denied (possible SELinux denial): {path}. {hint}")]
    PermissionDeniedSelinux { path: PathBuf, hint: String },

    // Serialization errors
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("TOML parse error: {0}")]
    TomlParse(#[from] toml::de::Error),

    // MCP errors
    #[error("MCP error: {server} - {message}")]
    Mcp { server: String, message: String },

    #[error("MCP server not found: {server}")]
    McpServerNotFound { server: String },

    // mDNS errors
    #[error("mDNS error: {0}")]
    MdnsError(String),

    // Internal errors
    #[error("Internal error: {0}")]
    Internal(String),

    #[error("Channel closed")]
    ChannelClosed,

    #[error("Operation cancelled")]
    Cancelled,

    #[error("Snapshot error: {0}")]
    Snapshot(String),

    // Validation errors
    #[error("Invalid input: {0}")]
    InvalidInput(String),

    // Not found error
    #[error("Not found: {0}")]
    NotFound(String),

    // Serialization error
    #[error("Serialization error: {0}")]
    Serialization(String),

    // Generic
    #[error("{0}")]
    Other(#[from] anyhow::Error),
}

impl CortexError {
    /// Create an invalid input error.
    pub fn invalid_input(message: impl Into<String>) -> Self {
        Self::InvalidInput(message.into())
    }
}

impl CortexError {
    /// Create a configuration error.
    pub fn config(message: impl Into<String>) -> Self {
        Self::Config(message.into())
    }

    /// Create a model error.
    pub fn model(message: impl Into<String>) -> Self {
        Self::Model(message.into())
    }

    /// Create a sandbox error.
    pub fn sandbox(message: impl Into<String>) -> Self {
        Self::Sandbox(message.into())
    }

    /// Create an internal error.
    pub fn internal(message: impl Into<String>) -> Self {
        Self::Internal(message.into())
    }

    /// Create a tool execution error.
    pub fn tool_execution(tool: impl Into<String>, message: impl Into<String>) -> Self {
        Self::ToolExecution {
            tool: tool.into(),
            message: message.into(),
        }
    }

    /// Create an MCP error.
    pub fn mcp_error(message: impl Into<String>) -> Self {
        Self::Mcp {
            server: "unknown".into(),
            message: message.into(),
        }
    }

    /// Create an MCP error with server name.
    pub fn mcp(server: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Mcp {
            server: server.into(),
            message: message.into(),
        }
    }

    /// Create an mDNS error.
    pub fn mdns(message: impl Into<String>) -> Self {
        Self::MdnsError(message.into())
    }

    /// Create a deprecated model error with a helpful suggestion.
    pub fn model_deprecated(model: impl Into<String>, raw_error: &str) -> Self {
        let model = model.into();
        let suggestion = suggest_model_replacement(&model, raw_error);
        Self::ModelDeprecated { model, suggestion }
    }

    /// Create a proxy error. (#2758)
    pub fn proxy_error(proxy: impl Into<String>, message: impl Into<String>) -> Self {
        Self::ProxyError {
            proxy: proxy.into(),
            message: message.into(),
        }
    }

    /// Check if this error is proxy-related and enhance the message if so. (#2758)
    pub fn from_reqwest_with_proxy_check(e: reqwest::Error, endpoint: &str) -> Self {
        let err_str = e.to_string();

        // Check for proxy-related errors
        if let Some(proxy_url) = Self::detect_proxy_in_use() {
            // Check if the error indicates proxy failure
            if err_str.contains("Connection refused")
                || err_str.contains("connection refused")
                || err_str.contains("502 Bad Gateway")
                || err_str.contains("503 Service Unavailable")
                || err_str.contains("proxyconnect")
                || err_str.contains("proxy")
            {
                return Self::ProxyError {
                    proxy: proxy_url,
                    message: format!(
                        "{}. This may be a proxy configuration issue. \
                         Check your HTTPS_PROXY/HTTP_PROXY settings or try unsetting them.",
                        err_str
                    ),
                };
            }
        }

        Self::ConnectionFailed {
            endpoint: endpoint.to_string(),
            message: err_str,
        }
    }

    /// Detect if a proxy is configured via environment variables. (#2758)
    fn detect_proxy_in_use() -> Option<String> {
        std::env::var("HTTPS_PROXY")
            .ok()
            .or_else(|| std::env::var("https_proxy").ok())
            .or_else(|| std::env::var("HTTP_PROXY").ok())
            .or_else(|| std::env::var("http_proxy").ok())
    }

    /// Check if this error is retriable.
    pub fn is_retriable(&self) -> bool {
        matches!(
            self,
            Self::Network(_)
                | Self::Timeout
                | Self::RateLimitExceeded
                | Self::RateLimit(_)
                | Self::RateLimitWithRetryAfter { .. }
                | Self::ConnectionFailed { .. }
                | Self::BackendUnavailable(_)
        )
    }

    /// Get the retry-after value in seconds if this is a rate limit error with that info.
    pub fn retry_after_secs(&self) -> Option<u64> {
        match self {
            Self::RateLimitWithRetryAfter {
                retry_after_secs, ..
            } => Some(*retry_after_secs),
            _ => None,
        }
    }

    /// Check if this error indicates authentication issues.
    pub fn is_auth_error(&self) -> bool {
        matches!(
            self,
            Self::Auth(_)
                | Self::ApiKeyNotFound { .. }
                | Self::TokenExpired
                | Self::AuthenticationError { .. }
        )
    }

    /// Check if this error is due to an invalid or expired API key.
    /// Returns true for errors that indicate the user should re-authenticate.
    pub fn is_invalid_api_key(&self) -> bool {
        match self {
            Self::Auth(msg) => {
                let lower = msg.to_lowercase();
                lower.contains("invalid")
                    || lower.contains("expired")
                    || lower.contains("revoked")
                    || lower.contains("unauthorized")
                    || lower.contains("401")
            }
            Self::TokenExpired => true,
            Self::AuthenticationError { message } => {
                let lower = message.to_lowercase();
                lower.contains("invalid")
                    || lower.contains("expired")
                    || lower.contains("revoked")
                    || lower.contains("401")
            }
            Self::BackendError { message } => {
                let lower = message.to_lowercase();
                (lower.contains("401") || lower.contains("unauthorized"))
                    && (lower.contains("api") || lower.contains("key") || lower.contains("token"))
            }
            _ => false,
        }
    }

    /// Get a user-friendly error message with guidance for resolution.
    /// Provides actionable instructions when authentication fails.
    pub fn user_friendly_message(&self) -> String {
        if self.is_invalid_api_key() {
            format!(
                "{}\n\nYour API key may be invalid, expired, or revoked.\n\
                 To fix this, try one of the following:\n\
                 - Run 'cortex login' to authenticate with a new key\n\
                 - Set a valid API key via CORTEX_AUTH_TOKEN environment variable\n\
                 - Check your API key in the provider's dashboard",
                self
            )
        } else if self.is_auth_error() {
            format!(
                "{}\n\nAuthentication required. Run 'cortex login' to authenticate.",
                self
            )
        } else {
            self.to_string()
        }
    }
}

/// Suggest a replacement model for deprecated models.
fn suggest_model_replacement(model: &str, _raw_error: &str) -> String {
    // Common deprecated model replacements
    let suggestion = match model {
        // OpenAI deprecated models
        m if m.contains("gpt-3.5-turbo-0301") => "Try 'gpt-3.5-turbo' or 'gpt-4o-mini' instead.",
        m if m.contains("gpt-3.5-turbo-0613") => "Try 'gpt-3.5-turbo' or 'gpt-4o-mini' instead.",
        m if m.contains("gpt-4-0314") => "Try 'gpt-4' or 'gpt-4-turbo' instead.",
        m if m.contains("gpt-4-0613") => "Try 'gpt-4' or 'gpt-4-turbo' instead.",
        m if m.contains("gpt-4-32k") => "Try 'gpt-4-turbo' (128K context) instead.",
        m if m.contains("text-davinci") => "Try 'gpt-3.5-turbo' or 'gpt-4o-mini' instead.",
        m if m.contains("code-davinci") => "Try 'gpt-4' or 'gpt-4-turbo' instead.",
        // Anthropic deprecated models
        m if m.contains("claude-instant") => "Try 'claude-3-haiku' instead.",
        m if m.contains("claude-2.0") => "Try 'claude-3-sonnet' or 'claude-3-opus' instead.",
        m if m.contains("claude-2.1") => "Try 'claude-3-sonnet' or 'claude-3-opus' instead.",
        // Generic suggestion
        _ => "Run 'cortex models list' to see available models.",
    };

    format!(
        "{} {}",
        suggestion, "Run 'cortex models list' for all available models."
    )
}

/// Check if an API error message indicates a deprecated model.
pub fn is_deprecated_model_error(error_message: &str) -> bool {
    let lower = error_message.to_lowercase();
    lower.contains("deprecated")
        || lower.contains("decommissioned")
        || lower.contains("no longer available")
        || lower.contains("model has been removed")
        || lower.contains("model is retired")
        || (lower.contains("model") && lower.contains("not found") && lower.contains("has been"))
}

/// Check if SELinux is enabled and enforcing on the system.
#[cfg(target_os = "linux")]
pub fn is_selinux_enforcing() -> bool {
    // Check /sys/fs/selinux/enforce (reads "1" if enforcing)
    if let Ok(content) = std::fs::read_to_string("/sys/fs/selinux/enforce") {
        return content.trim() == "1";
    }
    // Alternative: check getenforce command
    if let Ok(output) = std::process::Command::new("getenforce").output() {
        if let Ok(status) = String::from_utf8(output.stdout) {
            return status.trim().eq_ignore_ascii_case("enforcing");
        }
    }
    false
}

#[cfg(not(target_os = "linux"))]
pub fn is_selinux_enforcing() -> bool {
    false
}

/// Create a permission denied error with SELinux hint if applicable.
pub fn permission_denied_with_selinux_check(path: PathBuf) -> CortexError {
    if is_selinux_enforcing() {
        CortexError::PermissionDeniedSelinux {
            path: path.clone(),
            hint: format!(
                "SELinux may be blocking access. Check 'ausearch -m avc -ts recent' for denials. \
                 Try 'restorecon -Rv {}' to fix SELinux contexts.",
                path.display()
            ),
        }
    } else {
        CortexError::PermissionDenied { path }
    }
}

/// Convert protocol error info to CortexError.
impl From<cortex_protocol::CortexErrorInfo> for CortexError {
    fn from(info: cortex_protocol::CortexErrorInfo) -> Self {
        match info {
            cortex_protocol::CortexErrorInfo::ContextWindowExceeded => {
                Self::ContextWindowExceeded { used: 0, limit: 0 }
            }
            cortex_protocol::CortexErrorInfo::UsageLimitExceeded => Self::RateLimitExceeded,
            cortex_protocol::CortexErrorInfo::Unauthorized => Self::Auth("Unauthorized".into()),
            cortex_protocol::CortexErrorInfo::SandboxError => Self::Sandbox("Sandbox error".into()),
            _ => Self::Internal("Unknown error".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let err = CortexError::config("Missing API key");
        assert_eq!(err.to_string(), "Configuration error: Missing API key");
    }

    #[test]
    fn test_error_retriable() {
        assert!(CortexError::RateLimitExceeded.is_retriable());
        assert!(!CortexError::config("test").is_retriable());
    }

    #[test]
    fn test_error_auth() {
        assert!(CortexError::TokenExpired.is_auth_error());
        assert!(!CortexError::model("test").is_auth_error());
    }
}
