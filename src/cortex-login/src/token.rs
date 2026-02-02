//! Token management and refresh functionality.

use secrecy::SecretString;
use std::time::Duration;

use crate::constants::{API_BASE_URL, USER_AGENT};
use crate::keyring::save_to_keyring;
use crate::storage::load_auth_with_fallback;
use crate::types::{AuthMode, SecureAuthData};

/// Refresh an expired access token using the refresh token.
///
/// Makes a synchronous HTTP call to the token refresh endpoint.
/// Returns new auth data on success, None on failure.
///
/// Uses std::thread::spawn to avoid tokio runtime conflicts when called from async context.
fn refresh_token_sync(refresh_token: &str) -> Option<SecureAuthData> {
    let refresh_token = refresh_token.to_string();

    // Use std::thread to avoid "Cannot drop a runtime in a context where blocking is not allowed"
    // This happens when reqwest::blocking::Client is used within a tokio async context
    let handle = std::thread::spawn(move || {
        let client = reqwest::blocking::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(30))
            .build()
            .ok()?;

        let resp = client
            .post(format!("{}/auth/token/refresh", API_BASE_URL))
            .json(&serde_json::json!({"refresh_token": refresh_token}))
            .send()
            .ok()?;

        if !resp.status().is_success() {
            tracing::warn!(
                status = %resp.status(),
                "Token refresh request failed"
            );
            return None;
        }

        let json: serde_json::Value = resp.json().ok()?;

        let access_token = json.get("access_token")?.as_str()?;
        let new_refresh = json.get("refresh_token").and_then(|v| v.as_str());
        let expires_in = json
            .get("expires_in")
            .and_then(|v| v.as_i64())
            .unwrap_or(3600);

        tracing::info!("Successfully refreshed access token");

        Some(SecureAuthData::from_components(
            AuthMode::OAuth,
            None,
            Some(SecretString::from(access_token.to_string())),
            new_refresh.map(|s| SecretString::from(s.to_string())),
            Some(chrono::Utc::now().timestamp() + expires_in),
            None,
        ))
    });

    handle.join().ok().flatten()
}

/// Load authentication token if available and valid.
///
/// Returns the token string if authentication is valid, None otherwise.
/// If the token is expired but a refresh token exists, attempts to refresh automatically.
pub fn get_auth_token() -> Option<String> {
    // Check environment variable first (highest priority for CI/CD)
    if let Ok(token) = std::env::var("CORTEX_API_KEY") {
        if !token.is_empty() {
            tracing::debug!("Using API key from CORTEX_API_KEY environment variable");
            return Some(token);
        }
    }

    // Get default cortex home for fallback loading
    let cortex_home = dirs::home_dir()?.join(".cortex");

    let auth = match load_auth_with_fallback(&cortex_home) {
        Ok(Some(auth)) => {
            tracing::debug!(
                mode = ?auth.mode,
                has_access_token = auth.access_token.is_some(),
                has_api_key = auth.api_key.is_some(),
                expires_at = ?auth.expires_at,
                is_expired = auth.is_expired(),
                "Loaded auth with fallback"
            );
            auth
        }
        Ok(None) => {
            tracing::debug!("No auth data found");
            return None;
        }
        Err(e) => {
            tracing::debug!(error = %e, "Failed to load auth");
            return None;
        }
    };

    if !auth.is_expired() {
        let token = auth.get_token();
        tracing::debug!(has_token = token.is_some(), "Token not expired, returning");
        return token.map(|s| s.to_string());
    }

    // Token is expired - attempt refresh if we have a refresh token
    if let Some(refresh_token) = auth.get_refresh_token() {
        tracing::debug!("Access token expired, attempting refresh");
        if let Some(new_auth) = refresh_token_sync(refresh_token) {
            if let Err(e) = save_to_keyring(&new_auth) {
                tracing::warn!(error = %e, "Failed to save refreshed token to keyring");
            }
            return new_auth.get_token().map(|s| s.to_string());
        }
        tracing::debug!("Token refresh failed");
    }

    None
}
