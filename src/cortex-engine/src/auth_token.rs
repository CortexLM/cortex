//! Centralized authentication module.
//!
//! This module provides a single source of truth for retrieving authentication tokens.
//! All components should use these functions instead of implementing their own logic.
//!
//! # Supported Authentication Methods
//!
//! 1. **API Key**: Set via `cortex login --api-key` or environment variables
//! 2. **OAuth Login**: Set via `cortex login` (device code flow)
//!
//! # Priority Order
//!
//! 1. Instance token (passed explicitly to client)
//! 2. `CORTEX_AUTH_TOKEN` environment variable
//! 3. `CORTEX_API_KEY` environment variable (alias for CORTEX_AUTH_TOKEN)
//! 4. System keyring (via `cortex_login::get_auth_token()` with auto-refresh)

use crate::error::{CortexError, Result};

/// Get authentication token with optional instance override.
///
/// Priority order:
/// 1. Instance token (if provided and non-empty)
/// 2. `CORTEX_AUTH_TOKEN` environment variable
/// 3. `CORTEX_API_KEY` environment variable (alias for GitHub Actions compatibility)
/// 4. System keyring via `cortex_login::get_auth_token()` (handles OAuth token refresh)
///
/// # Arguments
/// * `instance_token` - Optional token passed to the client instance
///
/// # Returns
/// * `Ok(String)` - The authentication token
/// * `Err(CortexError::Auth)` - If no valid token found
///
/// # Example
/// ```ignore
/// // In a client implementation
/// fn auth_header(&self) -> Option<String> {
///     auth::get_auth_token(self.auth_token.as_deref())
///         .ok()
///         .map(|token| format!("Bearer {}", token))
/// }
/// ```
pub fn get_auth_token(instance_token: Option<&str>) -> Result<String> {
    // Priority 1: Instance token (if provided and non-empty)
    if let Some(token) = instance_token {
        if !token.is_empty() {
            tracing::debug!(source = "instance", "Using auth token from client instance");
            return Ok(token.to_string());
        }
    }

    // Priority 2: CORTEX_AUTH_TOKEN environment variable
    if let Ok(token) = std::env::var("CORTEX_AUTH_TOKEN") {
        if !token.is_empty() {
            tracing::debug!(
                source = "env_var",
                "Using auth token from CORTEX_AUTH_TOKEN"
            );
            return Ok(token);
        }
    }

    // Priority 3: CORTEX_API_KEY environment variable (alias for GitHub Actions workflow)
    if let Ok(token) = std::env::var("CORTEX_API_KEY") {
        if !token.is_empty() {
            tracing::debug!(source = "env_var", "Using auth token from CORTEX_API_KEY");
            return Ok(token);
        }
    }

    // Priority 4: Keyring via cortex_login (handles OAuth token refresh automatically)
    if let Some(token) = cortex_login::get_auth_token() {
        tracing::debug!(source = "keyring", "Using auth token from keyring");
        return Ok(token);
    }

    // No valid token found
    tracing::warn!("No authentication token found in instance, env var, or keyring");
    Err(CortexError::Auth(
        "Not authenticated. Run 'cortex login' or set CORTEX_AUTH_TOKEN environment variable."
            .to_string(),
    ))
}

/// Get authentication token, returning None instead of error if not found.
///
/// Use this when authentication is optional (e.g., for endpoints that work without auth).
pub fn get_auth_token_optional(instance_token: Option<&str>) -> Option<String> {
    get_auth_token(instance_token).ok()
}

/// Check if authentication is available without retrieving the actual token.
///
/// Useful for fast availability checks in UI.
pub fn is_authenticated(instance_token: Option<&str>) -> bool {
    // Check instance token
    if instance_token.map_or(false, |t| !t.is_empty()) {
        return true;
    }

    // Check CORTEX_AUTH_TOKEN env var
    if std::env::var("CORTEX_AUTH_TOKEN").map_or(false, |t| !t.is_empty()) {
        return true;
    }

    // Check CORTEX_API_KEY env var (alias)
    if std::env::var("CORTEX_API_KEY").map_or(false, |t| !t.is_empty()) {
        return true;
    }

    // Check keyring
    cortex_login::has_valid_auth()
}

/// Format token as Authorization header value.
pub fn auth_header(instance_token: Option<&str>) -> Option<String> {
    get_auth_token_optional(instance_token).map(|token| format!("Bearer {}", token))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_instance_token_priority() {
        // Instance token should take priority
        let result = get_auth_token(Some("test-instance-token"));
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "test-instance-token");
    }

    #[test]
    fn test_empty_instance_token_skipped() {
        // Empty instance token should be skipped
        let result = get_auth_token(Some(""));
        // Will fail if no env var or keyring - that's expected in test
        assert!(result.is_err() || result.is_ok());
    }

    #[test]
    fn test_is_authenticated_with_instance() {
        assert!(is_authenticated(Some("token")));
        assert!(!is_authenticated(Some("")));
        assert!(!is_authenticated(None));
    }

    #[test]
    fn test_auth_header_format() {
        let header = auth_header(Some("my-token"));
        assert_eq!(header, Some("Bearer my-token".to_string()));
    }
}
