//! Policy engine for network access control.

use super::{
    NetworkProxyError, Result,
    config::{NetworkMode, NetworkProxyConfig},
    host::Host,
    ip_validation::{host_resolves_to_non_public, is_loopback_host},
    pattern::{CompiledPatterns, compile_patterns},
};

/// Decision from host blocking check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostBlockDecision {
    /// Host is allowed.
    Allowed,

    /// Host is blocked.
    Blocked(HostBlockReason),
}

impl HostBlockDecision {
    /// Check if the decision allows access.
    pub fn is_allowed(&self) -> bool {
        matches!(self, HostBlockDecision::Allowed)
    }

    /// Check if the decision blocks access.
    pub fn is_blocked(&self) -> bool {
        matches!(self, HostBlockDecision::Blocked(_))
    }
}

/// Reason for blocking a host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostBlockReason {
    /// Host is explicitly denied.
    Denied,

    /// Host is not in the allowlist.
    NotAllowed,

    /// Host resolves to a local/private IP.
    NotAllowedLocal,

    /// Network mode is disabled.
    NetworkDisabled,
}

impl std::fmt::Display for HostBlockReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HostBlockReason::Denied => write!(f, "explicitly denied"),
            HostBlockReason::NotAllowed => write!(f, "not in allowlist"),
            HostBlockReason::NotAllowedLocal => write!(f, "resolves to private/local IP"),
            HostBlockReason::NetworkDisabled => write!(f, "network access disabled"),
        }
    }
}

/// Policy engine for network access control.
pub struct PolicyEngine {
    /// Network mode.
    mode: NetworkMode,

    /// Compiled allow patterns.
    allow_patterns: CompiledPatterns,

    /// Compiled deny patterns.
    deny_patterns: CompiledPatterns,

    /// Whether to allow local/private IP binding.
    allow_local_binding: bool,

    /// Whether the proxy is enabled.
    enabled: bool,
}

impl PolicyEngine {
    /// Create a new policy engine from config.
    pub fn new(config: NetworkProxyConfig) -> Result<Self> {
        let allow_patterns = compile_patterns(&config.allowed_domains)?;
        let deny_patterns = compile_patterns(&config.denied_domains)?;

        Ok(Self {
            mode: config.mode,
            allow_patterns,
            deny_patterns,
            allow_local_binding: config.allow_local_binding,
            enabled: config.enabled,
        })
    }

    /// Create a permissive policy (allows everything).
    pub fn permissive() -> Self {
        Self {
            mode: NetworkMode::Full,
            allow_patterns: CompiledPatterns::new(),
            deny_patterns: CompiledPatterns::new(),
            allow_local_binding: true,
            enabled: true,
        }
    }

    /// Check if the policy is enabled.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Get the network mode.
    pub fn mode(&self) -> NetworkMode {
        self.mode
    }

    /// Check if a method is allowed.
    pub fn check_method(&self, method: &str) -> bool {
        self.mode.allows_method(method)
    }

    /// Check if a host is blocked (synchronous check, no DNS resolution).
    pub fn check_host_sync(&self, host: &str) -> HostBlockDecision {
        // If disabled, block everything
        if self.mode == NetworkMode::Disabled {
            return HostBlockDecision::Blocked(HostBlockReason::NetworkDisabled);
        }

        let host_lower = host.to_lowercase();

        // Order matters for security:
        // 1) Explicit deny always wins
        if self.deny_patterns.matches(&host_lower) {
            return HostBlockDecision::Blocked(HostBlockReason::Denied);
        }

        // 2) Check loopback unless explicitly allowed
        if let Ok(parsed_host) = Host::parse(&host_lower)
            && !self.allow_local_binding
            && is_loopback_host(&parsed_host)
        {
            return HostBlockDecision::Blocked(HostBlockReason::NotAllowedLocal);
        }

        // 3) If allowlist is configured, check it
        if !self.allow_patterns.is_empty() && !self.allow_patterns.matches(&host_lower) {
            return HostBlockDecision::Blocked(HostBlockReason::NotAllowed);
        }

        HostBlockDecision::Allowed
    }

    /// Check if a host is blocked (async, includes DNS resolution for SSRF protection).
    pub async fn check_host(&self, host: &str, port: u16) -> HostBlockDecision {
        // First do synchronous checks
        let sync_result = self.check_host_sync(host);
        if sync_result.is_blocked() {
            return sync_result;
        }

        // 2) DNS rebinding protection - check if host resolves to non-public IP
        if !self.allow_local_binding {
            // Only resolve if it looks like a hostname (not an IP)
            if host.parse::<std::net::IpAddr>().is_err()
                && host_resolves_to_non_public(host, port).await
            {
                return HostBlockDecision::Blocked(HostBlockReason::NotAllowedLocal);
            }
        }

        HostBlockDecision::Allowed
    }

    /// Validate a full request (method + host).
    pub async fn validate_request(&self, method: &str, host: &str, port: u16) -> Result<()> {
        // Check method
        if !self.check_method(method) {
            return Err(NetworkProxyError::MethodNotAllowed(method.to_string()));
        }

        // Check host
        let decision = self.check_host(host, port).await;
        if let HostBlockDecision::Blocked(reason) = decision {
            return Err(NetworkProxyError::HostBlocked(host.to_string(), reason));
        }

        Ok(())
    }
}

impl Default for PolicyEngine {
    fn default() -> Self {
        Self::permissive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::NetworkProxyConfigBuilder;

    #[test]
    fn test_policy_deny_wins() {
        let config = NetworkProxyConfigBuilder::new()
            .allow_domain("**.example.com")
            .deny_domain("evil.example.com")
            .build();

        let policy = PolicyEngine::new(config).unwrap();

        // Allow pattern matches but deny wins
        assert!(matches!(
            policy.check_host_sync("evil.example.com"),
            HostBlockDecision::Blocked(HostBlockReason::Denied)
        ));

        // Other subdomains still allowed
        assert!(matches!(
            policy.check_host_sync("good.example.com"),
            HostBlockDecision::Allowed
        ));
    }

    #[test]
    fn test_policy_allowlist() {
        let config = NetworkProxyConfigBuilder::new()
            .allow_domain("**.github.com")
            .allow_domain("api.openai.com")
            .build();

        let policy = PolicyEngine::new(config).unwrap();

        assert!(matches!(
            policy.check_host_sync("api.github.com"),
            HostBlockDecision::Allowed
        ));
        assert!(matches!(
            policy.check_host_sync("api.openai.com"),
            HostBlockDecision::Allowed
        ));
        assert!(matches!(
            policy.check_host_sync("evil.example"),
            HostBlockDecision::Blocked(HostBlockReason::NotAllowed)
        ));
    }

    #[test]
    fn test_policy_network_disabled() {
        let config = NetworkProxyConfigBuilder::new()
            .mode(NetworkMode::Disabled)
            .build();

        let policy = PolicyEngine::new(config).unwrap();

        assert!(matches!(
            policy.check_host_sync("example.com"),
            HostBlockDecision::Blocked(HostBlockReason::NetworkDisabled)
        ));
    }

    #[test]
    fn test_policy_limited_mode() {
        let config = NetworkProxyConfigBuilder::new()
            .mode(NetworkMode::Limited)
            .build();

        let policy = PolicyEngine::new(config).unwrap();

        assert!(policy.check_method("GET"));
        assert!(policy.check_method("HEAD"));
        assert!(policy.check_method("OPTIONS"));
        assert!(!policy.check_method("POST"));
        assert!(!policy.check_method("PUT"));
        assert!(!policy.check_method("DELETE"));
    }

    #[test]
    fn test_policy_loopback_blocking() {
        let config = NetworkProxyConfigBuilder::new()
            .allow_local_binding(false)
            .build();

        let policy = PolicyEngine::new(config).unwrap();

        assert!(matches!(
            policy.check_host_sync("localhost"),
            HostBlockDecision::Blocked(HostBlockReason::NotAllowedLocal)
        ));
        assert!(matches!(
            policy.check_host_sync("127.0.0.1"),
            HostBlockDecision::Blocked(HostBlockReason::NotAllowedLocal)
        ));
    }
}
