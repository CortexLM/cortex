//! Configuration for Slack integration.
//!
//! Supports loading configuration from:
//! - Environment variables
//! - OS keyring (secure credential storage)
//! - Configuration files

use cortex_keyring_store::KeyringStore;
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::error::{SlackError, SlackResult};

/// Keyring service name for Slack credentials.
const SLACK_KEYRING_SERVICE: &str = "cortex-slack";

/// Keyring keys for different credentials.
const KEY_BOT_TOKEN: &str = "bot_token";
const KEY_APP_TOKEN: &str = "app_token";
const KEY_SIGNING_SECRET: &str = "signing_secret";
const KEY_CLIENT_ID: &str = "client_id";
const KEY_CLIENT_SECRET: &str = "client_secret";

/// Configuration for Slack integration.
#[derive(Clone)]
pub struct SlackConfig {
    /// Bot OAuth token (xoxb-...).
    bot_token: SecretString,
    /// App-level token for Socket Mode (xapp-...).
    app_token: SecretString,
    /// Signing secret for request verification.
    signing_secret: SecretString,
    /// OAuth client ID (optional, for OAuth flow).
    client_id: Option<String>,
    /// OAuth client secret (optional, for OAuth flow).
    client_secret: Option<SecretString>,
    /// Redirect URI for OAuth (optional).
    redirect_uri: Option<String>,
}

impl std::fmt::Debug for SlackConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SlackConfig")
            .field("bot_token", &"[REDACTED]")
            .field("app_token", &"[REDACTED]")
            .field("signing_secret", &"[REDACTED]")
            .field("client_id", &self.client_id)
            .field(
                "client_secret",
                &self.client_secret.as_ref().map(|_| "[REDACTED]"),
            )
            .field("redirect_uri", &self.redirect_uri)
            .finish()
    }
}

impl SlackConfig {
    /// Create a new configuration with required tokens.
    pub fn new(
        bot_token: impl Into<String>,
        app_token: impl Into<String>,
        signing_secret: impl Into<String>,
    ) -> Self {
        Self {
            bot_token: SecretString::new(bot_token.into().into()),
            app_token: SecretString::new(app_token.into().into()),
            signing_secret: SecretString::new(signing_secret.into().into()),
            client_id: None,
            client_secret: None,
            redirect_uri: None,
        }
    }

    /// Set OAuth client credentials.
    pub fn with_oauth(
        mut self,
        client_id: impl Into<String>,
        client_secret: impl Into<String>,
        redirect_uri: Option<String>,
    ) -> Self {
        self.client_id = Some(client_id.into());
        self.client_secret = Some(SecretString::new(client_secret.into().into()));
        self.redirect_uri = redirect_uri;
        self
    }

    /// Load configuration from environment variables.
    ///
    /// Required variables:
    /// - `SLACK_BOT_TOKEN`
    /// - `SLACK_APP_TOKEN`
    /// - `SLACK_SIGNING_SECRET`
    ///
    /// Optional variables:
    /// - `SLACK_CLIENT_ID`
    /// - `SLACK_CLIENT_SECRET`
    /// - `SLACK_REDIRECT_URI`
    pub fn from_env() -> SlackResult<Self> {
        let bot_token = std::env::var("SLACK_BOT_TOKEN")
            .map_err(|_| SlackError::Config("SLACK_BOT_TOKEN not set".to_string()))?;

        let app_token = std::env::var("SLACK_APP_TOKEN")
            .map_err(|_| SlackError::Config("SLACK_APP_TOKEN not set".to_string()))?;

        let signing_secret = std::env::var("SLACK_SIGNING_SECRET")
            .map_err(|_| SlackError::Config("SLACK_SIGNING_SECRET not set".to_string()))?;

        // Validate token formats
        if !bot_token.starts_with("xoxb-") {
            warn!("Bot token doesn't start with 'xoxb-', this may be incorrect");
        }
        if !app_token.starts_with("xapp-") {
            warn!("App token doesn't start with 'xapp-', this may be incorrect");
        }

        let mut config = Self::new(bot_token, app_token, signing_secret);

        // Optional OAuth credentials
        if let Ok(client_id) = std::env::var("SLACK_CLIENT_ID")
            && let Ok(client_secret) = std::env::var("SLACK_CLIENT_SECRET")
        {
            let redirect_uri = std::env::var("SLACK_REDIRECT_URI").ok();
            config = config.with_oauth(client_id, client_secret, redirect_uri);
        }

        Ok(config)
    }

    /// Load configuration from OS keyring.
    ///
    /// Uses the system's secure credential storage (Keychain on macOS,
    /// Credential Manager on Windows, Secret Service on Linux).
    pub fn from_keyring() -> SlackResult<Self> {
        let store = KeyringStore::with_service(SLACK_KEYRING_SERVICE);

        let bot_token = store
            .get(KEY_BOT_TOKEN)?
            .ok_or_else(|| SlackError::Config("Bot token not found in keyring".to_string()))?;

        let app_token = store
            .get(KEY_APP_TOKEN)?
            .ok_or_else(|| SlackError::Config("App token not found in keyring".to_string()))?;

        let signing_secret = store
            .get(KEY_SIGNING_SECRET)?
            .ok_or_else(|| SlackError::Config("Signing secret not found in keyring".to_string()))?;

        let mut config = Self::new(bot_token, app_token, signing_secret);

        // Optional OAuth credentials
        if let Ok(Some(client_id)) = store.get(KEY_CLIENT_ID)
            && let Ok(Some(client_secret)) = store.get(KEY_CLIENT_SECRET)
        {
            config = config.with_oauth(client_id, client_secret, None);
        }

        debug!("Loaded Slack config from keyring");
        Ok(config)
    }

    /// Load configuration, trying keyring first, then environment.
    pub fn load() -> SlackResult<Self> {
        // Try keyring first
        match Self::from_keyring() {
            Ok(config) => {
                debug!("Loaded Slack config from keyring");
                return Ok(config);
            }
            Err(e) => {
                debug!("Could not load from keyring: {}, trying environment", e);
            }
        }

        // Fall back to environment
        Self::from_env()
    }

    /// Save configuration to OS keyring.
    pub fn save_to_keyring(&self) -> SlackResult<()> {
        let store = KeyringStore::with_service(SLACK_KEYRING_SERVICE);

        store.set(KEY_BOT_TOKEN, self.bot_token.expose_secret())?;
        store.set(KEY_APP_TOKEN, self.app_token.expose_secret())?;
        store.set(KEY_SIGNING_SECRET, self.signing_secret.expose_secret())?;

        if let Some(ref client_id) = self.client_id {
            store.set(KEY_CLIENT_ID, client_id)?;
        }
        if let Some(ref client_secret) = self.client_secret {
            store.set(KEY_CLIENT_SECRET, client_secret.expose_secret())?;
        }

        debug!("Saved Slack config to keyring");
        Ok(())
    }

    /// Clear configuration from OS keyring.
    pub fn clear_keyring() -> SlackResult<()> {
        let store = KeyringStore::with_service(SLACK_KEYRING_SERVICE);

        let _ = store.delete(KEY_BOT_TOKEN);
        let _ = store.delete(KEY_APP_TOKEN);
        let _ = store.delete(KEY_SIGNING_SECRET);
        let _ = store.delete(KEY_CLIENT_ID);
        let _ = store.delete(KEY_CLIENT_SECRET);

        debug!("Cleared Slack config from keyring");
        Ok(())
    }

    /// Get the bot token.
    pub fn bot_token(&self) -> &str {
        self.bot_token.expose_secret()
    }

    /// Get the app token for Socket Mode.
    pub fn app_token(&self) -> &str {
        self.app_token.expose_secret()
    }

    /// Get the signing secret.
    pub fn signing_secret(&self) -> &str {
        self.signing_secret.expose_secret()
    }

    /// Get the OAuth client ID.
    pub fn client_id(&self) -> Option<&str> {
        self.client_id.as_deref()
    }

    /// Get the OAuth client secret.
    pub fn client_secret(&self) -> Option<&str> {
        self.client_secret.as_ref().map(|s| s.expose_secret())
    }

    /// Get the OAuth redirect URI.
    pub fn redirect_uri(&self) -> Option<&str> {
        self.redirect_uri.as_deref()
    }

    /// Check if OAuth is configured.
    pub fn has_oauth(&self) -> bool {
        self.client_id.is_some() && self.client_secret.is_some()
    }

    /// Validate the configuration.
    pub fn validate(&self) -> SlackResult<()> {
        if self.bot_token.expose_secret().is_empty() {
            return Err(SlackError::Config("Bot token is empty".to_string()));
        }
        if self.app_token.expose_secret().is_empty() {
            return Err(SlackError::Config("App token is empty".to_string()));
        }
        if self.signing_secret.expose_secret().is_empty() {
            return Err(SlackError::Config("Signing secret is empty".to_string()));
        }

        // Validate token formats
        if !self.bot_token.expose_secret().starts_with("xoxb-") {
            return Err(SlackError::Config(
                "Bot token must start with 'xoxb-'".to_string(),
            ));
        }
        if !self.app_token.expose_secret().starts_with("xapp-") {
            return Err(SlackError::Config(
                "App token must start with 'xapp-'".to_string(),
            ));
        }

        Ok(())
    }
}

/// Serializable configuration for export/import (without secrets).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackConfigMetadata {
    /// Whether bot token is configured.
    pub has_bot_token: bool,
    /// Whether app token is configured.
    pub has_app_token: bool,
    /// Whether signing secret is configured.
    pub has_signing_secret: bool,
    /// Whether OAuth is configured.
    pub has_oauth: bool,
    /// OAuth redirect URI (if configured).
    pub redirect_uri: Option<String>,
}

impl From<&SlackConfig> for SlackConfigMetadata {
    fn from(config: &SlackConfig) -> Self {
        Self {
            has_bot_token: !config.bot_token.expose_secret().is_empty(),
            has_app_token: !config.app_token.expose_secret().is_empty(),
            has_signing_secret: !config.signing_secret.expose_secret().is_empty(),
            has_oauth: config.has_oauth(),
            redirect_uri: config.redirect_uri.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_new() {
        let config = SlackConfig::new("xoxb-test-token", "xapp-test-token", "test-secret");

        assert_eq!(config.bot_token(), "xoxb-test-token");
        assert_eq!(config.app_token(), "xapp-test-token");
        assert_eq!(config.signing_secret(), "test-secret");
        assert!(!config.has_oauth());
    }

    #[test]
    fn test_config_with_oauth() {
        let config = SlackConfig::new("xoxb-test-token", "xapp-test-token", "test-secret")
            .with_oauth(
                "client-id",
                "client-secret",
                Some("https://example.com/callback".to_string()),
            );

        assert!(config.has_oauth());
        assert_eq!(config.client_id(), Some("client-id"));
        assert_eq!(config.client_secret(), Some("client-secret"));
        assert_eq!(config.redirect_uri(), Some("https://example.com/callback"));
    }

    #[test]
    fn test_config_validate() {
        let config = SlackConfig::new("xoxb-valid-token", "xapp-valid-token", "valid-secret");
        assert!(config.validate().is_ok());

        let config = SlackConfig::new("invalid-token", "xapp-valid-token", "valid-secret");
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_debug_redacts_secrets() {
        let config = SlackConfig::new("xoxb-secret-token", "xapp-secret-token", "super-secret");

        let debug_str = format!("{:?}", config);
        assert!(!debug_str.contains("xoxb-secret-token"));
        assert!(!debug_str.contains("xapp-secret-token"));
        assert!(!debug_str.contains("super-secret"));
        assert!(debug_str.contains("[REDACTED]"));
    }

    #[test]
    fn test_config_metadata() {
        let config = SlackConfig::new("xoxb-test-token", "xapp-test-token", "test-secret")
            .with_oauth(
                "client-id",
                "client-secret",
                Some("https://example.com".to_string()),
            );

        let metadata: SlackConfigMetadata = (&config).into();

        assert!(metadata.has_bot_token);
        assert!(metadata.has_app_token);
        assert!(metadata.has_signing_secret);
        assert!(metadata.has_oauth);
        assert_eq!(
            metadata.redirect_uri,
            Some("https://example.com".to_string())
        );
    }
}
