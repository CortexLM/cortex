//! Host normalization and validation.

use super::NetworkProxyError;

/// A normalized host string for policy evaluation.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Host(String);

impl Host {
    /// Parse and normalize a host string.
    pub fn parse(input: &str) -> Result<Self, NetworkProxyError> {
        let normalized = normalize_host(input);
        if normalized.is_empty() {
            return Err(NetworkProxyError::InvalidHost("host is empty".to_string()));
        }
        Ok(Self(normalized))
    }

    /// Get the normalized host string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Check if this host is a loopback address.
    pub fn is_loopback(&self) -> bool {
        super::ip_validation::is_loopback_host(self)
    }
}

impl std::fmt::Display for Host {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl AsRef<str> for Host {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl std::str::FromStr for Host {
    type Err = NetworkProxyError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

/// Normalize a host string.
/// - Lowercase
/// - Strip brackets from IPv6
/// - Remove trailing dots
/// - Remove port if present
fn normalize_host(input: &str) -> String {
    let mut host = input.trim().to_lowercase();

    // Strip IPv6 brackets
    if host.starts_with('[')
        && let Some(end) = host.find(']')
    {
        host = host[1..end].to_string();
    }

    // Remove port if present (for non-IPv6)
    if (!host.contains(':') || host.matches(':').count() == 1)
        && let Some(colon) = host.rfind(':')
    {
        // Check if what follows is a port number
        let potential_port = &host[colon + 1..];
        if potential_port.chars().all(|c| c.is_ascii_digit()) {
            host = host[..colon].to_string();
        }
    }

    // Remove trailing dots (FQDN normalization)
    while host.ends_with('.') {
        host.pop();
    }

    host
}

/// Extract the domain suffix for pattern matching.
/// Returns the domain parts from right to left.
pub fn domain_parts(host: &str) -> Vec<&str> {
    host.split('.').rev().collect()
}

/// Check if one domain equals another (case-insensitive).
pub fn domain_eq(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

/// Check if candidate is a subdomain of domain.
pub fn is_subdomain(candidate: &str, domain: &str) -> bool {
    if domain_eq(candidate, domain) {
        return true;
    }
    is_strict_subdomain(candidate, domain)
}

/// Check if candidate is a strict subdomain of domain (not equal).
pub fn is_strict_subdomain(candidate: &str, domain: &str) -> bool {
    let suffix = format!(".{}", domain.to_lowercase());
    candidate.to_lowercase().ends_with(&suffix)
}

/// Check if candidate is a subdomain or equal to domain.
pub fn is_subdomain_or_equal(candidate: &str, domain: &str) -> bool {
    domain_eq(candidate, domain) || is_strict_subdomain(candidate, domain)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_host() {
        assert_eq!(normalize_host("Example.COM"), "example.com");
        assert_eq!(normalize_host("example.com."), "example.com");
        assert_eq!(normalize_host("example.com:8080"), "example.com");
        assert_eq!(normalize_host("[::1]"), "::1");
        assert_eq!(normalize_host("  example.com  "), "example.com");
    }

    #[test]
    fn test_is_subdomain() {
        assert!(is_subdomain("api.github.com", "github.com"));
        assert!(is_subdomain("github.com", "github.com"));
        assert!(!is_subdomain("github.com", "api.github.com"));
        assert!(!is_subdomain("notgithub.com", "github.com"));
    }

    #[test]
    fn test_is_strict_subdomain() {
        assert!(is_strict_subdomain("api.github.com", "github.com"));
        assert!(!is_strict_subdomain("github.com", "github.com"));
    }
}
