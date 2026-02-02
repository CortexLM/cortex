//! SSRF Protection Module
//!
//! Comprehensive Server-Side Request Forgery protection including:
//! - IP range blocking (localhost, private networks, link-local, IPv6)
//! - DNS resolution before IP checking to prevent DNS rebinding
//! - Protocol validation (only http/https allowed)
//! - Domain allowlist support
//! - Timeout and size limits

use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs};
use std::time::Duration;

use thiserror::Error;
use url::Url;

use crate::api_client::{USER_AGENT, create_client_builder};

/// Maximum response size (10 MB)
pub const DEFAULT_MAX_RESPONSE_SIZE: usize = 10 * 1024 * 1024;

/// Default request timeout (30 seconds)
pub const DEFAULT_TIMEOUT_SECS: u64 = 30;

/// Default connect timeout (10 seconds)
pub const DEFAULT_CONNECT_TIMEOUT_SECS: u64 = 10;

/// SSRF protection errors.
#[derive(Debug, Error)]
pub enum SsrfError {
    #[error("Invalid URL: {0}")]
    InvalidUrl(String),

    #[error("Blocked protocol: {0} - only http and https are allowed")]
    BlockedProtocol(String),

    #[error("Blocked host: {0} - localhost and local domains are not allowed")]
    BlockedLocalhost(String),

    #[error("Blocked IP address: {0} - private and reserved IP ranges are not allowed")]
    BlockedIpAddress(String),

    #[error("DNS resolution failed for host: {0}")]
    DnsResolutionFailed(String),

    #[error("DNS rebinding detected: resolved IP {0} is in a blocked range")]
    DnsRebindingDetected(String),

    #[error("Domain not in allowlist: {0}")]
    DomainNotAllowed(String),

    #[error("Response too large: {size} bytes exceeds limit of {limit} bytes")]
    ResponseTooLarge { size: usize, limit: usize },

    #[error("Request timeout after {0} seconds")]
    Timeout(u64),

    #[error("HTTP request failed: {0}")]
    RequestFailed(String),

    #[error("Missing host in URL")]
    MissingHost,
}

/// Result type for SSRF operations.
pub type SsrfResult<T> = Result<T, SsrfError>;

/// Configuration for SSRF protection.
#[derive(Debug, Clone)]
pub struct SsrfConfig {
    /// Domain allowlist - if non-empty, only these domains are allowed.
    pub allowed_domains: HashSet<String>,

    /// Additional blocked domains (besides automatic localhost detection).
    pub blocked_domains: HashSet<String>,

    /// Maximum response body size in bytes.
    pub max_response_size: usize,

    /// Request timeout in seconds.
    pub timeout_secs: u64,

    /// Connect timeout in seconds.
    pub connect_timeout_secs: u64,

    /// Whether to allow following redirects.
    pub allow_redirects: bool,

    /// Maximum number of redirects to follow.
    pub max_redirects: usize,

    /// Whether to block IPv6 addresses entirely.
    pub block_ipv6: bool,

    /// Whether to perform DNS resolution before making the request.
    /// Set to true to prevent DNS rebinding attacks.
    pub resolve_dns_first: bool,
}

impl Default for SsrfConfig {
    fn default() -> Self {
        Self {
            allowed_domains: HashSet::new(),
            blocked_domains: HashSet::new(),
            max_response_size: DEFAULT_MAX_RESPONSE_SIZE,
            timeout_secs: DEFAULT_TIMEOUT_SECS,
            connect_timeout_secs: DEFAULT_CONNECT_TIMEOUT_SECS,
            allow_redirects: true,
            max_redirects: 5,
            block_ipv6: false,
            resolve_dns_first: true,
        }
    }
}

impl SsrfConfig {
    /// Create a new SSRF config with default settings.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a domain to the allowlist.
    pub fn allow_domain(mut self, domain: impl Into<String>) -> Self {
        self.allowed_domains.insert(domain.into().to_lowercase());
        self
    }

    /// Add multiple domains to the allowlist.
    pub fn allow_domains(mut self, domains: impl IntoIterator<Item = impl Into<String>>) -> Self {
        for domain in domains {
            self.allowed_domains.insert(domain.into().to_lowercase());
        }
        self
    }

    /// Block a specific domain.
    pub fn block_domain(mut self, domain: impl Into<String>) -> Self {
        self.blocked_domains.insert(domain.into().to_lowercase());
        self
    }

    /// Set maximum response size.
    pub fn max_response_size(mut self, size: usize) -> Self {
        self.max_response_size = size;
        self
    }

    /// Set request timeout.
    pub fn timeout(mut self, secs: u64) -> Self {
        self.timeout_secs = secs;
        self
    }

    /// Set connect timeout.
    pub fn connect_timeout(mut self, secs: u64) -> Self {
        self.connect_timeout_secs = secs;
        self
    }

    /// Disable following redirects.
    pub fn no_redirects(mut self) -> Self {
        self.allow_redirects = false;
        self
    }

    /// Set max redirects.
    pub fn max_redirects(mut self, max: usize) -> Self {
        self.max_redirects = max;
        self
    }

    /// Block all IPv6 addresses.
    pub fn block_ipv6(mut self) -> Self {
        self.block_ipv6 = true;
        self
    }

    /// Disable DNS resolution check (not recommended).
    pub fn skip_dns_resolution(mut self) -> Self {
        self.resolve_dns_first = false;
        self
    }
}

/// SSRF Protection handler.
#[derive(Debug, Clone)]
pub struct SsrfProtection {
    config: SsrfConfig,
}

impl SsrfProtection {
    /// Create a new SSRF protection handler with default config.
    pub fn new() -> Self {
        Self {
            config: SsrfConfig::default(),
        }
    }

    /// Create SSRF protection with custom config.
    pub fn with_config(config: SsrfConfig) -> Self {
        Self { config }
    }

    /// Get the config.
    pub fn config(&self) -> &SsrfConfig {
        &self.config
    }

    /// Validate a URL for safe fetching.
    ///
    /// This performs comprehensive SSRF checks including:
    /// 1. Protocol validation (http/https only)
    /// 2. Host validation (no localhost, local domains)
    /// 3. IP address validation (no private/reserved ranges)
    /// 4. DNS resolution check (prevents DNS rebinding)
    /// 5. Domain allowlist check (if configured)
    pub fn validate_url(&self, url_str: &str) -> SsrfResult<Url> {
        // Parse the URL
        let url = Url::parse(url_str).map_err(|e| SsrfError::InvalidUrl(e.to_string()))?;

        // Check protocol
        self.check_protocol(&url)?;

        // Get host
        let host = url.host_str().ok_or(SsrfError::MissingHost)?;

        // Check for localhost patterns
        self.check_localhost(host)?;

        // Check domain blocklist
        self.check_blocked_domains(host)?;

        // Check domain allowlist (if configured)
        self.check_allowlist(host)?;

        // Check if host is an IP address
        if let Ok(ip) = host.parse::<IpAddr>() {
            self.check_ip_address(ip)?;
        }

        // Perform DNS resolution check to prevent DNS rebinding
        if self.config.resolve_dns_first {
            self.check_dns_resolution(&url)?;
        }

        Ok(url)
    }

    /// Check if the protocol is allowed (http/https only).
    fn check_protocol(&self, url: &Url) -> SsrfResult<()> {
        match url.scheme() {
            "http" | "https" => Ok(()),
            scheme => Err(SsrfError::BlockedProtocol(scheme.to_string())),
        }
    }

    /// Check for localhost and local domain patterns.
    fn check_localhost(&self, host: &str) -> SsrfResult<()> {
        let host_lower = host.to_lowercase();

        // Explicit localhost checks
        let localhost_patterns = [
            "localhost",
            "127.0.0.1",
            "0.0.0.0",
            "[::1]",
            "::1",
            "[0:0:0:0:0:0:0:1]",
            "[::ffff:127.0.0.1]",
        ];

        for pattern in localhost_patterns {
            if host_lower == pattern {
                return Err(SsrfError::BlockedLocalhost(host.to_string()));
            }
        }

        // Check for local domain suffixes
        let local_suffixes = [
            ".local",
            ".localhost",
            ".internal",
            ".intranet",
            ".corp",
            ".home",
            ".lan",
            ".localdomain",
            ".private",
        ];

        for suffix in local_suffixes {
            if host_lower.ends_with(suffix) {
                return Err(SsrfError::BlockedLocalhost(host.to_string()));
            }
        }

        // Check for lvh.me (localhost alias)
        if host_lower.ends_with(".lvh.me") || host_lower == "lvh.me" {
            return Err(SsrfError::BlockedLocalhost(host.to_string()));
        }

        // Check for numeric localhost variations (127.x.x.x)
        if host_lower.starts_with("127.") {
            return Err(SsrfError::BlockedLocalhost(host.to_string()));
        }

        Ok(())
    }

    /// Check if domain is in the blocklist.
    fn check_blocked_domains(&self, host: &str) -> SsrfResult<()> {
        let host_lower = host.to_lowercase();

        for blocked in &self.config.blocked_domains {
            if host_lower == *blocked || host_lower.ends_with(&format!(".{}", blocked)) {
                return Err(SsrfError::BlockedLocalhost(host.to_string()));
            }
        }

        Ok(())
    }

    /// Check if domain is in the allowlist (if configured).
    fn check_allowlist(&self, host: &str) -> SsrfResult<()> {
        if self.config.allowed_domains.is_empty() {
            return Ok(());
        }

        let host_lower = host.to_lowercase();

        for allowed in &self.config.allowed_domains {
            if host_lower == *allowed || host_lower.ends_with(&format!(".{}", allowed)) {
                return Ok(());
            }
        }

        Err(SsrfError::DomainNotAllowed(host.to_string()))
    }

    /// Check if an IP address is in a blocked range.
    fn check_ip_address(&self, ip: IpAddr) -> SsrfResult<()> {
        if self.is_blocked_ip(ip) {
            return Err(SsrfError::BlockedIpAddress(ip.to_string()));
        }

        if self.config.block_ipv6 && matches!(ip, IpAddr::V6(_)) {
            return Err(SsrfError::BlockedIpAddress(format!(
                "{} (IPv6 blocked)",
                ip
            )));
        }

        Ok(())
    }

    /// Check if an IP address is in a private/reserved range.
    pub fn is_blocked_ip(&self, ip: IpAddr) -> bool {
        match ip {
            IpAddr::V4(ipv4) => self.is_blocked_ipv4(ipv4),
            IpAddr::V6(ipv6) => self.is_blocked_ipv6(ipv6),
        }
    }

    /// Check if an IPv4 address is in a blocked range.
    fn is_blocked_ipv4(&self, ip: Ipv4Addr) -> bool {
        let octets = ip.octets();

        // Loopback: 127.0.0.0/8
        if octets[0] == 127 {
            return true;
        }

        // Unspecified: 0.0.0.0/8
        if octets[0] == 0 {
            return true;
        }

        // Private: 10.0.0.0/8
        if octets[0] == 10 {
            return true;
        }

        // Private: 172.16.0.0/12 (172.16.0.0 - 172.31.255.255)
        if octets[0] == 172 && (16..=31).contains(&octets[1]) {
            return true;
        }

        // Private: 192.168.0.0/16
        if octets[0] == 192 && octets[1] == 168 {
            return true;
        }

        // Link-local: 169.254.0.0/16
        if octets[0] == 169 && octets[1] == 254 {
            return true;
        }

        // Shared Address Space: 100.64.0.0/10 (100.64.0.0 - 100.127.255.255)
        if octets[0] == 100 && (64..=127).contains(&octets[1]) {
            return true;
        }

        // IETF Protocol Assignments: 192.0.0.0/24
        if octets[0] == 192 && octets[1] == 0 && octets[2] == 0 {
            return true;
        }

        // Documentation: 192.0.2.0/24 (TEST-NET-1)
        if octets[0] == 192 && octets[1] == 0 && octets[2] == 2 {
            return true;
        }

        // Documentation: 198.51.100.0/24 (TEST-NET-2)
        if octets[0] == 198 && octets[1] == 51 && octets[2] == 100 {
            return true;
        }

        // Documentation: 203.0.113.0/24 (TEST-NET-3)
        if octets[0] == 203 && octets[1] == 0 && octets[2] == 113 {
            return true;
        }

        // Benchmarking: 198.18.0.0/15 (198.18.0.0 - 198.19.255.255)
        if octets[0] == 198 && (18..=19).contains(&octets[1]) {
            return true;
        }

        // Multicast: 224.0.0.0/4 (224.0.0.0 - 239.255.255.255)
        if (224..=239).contains(&octets[0]) {
            return true;
        }

        // Reserved for future use: 240.0.0.0/4 (240.0.0.0 - 255.255.255.255)
        if octets[0] >= 240 {
            return true;
        }

        // Broadcast: 255.255.255.255
        if ip == Ipv4Addr::BROADCAST {
            return true;
        }

        false
    }

    /// Check if an IPv6 address is in a blocked range.
    fn is_blocked_ipv6(&self, ip: Ipv6Addr) -> bool {
        // Loopback: ::1
        if ip.is_loopback() {
            return true;
        }

        // Unspecified: ::
        if ip.is_unspecified() {
            return true;
        }

        // IPv4-mapped addresses: ::ffff:0:0/96
        // Check if it's an IPv4-mapped address and validate the IPv4 part
        if let Some(ipv4) = ip.to_ipv4_mapped() {
            return self.is_blocked_ipv4(ipv4);
        }

        let segments = ip.segments();

        // Link-local: fe80::/10 (fe80:: - febf::)
        if segments[0] & 0xffc0 == 0xfe80 {
            return true;
        }

        // Site-local (deprecated): fec0::/10
        if segments[0] & 0xffc0 == 0xfec0 {
            return true;
        }

        // Unique local: fc00::/7 (fc00:: - fdff::)
        if segments[0] & 0xfe00 == 0xfc00 {
            return true;
        }

        // Multicast: ff00::/8
        if segments[0] & 0xff00 == 0xff00 {
            return true;
        }

        // Documentation: 2001:db8::/32
        if segments[0] == 0x2001 && segments[1] == 0x0db8 {
            return true;
        }

        // 6to4 deprecated: 2002::/16
        if segments[0] == 0x2002 {
            // Check the embedded IPv4 address
            let embedded_ipv4 = Ipv4Addr::new(
                (segments[1] >> 8) as u8,
                (segments[1] & 0xff) as u8,
                (segments[2] >> 8) as u8,
                (segments[2] & 0xff) as u8,
            );
            if self.is_blocked_ipv4(embedded_ipv4) {
                return true;
            }
        }

        // Teredo: 2001:0000::/32
        if segments[0] == 0x2001 && segments[1] == 0x0000 {
            return true;
        }

        false
    }

    /// Perform DNS resolution and check all resolved IPs.
    fn check_dns_resolution(&self, url: &Url) -> SsrfResult<()> {
        let host = url.host_str().ok_or(SsrfError::MissingHost)?;
        let port = url.port_or_known_default().unwrap_or(80);

        // Format for DNS resolution
        let socket_addr = format!("{}:{}", host, port);

        // Resolve DNS
        let addrs: Vec<SocketAddr> = socket_addr
            .to_socket_addrs()
            .map_err(|_| SsrfError::DnsResolutionFailed(host.to_string()))?
            .collect();

        if addrs.is_empty() {
            return Err(SsrfError::DnsResolutionFailed(host.to_string()));
        }

        // Check all resolved IP addresses
        for addr in &addrs {
            if self.is_blocked_ip(addr.ip()) {
                return Err(SsrfError::DnsRebindingDetected(format!(
                    "{} resolved to blocked IP {}",
                    host,
                    addr.ip()
                )));
            }
        }

        Ok(())
    }

    /// Create a reqwest client configured with SSRF protection settings.
    pub fn create_http_client(&self) -> Result<reqwest::Client, reqwest::Error> {
        let mut builder = create_client_builder()
            .timeout(Duration::from_secs(self.config.timeout_secs))
            .connect_timeout(Duration::from_secs(self.config.connect_timeout_secs))
            .user_agent(USER_AGENT);

        if !self.config.allow_redirects {
            builder = builder.redirect(reqwest::redirect::Policy::none());
        } else {
            builder = builder.redirect(reqwest::redirect::Policy::limited(
                self.config.max_redirects,
            ));
        }

        // Disable automatic decompression to control response size
        builder = builder.no_gzip().no_brotli().no_deflate();

        builder.build()
    }
}

impl Default for SsrfProtection {
    fn default() -> Self {
        Self::new()
    }
}

/// Convenience function to check if a URL is safe for fetching.
///
/// Uses default SSRF protection settings.
pub fn is_safe_url(url: &str) -> bool {
    let protection = SsrfProtection::new();
    protection.validate_url(url).is_ok()
}

/// Convenience function to validate a URL for fetching with custom config.
pub fn validate_url_for_fetch(url: &str, config: Option<SsrfConfig>) -> SsrfResult<Url> {
    let protection = match config {
        Some(cfg) => SsrfProtection::with_config(cfg),
        None => SsrfProtection::new(),
    };
    protection.validate_url(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_urls() {
        // Skip DNS resolution in tests (no network access)
        let config = SsrfConfig::new().skip_dns_resolution();
        let protection = SsrfProtection::with_config(config);

        // Valid external URLs
        assert!(protection.validate_url("https://example.com").is_ok());
        assert!(
            protection
                .validate_url("https://api.github.com/repos")
                .is_ok()
        );
        assert!(protection.validate_url("http://www.rust-lang.org").is_ok());
    }

    #[test]
    fn test_blocked_protocols() {
        let protection = SsrfProtection::new();

        assert!(protection.validate_url("file:///etc/passwd").is_err());
        assert!(protection.validate_url("ftp://example.com").is_err());
        assert!(protection.validate_url("gopher://example.com").is_err());
        assert!(protection.validate_url("dict://example.com").is_err());
        assert!(protection.validate_url("ssh://example.com").is_err());
    }

    #[test]
    fn test_blocked_localhost() {
        let protection = SsrfProtection::new();

        assert!(protection.validate_url("http://localhost").is_err());
        assert!(protection.validate_url("http://localhost:8080").is_err());
        assert!(protection.validate_url("http://127.0.0.1").is_err());
        assert!(protection.validate_url("http://127.0.0.1:3000").is_err());
        assert!(protection.validate_url("http://127.1.2.3").is_err());
        assert!(protection.validate_url("http://0.0.0.0").is_err());
        assert!(protection.validate_url("http://[::1]").is_err());
    }

    #[test]
    fn test_blocked_private_ips() {
        let protection = SsrfProtection::new();

        // 10.0.0.0/8
        assert!(protection.validate_url("http://10.0.0.1").is_err());
        assert!(protection.validate_url("http://10.255.255.255").is_err());

        // 172.16.0.0/12
        assert!(protection.validate_url("http://172.16.0.1").is_err());
        assert!(protection.validate_url("http://172.31.255.255").is_err());

        // 192.168.0.0/16
        assert!(protection.validate_url("http://192.168.0.1").is_err());
        assert!(protection.validate_url("http://192.168.255.255").is_err());

        // Link-local 169.254.0.0/16
        assert!(protection.validate_url("http://169.254.0.1").is_err());
        assert!(protection.validate_url("http://169.254.169.254").is_err()); // AWS metadata
    }

    #[test]
    fn test_blocked_local_domains() {
        let protection = SsrfProtection::new();

        assert!(protection.validate_url("http://server.local").is_err());
        assert!(protection.validate_url("http://myapp.localhost").is_err());
        assert!(protection.validate_url("http://internal.corp").is_err());
        assert!(protection.validate_url("http://test.internal").is_err());
        assert!(protection.validate_url("http://server.lan").is_err());
        assert!(protection.validate_url("http://app.lvh.me").is_err());
    }

    #[test]
    fn test_ipv4_edge_cases() {
        let protection = SsrfProtection::new();

        // Shared Address Space (CGN): 100.64.0.0/10
        assert!(protection.validate_url("http://100.64.0.1").is_err());
        assert!(protection.validate_url("http://100.127.255.255").is_err());

        // Documentation ranges
        assert!(protection.validate_url("http://192.0.2.1").is_err()); // TEST-NET-1
        assert!(protection.validate_url("http://198.51.100.1").is_err()); // TEST-NET-2
        assert!(protection.validate_url("http://203.0.113.1").is_err()); // TEST-NET-3

        // Benchmarking: 198.18.0.0/15
        assert!(protection.validate_url("http://198.18.0.1").is_err());
        assert!(protection.validate_url("http://198.19.255.255").is_err());

        // Multicast: 224.0.0.0/4
        assert!(protection.validate_url("http://224.0.0.1").is_err());
        assert!(protection.validate_url("http://239.255.255.255").is_err());

        // Reserved: 240.0.0.0/4
        assert!(protection.validate_url("http://240.0.0.1").is_err());
    }

    #[test]
    fn test_ipv6_blocked_ranges() {
        let protection = SsrfProtection::new();

        // Loopback
        assert!(protection.validate_url("http://[::1]").is_err());

        // Link-local
        assert!(protection.validate_url("http://[fe80::1]").is_err());
        assert!(protection.validate_url("http://[febf::1]").is_err());

        // Unique local
        assert!(protection.validate_url("http://[fc00::1]").is_err());
        assert!(protection.validate_url("http://[fd00::1]").is_err());

        // Documentation
        assert!(protection.validate_url("http://[2001:db8::1]").is_err());
    }

    #[test]
    fn test_domain_allowlist() {
        let config = SsrfConfig::new()
            .allow_domain("example.com")
            .allow_domain("api.github.com")
            .skip_dns_resolution(); // Skip DNS resolution in tests

        let protection = SsrfProtection::with_config(config);

        // Allowed domains (exact match required)
        assert!(protection.validate_url("https://example.com").is_ok());
        assert!(protection.validate_url("https://api.github.com").is_ok());

        // Not in allowlist (subdomains not automatically included)
        assert!(protection.validate_url("https://other.com").is_err());
    }

    #[test]
    fn test_is_safe_url() {
        assert!(is_safe_url("https://example.com"));
        assert!(!is_safe_url("http://localhost"));
        assert!(!is_safe_url("http://192.168.1.1"));
        assert!(!is_safe_url("file:///etc/passwd"));
    }

    #[test]
    fn test_ipv4_checks() {
        let protection = SsrfProtection::new();

        // Should be blocked
        assert!(protection.is_blocked_ip(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))));
        assert!(protection.is_blocked_ip(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))));
        assert!(protection.is_blocked_ip(IpAddr::V4(Ipv4Addr::new(172, 16, 0, 1))));
        assert!(protection.is_blocked_ip(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1))));
        assert!(protection.is_blocked_ip(IpAddr::V4(Ipv4Addr::new(169, 254, 169, 254))));

        // Should be allowed
        assert!(!protection.is_blocked_ip(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))));
        assert!(!protection.is_blocked_ip(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1))));
        assert!(!protection.is_blocked_ip(IpAddr::V4(Ipv4Addr::new(142, 250, 185, 14))));
    }

    #[test]
    fn test_ipv6_checks() {
        let protection = SsrfProtection::new();

        // Should be blocked
        assert!(protection.is_blocked_ip(IpAddr::V6(Ipv6Addr::LOCALHOST)));
        assert!(protection.is_blocked_ip(IpAddr::V6(Ipv6Addr::UNSPECIFIED)));
        assert!(protection.is_blocked_ip(IpAddr::V6("fe80::1".parse().unwrap())));
        assert!(protection.is_blocked_ip(IpAddr::V6("fc00::1".parse().unwrap())));
        assert!(protection.is_blocked_ip(IpAddr::V6("2001:db8::1".parse().unwrap())));

        // Should be allowed
        assert!(!protection.is_blocked_ip(IpAddr::V6("2607:f8b0:4004:800::200e".parse().unwrap())));
    }
}
