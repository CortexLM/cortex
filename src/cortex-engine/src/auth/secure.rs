//! Secure credential wrappers.
//!
//! Provides memory-safe wrappers for sensitive credentials with
//! automatic zeroization and redaction.

use std::time::Duration;

use secrecy::{ExposeSecret, SecretString};

use super::types::{ApiKeyCredential, OAuth2Token};
use super::utils::{current_timestamp, hash_key};

/// Secure API key wrapper with automatic memory cleanup.
pub struct SecureApiKey {
    /// The API key (protected in memory).
    key: SecretString,
    /// Provider name.
    provider: String,
}

impl std::fmt::Debug for SecureApiKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecureApiKey")
            .field("key", &"[REDACTED]")
            .field("provider", &self.provider)
            .finish()
    }
}

impl SecureApiKey {
    /// Create a new secure API key.
    pub fn new(provider: impl Into<String>, key: impl Into<String>) -> Self {
        Self {
            key: SecretString::from(key.into()),
            provider: provider.into(),
        }
    }

    /// Get the key value (use sparingly).
    pub fn expose(&self) -> &str {
        self.key.expose_secret()
    }

    /// Get the provider name.
    pub fn provider(&self) -> &str {
        &self.provider
    }

    /// Get a redacted version for display.
    pub fn redacted(&self) -> String {
        let exposed = self.key.expose_secret();
        if exposed.len() <= 8 {
            "****".to_string()
        } else {
            format!("{}...{}", &exposed[..8], &exposed[exposed.len() - 4..])
        }
    }

    /// Create credential metadata from this key.
    pub fn to_credential(&self) -> ApiKeyCredential {
        let exposed = self.key.expose_secret();
        ApiKeyCredential {
            provider: self.provider.clone(),
            key_hash: hash_key(exposed),
            key_prefix: if exposed.len() > 8 {
                format!("{}...", &exposed[..8])
            } else {
                exposed.to_string()
            },
            created_at: current_timestamp(),
            last_used: None,
            label: None,
            env_var: None,
        }
    }
}

/// Secure OAuth token wrapper.
pub struct SecureOAuth2Token {
    /// Access token (protected).
    access_token: SecretString,
    /// Refresh token if available (protected).
    refresh_token: Option<SecretString>,
    /// Token metadata.
    metadata: OAuth2Token,
}

impl std::fmt::Debug for SecureOAuth2Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecureOAuth2Token")
            .field("access_token", &"[REDACTED]")
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "[REDACTED]"),
            )
            .field("metadata", &self.metadata)
            .finish()
    }
}

impl SecureOAuth2Token {
    /// Create a new secure OAuth token.
    pub fn new(
        access_token: impl Into<String>,
        token_type: impl Into<String>,
        expires_at: u64,
        refresh_token: Option<String>,
        scopes: Vec<String>,
        provider: impl Into<String>,
    ) -> Self {
        let access = access_token.into();
        Self {
            access_token: SecretString::from(access.clone()),
            refresh_token: refresh_token.map(SecretString::from),
            metadata: OAuth2Token {
                token_hash: Some(hash_key(&access)),
                token_type: token_type.into(),
                expires_at,
                has_refresh_token: true,
                scopes,
                provider: provider.into(),
            },
        }
    }

    /// Get the access token (use sparingly).
    pub fn access_token(&self) -> &str {
        self.access_token.expose_secret()
    }

    /// Get the refresh token if available.
    pub fn refresh_token(&self) -> Option<&str> {
        self.refresh_token.as_ref().map(|t| t.expose_secret())
    }

    /// Check if token is expired.
    pub fn is_expired(&self) -> bool {
        current_timestamp() >= self.metadata.expires_at
    }

    /// Check if token will expire soon.
    pub fn expires_soon(&self, threshold_secs: u64) -> bool {
        current_timestamp() + threshold_secs >= self.metadata.expires_at
    }

    /// Get time until expiration.
    pub fn time_until_expiry(&self) -> Duration {
        let now = current_timestamp();
        if now >= self.metadata.expires_at {
            Duration::ZERO
        } else {
            Duration::from_secs(self.metadata.expires_at - now)
        }
    }

    /// Check if token can be refreshed.
    pub fn can_refresh(&self) -> bool {
        self.refresh_token.is_some()
    }

    /// Get authorization header value.
    pub fn authorization_header(&self) -> SecretString {
        SecretString::from(format!(
            "{} {}",
            self.metadata.token_type,
            self.access_token.expose_secret()
        ))
    }

    /// Get metadata (safe to serialize).
    pub fn metadata(&self) -> &OAuth2Token {
        &self.metadata
    }
}

/// Authentication credential types with secure storage.
#[derive(Debug)]
pub enum AuthCredential {
    /// API key.
    ApiKey(SecureApiKey),
    /// OAuth2 token.
    OAuth2(SecureOAuth2Token),
    /// JWT.
    Jwt(SecretString),
}

impl AuthCredential {
    /// Get authorization header value (returns SecretString).
    pub fn authorization_header(&self) -> SecretString {
        match self {
            Self::ApiKey(key) => SecretString::from(format!("Bearer {}", key.expose())),
            Self::OAuth2(token) => token.authorization_header(),
            Self::Jwt(token) => SecretString::from(format!("Bearer {}", token.expose_secret())),
        }
    }

    /// Check if credential is expired.
    pub fn is_expired(&self) -> bool {
        match self {
            Self::ApiKey(_) => false,
            Self::OAuth2(token) => token.is_expired(),
            Self::Jwt(_) => false, // Would need to decode to check
        }
    }
}
