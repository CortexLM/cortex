//! OAuth flow for Slack workspace installation.
//!
//! Provides HTTP routes for:
//! - `/slack/oauth/authorize` - Initiate OAuth flow
//! - `/slack/oauth/callback` - Handle OAuth callback
//!
//! # OAuth Scopes
//!
//! The following bot scopes are required:
//! - `app_mentions:read` - Read @mentions
//! - `channels:history` - Read channel messages
//! - `chat:write` - Send messages
//! - `commands` - Handle slash commands
//! - `im:history` - Read direct messages
//! - `im:write` - Send direct messages

use axum::{
    Router,
    extract::{Query, State},
    response::{Html, IntoResponse, Redirect, Response},
    routing::get,
};
use serde::{Deserialize, Serialize};
use tracing::{debug, error, info};

use crate::{
    config::SlackConfig,
    error::{SlackError, SlackResult},
};

/// Required OAuth scopes for the Slack bot.
pub const REQUIRED_SCOPES: &[&str] = &[
    "app_mentions:read",
    "channels:history",
    "chat:write",
    "commands",
    "im:history",
    "im:write",
];

/// OAuth state for the router.
#[derive(Clone)]
pub struct OAuthState {
    /// Client ID for OAuth.
    pub client_id: String,
    /// Client secret for OAuth.
    pub client_secret: String,
    /// Redirect URI after OAuth.
    pub redirect_uri: String,
    /// Optional callback for storing tokens.
    pub token_callback: Option<TokenCallback>,
}

/// Callback function type for storing OAuth tokens.
pub type TokenCallback = std::sync::Arc<
    dyn Fn(
            OAuthTokenResponse,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = SlackResult<()>> + Send>>
        + Send
        + Sync,
>;

impl OAuthState {
    /// Create new OAuth state from config.
    pub fn from_config(config: &SlackConfig) -> SlackResult<Self> {
        let client_id = config
            .client_id()
            .ok_or_else(|| SlackError::Config("OAuth client_id not configured".to_string()))?
            .to_string();

        let client_secret = config
            .client_secret()
            .ok_or_else(|| SlackError::Config("OAuth client_secret not configured".to_string()))?
            .to_string();

        let redirect_uri = config
            .redirect_uri()
            .ok_or_else(|| SlackError::Config("OAuth redirect_uri not configured".to_string()))?
            .to_string();

        Ok(Self {
            client_id,
            client_secret,
            redirect_uri,
            token_callback: None,
        })
    }

    /// Set a callback for storing tokens after successful OAuth.
    pub fn with_token_callback<F, Fut>(mut self, callback: F) -> Self
    where
        F: Fn(OAuthTokenResponse) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = SlackResult<()>> + Send + 'static,
    {
        self.token_callback = Some(std::sync::Arc::new(move |token| {
            Box::pin(callback(token))
                as std::pin::Pin<Box<dyn std::future::Future<Output = SlackResult<()>> + Send>>
        }));
        self
    }
}

/// Query parameters for OAuth callback.
#[derive(Debug, Deserialize)]
pub struct OAuthCallbackParams {
    /// Authorization code from Slack.
    pub code: Option<String>,
    /// Error from Slack (if authorization failed).
    pub error: Option<String>,
    /// State parameter (for CSRF protection).
    pub state: Option<String>,
}

/// Response from Slack OAuth token exchange.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthTokenResponse {
    /// Whether the request was successful.
    pub ok: bool,
    /// Access token for the bot.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub access_token: Option<String>,
    /// Token type (usually "bot").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_type: Option<String>,
    /// Scopes granted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// Bot user ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bot_user_id: Option<String>,
    /// App ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_id: Option<String>,
    /// Team information.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub team: Option<OAuthTeam>,
    /// Authed user information.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authed_user: Option<OAuthAuthedUser>,
    /// Error message (if ok is false).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Team information from OAuth response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthTeam {
    /// Team ID.
    pub id: String,
    /// Team name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Authed user information from OAuth response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthAuthedUser {
    /// User ID.
    pub id: String,
    /// User's access token (if user scopes requested).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub access_token: Option<String>,
    /// User's token type.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_type: Option<String>,
    /// User's granted scopes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
}

/// Create OAuth routes for Slack.
pub fn oauth_routes(state: OAuthState) -> Router {
    Router::new()
        .route("/slack/oauth/authorize", get(authorize))
        .route("/slack/oauth/callback", get(callback))
        .with_state(state)
}

/// Initiate OAuth flow - redirects to Slack's authorization page.
async fn authorize(State(state): State<OAuthState>) -> Response {
    let scopes = REQUIRED_SCOPES.join(",");
    let url = format!(
        "https://slack.com/oauth/v2/authorize?client_id={}&scope={}&redirect_uri={}",
        urlencoding::encode(&state.client_id),
        urlencoding::encode(&scopes),
        urlencoding::encode(&state.redirect_uri),
    );

    debug!("Initiating OAuth flow, redirecting to: {}", url);
    Redirect::temporary(&url).into_response()
}

/// Handle OAuth callback from Slack.
async fn callback(
    State(state): State<OAuthState>,
    Query(params): Query<OAuthCallbackParams>,
) -> Response {
    // Check for errors
    if let Some(error) = params.error {
        error!("OAuth error from Slack: {}", error);
        return Html(format!(
            r#"<!DOCTYPE html>
<html>
<head><title>Cortex - Installation Failed</title></head>
<body>
<h1>❌ Installation Failed</h1>
<p>Error: {}</p>
<p><a href="javascript:window.close()">Close this window</a></p>
</body>
</html>"#,
            error
        ))
        .into_response();
    }

    // Get authorization code
    let code = match params.code {
        Some(code) => code,
        None => {
            error!("OAuth callback missing code parameter");
            return Html(
                r#"<!DOCTYPE html>
<html>
<head><title>Cortex - Installation Failed</title></head>
<body>
<h1>❌ Installation Failed</h1>
<p>Missing authorization code.</p>
<p><a href="javascript:window.close()">Close this window</a></p>
</body>
</html>"#,
            )
            .into_response();
        }
    };

    // Exchange code for token
    match exchange_code(&state, &code).await {
        Ok(token_response) => {
            if !token_response.ok {
                let error = token_response
                    .error
                    .unwrap_or_else(|| "Unknown error".to_string());
                error!("Token exchange failed: {}", error);
                return Html(format!(
                    r#"<!DOCTYPE html>
<html>
<head><title>Cortex - Installation Failed</title></head>
<body>
<h1>❌ Installation Failed</h1>
<p>Token exchange error: {}</p>
<p><a href="javascript:window.close()">Close this window</a></p>
</body>
</html>"#,
                    error
                ))
                .into_response();
            }

            let team_name = token_response
                .team
                .as_ref()
                .and_then(|t| t.name.clone())
                .unwrap_or_else(|| "your workspace".to_string());

            info!(
                "OAuth successful for team: {} ({})",
                team_name,
                token_response
                    .team
                    .as_ref()
                    .map(|t| t.id.as_str())
                    .unwrap_or("unknown")
            );

            // Call token callback if set
            if let Some(callback) = &state.token_callback
                && let Err(e) = callback(token_response.clone()).await
            {
                error!("Token callback failed: {}", e);
                return Html(format!(
                    r#"<!DOCTYPE html>
<html>
<head><title>Cortex - Installation Warning</title></head>
<body>
<h1>⚠️ Installation Partially Complete</h1>
<p>Cortex was installed to {}, but there was an error saving the configuration: {}</p>
<p>You may need to reinstall or manually configure the bot.</p>
<p><a href="javascript:window.close()">Close this window</a></p>
</body>
</html>"#,
                    team_name, e
                ))
                .into_response();
            }

            Html(format!(
                r#"<!DOCTYPE html>
<html>
<head>
<title>Cortex - Installation Successful</title>
<style>
body {{ font-family: -apple-system, BlinkMacSystemFont, sans-serif; text-align: center; padding: 50px; }}
h1 {{ color: #2eb67d; }}
</style>
</head>
<body>
<h1>✅ Cortex Installed Successfully!</h1>
<p>Cortex has been installed to <strong>{}</strong>.</p>
<p>You can now:</p>
<ul style="list-style: none; padding: 0;">
<li>📢 Mention <code>@cortex</code> in any channel</li>
<li>💬 Send a direct message to Cortex</li>
<li>⚡ Use <code>/cortex</code> slash commands</li>
</ul>
<p><a href="javascript:window.close()">Close this window</a></p>
</body>
</html>"#,
                team_name
            ))
            .into_response()
        }
        Err(e) => {
            error!("Token exchange failed: {}", e);
            Html(format!(
                r#"<!DOCTYPE html>
<html>
<head><title>Cortex - Installation Failed</title></head>
<body>
<h1>❌ Installation Failed</h1>
<p>Error exchanging authorization code: {}</p>
<p><a href="javascript:window.close()">Close this window</a></p>
</body>
</html>"#,
                e
            ))
            .into_response()
        }
    }
}

/// Exchange authorization code for access token.
async fn exchange_code(state: &OAuthState, code: &str) -> SlackResult<OAuthTokenResponse> {
    let client = reqwest::Client::new();

    let response = client
        .post("https://slack.com/api/oauth.v2.access")
        .form(&[
            ("client_id", state.client_id.as_str()),
            ("client_secret", state.client_secret.as_str()),
            ("code", code),
            ("redirect_uri", state.redirect_uri.as_str()),
        ])
        .send()
        .await?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(SlackError::Api(format!(
            "Token exchange failed with status {}: {}",
            status, body
        )));
    }

    let token_response: OAuthTokenResponse = response.json().await?;
    Ok(token_response)
}

/// Generate the "Add to Slack" button HTML.
pub fn add_to_slack_button(client_id: &str, scopes: Option<&[&str]>) -> String {
    let scopes = scopes.unwrap_or(REQUIRED_SCOPES);
    let scope_str = scopes.join(",");

    format!(
        r#"<a href="https://slack.com/oauth/v2/authorize?client_id={}&scope={}&user_scope=">
<img alt="Add to Slack" height="40" width="139" src="https://platform.slack-edge.com/img/add_to_slack.png" 
srcSet="https://platform.slack-edge.com/img/add_to_slack.png 1x, https://platform.slack-edge.com/img/add_to_slack@2x.png 2x" />
</a>"#,
        urlencoding::encode(client_id),
        urlencoding::encode(&scope_str),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_required_scopes() {
        assert!(REQUIRED_SCOPES.contains(&"app_mentions:read"));
        assert!(REQUIRED_SCOPES.contains(&"chat:write"));
        assert!(REQUIRED_SCOPES.contains(&"commands"));
    }

    #[test]
    fn test_oauth_state_from_config() {
        let config = SlackConfig::new("xoxb-token", "xapp-token", "secret").with_oauth(
            "client-id",
            "client-secret",
            Some("https://example.com/callback".to_string()),
        );

        let state = OAuthState::from_config(&config).unwrap();
        assert_eq!(state.client_id, "client-id");
        assert_eq!(state.client_secret, "client-secret");
        assert_eq!(state.redirect_uri, "https://example.com/callback");
    }

    #[test]
    fn test_oauth_state_missing_config() {
        let config = SlackConfig::new("xoxb-token", "xapp-token", "secret");
        // No OAuth configured

        let result = OAuthState::from_config(&config);
        assert!(result.is_err());
    }

    #[test]
    fn test_add_to_slack_button() {
        let html = add_to_slack_button("test-client-id", None);
        assert!(html.contains("test-client-id"));
        assert!(html.contains("Add to Slack"));
        // URL encoding converts : to %3A
        assert!(html.contains("app_mentions%3Aread") || html.contains("app_mentions:read"));
    }

    #[test]
    fn test_oauth_token_response_serialization() {
        let response = OAuthTokenResponse {
            ok: true,
            access_token: Some("xoxb-test".to_string()),
            token_type: Some("bot".to_string()),
            scope: Some("chat:write".to_string()),
            bot_user_id: Some("U12345".to_string()),
            app_id: Some("A12345".to_string()),
            team: Some(OAuthTeam {
                id: "T12345".to_string(),
                name: Some("Test Team".to_string()),
            }),
            authed_user: None,
            error: None,
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("xoxb-test"));
        assert!(json.contains("Test Team"));
    }

    #[test]
    fn test_oauth_token_response_error() {
        let json = r#"{"ok": false, "error": "invalid_code"}"#;
        let response: OAuthTokenResponse = serde_json::from_str(json).unwrap();

        assert!(!response.ok);
        assert_eq!(response.error, Some("invalid_code".to_string()));
        assert!(response.access_token.is_none());
    }
}
