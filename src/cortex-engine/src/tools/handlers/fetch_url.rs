//! Fetch URL tool handler.
//!
//! This handler provides secure URL fetching with comprehensive SSRF protection.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashSet;

use super::{ToolContext, ToolHandler, ToolResult};
use crate::error::Result;
use crate::security::ssrf::{DEFAULT_MAX_RESPONSE_SIZE, SsrfConfig, SsrfProtection};

/// Maximum content size for response (10 MB)
const MAX_CONTENT_SIZE: usize = DEFAULT_MAX_RESPONSE_SIZE;

/// Content truncation threshold for display (100 KB)
const CONTENT_TRUNCATE_THRESHOLD: usize = 100_000;

/// Handler for fetch_url tool with comprehensive SSRF protection.
pub struct FetchUrlHandler {
    ssrf_protection: SsrfProtection,
    client: reqwest::Client,
}

#[derive(Debug, Deserialize)]
struct FetchUrlArgs {
    url: String,
    /// Optional list of allowed domains (overrides default behavior)
    #[serde(default)]
    allowed_domains: Option<Vec<String>>,
}

impl FetchUrlHandler {
    /// Create a new FetchUrlHandler with default SSRF protection.
    pub fn new() -> Self {
        let ssrf_protection = SsrfProtection::new();
        let client = ssrf_protection.create_http_client().expect("HTTP client");

        Self {
            ssrf_protection,
            client,
        }
    }

    /// Create a new FetchUrlHandler with custom allowed domains.
    pub fn with_allowed_domains(domains: impl IntoIterator<Item = impl Into<String>>) -> Self {
        let config = SsrfConfig::new().allow_domains(domains);
        let ssrf_protection = SsrfProtection::with_config(config);
        let client = ssrf_protection.create_http_client().expect("HTTP client");

        Self {
            ssrf_protection,
            client,
        }
    }

    /// Create a new FetchUrlHandler with custom SSRF config.
    pub fn with_config(config: SsrfConfig) -> Self {
        let ssrf_protection = SsrfProtection::with_config(config);
        let client = ssrf_protection.create_http_client().expect("HTTP client");

        Self {
            ssrf_protection,
            client,
        }
    }

    /// Validate URL with optional per-request domain allowlist.
    fn validate_url_with_allowlist(
        &self,
        url: &str,
        allowed_domains: Option<&[String]>,
    ) -> std::result::Result<url::Url, String> {
        // If per-request allowlist is provided, create a temporary protection instance
        if let Some(domains) = allowed_domains {
            let mut allowed_set = HashSet::new();
            for domain in domains {
                allowed_set.insert(domain.to_lowercase());
            }

            let config = SsrfConfig {
                allowed_domains: allowed_set,
                ..SsrfConfig::default()
            };

            let temp_protection = SsrfProtection::with_config(config);
            temp_protection.validate_url(url).map_err(|e| e.to_string())
        } else {
            self.ssrf_protection
                .validate_url(url)
                .map_err(|e| e.to_string())
        }
    }

    /// Fetch content with size limits.
    async fn fetch_with_size_limit(&self, url: &str) -> std::result::Result<String, String> {
        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| format!("Request failed: {e}"))?;

        if !response.status().is_success() {
            return Err(format!(
                "HTTP error: {} {}",
                response.status().as_u16(),
                response.status().canonical_reason().unwrap_or("Unknown")
            ));
        }

        // Check Content-Length header if available
        if let Some(content_length) = response.content_length() {
            if content_length as usize > MAX_CONTENT_SIZE {
                return Err(format!(
                    "Response too large: {} bytes exceeds limit of {} bytes",
                    content_length, MAX_CONTENT_SIZE
                ));
            }
        }

        // Read response with size limit
        let bytes = response
            .bytes()
            .await
            .map_err(|e| format!("Failed to read response: {e}"))?;

        if bytes.len() > MAX_CONTENT_SIZE {
            return Err(format!(
                "Response too large: {} bytes exceeds limit of {} bytes",
                bytes.len(),
                MAX_CONTENT_SIZE
            ));
        }

        String::from_utf8(bytes.to_vec()).map_err(|e| format!("Response is not valid UTF-8: {e}"))
    }
}

impl Default for FetchUrlHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolHandler for FetchUrlHandler {
    fn name(&self) -> &str {
        "FetchUrl"
    }

    async fn execute(&self, arguments: Value, _context: &ToolContext) -> Result<ToolResult> {
        let args: FetchUrlArgs = serde_json::from_value(arguments)?;

        // Validate URL with comprehensive SSRF protection
        // This performs:
        // 1. Protocol validation (http/https only)
        // 2. Localhost/local domain blocking
        // 3. Private/reserved IP range blocking
        // 4. DNS resolution to prevent DNS rebinding attacks
        // 5. Domain allowlist check (if configured)
        let validated_url =
            match self.validate_url_with_allowlist(&args.url, args.allowed_domains.as_deref()) {
                Ok(url) => url,
                Err(e) => {
                    return Ok(ToolResult::error(format!("URL validation failed: {e}")));
                }
            };

        // Fetch content with size limits
        match self.fetch_with_size_limit(validated_url.as_str()).await {
            Ok(text) => {
                // Truncate if too long for display
                let content = if text.len() > CONTENT_TRUNCATE_THRESHOLD {
                    format!(
                        "{}...\n[Content truncated at {} chars, full size: {} chars]",
                        &text[..CONTENT_TRUNCATE_THRESHOLD],
                        CONTENT_TRUNCATE_THRESHOLD,
                        text.len()
                    )
                } else {
                    text
                };
                Ok(ToolResult::success(content))
            }
            Err(e) => Ok(ToolResult::error(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_handler_creation() {
        let handler = FetchUrlHandler::new();
        assert_eq!(handler.name(), "FetchUrl");
    }

    #[test]
    fn test_blocked_urls() {
        let handler = FetchUrlHandler::new();

        // Localhost
        assert!(
            handler
                .validate_url_with_allowlist("http://localhost", None)
                .is_err()
        );
        assert!(
            handler
                .validate_url_with_allowlist("http://127.0.0.1", None)
                .is_err()
        );
        assert!(
            handler
                .validate_url_with_allowlist("http://[::1]", None)
                .is_err()
        );

        // Private IPs
        assert!(
            handler
                .validate_url_with_allowlist("http://10.0.0.1", None)
                .is_err()
        );
        assert!(
            handler
                .validate_url_with_allowlist("http://172.16.0.1", None)
                .is_err()
        );
        assert!(
            handler
                .validate_url_with_allowlist("http://192.168.1.1", None)
                .is_err()
        );

        // Link-local
        assert!(
            handler
                .validate_url_with_allowlist("http://169.254.169.254", None)
                .is_err()
        );

        // Local domains
        assert!(
            handler
                .validate_url_with_allowlist("http://server.local", None)
                .is_err()
        );
        assert!(
            handler
                .validate_url_with_allowlist("http://app.internal", None)
                .is_err()
        );

        // Blocked protocols
        assert!(
            handler
                .validate_url_with_allowlist("file:///etc/passwd", None)
                .is_err()
        );
        assert!(
            handler
                .validate_url_with_allowlist("ftp://example.com", None)
                .is_err()
        );
    }

    #[test]
    fn test_allowed_urls() {
        // Skip DNS resolution in tests (no network access)
        let config = SsrfConfig::new().skip_dns_resolution();
        let handler = FetchUrlHandler::with_config(config);

        assert!(
            handler
                .validate_url_with_allowlist("https://example.com", None)
                .is_ok()
        );
        assert!(
            handler
                .validate_url_with_allowlist("https://api.github.com", None)
                .is_ok()
        );
        assert!(
            handler
                .validate_url_with_allowlist("http://rust-lang.org", None)
                .is_ok()
        );
    }

    #[test]
    fn test_domain_allowlist() {
        // Skip DNS resolution in tests (no network access)
        let config = SsrfConfig::new()
            .allow_domain("example.com")
            .allow_domain("api.github.com")
            .skip_dns_resolution();
        let handler = FetchUrlHandler::with_config(config);

        // Allowed (exact match required)
        assert!(
            handler
                .validate_url_with_allowlist("https://example.com", None)
                .is_ok()
        );
        assert!(
            handler
                .validate_url_with_allowlist("https://api.github.com", None)
                .is_ok()
        );

        // Not in allowlist (subdomains not automatically included)
        assert!(
            handler
                .validate_url_with_allowlist("https://other.com", None)
                .is_err()
        );
    }

    #[test]
    fn test_per_request_allowlist() {
        // Skip DNS resolution in tests (no network access)
        let config = SsrfConfig::new().skip_dns_resolution();
        let handler = FetchUrlHandler::with_config(config);
        let allowed = vec!["specific.com".to_string()];

        // With per-request allowlist
        assert!(
            handler
                .validate_url_with_allowlist("https://specific.com", Some(&allowed))
                .is_ok()
        );
        assert!(
            handler
                .validate_url_with_allowlist("https://other.com", Some(&allowed))
                .is_err()
        );
    }
}
