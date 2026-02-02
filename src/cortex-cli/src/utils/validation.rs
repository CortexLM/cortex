//! Input validation utilities for the Cortex CLI.
//!
//! Provides comprehensive validation for URLs, server names, environment
//! variables, and model names used across multiple commands.

use anyhow::{Result, bail};

/// Maximum length for server names (prevents DoS and storage issues).
pub const MAX_SERVER_NAME_LENGTH: usize = 64;

/// Maximum length for URLs (reasonable limit for HTTP URLs).
pub const MAX_URL_LENGTH: usize = 2048;

/// Maximum length for environment variable names.
pub const MAX_ENV_VAR_NAME_LENGTH: usize = 256;

/// Maximum length for environment variable values.
pub const MAX_ENV_VAR_VALUE_LENGTH: usize = 4096;

/// Allowed URL schemes for HTTP/WebSocket transport.
pub const ALLOWED_URL_SCHEMES: &[&str] = &["http", "https", "ws", "wss"];

/// Dangerous URL patterns that should be blocked by default.
pub const BLOCKED_URL_PATTERNS: &[&str] = &[
    "javascript:",
    "data:",
    "file:",
    "ftp:",
    "localhost",
    "127.0.0.1",
    "0.0.0.0",
    "[::1]",
    "169.254.", // Link-local
    "10.",      // Private network
    "192.168.", // Private network
    "172.16.",
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
    "172.31.",
];

/// Validates and sanitizes a URL for HTTP/WebSocket transport.
///
/// # Validation Rules:
/// - Must not exceed maximum length
/// - Must use allowed schemes (http/https/ws/wss)
/// - Must not contain dangerous patterns (unless allow_local is true)
/// - Must be a valid URL format
///
/// # Arguments
/// * `url` - The URL to validate
///
/// # Returns
/// `Ok(())` if valid, or an error describing the issue.
pub fn validate_url(url: &str) -> Result<()> {
    validate_url_internal(url, false)
}

/// Validates URL with option to allow local addresses.
///
/// # Arguments
/// * `url` - The URL to validate
/// * `allow_local` - If true, allows localhost and private network URLs
pub fn validate_url_allowing_local(url: &str) -> Result<()> {
    validate_url_internal(url, true)
}

fn validate_url_internal(url: &str, allow_local: bool) -> Result<()> {
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
                    "URL contains blocked pattern '{}'. For security, local/private network URLs \
                     are not allowed by default. Use --allow-local flag to enable local development servers.",
                    pattern
                );
            }
        }
    }

    // Extract host portion for validation
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

    // Host should be non-empty
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

    // Check for path traversal attempts
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

/// Validates environment variable name.
///
/// Environment variable names must:
/// - Not be empty
/// - Not exceed maximum length
/// - Contain only alphanumeric characters and underscores
/// - Not start with a digit
pub fn validate_env_var_name(name: &str) -> Result<()> {
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
            "Environment variable name '{}' contains invalid characters. \
             Use only letters, numbers, and underscores.",
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
///
/// Values must:
/// - Not exceed maximum length
/// - Not contain null bytes
pub fn validate_env_var_value(value: &str) -> Result<()> {
    if value.len() > MAX_ENV_VAR_VALUE_LENGTH {
        bail!(
            "Environment variable value exceeds maximum length of {} characters",
            MAX_ENV_VAR_VALUE_LENGTH
        );
    }
    if value.contains('\0') {
        bail!("Environment variable value contains null bytes");
    }
    Ok(())
}

/// Validates a server name (used for MCP servers, etc.).
///
/// Server names must:
/// - Not be empty
/// - Not exceed maximum length
/// - Contain only alphanumeric characters, hyphens, and underscores
/// - Not start with a digit or hyphen
/// - Not be a reserved name
pub fn validate_server_name(name: &str) -> Result<()> {
    if name.is_empty() {
        bail!("Server name cannot be empty");
    }
    if name.len() > MAX_SERVER_NAME_LENGTH {
        bail!(
            "Server name '{}' exceeds maximum length of {} characters",
            name,
            MAX_SERVER_NAME_LENGTH
        );
    }

    // Check for valid characters
    let is_valid_chars = name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');

    if !is_valid_chars {
        bail!("Invalid server name '{name}' (use letters, numbers, '-', '_')");
    }

    // Must start with a letter or underscore
    let first_char = name.chars().next().unwrap();
    if first_char.is_ascii_digit() || first_char == '-' {
        bail!(
            "Server name must start with a letter or underscore, not '{}'",
            first_char
        );
    }

    // Check for reserved names
    let reserved_names = [".", "..", "con", "prn", "aux", "nul"];
    if reserved_names.contains(&name.to_lowercase().as_str()) {
        bail!("Server name '{}' is reserved and cannot be used", name);
    }

    Ok(())
}

/// Validates a model name.
///
/// Model names must:
/// - Not be empty
/// - If containing '/', be in provider/model format
/// - Contain only valid characters
pub fn validate_model_name(model: &str) -> Result<String> {
    use cortex_common::resolve_model_alias;

    // First, resolve any alias
    let resolved = resolve_model_alias(model);

    // If the model contains a '/', validate provider/model format
    if resolved.contains('/') {
        let parts: Vec<&str> = resolved.splitn(2, '/').collect();
        if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
            bail!(
                "Invalid model format: '{}'. Expected 'provider/model' format.\n\
                 Examples: anthropic/claude-sonnet-4-20250514, openai/gpt-4o\n\
                 Run 'cortex models list' to see available models.",
                model
            );
        }

        // Validate provider is known (warn but don't block)
        let valid_providers = [
            "anthropic",
            "openai",
            "google",
            "mistral",
            "xai",
            "deepseek",
            "groq",
        ];
        let provider = parts[0].to_lowercase();
        if !valid_providers.contains(&provider.as_str()) {
            tracing::warn!(
                "Unknown provider '{}'. Known providers: {}",
                provider,
                valid_providers.join(", ")
            );
        }
    } else {
        // Model name without provider
        let valid_chars = resolved
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' || c == ':');
        if !valid_chars || resolved.is_empty() {
            bail!(
                "Invalid model name: '{}'. Model names should contain only alphanumeric \
                 characters, hyphens, underscores, dots, and colons.\n\
                 Run 'cortex models list' to see available models.",
                model
            );
        }
    }

    Ok(resolved.to_string())
}

/// Reserved command names that cannot be used as agent names.
pub const RESERVED_COMMAND_NAMES: &[&str] = &[
    "help",
    "version",
    "run",
    "exec",
    "login",
    "logout",
    "mcp",
    "agent",
    "resume",
    "sessions",
    "export",
    "import",
    "config",
    "serve",
    "models",
    "upgrade",
    "uninstall",
    "stats",
    "github",
    "pr",
    "scrape",
    "acp",
    "debug",
    "servers",
    "sandbox",
    "completion",
    "features",
];

/// Check if a name is a reserved command name.
pub fn is_reserved_command(name: &str) -> bool {
    RESERVED_COMMAND_NAMES.contains(&name.to_lowercase().as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_url_validation_empty() {
        assert!(validate_url("").is_err());
    }

    #[test]
    fn test_url_validation_valid() {
        assert!(validate_url("https://api.example.com/v1").is_ok());
        assert!(validate_url("wss://socket.example.com").is_ok());
    }

    #[test]
    fn test_url_validation_blocked_local() {
        assert!(validate_url("http://localhost:8080").is_err());
        assert!(validate_url("http://127.0.0.1:8080").is_err());

        // But allowed with allow_local
        assert!(validate_url_allowing_local("http://localhost:8080").is_ok());
    }

    #[test]
    fn test_env_var_name_validation() {
        assert!(validate_env_var_name("MY_VAR").is_ok());
        assert!(validate_env_var_name("VAR123").is_ok());
        assert!(validate_env_var_name("").is_err());
        assert!(validate_env_var_name("123VAR").is_err()); // Starts with digit
        assert!(validate_env_var_name("MY-VAR").is_err()); // Contains hyphen
    }

    #[test]
    fn test_server_name_validation() {
        assert!(validate_server_name("my-server").is_ok());
        assert!(validate_server_name("server_123").is_ok());
        assert!(validate_server_name("_hidden").is_ok());
        assert!(validate_server_name("").is_err());
        assert!(validate_server_name("-invalid").is_err());
        assert!(validate_server_name("123invalid").is_err());
        assert!(validate_server_name("con").is_err()); // Reserved
    }

    #[test]
    fn test_reserved_commands() {
        assert!(is_reserved_command("help"));
        assert!(is_reserved_command("HELP")); // Case insensitive
        assert!(!is_reserved_command("mycommand"));
    }
}
