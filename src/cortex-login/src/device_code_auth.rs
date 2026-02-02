//! Device code OAuth flow implementation.
//!
//! This implements the OAuth 2.0 Device Authorization Grant (RFC 8628)
//! which allows users to authenticate on a separate device.
//!
//! # Connection Reliability
//! The client includes connection warmup and retry logic to ensure reliable
//! authentication even on cold starts or flaky network conditions.

use anyhow::{Context, Result};
use reqwest::Client;
use serde::Deserialize;
use std::time::Duration;
use tracing::{debug, error, info, warn};

use crate::{CredentialsStoreMode, DEFAULT_ISSUER, SecureAuthData, save_auth_with_fallback};

/// User-Agent string for HTTP requests
const USER_AGENT: &str = concat!("cortex-cli/", env!("CARGO_PKG_VERSION"));

/// Default timeout for HTTP requests
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Maximum number of retries for initial device code request.
const DEVICE_CODE_MAX_RETRIES: u32 = 3;

/// Delay between device code request retries.
const DEVICE_CODE_RETRY_DELAY: Duration = Duration::from_millis(500);

/// Device code response from the authorization server.
#[derive(Debug, Deserialize)]
pub struct DeviceCodeResponse {
    /// The device verification code.
    pub device_code: String,
    /// The end-user verification code.
    pub user_code: String,
    /// The end-user verification URI.
    pub verification_uri: String,
    /// Optional verification URI with the user code embedded.
    pub verification_uri_complete: Option<String>,
    /// The lifetime in seconds of the device code.
    pub expires_in: u64,
    /// The minimum amount of time in seconds to wait between polling requests.
    #[serde(default = "default_interval")]
    pub interval: u64,
}

fn default_interval() -> u64 {
    5
}

/// Token response from the authorization server.
///
/// Handles both required and optional fields to support different server implementations.
/// The server may return `expires_in` as either `u32` or `u64`, and `refresh_token` may
/// or may not be present depending on the grant type.
#[derive(Debug, Deserialize)]
pub struct TokenResponse {
    /// The access token (JWT).
    pub access_token: String,
    /// The type of token (usually "Bearer").
    #[serde(default = "default_token_type")]
    pub token_type: String,
    /// The lifetime in seconds of the access token.
    /// Server may send this as u32 or u64, serde handles the conversion.
    #[serde(default)]
    pub expires_in: Option<u64>,
    /// The refresh token (may not be present for all grant types).
    #[serde(default)]
    pub refresh_token: Option<String>,
    /// The scope of the access token.
    #[serde(default)]
    pub scope: Option<String>,
}

fn default_token_type() -> String {
    "Bearer".to_string()
}

/// Error response during token polling.
#[derive(Debug, Deserialize)]
pub struct TokenErrorResponse {
    /// Error code.
    pub error: String,
    /// Error description.
    pub error_description: Option<String>,
}

/// Server options for device code login.
pub struct DeviceCodeOptions {
    /// The OAuth issuer URL.
    pub issuer: String,
    /// The client ID.
    pub client_id: String,
    /// The scopes to request.
    pub scopes: Vec<String>,
    /// Path to cortex home directory.
    pub cortex_home: std::path::PathBuf,
    /// Credential storage mode.
    pub credentials_store_mode: CredentialsStoreMode,
}

impl DeviceCodeOptions {
    /// Create new device code options with defaults.
    pub fn new(cortex_home: std::path::PathBuf, client_id: String) -> Self {
        Self {
            issuer: DEFAULT_ISSUER.to_string(),
            client_id,
            scopes: vec!["openid".to_string(), "profile".to_string()],
            cortex_home,
            credentials_store_mode: CredentialsStoreMode::default(),
        }
    }
}

/// Request payload for device code endpoint (matches server DeviceCodeRequest).
#[derive(Debug, serde::Serialize)]
struct DeviceCodeRequestPayload {
    /// Optional device fingerprint/machine ID
    #[serde(skip_serializing_if = "Option::is_none")]
    device_id: Option<String>,
    /// Optional device name (e.g., "MacBook Pro")
    #[serde(skip_serializing_if = "Option::is_none")]
    device_name: Option<String>,
    /// Requested scopes
    #[serde(skip_serializing_if = "Option::is_none")]
    scopes: Option<Vec<String>>,
}

/// Request device code with retry logic for transient failures.
///
/// Retries on connection errors, timeouts, and 5xx server errors.
/// Does not retry on 4xx client errors (those indicate configuration problems).
async fn request_device_code_with_retry(
    client: &reqwest::Client,
    url: &str,
    device_name: Option<&str>,
    scopes: &[String],
    max_retries: u32,
) -> Result<DeviceCodeResponse> {
    let mut last_error = None;

    let payload = DeviceCodeRequestPayload {
        device_id: None,
        device_name: device_name.map(String::from),
        scopes: if scopes.is_empty() {
            None
        } else {
            Some(scopes.to_vec())
        },
    };

    debug!(
        url = %url,
        device_name = ?device_name,
        scopes = ?scopes,
        "Requesting device code"
    );

    for attempt in 0..=max_retries {
        if attempt > 0 {
            debug!(
                attempt = attempt + 1,
                max_attempts = max_retries + 1,
                "Retrying device code request"
            );
            tokio::time::sleep(DEVICE_CODE_RETRY_DELAY).await;
        }

        match client.post(url).json(&payload).send().await {
            Ok(response) => {
                let status = response.status();

                if status.is_success() {
                    let body = response.text().await.unwrap_or_default();
                    debug!(
                        status = %status,
                        body_len = body.len(),
                        "Received device code response"
                    );
                    return serde_json::from_str::<DeviceCodeResponse>(&body).with_context(|| {
                        error!(
                            body = %body,
                            "Failed to parse device code response JSON"
                        );
                        "failed to parse device code response"
                    });
                }

                // 4xx errors are not retryable (client configuration issues)
                if status.is_client_error() {
                    let body = response.text().await.unwrap_or_default();
                    error!(
                        status = %status,
                        body = %body,
                        "Device code request failed with client error (not retrying)"
                    );
                    anyhow::bail!("device code request failed with status {status}: {body}");
                }

                // 5xx errors are retryable
                let body = response.text().await.unwrap_or_default();
                warn!(
                    status = %status,
                    body = %body,
                    attempt = attempt + 1,
                    "Device code request failed with server error (will retry)"
                );
                last_error = Some(format!("server error {status}: {body}"));
            }
            Err(e) => {
                // Network errors are retryable
                warn!(
                    error = %e,
                    attempt = attempt + 1,
                    "Device code request connection error (will retry)"
                );
                last_error = Some(format!("connection error: {e}"));
            }
        }
    }

    let err_msg = last_error.unwrap_or_else(|| "unknown error".to_string());
    error!(
        attempts = max_retries + 1,
        error = %err_msg,
        "Failed to request device code after all retry attempts"
    );
    anyhow::bail!(
        "failed to request device code after {} attempts: {}",
        max_retries + 1,
        err_msg
    )
}

/// Run the device code login flow.
///
/// # Security
/// - Validates issuer URL uses HTTPS
/// - Uses secure HTTP client with timeout
///
/// # Reliability
/// - Includes connection warmup to avoid cold-start failures
/// - Retries device code request up to 3 times on transient failures
pub async fn run_device_code_login(opts: DeviceCodeOptions) -> Result<()> {
    info!(
        issuer = %opts.issuer,
        client_id = %opts.client_id,
        scopes = ?opts.scopes,
        "Starting device code authentication flow"
    );

    // SECURITY: Validate issuer URL uses HTTPS
    let issuer_url = url::Url::parse(&opts.issuer).context("invalid issuer URL")?;
    if issuer_url.scheme() != "https" {
        error!(
            scheme = %issuer_url.scheme(),
            "Issuer URL must use HTTPS for security"
        );
        anyhow::bail!("issuer URL must use HTTPS for security");
    }

    // Create HTTP client with proper configuration
    let client = Client::builder()
        .user_agent(USER_AGENT)
        .timeout(DEFAULT_TIMEOUT)
        .tcp_nodelay(true)
        .build()
        .context("Failed to create HTTP client")?;

    // Connection warmup: Perform a lightweight request to establish the connection pool.
    // This prevents "connection failed" errors on the first actual request.
    let warmup_url = format!("{}/.well-known/openid-configuration", opts.issuer);
    debug!(url = %warmup_url, "Performing connection warmup");
    let _ = client
        .get(&warmup_url)
        .timeout(Duration::from_secs(5))
        .send()
        .await;
    // Ignore warmup errors - the server may not support this endpoint

    // Step 1: Request device code with retry logic
    let device_auth_url = format!("{}/auth/device/code", opts.issuer);

    // Get device name from hostname
    let device_name = hostname::get().ok().and_then(|h| h.into_string().ok());
    debug!(device_name = ?device_name, "Retrieved device hostname");

    let device_code = request_device_code_with_retry(
        &client,
        &device_auth_url,
        device_name.as_deref(),
        &opts.scopes,
        DEVICE_CODE_MAX_RETRIES,
    )
    .await?;

    info!(
        user_code = %device_code.user_code,
        expires_in_secs = device_code.expires_in,
        poll_interval_secs = device_code.interval,
        "Device code obtained successfully"
    );

    // Step 2: Display user instructions
    // Always show the URL first, so headless environments can still authenticate
    eprintln!("\nTo authenticate, visit:");
    eprintln!("\n  {}", device_code.verification_uri);
    eprintln!("\nAnd enter code: {}", device_code.user_code);

    if let Some(complete_uri) = &device_code.verification_uri_complete {
        eprintln!("\nOr open this link directly:");
        eprintln!("  {complete_uri}");

        // Check if we're in a headless environment
        let is_headless = is_headless_environment();

        if is_headless {
            eprintln!(
                "\n(Headless environment detected - please open the URL in a browser on another device)"
            );
            info!(
                is_headless = true,
                "Headless environment detected, skipping browser open"
            );
        } else {
            // Try to open browser
            if let Err(e) = webbrowser_open(complete_uri) {
                debug!(error = %e, "Failed to open browser automatically");
                eprintln!(
                    "\n(Could not open browser automatically - please open the URL manually)"
                );
            } else {
                eprintln!("\n(Opening browser...)");
            }
        }
    } else {
        eprintln!("\n(Please open the URL above in your browser)");
    }

    eprintln!("\nWaiting for authentication...");

    // Step 3: Poll for token
    let token_url = format!("{}/auth/device/token", opts.issuer);
    let interval = Duration::from_secs(device_code.interval);
    let expires_at = std::time::Instant::now() + Duration::from_secs(device_code.expires_in);
    let mut poll_count: u32 = 0;

    loop {
        if std::time::Instant::now() > expires_at {
            error!(
                poll_count = poll_count,
                "Device code expired before authorization"
            );
            anyhow::bail!("device code expired");
        }

        tokio::time::sleep(interval).await;
        poll_count += 1;

        debug!(
            poll_count = poll_count,
            url = %token_url,
            "Polling for token"
        );

        let response = match client
            .post(&token_url)
            .json(&serde_json::json!({
                "device_code": device_code.device_code
            }))
            .send()
            .await
        {
            Ok(resp) => resp,
            Err(e) => {
                warn!(
                    error = %e,
                    poll_count = poll_count,
                    "Token poll request failed, will retry"
                );
                continue;
            }
        };

        let status = response.status();
        let body = response.text().await.unwrap_or_default();

        debug!(
            status = %status,
            body_len = body.len(),
            poll_count = poll_count,
            "Token poll response received"
        );

        if status.is_success() {
            debug!(
                body = %body,
                "Parsing successful token response"
            );

            let token: TokenResponse = match serde_json::from_str(&body) {
                Ok(t) => t,
                Err(e) => {
                    error!(
                        error = %e,
                        body = %body,
                        "Failed to parse token response JSON"
                    );
                    anyhow::bail!("failed to parse token response: {e}");
                }
            };

            info!(
                token_type = %token.token_type,
                expires_in = ?token.expires_in,
                has_refresh_token = token.refresh_token.is_some(),
                scope = ?token.scope,
                "Token received successfully"
            );

            // Calculate expiration
            let expires_at = token
                .expires_in
                .map(|secs| chrono::Utc::now().timestamp() + secs as i64);

            // Save auth data securely with automatic fallback
            let auth_data =
                SecureAuthData::with_oauth(token.access_token, token.refresh_token, expires_at);

            match save_auth_with_fallback(&opts.cortex_home, &auth_data) {
                Ok(mode) => {
                    match mode {
                        CredentialsStoreMode::Keyring => {
                            info!("Authentication credentials saved to system keyring");
                        }
                        CredentialsStoreMode::EncryptedFile => {
                            info!(
                                "Authentication credentials saved to encrypted file (keyring unavailable)"
                            );
                        }
                        CredentialsStoreMode::File => {
                            info!("Authentication credentials saved to legacy file");
                        }
                    }
                    return Ok(());
                }
                Err(e) => {
                    error!(
                        error = %e,
                        "Failed to save authentication credentials"
                    );
                    return Err(e);
                }
            }
        }

        // Check for expected polling errors
        if let Ok(error_resp) = serde_json::from_str::<TokenErrorResponse>(&body) {
            match error_resp.error.as_str() {
                "authorization_pending" => {
                    debug!(
                        poll_count = poll_count,
                        "Authorization still pending, continuing to poll"
                    );
                    continue;
                }
                "slow_down" => {
                    debug!(
                        poll_count = poll_count,
                        "Server requested slow down, adding extra delay"
                    );
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    continue;
                }
                "expired_token" => {
                    error!(
                        poll_count = poll_count,
                        "Device code expired according to server"
                    );
                    anyhow::bail!("device code expired");
                }
                "access_denied" => {
                    error!(
                        poll_count = poll_count,
                        description = ?error_resp.error_description,
                        "Access denied by user"
                    );
                    anyhow::bail!("access denied by user");
                }
                _ => {
                    let desc = error_resp.error_description.clone().unwrap_or_default();
                    error!(
                        error_code = %error_resp.error,
                        error_description = %desc,
                        poll_count = poll_count,
                        "Token exchange failed with error"
                    );
                    anyhow::bail!("token error: {} - {}", error_resp.error, desc);
                }
            }
        }

        // Unknown error - try to parse as JSON for better diagnostics
        error!(
            status = %status,
            body = %body,
            poll_count = poll_count,
            "Unexpected token response (not a recognized error format)"
        );
        anyhow::bail!("unexpected token response: {status} - {body}");
    }
}

/// Check if we're running in a headless environment (no display/browser available).
fn is_headless_environment() -> bool {
    // Check for SSH session without X forwarding
    if std::env::var("SSH_CLIENT").is_ok() || std::env::var("SSH_TTY").is_ok() {
        // Check if DISPLAY is set (X11 forwarding)
        if std::env::var("DISPLAY").is_err() {
            return true;
        }
    }

    // Check for common headless/container environments
    if std::env::var("DISPLAY").is_err() {
        #[cfg(target_os = "linux")]
        {
            // On Linux, no DISPLAY usually means headless
            // Unless we're on Wayland
            if std::env::var("WAYLAND_DISPLAY").is_err() {
                return true;
            }
        }
    }

    // Check for Docker/container environment
    if std::path::Path::new("/.dockerenv").exists() {
        return true;
    }

    // Check for CI environment
    if std::env::var("CI").is_ok() || std::env::var("GITHUB_ACTIONS").is_ok() {
        return true;
    }

    false
}

/// Try to open a URL in the default browser.
///
/// # Security
/// - Validates URL is HTTP or HTTPS only
/// - Uses proper escaping to prevent shell injection
/// - Rejects URLs with potentially dangerous characters
fn webbrowser_open(url: &str) -> Result<()> {
    // SECURITY: Parse and validate the URL
    let parsed_url = url::Url::parse(url).context("invalid URL")?;

    // SECURITY: Only allow HTTP and HTTPS URLs
    match parsed_url.scheme() {
        "http" | "https" => {}
        scheme => {
            anyhow::bail!(
                "refusing to open URL with scheme '{scheme}': only http and https are allowed"
            );
        }
    }

    // SECURITY: Reject URLs with embedded credentials
    if !parsed_url.username().is_empty() || parsed_url.password().is_some() {
        anyhow::bail!("refusing to open URL with embedded credentials");
    }

    // SECURITY: Validate there are no shell metacharacters in the URL
    // This is defense in depth since we pass as argument, not through shell
    const DANGEROUS_CHARS: &[char] = &[
        '`', '$', '|', ';', '&', '<', '>', '(', ')', '{', '}', '[', ']', '!', '\n', '\r',
    ];
    if url.chars().any(|c| DANGEROUS_CHARS.contains(&c)) {
        anyhow::bail!("URL contains potentially dangerous characters");
    }

    // Use the validated and normalized URL string
    let safe_url = parsed_url.as_str();

    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg("--") // End of options
            .arg(safe_url)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .context("failed to open browser")?;
    }

    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(safe_url)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .context("failed to open browser")?;
    }

    #[cfg(target_os = "windows")]
    {
        // On Windows, use cmd /C start to open URLs in default browser
        // The empty string argument after "start" is the window title (required)
        std::process::Command::new("cmd")
            .args(["/C", "start", "", safe_url])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .context("failed to open browser")?;
    }

    Ok(())
}
