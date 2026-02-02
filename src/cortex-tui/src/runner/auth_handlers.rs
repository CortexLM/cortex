//! Authentication handlers for the TUI.
//!
//! This module contains handlers for authentication-related commands:
//! - /login - Device code OAuth flow
//! - /logout - Clear stored credentials
//! - /account - Display account information
//!
//! These handlers were extracted from `event_loop.rs` to improve modularity
//! and reduce file size.

use std::time::Duration;

use anyhow::Result;
use tokio::sync::mpsc;

use cortex_login::{
    AuthMode, CredentialsStoreMode, SecureAuthData, load_auth, logout_with_fallback,
    save_auth_with_fallback,
};

use crate::events::ToolEvent;

/// API base URL for Cortex authentication.
const API_BASE_URL: &str = "https://api.cortex.foundation";
/// Auth base URL for device code verification.
const AUTH_BASE_URL: &str = "https://auth.cortex.foundation";

/// Result of an auth operation for UI updates.
pub enum AuthResult {
    /// Already logged in, no action needed.
    AlreadyLoggedIn,
    /// Login flow started, show verification URL.
    LoginStarted {
        verification_url: String,
        user_code: String,
    },
    /// Logout successful.
    LoggedOut,
    /// No credentials found.
    NotLoggedIn,
    /// Account info loaded.
    AccountInfo {
        auth_method: String,
        expires_at: Option<String>,
        account_id: Option<String>,
    },
    /// Error occurred.
    Error(String),
}

/// Get the cortex home directory.
pub fn get_cortex_home() -> Option<std::path::PathBuf> {
    dirs::home_dir().map(|home| home.join(".cortex"))
}

/// Check if user is already logged in.
pub fn is_logged_in() -> bool {
    let Some(cortex_home) = get_cortex_home() else {
        return false;
    };

    if let Ok(Some(auth)) = load_auth(&cortex_home, CredentialsStoreMode::default()) {
        !auth.is_expired()
    } else {
        false
    }
}

/// Start the login flow asynchronously.
/// Returns the device code response or an error.
pub async fn start_login_flow() -> Result<(String, String, String)> {
    let client = cortex_engine::create_default_client()?;

    let device_name = hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_else(|_| "Cortex CLI".to_string());

    let response = client
        .post(format!("{}/auth/device/code", API_BASE_URL))
        .json(&serde_json::json!({
            "device_name": device_name,
            "scopes": ["chat", "models"]
        }))
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();

        let error_msg = if status.as_u16() == 403 {
            "Cannot connect to Cortex API. Service may be unavailable.".to_string()
        } else if status.as_u16() == 429 {
            "Too many login attempts. Please wait.".to_string()
        } else {
            format!("API error ({}): {}", status, body)
        };

        anyhow::bail!(error_msg);
    }

    #[derive(serde::Deserialize)]
    struct DeviceCodeResponse {
        user_code: String,
        device_code: String,
        #[allow(dead_code)]
        verification_uri: String,
    }

    let data: DeviceCodeResponse = response.json().await?;
    let verification_url = format!("{}/device", AUTH_BASE_URL);

    Ok((data.device_code, data.user_code, verification_url))
}

/// Poll for login completion in the background.
pub fn spawn_login_poll(device_code: String, tx: mpsc::Sender<ToolEvent>) {
    let Some(cortex_home) = get_cortex_home() else {
        return;
    };

    tokio::spawn(async move {
        let poll_client = match cortex_engine::create_default_client() {
            Ok(c) => c,
            Err(e) => {
                tracing::error!("Login polling failed: Could not create HTTP client: {}", e);
                let _ = tx
                    .send(ToolEvent::Failed {
                        id: "login".to_string(),
                        name: "login".to_string(),
                        error: format!("Login failed: {}", e),
                        duration: Duration::from_secs(0),
                    })
                    .await;
                return;
            }
        };

        let interval = Duration::from_secs(5);
        let max_attempts = 180; // 15 minutes total

        for _ in 0..max_attempts {
            tokio::time::sleep(interval).await;

            let poll_response = match poll_client
                .post(format!("{}/auth/device/token", API_BASE_URL))
                .json(&serde_json::json!({ "device_code": device_code }))
                .send()
                .await
            {
                Ok(r) => r,
                Err(_) => continue,
            };

            let status = poll_response.status();
            let body = poll_response.text().await.unwrap_or_default();

            if status.is_success() {
                #[derive(serde::Deserialize)]
                struct TokenResponse {
                    access_token: String,
                    refresh_token: String,
                }

                if let Ok(token) = serde_json::from_str::<TokenResponse>(&body) {
                    let expires_at = chrono::Utc::now().timestamp() + 3600;
                    let auth_data = SecureAuthData::with_oauth(
                        token.access_token,
                        Some(token.refresh_token),
                        Some(expires_at),
                    );

                    match save_auth_with_fallback(&cortex_home, &auth_data) {
                        Ok(mode) => {
                            tracing::info!(
                                "Login successful, credentials saved using {:?} storage",
                                mode
                            );
                            let _ = tx
                                .send(ToolEvent::Completed {
                                    id: "login".to_string(),
                                    name: "login".to_string(),
                                    output: "Login successful! You are now authenticated."
                                        .to_string(),
                                    success: true,
                                    duration: Duration::from_secs(0),
                                })
                                .await;
                            return;
                        }
                        Err(e) => {
                            tracing::error!("Login failed: Could not save credentials: {}", e);
                            let _ = tx
                                .send(ToolEvent::Failed {
                                    id: "login".to_string(),
                                    name: "login".to_string(),
                                    error: format!(
                                        "Login failed: Could not save credentials: {}",
                                        e
                                    ),
                                    duration: Duration::from_secs(0),
                                })
                                .await;
                            return;
                        }
                    }
                }
                continue;
            }

            // Handle error responses
            if let Ok(error) = serde_json::from_str::<serde_json::Value>(&body)
                && let Some(err) = error.get("error").and_then(|e| e.as_str())
            {
                match err {
                    "authorization_pending" | "slow_down" => continue,
                    "expired_token" => {
                        tracing::error!("Login failed: Device code expired");
                        let _ = tx
                            .send(ToolEvent::Failed {
                                id: "login".to_string(),
                                name: "login".to_string(),
                                error: "Login failed: Device code expired. Please try again."
                                    .to_string(),
                                duration: Duration::from_secs(0),
                            })
                            .await;
                        return;
                    }
                    "access_denied" => {
                        tracing::error!("Login failed: Access denied");
                        let _ = tx
                            .send(ToolEvent::Failed {
                                id: "login".to_string(),
                                name: "login".to_string(),
                                error: "Login failed: Access denied.".to_string(),
                                duration: Duration::from_secs(0),
                            })
                            .await;
                        return;
                    }
                    _ => {}
                }
            }
        }

        // Max attempts reached
        tracing::error!("Login failed: Authentication timed out");
        let _ = tx
            .send(ToolEvent::Failed {
                id: "login".to_string(),
                name: "login".to_string(),
                error: "Login failed: Authentication timed out. Please try again.".to_string(),
                duration: Duration::from_secs(0),
            })
            .await;
    });
}

/// Handle logout command.
pub fn handle_logout() -> AuthResult {
    let Some(cortex_home) = get_cortex_home() else {
        return AuthResult::Error("Could not determine home directory.".to_string());
    };

    match logout_with_fallback(&cortex_home) {
        Ok(true) => AuthResult::LoggedOut,
        Ok(false) => AuthResult::NotLoggedIn,
        Err(e) => AuthResult::Error(format!("Error logging out: {}", e)),
    }
}

/// Load account information.
pub fn load_account_info() -> AuthResult {
    let Some(cortex_home) = get_cortex_home() else {
        return AuthResult::Error("Could not determine home directory.".to_string());
    };

    let auth = match load_auth(&cortex_home, CredentialsStoreMode::default()) {
        Ok(Some(auth)) => auth,
        Ok(None) => return AuthResult::NotLoggedIn,
        Err(e) => return AuthResult::Error(format!("Error loading credentials: {}", e)),
    };

    if auth.is_expired() {
        return AuthResult::Error("Session expired. Use /login to re-authenticate.".to_string());
    }

    let auth_method = match auth.mode {
        AuthMode::ApiKey => "API Key".to_string(),
        AuthMode::OAuth => "OAuth".to_string(),
    };

    let expires_at = auth.expires_at.and_then(|ts| {
        chrono::DateTime::from_timestamp(ts, 0)
            .map(|dt| dt.format("%Y-%m-%d %H:%M UTC").to_string())
    });

    let account_id = auth.account_id.clone();

    AuthResult::AccountInfo {
        auth_method,
        expires_at,
        account_id,
    }
}

/// Opens a URL in the default browser.
///
/// This function validates URLs for security (only http/https allowed).
pub fn open_browser_url(url: &str) -> Result<()> {
    let parsed_url = url::Url::parse(url)?;

    // Only allow HTTP and HTTPS URLs
    match parsed_url.scheme() {
        "http" | "https" => {}
        scheme => {
            anyhow::bail!(
                "Refusing to open URL with scheme '{}': only http and https are allowed",
                scheme
            );
        }
    }

    // Reject URLs with embedded credentials
    if !parsed_url.username().is_empty() || parsed_url.password().is_some() {
        anyhow::bail!("Refusing to open URL with embedded credentials");
    }

    // Try to open in browser
    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new("xdg-open").arg(url).spawn();
    }

    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(url).spawn();
    }

    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("cmd")
            .args(["/c", "start", "", url])
            .spawn();
    }

    Ok(())
}
