//! Web operations (fetch URL, web search).

use serde_json::Value;
use tokio::process::Command;

use super::types::ToolResult;

/// Fetch content from a URL.
pub async fn fetch_url(args: Value) -> ToolResult {
    let url = match args.get("url").and_then(|v| v.as_str()) {
        Some(u) => u,
        None => return ToolResult::error("url is required"),
    };

    // Comprehensive SSRF protection using cortex-core security module
    use cortex_engine::security::ssrf::{DEFAULT_MAX_RESPONSE_SIZE, SsrfProtection};

    let ssrf_protection = SsrfProtection::new();

    // Validate URL with comprehensive SSRF checks:
    // 1. Protocol validation (http/https only)
    // 2. Localhost/local domain blocking
    // 3. Private/reserved IP range blocking (10.x, 172.16-31.x, 192.168.x, 169.254.x, etc.)
    // 4. DNS resolution to prevent DNS rebinding attacks
    // 5. IPv6 loopback and link-local blocking
    let validated_url = match ssrf_protection.validate_url(url) {
        Ok(u) => u,
        Err(e) => return ToolResult::error(format!("URL validation failed: {e}")),
    };

    // Create HTTP client with security settings
    let client = match ssrf_protection.create_http_client() {
        Ok(c) => c,
        Err(e) => return ToolResult::error(format!("Failed to create HTTP client: {e}")),
    };

    // Fetch with timeout and size limits
    let response = match client.get(validated_url.as_str()).send().await {
        Ok(r) => r,
        Err(e) => return ToolResult::error(format!("Request failed: {e}")),
    };

    if !response.status().is_success() {
        return ToolResult::error(format!(
            "HTTP error: {} {}",
            response.status().as_u16(),
            response.status().canonical_reason().unwrap_or("Unknown")
        ));
    }

    // Check Content-Length before reading
    if let Some(content_length) = response.content_length()
        && content_length as usize > DEFAULT_MAX_RESPONSE_SIZE
    {
        return ToolResult::error(format!(
            "Response too large: {} bytes exceeds limit of {} bytes",
            content_length, DEFAULT_MAX_RESPONSE_SIZE
        ));
    }

    // Read response with size limit
    let bytes = match response.bytes().await {
        Ok(b) => b,
        Err(e) => return ToolResult::error(format!("Failed to read response: {e}")),
    };

    if bytes.len() > DEFAULT_MAX_RESPONSE_SIZE {
        return ToolResult::error(format!(
            "Response too large: {} bytes exceeds limit of {} bytes",
            bytes.len(),
            DEFAULT_MAX_RESPONSE_SIZE
        ));
    }

    let content = match String::from_utf8(bytes.to_vec()) {
        Ok(s) => s,
        Err(e) => return ToolResult::error(format!("Response is not valid UTF-8: {e}")),
    };

    // Truncate for display if too long
    let truncated = if content.len() > 100_000 {
        format!(
            "{}...\n[Truncated at 100000 chars, full size: {} chars]",
            &content[..100_000],
            content.len()
        )
    } else {
        content
    };

    ToolResult::success(truncated)
}

/// Search the web.
pub async fn web_search(args: Value) -> ToolResult {
    let query = match args.get("query").and_then(|v| v.as_str()) {
        Some(q) => q,
        None => return ToolResult::error("query is required"),
    };

    // Simple URL encoding
    let encoded: String = query
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c.to_string()
            } else if c == ' ' {
                "+".to_string()
            } else {
                format!("%{:02X}", c as u8)
            }
        })
        .collect();

    let url = format!("https://duckduckgo.com/html/?q={encoded}");

    let output = Command::new("curl")
        .args(["-s", "-L", "--max-time", "30", &url])
        .output()
        .await;

    match output {
        Ok(output) => {
            let html = String::from_utf8_lossy(&output.stdout);
            // Simple extraction of text
            let truncated = if html.len() > 10_000 {
                &html[..10_000]
            } else {
                &html
            };
            ToolResult::success(format!("Search results for: {query}\n{truncated}"))
        }
        Err(e) => ToolResult::error(format!("Web search failed: {e}")),
    }
}
