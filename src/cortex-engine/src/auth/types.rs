//! Authentication types and data structures.
//!
//! Contains configuration, credentials, and token structures.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::utils::current_timestamp;

/// Authentication configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    /// Path to credentials file (fallback).
    pub credentials_path: PathBuf,
    /// Token refresh threshold (seconds before expiry).
    pub refresh_threshold: u64,
    /// Maximum cached tokens.
    pub max_cached_tokens: usize,
    /// Enable token caching.
    pub cache_enabled: bool,
    /// Default token TTL in seconds.
    pub default_ttl: u64,
    /// Enable automatic token refresh.
    pub auto_refresh: bool,
    /// Use keyring for credential storage.
    pub use_keyring: bool,
}

impl Default for AuthConfig {
    fn default() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        Self {
            credentials_path: home.join(".config/cortex/credentials.enc"),
            refresh_threshold: 300,
            max_cached_tokens: 100,
            cache_enabled: true,
            default_ttl: 3600,
            auto_refresh: true,
            use_keyring: true,
        }
    }
}

/// API key credential with secure storage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiKeyCredential {
    /// Provider name (openai, anthropic, etc).
    pub provider: String,
    /// Key hash for validation (never store plaintext in serialized form).
    pub key_hash: String,
    /// Key prefix for display.
    pub key_prefix: String,
    /// Created timestamp.
    pub created_at: u64,
    /// Last used timestamp.
    pub last_used: Option<u64>,
    /// Custom name/label.
    pub label: Option<String>,
    /// Environment variable name if sourced from env.
    pub env_var: Option<String>,
}

impl ApiKeyCredential {
    /// Create a new API key credential (stores hash only).
    pub fn new(provider: &str, api_key: &str) -> Self {
        let key_hash = super::utils::hash_key(api_key);
        let key_prefix = if api_key.len() > 8 {
            format!("{}...", &api_key[..8])
        } else {
            api_key.to_string()
        };

        Self {
            provider: provider.to_string(),
            key_hash,
            key_prefix,
            created_at: current_timestamp(),
            last_used: None,
            label: None,
            env_var: None,
        }
    }

    /// Create from environment variable.
    pub fn from_env(provider: &str, env_var: &str) -> Option<Self> {
        std::env::var(env_var).ok().map(|key| {
            let mut cred = Self::new(provider, &key);
            cred.env_var = Some(env_var.to_string());
            cred
        })
    }

    /// Validate the key matches the hash.
    pub fn validate(&self, key: &str) -> bool {
        super::utils::hash_key(key) == self.key_hash
    }

    /// Update last used timestamp.
    pub fn touch(&mut self) {
        self.last_used = Some(current_timestamp());
    }

    /// Check if key is from environment.
    pub fn is_from_env(&self) -> bool {
        self.env_var.is_some()
    }

    /// Redact the key for display.
    pub fn redacted(&self) -> String {
        self.key_prefix.clone()
    }
}

/// OAuth2 token with secure storage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuth2Token {
    /// Access token hash (for validation).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_hash: Option<String>,
    /// Token type (usually "Bearer").
    pub token_type: String,
    /// Expiration timestamp.
    pub expires_at: u64,
    /// Whether refresh token exists (never serialize actual token).
    pub has_refresh_token: bool,
    /// Scopes granted.
    pub scopes: Vec<String>,
    /// Provider/issuer.
    pub provider: String,
}

impl OAuth2Token {
    /// Check if token is expired.
    pub fn is_expired(&self) -> bool {
        current_timestamp() >= self.expires_at
    }

    /// Check if token will expire soon.
    pub fn expires_soon(&self, threshold_secs: u64) -> bool {
        current_timestamp() + threshold_secs >= self.expires_at
    }

    /// Get time until expiration.
    pub fn time_until_expiry(&self) -> Duration {
        let now = current_timestamp();
        if now >= self.expires_at {
            Duration::ZERO
        } else {
            Duration::from_secs(self.expires_at - now)
        }
    }

    /// Check if token can be refreshed.
    pub fn can_refresh(&self) -> bool {
        self.has_refresh_token
    }
}

/// JWT claims.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JwtClaims {
    /// Subject (user ID).
    pub sub: String,
    /// Issuer.
    pub iss: String,
    /// Audience.
    pub aud: Vec<String>,
    /// Expiration time.
    pub exp: u64,
    /// Issued at.
    pub iat: u64,
    /// Not before.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nbf: Option<u64>,
    /// JWT ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jti: Option<String>,
    /// Custom claims.
    #[serde(flatten)]
    pub custom: HashMap<String, serde_json::Value>,
}

impl JwtClaims {
    /// Create new claims.
    pub fn new(sub: &str, iss: &str, ttl_secs: u64) -> Self {
        let now = current_timestamp();
        Self {
            sub: sub.to_string(),
            iss: iss.to_string(),
            aud: Vec::new(),
            exp: now + ttl_secs,
            iat: now,
            nbf: None,
            jti: None,
            custom: HashMap::new(),
        }
    }

    /// Check if expired.
    pub fn is_expired(&self) -> bool {
        current_timestamp() >= self.exp
    }

    /// Check if valid (not before).
    pub fn is_valid_time(&self) -> bool {
        let now = current_timestamp();
        if let Some(nbf) = self.nbf
            && now < nbf
        {
            return false;
        }
        now < self.exp
    }

    /// Add a custom claim.
    pub fn with_claim(mut self, key: &str, value: serde_json::Value) -> Self {
        self.custom.insert(key.to_string(), value);
        self
    }

    /// Add audience.
    pub fn with_audience(mut self, aud: &str) -> Self {
        self.aud.push(aud.to_string());
        self
    }
}

/// Provider credential info for listing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderCredentialInfo {
    /// Provider name.
    pub provider: String,
    /// Key prefix (redacted).
    pub key_prefix: String,
    /// Whether from environment.
    pub from_env: bool,
    /// Last used timestamp.
    pub last_used: Option<u64>,
    /// Custom label.
    pub label: Option<String>,
}

/// Credential validation result.
#[derive(Debug, Clone)]
pub struct CredentialValidation {
    /// Whether credentials are valid.
    pub valid: bool,
    /// Provider name.
    pub provider: String,
    /// Type of credential found.
    pub credential_type: CredentialType,
    /// Validation message.
    pub message: Option<String>,
}

/// Credential type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialType {
    /// No credential.
    None,
    /// API key.
    ApiKey,
    /// OAuth2 token.
    OAuth2,
    /// JWT.
    Jwt,
}
