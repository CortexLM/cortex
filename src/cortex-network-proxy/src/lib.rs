//! Network proxy with domain filtering and SSRF protection for Cortex CLI.
//!
//! This crate implements a network proxy with:
//! - Domain allowlist/denylist with wildcard patterns
//! - SSRF protection (blocks private/local IPs)
//! - Network mode control (Full/Limited/Disabled)
//! - Request logging and metrics
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                    NetworkProxy                              │
//! │  ┌─────────────────────────────────────────────────────────┐│
//! │  │                    PolicyEngine                          ││
//! │  │  ┌─────────────┐  ┌─────────────┐  ┌─────────────────┐ ││
//! │  │  │ DomainMatcher│  │ IpValidator │  │ NetworkMode     │ ││
//! │  │  └─────────────┘  └─────────────┘  └─────────────────┘ ││
//! │  └─────────────────────────────────────────────────────────┘│
//! │  ┌─────────────────────────────────────────────────────────┐│
//! │  │                    ProxyState                            ││
//! │  │  - request_count                                         ││
//! │  │  - blocked_count                                         ││
//! │  │  - bytes_transferred                                     ││
//! │  └─────────────────────────────────────────────────────────┘│
//! └─────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Example
//!
//! ```rust,ignore
//! use cortex_network_proxy::{NetworkProxyConfig, PolicyEngine, NetworkMode};
//!
//! let config = NetworkProxyConfig::builder()
//!     .mode(NetworkMode::Limited)
//!     .allow_domain("*.github.com")
//!     .allow_domain("api.openai.com")
//!     .deny_domain("evil.example")
//!     .build();
//!
//! let policy = PolicyEngine::new(config);
//!
//! // Check if a host is allowed
//! let decision = policy.check_host("api.github.com", 443)?;
//! assert!(matches!(decision, HostBlockDecision::Allowed));
//! ```

pub mod config;
pub mod dns;
pub mod host;
pub mod ip_validation;
pub mod pattern;
pub mod policy;
pub mod state;

pub use config::{NetworkMode, NetworkProxyConfig, NetworkProxyConfigBuilder};
pub use dns::{
    DnsCheckResult, check_dns_resolution, host_resolves_to_non_public, safe_connect,
    safe_connect_with_timeout, verify_peer_ip,
};
pub use host::Host;
pub use ip_validation::{
    is_loopback_host, is_non_public_ip, is_non_public_ipv4, is_non_public_ipv6,
};
pub use pattern::{DomainPattern, compile_patterns};
pub use policy::{HostBlockDecision, HostBlockReason, PolicyEngine};
pub use state::{NetworkProxyState, RequestMetrics};

use thiserror::Error;

/// Errors for the network proxy.
#[derive(Debug, Error)]
pub enum NetworkProxyError {
    /// Invalid host format.
    #[error("Invalid host: {0}")]
    InvalidHost(String),

    /// Invalid pattern.
    #[error("Invalid pattern: {0}")]
    InvalidPattern(String),

    /// Host is blocked.
    #[error("Host blocked: {0} ({1})")]
    HostBlocked(String, HostBlockReason),

    /// Method not allowed.
    #[error("Method not allowed: {0}")]
    MethodNotAllowed(String),

    /// Configuration error.
    #[error("Configuration error: {0}")]
    ConfigError(String),

    /// DNS resolution error.
    #[error("DNS resolution error: {0}")]
    DnsError(String),

    /// Internal error.
    #[error("Internal error: {0}")]
    Internal(String),
}

pub type Result<T> = std::result::Result<T, NetworkProxyError>;
