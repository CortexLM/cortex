//! HTTP utilities for the scrape command.

use std::collections::HashMap;

use anyhow::{Result, bail};

use super::types::MAX_HEADER_VALUE_LENGTH;

/// Get the proxy URL from environment variables if configured.
pub fn get_proxy_from_env() -> Option<String> {
    // Check common proxy environment variables in order of preference
    std::env::var("HTTPS_PROXY")
        .or_else(|_| std::env::var("https_proxy"))
        .or_else(|_| std::env::var("HTTP_PROXY"))
        .or_else(|_| std::env::var("http_proxy"))
        .ok()
}

/// Add proxy context to an error message if a proxy is configured.
#[allow(dead_code)]
pub fn add_proxy_context(err: anyhow::Error) -> anyhow::Error {
    if let Some(proxy_url) = get_proxy_from_env() {
        anyhow::anyhow!("{} (via proxy {})", err, proxy_url)
    } else {
        err
    }
}

/// Format HTTP error with additional context for rate limiting (429) and server errors (5xx).
pub fn format_http_error(response: &reqwest::Response) -> String {
    let status = response.status();
    let code = status.as_u16();
    let reason = status.canonical_reason().unwrap_or("Unknown");
    let proxy_info = get_proxy_from_env()
        .map(|p| format!(" (via proxy {})", p))
        .unwrap_or_default();

    // Handle 429 Too Many Requests - show Retry-After if available
    if code == 429 {
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .map(|v| {
                // Retry-After can be a number of seconds or an HTTP-date
                if let Ok(seconds) = v.parse::<u64>() {
                    format!("Retry after {} seconds.", seconds)
                } else {
                    format!("Retry after: {}", v)
                }
            })
            .unwrap_or_else(|| "No retry information provided.".to_string());

        let rate_limit_info = response
            .headers()
            .get("x-ratelimit-remaining")
            .and_then(|v| v.to_str().ok())
            .map(|v| format!(" Rate limit remaining: {}.", v))
            .unwrap_or_default();

        return format!(
            "HTTP error: {} {} - Rate limited. {}{}{}",
            code, reason, retry_after, rate_limit_info, proxy_info
        );
    }

    // Handle 5xx server errors with suggestion
    if (500..600).contains(&code) {
        return format!(
            "HTTP error: {} {} - Server error.{} Consider retrying later.",
            code, reason, proxy_info
        );
    }

    format!("HTTP error: {} {}{}", code, reason, proxy_info)
}

/// Parse custom headers from command line arguments.
/// Validates header length and warns about duplicate headers.
pub fn parse_headers(headers: &[String]) -> Result<HashMap<String, String>> {
    let mut result = HashMap::new();
    let mut duplicates = Vec::new();

    for header in headers {
        let parts: Vec<&str> = header.splitn(2, ':').collect();
        if parts.len() != 2 {
            bail!("Invalid header format: {header}. Use 'Header-Name: value'");
        }
        let name = parts[0].trim().to_string();
        let value = parts[1].trim().to_string();

        // Validate that both name and value are non-empty
        if name.is_empty() || value.is_empty() {
            bail!("Invalid header format - name and value required");
        }

        // Validate header value length (Issue #1987)
        if value.len() > MAX_HEADER_VALUE_LENGTH {
            bail!(
                "Header '{}' value exceeds maximum allowed length of {} bytes. \
                HTTP servers typically reject headers larger than 8KB.",
                name,
                MAX_HEADER_VALUE_LENGTH
            );
        }

        // Check for duplicate headers (Issue #1988)
        if result.contains_key(&name) {
            duplicates.push(name.clone());
        }

        result.insert(name, value);
    }

    // Warn about duplicate headers
    if !duplicates.is_empty() {
        eprintln!(
            "Warning: Duplicate header(s) detected: {}. Using last value for each.",
            duplicates.join(", ")
        );
    }

    Ok(result)
}
