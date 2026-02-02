//! Input validation utilities for MCP commands.
//!
//! This module provides validation functions for server names, URLs,
//! environment variables, and command arguments to ensure security and correctness.

use anyhow::{Result, bail};

// ============================================================================
// Input Validation Constants
// ============================================================================

/// Maximum length for server names (prevents DoS and storage issues)
pub(crate) const MAX_SERVER_NAME_LENGTH: usize = 64;

/// Maximum length for URLs (reasonable limit for HTTP URLs)
pub(crate) const MAX_URL_LENGTH: usize = 2048;

/// Maximum length for environment variable names
pub(crate) const MAX_ENV_VAR_NAME_LENGTH: usize = 256;

/// Maximum length for environment variable values
pub(crate) const MAX_ENV_VAR_VALUE_LENGTH: usize = 4096;

/// Maximum number of environment variables per server
pub(crate) const MAX_ENV_VARS: usize = 50;

/// Maximum number of command arguments
pub(crate) const MAX_COMMAND_ARGS: usize = 100;

/// Maximum length for a single command argument
pub(crate) const MAX_COMMAND_ARG_LENGTH: usize = 4096;

/// Allowed URL schemes for MCP HTTP transport (including WebSocket)
const ALLOWED_URL_SCHEMES: &[&str] = &["http", "https", "ws", "wss"];

/// Dangerous URL patterns that should be blocked
const BLOCKED_URL_PATTERNS: &[&str] = &[
    "javascript:",
    "data:",
    "file:",
    "ftp:",
    "localhost", // Require explicit localhost allowance
    "127.0.0.1",
    "0.0.0.0",
    "[::1]",
    "169.254.", // Link-local
    "10.",      // Private network
    "192.168.", // Private network
    "172.16.",  // Private network start
    "172.17.",
    "172.18.",
    "172.19.",
    "172.20.",
    "172.21.",
    "172.22.",
    "172.23.",
    "172.24.",
    "172.25.",
    "172.26.",
    "172.27.",
    "172.28.",
    "172.29.",
    "172.30.",
    "172.31.", // Private network end
];

// ============================================================================
// URL Validation
// ============================================================================

/// Validates and sanitizes a URL for MCP HTTP/WebSocket transport.
///
/// # Validation Rules:
/// - Must not exceed maximum length
/// - Must use allowed schemes (http/https/ws/wss)
/// - Must not contain dangerous patterns (unless allow_local is true)
/// - Must be a valid URL format
pub(crate) fn validate_url(url: &str) -> Result<()> {
    validate_url_internal(url, false)
}

/// Validates URL with option to allow local addresses.
pub(crate) fn validate_url_internal(url: &str, allow_local: bool) -> Result<()> {
    // Check length
    if url.is_empty() {
        bail!("URL cannot be empty");
    }
    if url.len() > MAX_URL_LENGTH {
        bail!(
            "URL exceeds maximum length of {} characters",
            MAX_URL_LENGTH
        );
    }

    // Check for null bytes
    if url.contains('\0') {
        bail!("URL contains null bytes");
    }

    let url_lower = url.to_lowercase();

    // Check scheme - must start with http://, https://, ws://, or wss://
    let has_valid_scheme = ALLOWED_URL_SCHEMES
        .iter()
        .any(|&scheme| url_lower.starts_with(&format!("{}://", scheme)));
    if !has_valid_scheme {
        bail!(
            "URL must start with http://, https://, ws://, or wss://. Got: {}",
            url.chars().take(20).collect::<String>()
        );
    }

    // Check for blocked patterns (skip if allow_local is true)
    if !allow_local {
        for pattern in BLOCKED_URL_PATTERNS {
            if url_lower.contains(pattern) {
                bail!(
                    "URL contains blocked pattern '{}'. For security, local/private network URLs are not allowed by default. \
                     Use --allow-local flag to enable local development servers.",
                    pattern
                );
            }
        }
    }

    // Extract host portion for validation (simple parsing)
    let after_scheme = if url_lower.starts_with("https://") {
        &url[8..]
    } else if url_lower.starts_with("http://") {
        &url[7..]
    } else if url_lower.starts_with("wss://") {
        &url[6..]
    } else if url_lower.starts_with("ws://") {
        &url[5..]
    } else {
        bail!("Invalid URL scheme");
    };

    // Host should be non-empty (check up to first / or end)
    let host_end = after_scheme.find('/').unwrap_or(after_scheme.len());
    let host_port = &after_scheme[..host_end];

    if host_port.is_empty() {
        bail!("URL must contain a valid host");
    }

    // Remove port if present to check host
    let host = host_port.split(':').next().unwrap_or(host_port);
    if host.is_empty() {
        bail!("URL must contain a valid host");
    }

    // Check for path traversal attempts in the path portion
    if let Some(path_start) = after_scheme.find('/') {
        let path = &after_scheme[path_start..];
        if path.contains("..") {
            bail!("URL path contains potentially dangerous path traversal patterns");
        }
    }

    // Check for control characters
    for c in url.chars() {
        if c.is_control() && c != '\t' {
            bail!("URL contains control characters");
        }
    }

    Ok(())
}

// ============================================================================
// Environment Variable Validation
// ============================================================================

/// Validates environment variable name.
pub(crate) fn validate_env_var_name(name: &str) -> Result<()> {
    if name.is_empty() {
        bail!("Environment variable name cannot be empty");
    }
    if name.len() > MAX_ENV_VAR_NAME_LENGTH {
        bail!(
            "Environment variable name exceeds maximum length of {} characters",
            MAX_ENV_VAR_NAME_LENGTH
        );
    }
    // Env var names should be alphanumeric with underscores
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        bail!(
            "Environment variable name '{}' contains invalid characters. Use only letters, numbers, and underscores.",
            name
        );
    }
    // Should not start with a digit
    if name
        .chars()
        .next()
        .map(|c| c.is_ascii_digit())
        .unwrap_or(false)
    {
        bail!("Environment variable name cannot start with a digit");
    }
    Ok(())
}

/// Validates environment variable value.
pub(crate) fn validate_env_var_value(value: &str) -> Result<()> {
    if value.len() > MAX_ENV_VAR_VALUE_LENGTH {
        bail!(
            "Environment variable value exceeds maximum length of {} characters",
            MAX_ENV_VAR_VALUE_LENGTH
        );
    }
    // Check for null bytes which could cause issues
    if value.contains('\0') {
        bail!("Environment variable value contains null bytes");
    }
    Ok(())
}

/// Validates bearer token environment variable name.
pub(crate) fn validate_bearer_token_env_var(var_name: &str) -> Result<()> {
    validate_env_var_name(var_name)?;
    // Additional checks for token env vars
    let upper = var_name.to_uppercase();
    if upper.contains("PASSWORD") || upper.contains("PASSWD") {
        // Warn but don't block - just log
        tracing::warn!(
            "Bearer token env var '{}' contains 'PASSWORD' - ensure this is intentional",
            var_name
        );
    }
    Ok(())
}

// ============================================================================
// Command Argument Validation
// ============================================================================

/// Check if a string looks like a URL (http:// or https://).
fn looks_like_url(s: &str) -> bool {
    let lower = s.to_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

/// Validates command arguments for stdio transport.
pub(crate) fn validate_command_args(args: &[String]) -> Result<()> {
    if args.is_empty() {
        bail!("Command cannot be empty");
    }
    if args.len() > MAX_COMMAND_ARGS {
        bail!(
            "Too many command arguments ({}). Maximum allowed is {}",
            args.len(),
            MAX_COMMAND_ARGS
        );
    }

    // Check if the first argument (command) looks like a URL - common mistake (#2044, #2046)
    if let Some(first_arg) = args.first()
        && looks_like_url(first_arg)
    {
        bail!(
            "Remote MCP URLs are not supported for stdio transport.\n\n\
                 The command '{}' looks like a URL. For remote MCP servers, use:\n\
                 \x20 cortex mcp add <name> --url {}\n\n\
                 For local stdio servers, provide a command to execute:\n\
                 \x20 cortex mcp add <name> -- npx @example/server\n\
                 \x20 cortex mcp add <name> -- python -m my_server",
            first_arg,
            first_arg
        );
    }

    for (i, arg) in args.iter().enumerate() {
        if arg.len() > MAX_COMMAND_ARG_LENGTH {
            bail!(
                "Command argument {} exceeds maximum length of {} characters",
                i + 1,
                MAX_COMMAND_ARG_LENGTH
            );
        }
        // Check for null bytes
        if arg.contains('\0') {
            bail!("Command argument {} contains null bytes", i + 1);
        }
    }
    Ok(())
}

// ============================================================================
// Server Name Validation
// ============================================================================

/// Validates a server name for safety and correctness.
pub(crate) fn validate_server_name(name: &str) -> Result<()> {
    // Check for empty name
    if name.is_empty() {
        bail!("server name cannot be empty");
    }

    // Check length limits
    if name.len() > MAX_SERVER_NAME_LENGTH {
        bail!(
            "server name '{}' exceeds maximum length of {} characters",
            name,
            MAX_SERVER_NAME_LENGTH
        );
    }

    // Check for valid characters
    let is_valid_chars = name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');

    if !is_valid_chars {
        bail!("invalid server name '{name}' (use letters, numbers, '-', '_')");
    }

    // Must start with a letter or underscore
    let first_char = name.chars().next().unwrap();
    if first_char.is_ascii_digit() || first_char == '-' {
        bail!(
            "server name must start with a letter or underscore, not '{}'",
            first_char
        );
    }

    // Check for reserved names
    let reserved_names = [".", "..", "con", "prn", "aux", "nul"];
    if reserved_names.contains(&name.to_lowercase().as_str()) {
        bail!("server name '{}' is reserved and cannot be used", name);
    }

    Ok(())
}

// ============================================================================
// Utility Functions
// ============================================================================

/// Parse an environment variable pair from KEY=VALUE format.
pub(crate) fn parse_env_pair(raw: &str) -> Result<(String, String), String> {
    let mut parts = raw.splitn(2, '=');
    let key = parts
        .next()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "environment entries must be in KEY=VALUE form".to_string())?;
    let value = parts
        .next()
        .map(str::to_string)
        .ok_or_else(|| "environment entries must be in KEY=VALUE form".to_string())?;

    // Validate that value is not empty
    if value.is_empty() {
        return Err(format!(
            "environment variable '{}' has empty value. Use KEY=VALUE format with a non-empty value.",
            key
        ));
    }

    Ok((key.to_string(), value))
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------------------------------
    // validate_url and validate_url_internal tests
    // ------------------------------------------------------------------------

    #[test]
    fn test_validate_url_valid_https() {
        assert!(validate_url("https://api.example.com/v1/mcp").is_ok());
    }

    #[test]
    fn test_validate_url_valid_http() {
        assert!(validate_url("http://api.example.com/v1/mcp").is_ok());
    }

    #[test]
    fn test_validate_url_valid_ws() {
        assert!(validate_url("ws://api.example.com/ws").is_ok());
    }

    #[test]
    fn test_validate_url_valid_wss() {
        assert!(validate_url("wss://api.example.com/ws").is_ok());
    }

    #[test]
    fn test_validate_url_valid_with_port() {
        assert!(validate_url("https://api.example.com:8080/v1").is_ok());
    }

    #[test]
    fn test_validate_url_valid_with_query() {
        assert!(validate_url("https://api.example.com/v1?key=value&foo=bar").is_ok());
    }

    #[test]
    fn test_validate_url_empty() {
        let result = validate_url("");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("cannot be empty"));
    }

    #[test]
    fn test_validate_url_exceeds_max_length() {
        let long_url = format!("https://example.com/{}", "a".repeat(MAX_URL_LENGTH));
        let result = validate_url(&long_url);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("exceeds maximum length")
        );
    }

    #[test]
    fn test_validate_url_null_bytes() {
        let result = validate_url("https://example.com/\0path");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("null bytes"));
    }

    #[test]
    fn test_validate_url_invalid_scheme_ftp() {
        let result = validate_url("ftp://example.com/file");
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("must start with http://")
        );
    }

    #[test]
    fn test_validate_url_invalid_scheme_javascript() {
        let result = validate_url("javascript:alert('xss')");
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_url_invalid_scheme_data() {
        let result = validate_url("data:text/html,<script>alert(1)</script>");
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_url_invalid_scheme_file() {
        let result = validate_url("file:///etc/passwd");
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_url_no_scheme() {
        let result = validate_url("example.com/path");
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("must start with http://")
        );
    }

    #[test]
    fn test_validate_url_blocked_localhost() {
        let result = validate_url("http://localhost:3000/api");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("blocked pattern"));
    }

    #[test]
    fn test_validate_url_blocked_127_0_0_1() {
        let result = validate_url("http://127.0.0.1:8080/api");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("blocked pattern"));
    }

    #[test]
    fn test_validate_url_blocked_0_0_0_0() {
        let result = validate_url("http://0.0.0.0:8080/api");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("blocked pattern"));
    }

    #[test]
    fn test_validate_url_blocked_ipv6_localhost() {
        let result = validate_url("http://[::1]:8080/api");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("blocked pattern"));
    }

    #[test]
    fn test_validate_url_blocked_private_10_network() {
        let result = validate_url("http://10.0.0.1/api");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("blocked pattern"));
    }

    #[test]
    fn test_validate_url_blocked_private_192_168_network() {
        let result = validate_url("http://192.168.1.1/api");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("blocked pattern"));
    }

    #[test]
    fn test_validate_url_blocked_private_172_16_network() {
        let result = validate_url("http://172.16.0.1/api");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("blocked pattern"));
    }

    #[test]
    fn test_validate_url_blocked_link_local() {
        let result = validate_url("http://169.254.1.1/api");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("blocked pattern"));
    }

    #[test]
    fn test_validate_url_internal_allow_local_localhost() {
        assert!(validate_url_internal("http://localhost:3000/api", true).is_ok());
    }

    #[test]
    fn test_validate_url_internal_allow_local_127() {
        assert!(validate_url_internal("http://127.0.0.1:8080/api", true).is_ok());
    }

    #[test]
    fn test_validate_url_empty_host() {
        let result = validate_url("http:///path");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("valid host"));
    }

    #[test]
    fn test_validate_url_path_traversal() {
        let result = validate_url("https://example.com/../../../etc/passwd");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("path traversal"));
    }

    #[test]
    fn test_validate_url_control_characters() {
        let result = validate_url("https://example.com/\x01path");
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("control characters")
        );
    }

    #[test]
    fn test_validate_url_case_insensitive_scheme() {
        assert!(validate_url("HTTPS://api.example.com/v1").is_ok());
        assert!(validate_url("Http://api.example.com/v1").is_ok());
    }

    // ------------------------------------------------------------------------
    // validate_env_var_name tests
    // ------------------------------------------------------------------------

    #[test]
    fn test_validate_env_var_name_valid_simple() {
        assert!(validate_env_var_name("API_KEY").is_ok());
    }

    #[test]
    fn test_validate_env_var_name_valid_lowercase() {
        assert!(validate_env_var_name("api_key").is_ok());
    }

    #[test]
    fn test_validate_env_var_name_valid_mixed_case() {
        assert!(validate_env_var_name("MyApiKey").is_ok());
    }

    #[test]
    fn test_validate_env_var_name_valid_with_numbers() {
        assert!(validate_env_var_name("API_KEY_2").is_ok());
    }

    #[test]
    fn test_validate_env_var_name_valid_underscore_start() {
        assert!(validate_env_var_name("_PRIVATE_VAR").is_ok());
    }

    #[test]
    fn test_validate_env_var_name_valid_single_char() {
        assert!(validate_env_var_name("X").is_ok());
    }

    #[test]
    fn test_validate_env_var_name_empty() {
        let result = validate_env_var_name("");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("cannot be empty"));
    }

    #[test]
    fn test_validate_env_var_name_exceeds_max_length() {
        let long_name = "A".repeat(MAX_ENV_VAR_NAME_LENGTH + 1);
        let result = validate_env_var_name(&long_name);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("exceeds maximum length")
        );
    }

    #[test]
    fn test_validate_env_var_name_at_max_length() {
        let max_name = "A".repeat(MAX_ENV_VAR_NAME_LENGTH);
        assert!(validate_env_var_name(&max_name).is_ok());
    }

    #[test]
    fn test_validate_env_var_name_starts_with_digit() {
        let result = validate_env_var_name("2ND_VAR");
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("cannot start with a digit")
        );
    }

    #[test]
    fn test_validate_env_var_name_invalid_dash() {
        let result = validate_env_var_name("API-KEY");
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("invalid characters")
        );
    }

    #[test]
    fn test_validate_env_var_name_invalid_space() {
        let result = validate_env_var_name("API KEY");
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("invalid characters")
        );
    }

    #[test]
    fn test_validate_env_var_name_invalid_dot() {
        let result = validate_env_var_name("API.KEY");
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("invalid characters")
        );
    }

    #[test]
    fn test_validate_env_var_name_invalid_special_chars() {
        let result = validate_env_var_name("API$KEY");
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("invalid characters")
        );
    }

    // ------------------------------------------------------------------------
    // validate_env_var_value tests
    // ------------------------------------------------------------------------

    #[test]
    fn test_validate_env_var_value_valid_simple() {
        assert!(validate_env_var_value("some_value").is_ok());
    }

    #[test]
    fn test_validate_env_var_value_valid_empty() {
        assert!(validate_env_var_value("").is_ok());
    }

    #[test]
    fn test_validate_env_var_value_valid_with_spaces() {
        assert!(validate_env_var_value("value with spaces").is_ok());
    }

    #[test]
    fn test_validate_env_var_value_valid_with_special_chars() {
        assert!(validate_env_var_value("val!@#$%^&*()").is_ok());
    }

    #[test]
    fn test_validate_env_var_value_valid_at_max_length() {
        let max_value = "x".repeat(MAX_ENV_VAR_VALUE_LENGTH);
        assert!(validate_env_var_value(&max_value).is_ok());
    }

    #[test]
    fn test_validate_env_var_value_exceeds_max_length() {
        let long_value = "x".repeat(MAX_ENV_VAR_VALUE_LENGTH + 1);
        let result = validate_env_var_value(&long_value);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("exceeds maximum length")
        );
    }

    #[test]
    fn test_validate_env_var_value_null_bytes() {
        let result = validate_env_var_value("value\0with\0nulls");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("null bytes"));
    }

    #[test]
    fn test_validate_env_var_value_valid_newlines() {
        assert!(validate_env_var_value("line1\nline2").is_ok());
    }

    #[test]
    fn test_validate_env_var_value_valid_json() {
        assert!(validate_env_var_value("{\"key\": \"value\"}").is_ok());
    }

    // ------------------------------------------------------------------------
    // validate_bearer_token_env_var tests
    // ------------------------------------------------------------------------

    #[test]
    fn test_validate_bearer_token_env_var_valid() {
        assert!(validate_bearer_token_env_var("MCP_AUTH_TOKEN").is_ok());
    }

    #[test]
    fn test_validate_bearer_token_env_var_valid_api_key() {
        assert!(validate_bearer_token_env_var("OPENAI_API_KEY").is_ok());
    }

    #[test]
    fn test_validate_bearer_token_env_var_invalid_empty() {
        let result = validate_bearer_token_env_var("");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("cannot be empty"));
    }

    #[test]
    fn test_validate_bearer_token_env_var_invalid_starts_digit() {
        let result = validate_bearer_token_env_var("1TOKEN");
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("cannot start with a digit")
        );
    }

    #[test]
    fn test_validate_bearer_token_env_var_with_password_in_name() {
        // Should succeed but logs a warning
        assert!(validate_bearer_token_env_var("MY_PASSWORD_TOKEN").is_ok());
    }

    #[test]
    fn test_validate_bearer_token_env_var_with_passwd_in_name() {
        // Should succeed but logs a warning
        assert!(validate_bearer_token_env_var("MY_PASSWD_VAR").is_ok());
    }

    // ------------------------------------------------------------------------
    // validate_command_args tests
    // ------------------------------------------------------------------------

    #[test]
    fn test_validate_command_args_valid_single() {
        let args = vec!["npx".to_string()];
        assert!(validate_command_args(&args).is_ok());
    }

    #[test]
    fn test_validate_command_args_valid_multiple() {
        let args = vec![
            "python".to_string(),
            "-m".to_string(),
            "mcp_server".to_string(),
        ];
        assert!(validate_command_args(&args).is_ok());
    }

    #[test]
    fn test_validate_command_args_valid_with_flags() {
        let args = vec![
            "npx".to_string(),
            "-y".to_string(),
            "@modelcontextprotocol/server-github".to_string(),
        ];
        assert!(validate_command_args(&args).is_ok());
    }

    #[test]
    fn test_validate_command_args_empty() {
        let args: Vec<String> = vec![];
        let result = validate_command_args(&args);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("cannot be empty"));
    }

    #[test]
    fn test_validate_command_args_too_many() {
        let args: Vec<String> = (0..=MAX_COMMAND_ARGS)
            .map(|i| format!("arg{}", i))
            .collect();
        let result = validate_command_args(&args);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Too many command arguments")
        );
    }

    #[test]
    fn test_validate_command_args_at_max() {
        let args: Vec<String> = (0..MAX_COMMAND_ARGS).map(|i| format!("arg{}", i)).collect();
        assert!(validate_command_args(&args).is_ok());
    }

    #[test]
    fn test_validate_command_args_arg_too_long() {
        let long_arg = "x".repeat(MAX_COMMAND_ARG_LENGTH + 1);
        let args = vec!["cmd".to_string(), long_arg];
        let result = validate_command_args(&args);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("exceeds maximum length")
        );
    }

    #[test]
    fn test_validate_command_args_arg_at_max_length() {
        let max_arg = "x".repeat(MAX_COMMAND_ARG_LENGTH);
        let args = vec!["cmd".to_string(), max_arg];
        assert!(validate_command_args(&args).is_ok());
    }

    #[test]
    fn test_validate_command_args_null_bytes() {
        let args = vec!["cmd".to_string(), "arg\0with\0null".to_string()];
        let result = validate_command_args(&args);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("null bytes"));
    }

    #[test]
    fn test_validate_command_args_url_as_command_http() {
        let args = vec!["http://example.com/mcp".to_string()];
        let result = validate_command_args(&args);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Remote MCP URLs are not supported")
        );
    }

    #[test]
    fn test_validate_command_args_url_as_command_https() {
        let args = vec!["https://example.com/mcp".to_string()];
        let result = validate_command_args(&args);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Remote MCP URLs are not supported")
        );
    }

    #[test]
    fn test_validate_command_args_url_not_first_arg() {
        // URLs as non-first arguments are allowed (e.g., for server configuration)
        let args = vec![
            "node".to_string(),
            "server.js".to_string(),
            "https://api.example.com".to_string(),
        ];
        assert!(validate_command_args(&args).is_ok());
    }

    // ------------------------------------------------------------------------
    // validate_server_name tests
    // ------------------------------------------------------------------------

    #[test]
    fn test_validate_server_name_valid_simple() {
        assert!(validate_server_name("github").is_ok());
    }

    #[test]
    fn test_validate_server_name_valid_with_numbers() {
        assert!(validate_server_name("server1").is_ok());
    }

    #[test]
    fn test_validate_server_name_valid_with_dashes() {
        assert!(validate_server_name("my-server").is_ok());
    }

    #[test]
    fn test_validate_server_name_valid_with_underscores() {
        assert!(validate_server_name("my_server").is_ok());
    }

    #[test]
    fn test_validate_server_name_valid_mixed() {
        assert!(validate_server_name("my-server_v2").is_ok());
    }

    #[test]
    fn test_validate_server_name_valid_uppercase() {
        assert!(validate_server_name("MyServer").is_ok());
    }

    #[test]
    fn test_validate_server_name_valid_underscore_start() {
        assert!(validate_server_name("_internal").is_ok());
    }

    #[test]
    fn test_validate_server_name_empty() {
        let result = validate_server_name("");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("cannot be empty"));
    }

    #[test]
    fn test_validate_server_name_exceeds_max_length() {
        let long_name = "a".repeat(MAX_SERVER_NAME_LENGTH + 1);
        let result = validate_server_name(&long_name);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("exceeds maximum length")
        );
    }

    #[test]
    fn test_validate_server_name_at_max_length() {
        let max_name = "a".repeat(MAX_SERVER_NAME_LENGTH);
        assert!(validate_server_name(&max_name).is_ok());
    }

    #[test]
    fn test_validate_server_name_starts_with_digit() {
        let result = validate_server_name("2server");
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("must start with a letter or underscore")
        );
    }

    #[test]
    fn test_validate_server_name_starts_with_dash() {
        let result = validate_server_name("-server");
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("must start with a letter or underscore")
        );
    }

    #[test]
    fn test_validate_server_name_invalid_space() {
        let result = validate_server_name("my server");
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("invalid server name")
        );
    }

    #[test]
    fn test_validate_server_name_invalid_dot() {
        let result = validate_server_name("my.server");
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("invalid server name")
        );
    }

    #[test]
    fn test_validate_server_name_invalid_special_chars() {
        let result = validate_server_name("my@server");
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("invalid server name")
        );
    }

    #[test]
    fn test_validate_server_name_reserved_dot() {
        let result = validate_server_name(".");
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_server_name_reserved_double_dot() {
        let result = validate_server_name("..");
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_server_name_reserved_con() {
        let result = validate_server_name("con");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("reserved"));
    }

    #[test]
    fn test_validate_server_name_reserved_prn() {
        let result = validate_server_name("prn");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("reserved"));
    }

    #[test]
    fn test_validate_server_name_reserved_aux() {
        let result = validate_server_name("aux");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("reserved"));
    }

    #[test]
    fn test_validate_server_name_reserved_nul() {
        let result = validate_server_name("nul");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("reserved"));
    }

    #[test]
    fn test_validate_server_name_reserved_case_insensitive() {
        let result = validate_server_name("CON");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("reserved"));
    }

    // ------------------------------------------------------------------------
    // parse_env_pair tests
    // ------------------------------------------------------------------------

    #[test]
    fn test_parse_env_pair_valid_simple() {
        let result = parse_env_pair("API_KEY=secret123");
        assert!(result.is_ok());
        let (key, value) = result.unwrap();
        assert_eq!(key, "API_KEY");
        assert_eq!(value, "secret123");
    }

    #[test]
    fn test_parse_env_pair_valid_with_equals_in_value() {
        let result = parse_env_pair("CONFIG=key=value");
        assert!(result.is_ok());
        let (key, value) = result.unwrap();
        assert_eq!(key, "CONFIG");
        assert_eq!(value, "key=value");
    }

    #[test]
    fn test_parse_env_pair_valid_with_spaces_in_value() {
        let result = parse_env_pair("MESSAGE=hello world");
        assert!(result.is_ok());
        let (key, value) = result.unwrap();
        assert_eq!(key, "MESSAGE");
        assert_eq!(value, "hello world");
    }

    #[test]
    fn test_parse_env_pair_valid_key_trimmed() {
        let result = parse_env_pair("  API_KEY  =value");
        assert!(result.is_ok());
        let (key, value) = result.unwrap();
        assert_eq!(key, "API_KEY");
        assert_eq!(value, "value");
    }

    #[test]
    fn test_parse_env_pair_value_not_trimmed() {
        let result = parse_env_pair("KEY=  value  ");
        assert!(result.is_ok());
        let (key, value) = result.unwrap();
        assert_eq!(key, "KEY");
        assert_eq!(value, "  value  ");
    }

    #[test]
    fn test_parse_env_pair_empty_string() {
        let result = parse_env_pair("");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("KEY=VALUE"));
    }

    #[test]
    fn test_parse_env_pair_no_equals() {
        let result = parse_env_pair("JUST_KEY");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("KEY=VALUE"));
    }

    #[test]
    fn test_parse_env_pair_empty_key() {
        let result = parse_env_pair("=value");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("KEY=VALUE"));
    }

    #[test]
    fn test_parse_env_pair_empty_value() {
        let result = parse_env_pair("KEY=");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("empty value"));
    }

    #[test]
    fn test_parse_env_pair_whitespace_only_key() {
        let result = parse_env_pair("   =value");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("KEY=VALUE"));
    }

    #[test]
    fn test_parse_env_pair_valid_json_value() {
        let result = parse_env_pair("CONFIG={\"host\":\"localhost\",\"port\":8080}");
        assert!(result.is_ok());
        let (key, value) = result.unwrap();
        assert_eq!(key, "CONFIG");
        assert_eq!(value, "{\"host\":\"localhost\",\"port\":8080}");
    }

    #[test]
    fn test_parse_env_pair_valid_url_value() {
        let result = parse_env_pair("ENDPOINT=https://api.example.com/v1?key=abc&test=123");
        assert!(result.is_ok());
        let (key, value) = result.unwrap();
        assert_eq!(key, "ENDPOINT");
        assert_eq!(value, "https://api.example.com/v1?key=abc&test=123");
    }

    #[test]
    fn test_parse_env_pair_valid_special_chars_value() {
        let result = parse_env_pair("SPECIAL=!@#$%^&*()");
        assert!(result.is_ok());
        let (key, value) = result.unwrap();
        assert_eq!(key, "SPECIAL");
        assert_eq!(value, "!@#$%^&*()");
    }
}
