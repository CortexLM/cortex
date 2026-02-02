//! Domain pattern matching.

use super::{NetworkProxyError, host};

/// Domain pattern for access control.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DomainPattern {
    /// Match any domain ("*").
    Any,

    /// Match apex domain and all subdomains ("**.example.com").
    ApexAndSubdomains(String),

    /// Match only subdomains, not apex ("*.example.com").
    SubdomainsOnly(String),

    /// Match exact domain ("example.com").
    Exact(String),
}

impl DomainPattern {
    /// Parse a pattern string.
    pub fn parse(pattern: &str) -> Result<Self, NetworkProxyError> {
        let pattern = pattern.trim().to_lowercase();

        if pattern.is_empty() {
            return Err(NetworkProxyError::InvalidPattern(
                "empty pattern".to_string(),
            ));
        }

        if pattern == "*" {
            return Ok(DomainPattern::Any);
        }

        if let Some(domain) = pattern.strip_prefix("**.") {
            if domain.is_empty() {
                return Err(NetworkProxyError::InvalidPattern(
                    "invalid apex pattern: empty domain".to_string(),
                ));
            }
            return Ok(DomainPattern::ApexAndSubdomains(domain.to_string()));
        }

        if let Some(domain) = pattern.strip_prefix("*.") {
            if domain.is_empty() {
                return Err(NetworkProxyError::InvalidPattern(
                    "invalid subdomain pattern: empty domain".to_string(),
                ));
            }
            return Ok(DomainPattern::SubdomainsOnly(domain.to_string()));
        }

        Ok(DomainPattern::Exact(pattern))
    }

    /// Check if this pattern matches a candidate host.
    pub fn matches(&self, candidate: &str) -> bool {
        let candidate = candidate.to_lowercase();

        match self {
            DomainPattern::Any => true,
            DomainPattern::Exact(domain) => host::domain_eq(&candidate, domain),
            DomainPattern::SubdomainsOnly(domain) => host::is_strict_subdomain(&candidate, domain),
            DomainPattern::ApexAndSubdomains(domain) => {
                host::is_subdomain_or_equal(&candidate, domain)
            }
        }
    }

    /// Check if this pattern allows another pattern.
    /// Used for policy composition.
    pub fn allows(&self, candidate: &DomainPattern) -> bool {
        match self {
            DomainPattern::Any => true,
            DomainPattern::Exact(domain) => match candidate {
                DomainPattern::Exact(cand) => host::domain_eq(cand, domain),
                _ => false,
            },
            DomainPattern::SubdomainsOnly(domain) => match candidate {
                DomainPattern::Any => false,
                DomainPattern::Exact(cand) => host::is_strict_subdomain(cand, domain),
                DomainPattern::SubdomainsOnly(cand) => host::is_subdomain_or_equal(cand, domain),
                DomainPattern::ApexAndSubdomains(cand) => host::is_strict_subdomain(cand, domain),
            },
            DomainPattern::ApexAndSubdomains(domain) => match candidate {
                DomainPattern::Any => false,
                DomainPattern::Exact(cand) => host::is_subdomain_or_equal(cand, domain),
                DomainPattern::SubdomainsOnly(cand) => host::is_subdomain_or_equal(cand, domain),
                DomainPattern::ApexAndSubdomains(cand) => host::is_subdomain_or_equal(cand, domain),
            },
        }
    }
}

impl std::fmt::Display for DomainPattern {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DomainPattern::Any => write!(f, "*"),
            DomainPattern::ApexAndSubdomains(d) => write!(f, "**.{}", d),
            DomainPattern::SubdomainsOnly(d) => write!(f, "*.{}", d),
            DomainPattern::Exact(d) => write!(f, "{}", d),
        }
    }
}

impl std::str::FromStr for DomainPattern {
    type Err = NetworkProxyError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

/// A set of compiled domain patterns for efficient matching.
#[derive(Debug, Clone, Default)]
pub struct CompiledPatterns {
    /// Has a wildcard "*" pattern.
    has_any: bool,

    /// Exact match patterns.
    exact: Vec<String>,

    /// Subdomain-only patterns.
    subdomains_only: Vec<String>,

    /// Apex and subdomain patterns.
    apex_and_subdomains: Vec<String>,
}

impl CompiledPatterns {
    /// Create an empty set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a pattern.
    pub fn add(&mut self, pattern: DomainPattern) {
        match pattern {
            DomainPattern::Any => self.has_any = true,
            DomainPattern::Exact(d) => self.exact.push(d),
            DomainPattern::SubdomainsOnly(d) => self.subdomains_only.push(d),
            DomainPattern::ApexAndSubdomains(d) => self.apex_and_subdomains.push(d),
        }
    }

    /// Check if a host matches any pattern.
    pub fn matches(&self, host: &str) -> bool {
        if self.has_any {
            return true;
        }

        let host_lower = host.to_lowercase();

        // Check exact matches
        for domain in &self.exact {
            if host::domain_eq(&host_lower, domain) {
                return true;
            }
        }

        // Check subdomain-only patterns
        for domain in &self.subdomains_only {
            if host::is_strict_subdomain(&host_lower, domain) {
                return true;
            }
        }

        // Check apex and subdomain patterns
        for domain in &self.apex_and_subdomains {
            if host::is_subdomain_or_equal(&host_lower, domain) {
                return true;
            }
        }

        false
    }

    /// Check if the set is empty.
    pub fn is_empty(&self) -> bool {
        !self.has_any
            && self.exact.is_empty()
            && self.subdomains_only.is_empty()
            && self.apex_and_subdomains.is_empty()
    }
}

/// Compile a list of pattern strings into a CompiledPatterns set.
pub fn compile_patterns(patterns: &[String]) -> Result<CompiledPatterns, NetworkProxyError> {
    let mut compiled = CompiledPatterns::new();

    for pattern_str in patterns {
        let pattern = DomainPattern::parse(pattern_str)?;
        compiled.add(pattern);
    }

    Ok(compiled)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pattern_parse() {
        assert!(matches!(
            DomainPattern::parse("*").unwrap(),
            DomainPattern::Any
        ));

        assert!(matches!(
            DomainPattern::parse("**.github.com").unwrap(),
            DomainPattern::ApexAndSubdomains(_)
        ));

        assert!(matches!(
            DomainPattern::parse("*.github.com").unwrap(),
            DomainPattern::SubdomainsOnly(_)
        ));

        assert!(matches!(
            DomainPattern::parse("github.com").unwrap(),
            DomainPattern::Exact(_)
        ));
    }

    #[test]
    fn test_pattern_matches() {
        let any = DomainPattern::Any;
        assert!(any.matches("anything.com"));

        let exact = DomainPattern::Exact("github.com".to_string());
        assert!(exact.matches("github.com"));
        assert!(exact.matches("GITHUB.COM"));
        assert!(!exact.matches("api.github.com"));

        let subdomains = DomainPattern::SubdomainsOnly("github.com".to_string());
        assert!(!subdomains.matches("github.com"));
        assert!(subdomains.matches("api.github.com"));
        assert!(subdomains.matches("raw.githubusercontent.github.com"));

        let apex = DomainPattern::ApexAndSubdomains("github.com".to_string());
        assert!(apex.matches("github.com"));
        assert!(apex.matches("api.github.com"));
    }

    #[test]
    fn test_compiled_patterns() {
        let patterns = vec!["**.github.com".to_string(), "api.openai.com".to_string()];

        let compiled = compile_patterns(&patterns).unwrap();

        assert!(compiled.matches("github.com"));
        assert!(compiled.matches("api.github.com"));
        assert!(compiled.matches("api.openai.com"));
        assert!(!compiled.matches("openai.com"));
        assert!(!compiled.matches("evil.example"));
    }
}
