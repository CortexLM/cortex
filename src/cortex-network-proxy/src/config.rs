//! Network proxy configuration.

use serde::{Deserialize, Serialize};

/// Network access mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NetworkMode {
    /// All HTTP methods allowed.
    #[default]
    Full,

    /// Only safe methods (GET, HEAD, OPTIONS) allowed.
    Limited,

    /// No network access.
    Disabled,
}

impl NetworkMode {
    /// Check if a method is allowed.
    pub fn allows_method(&self, method: &str) -> bool {
        match self {
            NetworkMode::Full => true,
            NetworkMode::Limited => {
                matches!(method.to_uppercase().as_str(), "GET" | "HEAD" | "OPTIONS")
            }
            NetworkMode::Disabled => false,
        }
    }

    /// Get a description of the mode.
    pub fn description(&self) -> &str {
        match self {
            NetworkMode::Full => "All HTTP methods allowed",
            NetworkMode::Limited => "Only GET, HEAD, OPTIONS allowed",
            NetworkMode::Disabled => "No network access",
        }
    }
}

impl std::fmt::Display for NetworkMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NetworkMode::Full => write!(f, "full"),
            NetworkMode::Limited => write!(f, "limited"),
            NetworkMode::Disabled => write!(f, "disabled"),
        }
    }
}

impl std::str::FromStr for NetworkMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "full" => Ok(NetworkMode::Full),
            "limited" | "readonly" | "read-only" => Ok(NetworkMode::Limited),
            "disabled" | "none" | "off" => Ok(NetworkMode::Disabled),
            _ => Err(format!("Unknown network mode: {}", s)),
        }
    }
}

/// Configuration for the network proxy.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NetworkProxyConfig {
    /// Whether the proxy is enabled.
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// Network access mode.
    #[serde(default)]
    pub mode: NetworkMode,

    /// Allowed domain patterns.
    #[serde(default)]
    pub allowed_domains: Vec<String>,

    /// Denied domain patterns.
    #[serde(default)]
    pub denied_domains: Vec<String>,

    /// Whether to allow connections to local/private IPs.
    #[serde(default)]
    pub allow_local_binding: bool,

    /// Allowed Unix socket paths.
    #[serde(default)]
    pub allow_unix_sockets: Vec<String>,

    /// Proxy URL (for upstream proxy).
    #[serde(default)]
    pub proxy_url: Option<String>,

    /// Admin interface URL.
    #[serde(default)]
    pub admin_url: Option<String>,

    /// Allow non-loopback proxy address.
    #[serde(default)]
    pub dangerously_allow_non_loopback_proxy: bool,

    /// Allow non-loopback admin address.
    #[serde(default)]
    pub dangerously_allow_non_loopback_admin: bool,
}

fn default_enabled() -> bool {
    true
}

impl NetworkProxyConfig {
    /// Create a new default config.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a builder for configuration.
    pub fn builder() -> NetworkProxyConfigBuilder {
        NetworkProxyConfigBuilder::new()
    }

    /// Create a permissive config (allows everything).
    pub fn permissive() -> Self {
        Self {
            enabled: true,
            mode: NetworkMode::Full,
            allow_local_binding: true,
            ..Default::default()
        }
    }

    /// Create a restrictive config (denies by default).
    pub fn restrictive() -> Self {
        Self {
            enabled: true,
            mode: NetworkMode::Limited,
            allow_local_binding: false,
            ..Default::default()
        }
    }
}

/// Builder for NetworkProxyConfig.
#[derive(Debug, Default)]
pub struct NetworkProxyConfigBuilder {
    config: NetworkProxyConfig,
}

impl NetworkProxyConfigBuilder {
    /// Create a new builder.
    pub fn new() -> Self {
        Self {
            config: NetworkProxyConfig::new(),
        }
    }

    /// Set enabled state.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.config.enabled = enabled;
        self
    }

    /// Set network mode.
    pub fn mode(mut self, mode: NetworkMode) -> Self {
        self.config.mode = mode;
        self
    }

    /// Add an allowed domain pattern.
    pub fn allow_domain(mut self, pattern: impl Into<String>) -> Self {
        self.config.allowed_domains.push(pattern.into());
        self
    }

    /// Add multiple allowed domain patterns.
    pub fn allow_domains(mut self, patterns: impl IntoIterator<Item = impl Into<String>>) -> Self {
        for pattern in patterns {
            self.config.allowed_domains.push(pattern.into());
        }
        self
    }

    /// Add a denied domain pattern.
    pub fn deny_domain(mut self, pattern: impl Into<String>) -> Self {
        self.config.denied_domains.push(pattern.into());
        self
    }

    /// Add multiple denied domain patterns.
    pub fn deny_domains(mut self, patterns: impl IntoIterator<Item = impl Into<String>>) -> Self {
        for pattern in patterns {
            self.config.denied_domains.push(pattern.into());
        }
        self
    }

    /// Set allow local binding.
    pub fn allow_local_binding(mut self, allow: bool) -> Self {
        self.config.allow_local_binding = allow;
        self
    }

    /// Add an allowed Unix socket path.
    pub fn allow_unix_socket(mut self, path: impl Into<String>) -> Self {
        self.config.allow_unix_sockets.push(path.into());
        self
    }

    /// Set proxy URL.
    pub fn proxy_url(mut self, url: impl Into<String>) -> Self {
        self.config.proxy_url = Some(url.into());
        self
    }

    /// Set admin URL.
    pub fn admin_url(mut self, url: impl Into<String>) -> Self {
        self.config.admin_url = Some(url.into());
        self
    }

    /// Build the config.
    pub fn build(self) -> NetworkProxyConfig {
        self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_network_mode() {
        assert!(NetworkMode::Full.allows_method("POST"));
        assert!(NetworkMode::Full.allows_method("GET"));

        assert!(!NetworkMode::Limited.allows_method("POST"));
        assert!(NetworkMode::Limited.allows_method("GET"));
        assert!(NetworkMode::Limited.allows_method("HEAD"));

        assert!(!NetworkMode::Disabled.allows_method("GET"));
    }

    #[test]
    fn test_config_builder() {
        let config = NetworkProxyConfig::builder()
            .mode(NetworkMode::Limited)
            .allow_domain("*.github.com")
            .allow_domain("api.openai.com")
            .deny_domain("evil.example")
            .build();

        assert_eq!(config.mode, NetworkMode::Limited);
        assert_eq!(config.allowed_domains.len(), 2);
        assert_eq!(config.denied_domains.len(), 1);
    }
}
