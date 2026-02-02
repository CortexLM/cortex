//! Server configuration.

use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Server configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    /// Listen address (e.g., "0.0.0.0:8080").
    #[serde(default = "default_listen_addr")]
    pub listen_addr: String,

    /// TLS configuration.
    #[serde(default)]
    pub tls: Option<TlsConfig>,

    /// Authentication configuration.
    #[serde(default)]
    pub auth: AuthConfig,

    /// Rate limiting configuration.
    #[serde(default)]
    pub rate_limit: RateLimitConfig,

    /// Session configuration.
    #[serde(default)]
    pub sessions: SessionConfig,

    /// Provider configuration.
    #[serde(default)]
    pub providers: ProviderConfig,

    /// Logging configuration.
    #[serde(default)]
    pub logging: LoggingConfig,

    /// Static file serving.
    #[serde(default)]
    pub static_files: Option<StaticFilesConfig>,

    /// mDNS/Bonjour service discovery configuration.
    #[serde(default)]
    pub mdns: MdnsConfig,

    /// Maximum request body size in bytes.
    #[serde(default = "default_max_body_size")]
    pub max_body_size: usize,

    /// Request timeout in seconds (applies to full request lifecycle).
    #[serde(default = "default_request_timeout")]
    pub request_timeout: u64,

    /// Read timeout for individual chunks in seconds.
    /// Applies to chunked transfer encoding to prevent indefinite hangs
    /// when clients disconnect without sending the terminal chunk.
    #[serde(default = "default_read_timeout")]
    pub read_timeout: u64,

    /// Enable metrics endpoint.
    #[serde(default = "default_true")]
    pub metrics_enabled: bool,

    /// Enable health check endpoint.
    #[serde(default = "default_true")]
    pub health_enabled: bool,

    /// CORS origins (empty = allow all).
    #[serde(default)]
    pub cors_origins: Vec<String>,

    /// Graceful shutdown timeout in seconds.
    #[serde(default = "default_shutdown_timeout")]
    pub shutdown_timeout: u64,
}

fn default_shutdown_timeout() -> u64 {
    30 // 30 seconds for graceful shutdown
}

fn default_listen_addr() -> String {
    "0.0.0.0:55554".to_string()
}

fn default_max_body_size() -> usize {
    10 * 1024 * 1024 // 10MB
}

fn default_request_timeout() -> u64 {
    300 // 5 minutes
}

fn default_read_timeout() -> u64 {
    30 // 30 seconds for individual chunk reads
}

fn default_true() -> bool {
    true
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            listen_addr: default_listen_addr(),
            tls: None,
            auth: AuthConfig::default(),
            rate_limit: RateLimitConfig::default(),
            sessions: SessionConfig::default(),
            providers: ProviderConfig::default(),
            logging: LoggingConfig::default(),
            static_files: None,
            mdns: MdnsConfig::default(),
            max_body_size: default_max_body_size(),
            request_timeout: default_request_timeout(),
            read_timeout: default_read_timeout(),
            metrics_enabled: true,
            health_enabled: true,
            cors_origins: vec![],
            shutdown_timeout: default_shutdown_timeout(),
        }
    }
}

impl ServerConfig {
    /// Load configuration from file.
    pub fn load(path: impl AsRef<std::path::Path>) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let config: Self = serde_json::from_str(&content)?;
        Ok(config)
    }

    /// Load from environment variables.
    pub fn from_env() -> anyhow::Result<Self> {
        let mut config = Self::default();

        if let Ok(addr) = std::env::var("CORTEX_LISTEN_ADDR") {
            config.listen_addr = addr;
        }

        if let Ok(key) = std::env::var("CORTEX_API_KEY") {
            config.auth.api_keys.push(key);
        }

        if let Ok(secret) = std::env::var("CORTEX_JWT_SECRET") {
            config.auth.jwt_secret = Some(secret);
        }

        if let Ok(openai_key) = std::env::var("OPENAI_API_KEY") {
            config.providers.openai_api_key = Some(openai_key);
        }

        if let Ok(anthropic_key) = std::env::var("ANTHROPIC_API_KEY") {
            config.providers.anthropic_api_key = Some(anthropic_key);
        }

        // mDNS configuration from environment
        if let Ok(mdns_enabled) = std::env::var("CORTEX_MDNS_ENABLED") {
            config.mdns.enabled = mdns_enabled.parse().unwrap_or(false);
        }

        if let Ok(mdns_name) = std::env::var("CORTEX_MDNS_SERVICE_NAME") {
            config.mdns.service_name = Some(mdns_name);
        }

        Ok(config)
    }

    /// Get request timeout as Duration.
    pub fn request_timeout_duration(&self) -> Duration {
        Duration::from_secs(self.request_timeout)
    }

    /// Get read timeout as Duration (for chunked transfers).
    pub fn read_timeout_duration(&self) -> Duration {
        Duration::from_secs(self.read_timeout)
    }
}

/// TLS configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TlsConfig {
    /// Path to certificate file.
    pub cert_path: PathBuf,
    /// Path to private key file.
    pub key_path: PathBuf,
    /// Minimum TLS version (1.2 or 1.3).
    #[serde(default = "default_tls_version")]
    pub min_version: String,
}

fn default_tls_version() -> String {
    "1.2".to_string()
}

/// Authentication configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    /// Enable authentication.
    #[serde(default)]
    pub enabled: bool,
    /// API keys for simple authentication.
    #[serde(default)]
    pub api_keys: Vec<String>,
    /// JWT secret for token-based auth.
    pub jwt_secret: Option<String>,
    /// JWT token expiry in seconds.
    #[serde(default = "default_jwt_expiry")]
    pub jwt_expiry: u64,
    /// OAuth2 configuration.
    pub oauth2: Option<OAuth2Config>,
    /// Allow anonymous access to certain endpoints.
    #[serde(default)]
    pub anonymous_endpoints: Vec<String>,
}

fn default_jwt_expiry() -> u64 {
    86400 // 24 hours
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            api_keys: vec![],
            jwt_secret: None,
            jwt_expiry: default_jwt_expiry(),
            oauth2: None,
            anonymous_endpoints: vec!["/health".to_string(), "/metrics".to_string()],
        }
    }
}

/// OAuth2 configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuth2Config {
    /// OAuth2 provider (e.g., "github", "google").
    pub provider: String,
    /// Client ID.
    pub client_id: String,
    /// Client secret.
    pub client_secret: String,
    /// Authorization URL.
    pub auth_url: String,
    /// Token URL.
    pub token_url: String,
    /// Redirect URL.
    pub redirect_url: String,
    /// Scopes.
    #[serde(default)]
    pub scopes: Vec<String>,
}

/// Rate limiting configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitConfig {
    /// Enable rate limiting.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Requests per minute.
    #[serde(default = "default_rpm")]
    pub requests_per_minute: u32,
    /// Burst size.
    #[serde(default = "default_burst")]
    pub burst_size: u32,
    /// Rate limit by IP address.
    #[serde(default = "default_true")]
    pub by_ip: bool,
    /// Rate limit by API key.
    #[serde(default)]
    pub by_api_key: bool,
    /// Rate limit by user.
    #[serde(default)]
    pub by_user: bool,
    /// Trust proxy headers (X-Forwarded-For, X-Real-IP) for client IP detection.
    /// Enable this when running behind a reverse proxy (nginx, traefik, etc.).
    #[serde(default)]
    pub trust_proxy: bool,
    /// Exempt paths from rate limiting.
    #[serde(default)]
    pub exempt_paths: Vec<String>,
}

fn default_rpm() -> u32 {
    60
}

fn default_burst() -> u32 {
    10
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            requests_per_minute: default_rpm(),
            burst_size: default_burst(),
            by_ip: true,
            by_api_key: false,
            by_user: false,
            trust_proxy: false,
            exempt_paths: vec!["/health".to_string()],
        }
    }
}

/// Session configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionConfig {
    /// Maximum concurrent sessions.
    #[serde(default = "default_max_sessions")]
    pub max_concurrent: usize,
    /// Session timeout in seconds.
    #[serde(default = "default_session_timeout")]
    pub timeout: u64,
    /// Maximum messages per session.
    #[serde(default = "default_max_messages")]
    pub max_messages: usize,
    /// Maximum tokens per session.
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u64,
    /// Enable session persistence.
    #[serde(default)]
    pub persistence_enabled: bool,
    /// Session storage path.
    pub storage_path: Option<PathBuf>,
}

fn default_max_sessions() -> usize {
    100
}

fn default_session_timeout() -> u64 {
    3600 // 1 hour
}

fn default_max_messages() -> usize {
    1000
}

fn default_max_tokens() -> u64 {
    1_000_000
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            max_concurrent: default_max_sessions(),
            timeout: default_session_timeout(),
            max_messages: default_max_messages(),
            max_tokens: default_max_tokens(),
            persistence_enabled: false,
            storage_path: None,
        }
    }
}

/// Provider configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProviderConfig {
    /// OpenAI API key.
    pub openai_api_key: Option<String>,
    /// OpenAI organization ID.
    pub openai_org_id: Option<String>,
    /// OpenAI base URL override.
    pub openai_base_url: Option<String>,
    /// Anthropic API key.
    pub anthropic_api_key: Option<String>,
    /// Anthropic base URL override.
    pub anthropic_base_url: Option<String>,
    /// Azure OpenAI endpoint.
    pub azure_endpoint: Option<String>,
    /// Azure OpenAI API key.
    pub azure_api_key: Option<String>,
    /// Azure OpenAI deployment.
    pub azure_deployment: Option<String>,
    /// Default provider.
    #[serde(default = "default_provider")]
    pub default_provider: String,
    /// Default model.
    #[serde(default = "default_model")]
    pub default_model: String,
}

fn default_provider() -> String {
    "openai".to_string()
}

fn default_model() -> String {
    "gpt-4o".to_string()
}

/// Logging configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
    /// Log level.
    #[serde(default = "default_log_level")]
    pub level: String,
    /// Log format (json or pretty).
    #[serde(default = "default_log_format")]
    pub format: String,
    /// Include request/response bodies in logs.
    #[serde(default)]
    pub include_bodies: bool,
    /// Log file path.
    pub file: Option<PathBuf>,
    /// Maximum log file size in bytes.
    #[serde(default = "default_max_log_size")]
    pub max_file_size: u64,
    /// Number of log files to keep.
    #[serde(default = "default_log_files")]
    pub max_files: usize,
}

fn default_log_level() -> String {
    "info".to_string()
}

fn default_log_format() -> String {
    "pretty".to_string()
}

fn default_max_log_size() -> u64 {
    10 * 1024 * 1024 // 10MB
}

fn default_log_files() -> usize {
    5
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: default_log_level(),
            format: default_log_format(),
            include_bodies: false,
            file: None,
            max_file_size: default_max_log_size(),
            max_files: default_log_files(),
        }
    }
}

/// Static files configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StaticFilesConfig {
    /// Root directory for static files.
    pub root: PathBuf,
    /// Index file name.
    #[serde(default = "default_index")]
    pub index: String,
    /// Enable directory listing.
    #[serde(default)]
    pub directory_listing: bool,
    /// Cache control header value.
    pub cache_control: Option<String>,
}

fn default_index() -> String {
    "index.html".to_string()
}

/// mDNS/Bonjour service discovery configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MdnsConfig {
    /// Enable mDNS service publishing.
    /// When enabled, the server will advertise itself on the local network.
    #[serde(default)]
    pub enabled: bool,

    /// Custom service name for mDNS.
    /// If not set, defaults to "cortex-{port}".
    #[serde(default)]
    pub service_name: Option<String>,

    /// Discovery timeout in seconds.
    /// Used when discovering other servers on the network.
    #[serde(default = "default_discovery_timeout")]
    pub discovery_timeout: u64,
}

fn default_discovery_timeout() -> u64 {
    3 // 3 seconds
}

impl Default for MdnsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            service_name: None,
            discovery_timeout: default_discovery_timeout(),
        }
    }
}

impl MdnsConfig {
    /// Creates an mDNS config with publishing enabled.
    pub fn enabled() -> Self {
        Self {
            enabled: true,
            ..Default::default()
        }
    }

    /// Creates an mDNS config with a custom service name.
    pub fn with_name(name: impl Into<String>) -> Self {
        Self {
            enabled: true,
            service_name: Some(name.into()),
            ..Default::default()
        }
    }

    /// Get the discovery timeout as a Duration.
    pub fn discovery_timeout_duration(&self) -> Duration {
        Duration::from_secs(self.discovery_timeout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = ServerConfig::default();
        assert_eq!(config.listen_addr, "0.0.0.0:55554");
        assert!(!config.auth.enabled);
        assert!(config.rate_limit.enabled);
    }

    #[test]
    fn test_config_serialization() {
        let config = ServerConfig::default();
        let json = serde_json::to_string_pretty(&config).unwrap();
        let parsed: ServerConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(config.listen_addr, parsed.listen_addr);
    }
}
