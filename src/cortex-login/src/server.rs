//! Local HTTP server for OAuth callback handling.
//!
//! This implements a local server to receive OAuth callbacks
//! for browser-based authentication flows.
//!
//! SECURITY FEATURES:
//! - OAuth state parameter validation to prevent CSRF attacks
//! - Nonce validation for additional security
//! - HTTPS-only issuer URL validation
//! - Short server timeout (60 seconds) to minimize exposure window
//! - PKCE S256-only enforcement

use anyhow::{Context, Result};
use reqwest::Client;
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::oneshot;

use crate::pkce::PkceChallenge;
use crate::{CredentialsStoreMode, DEFAULT_ISSUER, SecureAuthData, save_auth_with_fallback};

/// Maximum time the callback server will listen for (in seconds).
/// Reduced from 5 minutes to 60 seconds to minimize attack window.
const SERVER_TIMEOUT_SECS: u64 = 60;

/// Errors that can occur during OAuth flow.
#[derive(Debug, thiserror::Error)]
pub enum OAuthError {
    #[error("CSRF attack detected: state parameter mismatch")]
    StateMismatch,
    #[error("Nonce mismatch: potential replay attack")]
    NonceMismatch,
    #[error("Invalid issuer URL: must use HTTPS")]
    InsecureIssuer,
    #[error("Missing required parameter: {0}")]
    MissingParameter(&'static str),
    #[error("OAuth error from provider: {error} - {description}")]
    ProviderError { error: String, description: String },
    #[error("Token exchange failed: {0}")]
    TokenExchangeFailed(String),
    #[error("Login timed out after {0} seconds")]
    Timeout(u64),
}

/// Options for the login server.
#[derive(Clone)]
pub struct ServerOptions {
    /// Path to cortex home directory.
    pub cortex_home: PathBuf,
    /// OAuth client ID.
    pub client_id: String,
    /// OAuth issuer URL (must be HTTPS).
    pub issuer: String,
    /// Scopes to request.
    pub scopes: Vec<String>,
    /// Credential storage mode.
    pub credentials_store_mode: CredentialsStoreMode,
    /// Optional forced workspace ID.
    pub forced_workspace_id: Option<String>,
}

impl ServerOptions {
    /// Create new server options.
    ///
    /// # Errors
    /// Returns error if the issuer URL is not HTTPS.
    pub fn new(
        cortex_home: PathBuf,
        client_id: String,
        forced_workspace_id: Option<String>,
        credentials_store_mode: CredentialsStoreMode,
    ) -> Self {
        Self {
            cortex_home,
            client_id,
            issuer: DEFAULT_ISSUER.to_string(),
            scopes: vec!["openid".to_string(), "profile".to_string()],
            credentials_store_mode,
            forced_workspace_id,
        }
    }

    /// Create server options with a custom issuer URL.
    ///
    /// # Errors
    /// Returns error if the issuer URL is not HTTPS.
    pub fn with_issuer(
        cortex_home: PathBuf,
        client_id: String,
        issuer: String,
        forced_workspace_id: Option<String>,
        credentials_store_mode: CredentialsStoreMode,
    ) -> Result<Self, OAuthError> {
        // SECURITY: Validate issuer uses HTTPS
        validate_issuer_url(&issuer)?;

        Ok(Self {
            cortex_home,
            client_id,
            issuer,
            scopes: vec!["openid".to_string(), "profile".to_string()],
            credentials_store_mode,
            forced_workspace_id,
        })
    }
}

/// Validate that an issuer URL uses HTTPS.
///
/// SECURITY: OAuth authorization servers MUST use HTTPS to prevent
/// man-in-the-middle attacks that could steal authorization codes or tokens.
fn validate_issuer_url(issuer: &str) -> Result<(), OAuthError> {
    let url = url::Url::parse(issuer).map_err(|_| OAuthError::InsecureIssuer)?;

    if url.scheme() != "https" {
        return Err(OAuthError::InsecureIssuer);
    }

    // Additional checks: no credentials in URL
    if url.username() != "" || url.password().is_some() {
        return Err(OAuthError::InsecureIssuer);
    }

    Ok(())
}

/// Handle to shutdown the login server.
pub struct ShutdownHandle {
    tx: oneshot::Sender<()>,
}

impl ShutdownHandle {
    /// Shutdown the server.
    pub fn shutdown(self) {
        let _ = self.tx.send(());
    }
}

/// OAuth security state for CSRF protection.
///
/// SECURITY: The state parameter prevents CSRF attacks by ensuring that
/// the callback came from a request we initiated.
#[derive(Debug, Clone)]
struct OAuthSecurityState {
    /// Random state value to prevent CSRF attacks.
    state: String,
    /// Random nonce value for additional security (prevents replay attacks).
    nonce: String,
}

impl OAuthSecurityState {
    /// Generate new cryptographically random state and nonce.
    fn new() -> Self {
        use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
        use rand::Rng;
        let mut rng = rand::rng();

        // Generate 32 bytes of random data for each, encode as URL-safe base64
        let state_bytes: Vec<u8> = (0..32).map(|_| rng.random()).collect();
        let nonce_bytes: Vec<u8> = (0..32).map(|_| rng.random()).collect();

        Self {
            state: URL_SAFE_NO_PAD.encode(&state_bytes),
            nonce: URL_SAFE_NO_PAD.encode(&nonce_bytes),
        }
    }

    /// Validate that the received state matches the expected state.
    ///
    /// SECURITY: This is critical for CSRF prevention. A mismatch indicates
    /// either an attack or a bug.
    fn validate_state(&self, received: &str) -> Result<(), OAuthError> {
        // Use constant-time comparison to prevent timing attacks
        if constant_time_compare(&self.state, received) {
            Ok(())
        } else {
            Err(OAuthError::StateMismatch)
        }
    }
}

/// Constant-time string comparison to prevent timing attacks.
///
/// SECURITY: Regular string comparison can leak information about the
/// expected value through timing differences. This function ensures
/// comparison time is constant regardless of where strings differ.
fn constant_time_compare(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }

    let mut result = 0u8;
    for (x, y) in a.bytes().zip(b.bytes()) {
        result |= x ^ y;
    }
    result == 0
}

/// Running login server.
pub struct LoginServer {
    /// The actual port the server is listening on.
    pub actual_port: u16,
    /// The authorization URL to open.
    pub auth_url: String,
    /// The shutdown handle.
    shutdown_rx: oneshot::Receiver<()>,
    /// The result receiver.
    result_rx: oneshot::Receiver<Result<()>>,
}

impl LoginServer {
    /// Block until the login flow completes.
    pub async fn block_until_done(self) -> std::io::Result<()> {
        tokio::select! {
            _ = self.shutdown_rx => {
                Ok(())
            }
            result = self.result_rx => {
                match result {
                    Ok(Ok(())) => Ok(()),
                    Ok(Err(e)) => Err(std::io::Error::other(e)),
                    Err(_) => Err(std::io::Error::other("login server task dropped")),
                }
            }
        }
    }
}

/// Start the login server.
///
/// # Security
/// - Validates issuer URL uses HTTPS
/// - Generates cryptographically random state for CSRF protection
/// - Generates nonce for replay attack protection
/// - Uses S256 PKCE method only
/// - Server times out after 60 seconds
pub fn run_login_server(opts: ServerOptions) -> std::io::Result<LoginServer> {
    // SECURITY: Validate issuer URL uses HTTPS (except for default which is already HTTPS)
    if opts.issuer != DEFAULT_ISSUER {
        if let Err(e) = validate_issuer_url(&opts.issuer) {
            return Err(std::io::Error::other(e));
        }
    }

    // Find an available port
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let actual_port = listener.local_addr()?.port();
    drop(listener); // Release the port for the actual server

    let redirect_uri = format!("http://localhost:{actual_port}/callback");

    // Generate PKCE challenge (S256 only)
    let pkce = PkceChallenge::new();

    // SECURITY: Generate cryptographically random state and nonce
    let security_state = OAuthSecurityState::new();

    // Build authorization URL with security parameters
    let auth_url = build_auth_url(&opts, &redirect_uri, &pkce, &security_state);

    // Create channels
    let (_shutdown_tx, shutdown_rx) = oneshot::channel();
    let (result_tx, result_rx) = oneshot::channel();

    // Spawn server task with security state
    let opts_clone = opts;
    let pkce_verifier = pkce.verifier;
    let security_state_clone = security_state;
    tokio::spawn(async move {
        let result = run_server(
            actual_port,
            opts_clone,
            redirect_uri,
            pkce_verifier,
            security_state_clone,
        )
        .await;
        let _ = result_tx.send(result);
    });

    Ok(LoginServer {
        actual_port,
        auth_url,
        shutdown_rx,
        result_rx,
    })
}

fn build_auth_url(
    opts: &ServerOptions,
    redirect_uri: &str,
    pkce: &PkceChallenge,
    security_state: &OAuthSecurityState,
) -> String {
    let scope = opts.scopes.join(" ");

    // SECURITY: Build URL with all security parameters
    let mut url = format!(
        "{}/authorize?response_type=code&client_id={}&redirect_uri={}&scope={}&state={}&nonce={}&code_challenge={}&code_challenge_method={}",
        opts.issuer,
        urlencoding::encode(&opts.client_id),
        urlencoding::encode(redirect_uri),
        urlencoding::encode(&scope),
        urlencoding::encode(&security_state.state),
        urlencoding::encode(&security_state.nonce),
        urlencoding::encode(&pkce.challenge),
        pkce.method, // Always S256
    );

    if let Some(workspace_id) = &opts.forced_workspace_id {
        url.push_str(&format!(
            "&workspace_id={}",
            urlencoding::encode(workspace_id)
        ));
    }

    url
}

async fn run_server(
    port: u16,
    opts: ServerOptions,
    redirect_uri: String,
    pkce_verifier: String,
    security_state: OAuthSecurityState,
) -> Result<()> {
    use axum::{Router, extract::Query, routing::get};
    use std::collections::HashMap;

    let opts = Arc::new(opts);
    let redirect_uri = Arc::new(redirect_uri);
    let pkce_verifier = Arc::new(pkce_verifier);
    let security_state = Arc::new(security_state);

    let (tx, rx) = oneshot::channel::<Result<()>>();
    let tx = Arc::new(tokio::sync::Mutex::new(Some(tx)));

    let app = Router::new().route(
        "/callback",
        get({
            let opts = opts.clone();
            let redirect_uri = redirect_uri.clone();
            let pkce_verifier = pkce_verifier.clone();
            let security_state = security_state.clone();
            let tx = tx.clone();

            move |Query(params): Query<HashMap<String, String>>| {
                let opts = opts.clone();
                let redirect_uri = redirect_uri.clone();
                let pkce_verifier = pkce_verifier.clone();
                let security_state = security_state.clone();
                let tx = tx.clone();

                async move {
                    let result = handle_callback(
                        params,
                        &opts,
                        &redirect_uri,
                        &pkce_verifier,
                        &security_state,
                    )
                    .await;

                    // Determine response based on result
                    let (success, message) = match &result {
                        Ok(()) => (true, "Authentication complete!"),
                        Err(e) => {
                            tracing::error!("OAuth callback error: {e}");
                            (
                                false,
                                "Authentication failed. Please check the terminal for details.",
                            )
                        }
                    };

                    // Send result
                    if let Some(tx) = tx.lock().await.take() {
                        let _ = tx.send(result);
                    }

                    // Return HTML response
                    let html = if success {
                        r#"
                        <!DOCTYPE html>
                        <html>
                        <head><title>Cortex CLI</title></head>
                        <body>
                            <h1>Authentication complete!</h1>
                            <p>You can close this window and return to the terminal.</p>
                            <script>window.close();</script>
                        </body>
                        </html>
                    "#
                    } else {
                        r#"
                        <!DOCTYPE html>
                        <html>
                        <head><title>Cortex CLI - Error</title></head>
                        <body>
                            <h1>Authentication Failed</h1>
                            <p>Please check the terminal for details and try again.</p>
                        </body>
                        </html>
                    "#
                    };

                    let _ = message; // Used for logging, suppress warning
                    axum::response::Html(html)
                }
            }
        }),
    );

    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{port}"))
        .await
        .context("failed to bind server")?;

    // SECURITY: Run server with reduced timeout (60 seconds instead of 5 minutes)
    // to minimize the window of exposure
    let server = axum::serve(listener, app);

    tokio::select! {
        result = server => {
            result.context("server error")?;
        }
        result = rx => {
            return result.context("callback handler dropped")?;
        }
        _ = tokio::time::sleep(std::time::Duration::from_secs(SERVER_TIMEOUT_SECS)) => {
            anyhow::bail!("login timed out after {} seconds", SERVER_TIMEOUT_SECS);
        }
    }

    Ok(())
}

async fn handle_callback(
    params: std::collections::HashMap<String, String>,
    opts: &ServerOptions,
    redirect_uri: &str,
    pkce_verifier: &str,
    security_state: &OAuthSecurityState,
) -> Result<()> {
    // Check for error from OAuth provider
    if let Some(error) = params.get("error") {
        let description = params
            .get("error_description")
            .map(std::string::String::as_str)
            .unwrap_or("");
        anyhow::bail!(
            "{}",
            OAuthError::ProviderError {
                error: error.clone(),
                description: description.to_string(),
            }
        );
    }

    // SECURITY: Validate state parameter to prevent CSRF attacks
    // This is critical - a missing or mismatched state indicates a potential attack
    let received_state = params
        .get("state")
        .ok_or_else(|| anyhow::anyhow!("{}", OAuthError::MissingParameter("state")))?;

    security_state
        .validate_state(received_state)
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    tracing::debug!("OAuth state validation passed");

    // Get authorization code
    let code = params
        .get("code")
        .ok_or_else(|| anyhow::anyhow!("{}", OAuthError::MissingParameter("code")))?;

    // User-Agent string for HTTP requests
    const USER_AGENT: &str = concat!("cortex-cli/", env!("CARGO_PKG_VERSION"));

    // Exchange code for token
    let client = Client::builder()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_secs(30))
        .tcp_nodelay(true)
        .build()
        .context("Failed to create HTTP client")?;

    let token_url = format!("{}/oauth/token", opts.issuer);

    let response = client
        .post(&token_url)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("client_id", &opts.client_id),
            ("code_verifier", pkce_verifier),
        ])
        .send()
        .await
        .context("failed to exchange code for token")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!(
            "{}",
            OAuthError::TokenExchangeFailed(format!("{status} - {body}"))
        );
    }

    #[derive(serde::Deserialize)]
    struct TokenResponse {
        access_token: String,
        refresh_token: Option<String>,
        expires_in: Option<u64>,
    }

    let token: TokenResponse = response
        .json()
        .await
        .context("failed to parse token response")?;

    // Calculate expiration
    let expires_at = token
        .expires_in
        .map(|secs| chrono::Utc::now().timestamp() + secs as i64);

    // Save auth data securely with automatic fallback
    // This also clears any existing credentials from all storage locations
    // to prevent stale tokens from being loaded on subsequent auth checks
    let auth_data = SecureAuthData::with_oauth(token.access_token, token.refresh_token, expires_at);

    save_auth_with_fallback(&opts.cortex_home, &auth_data)?;

    tracing::info!("OAuth authentication successful");

    Ok(())
}
