//! OAuth callback server for handling OAuth 2.0 authorization code flow.
//!
//! This module provides a local HTTP server that listens for OAuth callbacks
//! and handles the authorization code exchange.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex, oneshot};
use tokio::time::timeout;
use tracing::{debug, error, info, warn};

use super::oauth::{
    OAUTH_CALLBACK_PATH, OAUTH_CALLBACK_PORT, OAuthFlow, OAuthStorage, OAuthTokens,
};
use crate::error::{CortexError, Result};

/// Callback result from OAuth flow.
#[derive(Debug, Clone)]
pub struct CallbackResult {
    /// Authorization code.
    pub code: String,
    /// State parameter (for CSRF validation).
    pub state: String,
}

/// OAuth callback server.
pub struct OAuthCallbackServer {
    /// Expected state for CSRF protection.
    expected_state: String,
    /// MCP server name.
    mcp_name: String,
    /// Timeout for waiting for callback.
    timeout_duration: Duration,
}

impl OAuthCallbackServer {
    /// Create a new callback server.
    pub fn new(mcp_name: impl Into<String>, expected_state: impl Into<String>) -> Self {
        Self {
            expected_state: expected_state.into(),
            mcp_name: mcp_name.into(),
            timeout_duration: Duration::from_secs(300), // 5 minute timeout
        }
    }

    /// Set timeout duration.
    pub fn with_timeout(mut self, duration: Duration) -> Self {
        self.timeout_duration = duration;
        self
    }

    /// Start the callback server and wait for the OAuth callback.
    ///
    /// Returns the authorization code when received, or an error on timeout/failure.
    pub async fn wait_for_callback(&self) -> Result<CallbackResult> {
        let addr = SocketAddr::from(([127, 0, 0, 1], OAUTH_CALLBACK_PORT));

        let listener = TcpListener::bind(addr).await.map_err(|e| {
            CortexError::mcp_error(format!(
                "Failed to bind callback server on port {OAUTH_CALLBACK_PORT}: {e}"
            ))
        })?;

        info!(
            mcp_name = %self.mcp_name,
            port = OAUTH_CALLBACK_PORT,
            "OAuth callback server started"
        );

        // Channel for receiving the callback result
        let (tx, rx) = oneshot::channel::<Result<CallbackResult>>();
        let tx = Arc::new(Mutex::new(Some(tx)));
        let expected_state = self.expected_state.clone();
        let mcp_name = self.mcp_name.clone();

        // Spawn connection handler
        let handle = tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, peer_addr)) => {
                        debug!(peer = %peer_addr, "Accepted connection");

                        let result =
                            Self::handle_connection(stream, &expected_state, &mcp_name).await;

                        // Send result through channel
                        if let Some(sender) = tx.lock().await.take() {
                            let _ = sender.send(result);
                            break;
                        }
                    }
                    Err(e) => {
                        warn!("Failed to accept connection: {e}");
                    }
                }
            }
        });

        // Wait for callback with timeout
        match timeout(self.timeout_duration, rx).await {
            Ok(Ok(result)) => {
                handle.abort();
                result
            }
            Ok(Err(_)) => {
                handle.abort();
                Err(CortexError::mcp_error(
                    "Callback channel closed unexpectedly",
                ))
            }
            Err(_) => {
                handle.abort();
                Err(CortexError::Timeout)
            }
        }
    }

    /// Handle an incoming HTTP connection.
    async fn handle_connection(
        mut stream: TcpStream,
        expected_state: &str,
        mcp_name: &str,
    ) -> Result<CallbackResult> {
        // Read the HTTP request
        let mut buffer = [0u8; 4096];
        let n = stream
            .read(&mut buffer)
            .await
            .map_err(|e| CortexError::mcp_error(format!("Failed to read from stream: {e}")))?;

        let request = String::from_utf8_lossy(&buffer[..n]);
        debug!(request = %request, "Received HTTP request");

        // Parse the request
        let result = Self::parse_callback_request(&request, expected_state, mcp_name);

        // Send HTTP response
        let response = match &result {
            Ok(_) => Self::success_response(),
            Err(e) => Self::error_response(&e.to_string()),
        };

        stream
            .write_all(response.as_bytes())
            .await
            .map_err(|e| CortexError::mcp_error(format!("Failed to write response: {e}")))?;
        stream.flush().await.ok();

        result
    }

    /// Parse the OAuth callback request.
    fn parse_callback_request(
        request: &str,
        expected_state: &str,
        mcp_name: &str,
    ) -> Result<CallbackResult> {
        // Parse the first line to get the path
        let first_line = request
            .lines()
            .next()
            .ok_or_else(|| CortexError::mcp_error("Empty request"))?;

        let parts: Vec<&str> = first_line.split_whitespace().collect();
        if parts.len() < 2 {
            return Err(CortexError::mcp_error("Invalid HTTP request"));
        }

        let method = parts[0];
        let path = parts[1];

        // Only handle GET requests
        if method != "GET" {
            return Err(CortexError::mcp_error(format!(
                "Unsupported HTTP method: {method}"
            )));
        }

        // Check if this is the callback path
        if !path.starts_with(OAUTH_CALLBACK_PATH) {
            return Err(CortexError::mcp_error(format!("Unexpected path: {path}")));
        }

        // Parse query parameters
        let query_string = path
            .split('?')
            .nth(1)
            .ok_or_else(|| CortexError::mcp_error("Missing query parameters"))?;

        let params = Self::parse_query_string(query_string);

        // Check for error response
        if let Some(error) = params.get("error") {
            let description = params
                .get("error_description")
                .map(|s| s.as_str())
                .unwrap_or("Unknown error");
            return Err(CortexError::Auth(format!(
                "OAuth error: {error} - {description}"
            )));
        }

        // Get authorization code
        let code = params
            .get("code")
            .ok_or_else(|| CortexError::mcp_error("Missing authorization code"))?
            .clone();

        // Get and validate state
        let state = params
            .get("state")
            .ok_or_else(|| CortexError::mcp_error("Missing state parameter"))?;

        // CSRF protection: validate state matches expected
        if state != expected_state {
            error!(
                mcp_name = %mcp_name,
                expected = %expected_state,
                received = %state,
                "State mismatch - possible CSRF attack"
            );
            return Err(CortexError::Auth(
                "State mismatch - possible CSRF attack. Please try again.".into(),
            ));
        }

        info!(mcp_name = %mcp_name, "OAuth callback received successfully");

        Ok(CallbackResult {
            code,
            state: state.clone(),
        })
    }

    /// Parse a query string into a HashMap.
    fn parse_query_string(query: &str) -> HashMap<String, String> {
        query
            .split('&')
            .filter_map(|pair| {
                let mut parts = pair.splitn(2, '=');
                let key = parts.next()?;
                let value = parts.next().unwrap_or("");
                // URL decode
                let key = urlencoding::decode(key).ok()?.into_owned();
                let value = urlencoding::decode(value).ok()?.into_owned();
                Some((key, value))
            })
            .collect()
    }

    /// Generate success HTML response.
    fn success_response() -> String {
        let body = r#"<!DOCTYPE html>
<html>
<head>
    <title>Cortex - OAuth Success</title>
    <style>
        body {
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, Oxygen, Ubuntu, sans-serif;
            display: flex;
            justify-content: center;
            align-items: center;
            height: 100vh;
            margin: 0;
            background: linear-gradient(135deg, #1a1a2e 0%, #16213e 100%);
            color: #e0e0e0;
        }
        .container {
            text-align: center;
            padding: 40px;
            background: rgba(255, 255, 255, 0.05);
            border-radius: 16px;
            backdrop-filter: blur(10px);
            border: 1px solid rgba(255, 255, 255, 0.1);
        }
        .success-icon {
            font-size: 64px;
            margin-bottom: 20px;
        }
        h1 {
            margin: 0 0 10px 0;
            color: #4ade80;
        }
        p {
            margin: 0;
            color: #a0a0a0;
        }
    </style>
</head>
<body>
    <div class="container">
        <div class="success-icon">✓</div>
        <h1>Authentication Successful!</h1>
        <p>You can close this window and return to Cortex.</p>
    </div>
</body>
</html>"#;

        format!(
            "HTTP/1.1 200 OK\r\n\
             Content-Type: text/html; charset=utf-8\r\n\
             Content-Length: {}\r\n\
             Connection: close\r\n\
             \r\n\
             {}",
            body.len(),
            body
        )
    }

    /// Generate error HTML response.
    fn error_response(error: &str) -> String {
        let escaped_error = error
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;");

        let body = format!(
            r#"<!DOCTYPE html>
<html>
<head>
    <title>Cortex - OAuth Error</title>
    <style>
        body {{
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, Oxygen, Ubuntu, sans-serif;
            display: flex;
            justify-content: center;
            align-items: center;
            height: 100vh;
            margin: 0;
            background: linear-gradient(135deg, #1a1a2e 0%, #16213e 100%);
            color: #e0e0e0;
        }}
        .container {{
            text-align: center;
            padding: 40px;
            background: rgba(255, 255, 255, 0.05);
            border-radius: 16px;
            backdrop-filter: blur(10px);
            border: 1px solid rgba(255, 255, 255, 0.1);
            max-width: 500px;
        }}
        .error-icon {{
            font-size: 64px;
            margin-bottom: 20px;
        }}
        h1 {{
            margin: 0 0 10px 0;
            color: #f87171;
        }}
        p {{
            margin: 0;
            color: #a0a0a0;
        }}
        .error-detail {{
            margin-top: 20px;
            padding: 15px;
            background: rgba(248, 113, 113, 0.1);
            border-radius: 8px;
            color: #fca5a5;
            font-family: monospace;
            font-size: 14px;
            word-break: break-word;
        }}
    </style>
</head>
<body>
    <div class="container">
        <div class="error-icon">✗</div>
        <h1>Authentication Failed</h1>
        <p>An error occurred during authentication.</p>
        <div class="error-detail">{}</div>
    </div>
</body>
</html>"#,
            escaped_error
        );

        format!(
            "HTTP/1.1 400 Bad Request\r\n\
             Content-Type: text/html; charset=utf-8\r\n\
             Content-Length: {}\r\n\
             Connection: close\r\n\
             \r\n\
             {}",
            body.len(),
            body
        )
    }
}

/// Run the complete OAuth flow for an MCP server.
///
/// This function:
/// 1. Discovers OAuth metadata from the server
/// 2. Builds the authorization URL
/// 3. Opens the browser for user authentication
/// 4. Waits for the OAuth callback
/// 5. Exchanges the authorization code for tokens
pub async fn run_oauth_flow(
    mcp_name: &str,
    server_url: &str,
    client_id: Option<&str>,
    client_secret: Option<&str>,
) -> Result<OAuthTokens> {
    let mut storage = OAuthStorage::load().await?;

    // Create OAuth flow handler
    let config = super::oauth::OAuthConfig {
        client_id: client_id.map(String::from),
        client_secret: client_secret.map(String::from),
        scope: None,
    };

    let flow = OAuthFlow::new(mcp_name, server_url).with_config(config);

    // Discover OAuth metadata
    info!(mcp_name = %mcp_name, "Discovering OAuth metadata...");
    let metadata = flow.discover_metadata().await?;

    // Build authorization URL
    let auth_url = flow
        .build_authorization_url(&metadata, &mut storage)
        .await?;

    // Get the state for CSRF validation
    let expected_state = storage
        .get_oauth_state(mcp_name)
        .ok_or_else(|| CortexError::internal("OAuth state not found"))?
        .to_string();

    // Open browser
    info!(mcp_name = %mcp_name, "Opening browser for authentication...");
    open_browser(&auth_url)?;

    // Start callback server and wait for callback
    let callback_server = OAuthCallbackServer::new(mcp_name, expected_state);
    let callback_result = callback_server.wait_for_callback().await?;

    // Exchange code for tokens
    info!(mcp_name = %mcp_name, "Exchanging authorization code for tokens...");
    let tokens = flow
        .exchange_code(&metadata, &callback_result.code, &mut storage)
        .await?;

    info!(mcp_name = %mcp_name, "OAuth flow completed successfully");
    Ok(tokens)
}

/// Open a URL in the default browser.
fn open_browser(url: &str) -> Result<()> {
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
            .spawn()
            .map_err(|e| CortexError::internal(format!("Failed to open browser: {e}")))?;
    }

    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(url)
            .spawn()
            .map_err(|e| CortexError::internal(format!("Failed to open browser: {e}")))?;
    }

    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(url)
            .spawn()
            .map_err(|e| CortexError::internal(format!("Failed to open browser: {e}")))?;
    }

    Ok(())
}

/// Refresh tokens if needed, running OAuth flow if refresh fails.
pub async fn ensure_valid_tokens(
    mcp_name: &str,
    server_url: &str,
    client_id: Option<&str>,
    client_secret: Option<&str>,
) -> Result<OAuthTokens> {
    let mut storage = OAuthStorage::load().await?;

    // Check if we have valid tokens
    if let Some(entry) = storage.get_for_url(mcp_name, server_url) {
        if let Some(tokens) = &entry.tokens {
            // Check if expired
            let now = chrono::Utc::now().timestamp();
            let is_expired = tokens.expires_at.map(|exp| exp < now).unwrap_or(false);

            if !is_expired {
                return Ok(tokens.clone());
            }

            // Try to refresh if we have a refresh token
            if tokens.refresh_token.is_some() {
                let config = super::oauth::OAuthConfig {
                    client_id: client_id.map(String::from),
                    client_secret: client_secret.map(String::from),
                    scope: None,
                };

                let flow = OAuthFlow::new(mcp_name, server_url).with_config(config);

                match flow.discover_metadata().await {
                    Ok(metadata) => match flow.refresh_tokens(&metadata, &mut storage).await {
                        Ok(tokens) => return Ok(tokens),
                        Err(e) => {
                            warn!(mcp_name = %mcp_name, error = %e, "Token refresh failed, will re-authenticate");
                        }
                    },
                    Err(e) => {
                        warn!(mcp_name = %mcp_name, error = %e, "Failed to discover metadata for refresh");
                    }
                }
            }
        }
    }

    // No valid tokens, run full OAuth flow
    run_oauth_flow(mcp_name, server_url, client_id, client_secret).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_query_string() {
        let query = "code=abc123&state=xyz789";
        let params = OAuthCallbackServer::parse_query_string(query);

        assert_eq!(params.get("code"), Some(&"abc123".to_string()));
        assert_eq!(params.get("state"), Some(&"xyz789".to_string()));
    }

    #[test]
    fn test_parse_query_string_with_encoding() {
        let query = "code=abc%20123&state=xyz%26789";
        let params = OAuthCallbackServer::parse_query_string(query);

        assert_eq!(params.get("code"), Some(&"abc 123".to_string()));
        assert_eq!(params.get("state"), Some(&"xyz&789".to_string()));
    }

    #[test]
    fn test_success_response() {
        let response = OAuthCallbackServer::success_response();
        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert!(response.contains("Authentication Successful"));
    }

    #[test]
    fn test_error_response() {
        let response = OAuthCallbackServer::error_response("Test error");
        assert!(response.starts_with("HTTP/1.1 400 Bad Request"));
        assert!(response.contains("Test error"));
    }

    #[test]
    fn test_error_response_escapes_html() {
        let response = OAuthCallbackServer::error_response("<script>alert('xss')</script>");
        assert!(!response.contains("<script>"));
        assert!(response.contains("&lt;script&gt;"));
    }
}
