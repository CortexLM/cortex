//! Type definitions for authentication data.

use secrecy::{ExposeSecret, SecretString};

/// Authentication mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum AuthMode {
    /// API key authentication.
    ApiKey,
    /// OAuth authentication.
    OAuth,
}

impl std::fmt::Display for AuthMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuthMode::ApiKey => write!(f, "api_key"),
            AuthMode::OAuth => write!(f, "oauth"),
        }
    }
}

/// Authentication data stored locally (metadata only - secrets in keyring).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AuthData {
    /// Authentication mode.
    pub mode: AuthMode,
    /// Whether API key exists (actual key in keyring).
    #[serde(default)]
    pub has_api_key: bool,
    /// Whether access token exists (actual token in keyring).
    #[serde(default)]
    pub has_access_token: bool,
    /// Whether refresh token exists (actual token in keyring).
    #[serde(default)]
    pub has_refresh_token: bool,
    /// Token expiration timestamp.
    pub expires_at: Option<i64>,
    /// Account ID.
    pub account_id: Option<String>,
}

impl AuthData {
    /// Create new auth data with API key (legacy compatibility).
    pub fn with_api_key(api_key: String) -> Self {
        // This is for migration - actual key stored separately
        let _ = api_key;
        Self {
            mode: AuthMode::ApiKey,
            has_api_key: true,
            has_access_token: false,
            has_refresh_token: false,
            expires_at: None,
            account_id: None,
        }
    }

    /// Create new auth data with OAuth tokens (legacy compatibility).
    pub fn with_oauth(
        _access_token: String,
        refresh_token: Option<String>,
        expires_at: Option<i64>,
    ) -> Self {
        Self {
            mode: AuthMode::OAuth,
            has_api_key: false,
            has_access_token: true,
            has_refresh_token: refresh_token.is_some(),
            expires_at,
            account_id: None,
        }
    }

    /// Check if the token is expired.
    pub fn is_expired(&self) -> bool {
        match self.expires_at {
            Some(expires_at) => {
                let now = chrono::Utc::now().timestamp();
                now >= expires_at
            }
            None => false,
        }
    }

    /// Get the current token - returns None as actual tokens are in keyring.
    pub fn get_token(&self) -> Option<&str> {
        // Actual tokens are stored in keyring, not in AuthData
        None
    }
}

/// Secure auth data wrapper for runtime use.
pub struct SecureAuthData {
    /// Authentication mode.
    pub mode: AuthMode,
    /// API key (protected in memory).
    pub(crate) api_key: Option<SecretString>,
    /// Access token (protected in memory).
    pub(crate) access_token: Option<SecretString>,
    /// Refresh token (protected in memory).
    pub(crate) refresh_token: Option<SecretString>,
    /// Token expiration timestamp.
    pub expires_at: Option<i64>,
    /// Account ID.
    pub account_id: Option<String>,
}

impl SecureAuthData {
    /// Create new secure auth data with API key.
    pub fn with_api_key(api_key: String) -> Self {
        Self {
            mode: AuthMode::ApiKey,
            api_key: Some(SecretString::from(api_key)),
            access_token: None,
            refresh_token: None,
            expires_at: None,
            account_id: None,
        }
    }

    /// Create new secure auth data with OAuth tokens.
    pub fn with_oauth(
        access_token: String,
        refresh_token: Option<String>,
        expires_at: Option<i64>,
    ) -> Self {
        Self {
            mode: AuthMode::OAuth,
            api_key: None,
            access_token: Some(SecretString::from(access_token)),
            refresh_token: refresh_token.map(SecretString::from),
            expires_at,
            account_id: None,
        }
    }

    /// Create SecureAuthData from raw components.
    pub(crate) fn from_components(
        mode: AuthMode,
        api_key: Option<SecretString>,
        access_token: Option<SecretString>,
        refresh_token: Option<SecretString>,
        expires_at: Option<i64>,
        account_id: Option<String>,
    ) -> Self {
        Self {
            mode,
            api_key,
            access_token,
            refresh_token,
            expires_at,
            account_id,
        }
    }

    /// Check if the token is expired.
    pub fn is_expired(&self) -> bool {
        match self.expires_at {
            Some(expires_at) => {
                let now = chrono::Utc::now().timestamp();
                now >= expires_at
            }
            None => false,
        }
    }

    /// Check if the token expires within the given threshold (in seconds).
    pub fn expires_soon(&self, threshold_secs: i64) -> bool {
        match self.expires_at {
            Some(expires_at) => {
                let now = chrono::Utc::now().timestamp();
                expires_at - now < threshold_secs
            }
            None => false,
        }
    }

    /// Get the time remaining until token expiry in seconds.
    /// Returns None if no expiration is set.
    pub fn time_until_expiry(&self) -> Option<i64> {
        self.expires_at
            .map(|exp| exp - chrono::Utc::now().timestamp())
    }

    /// Get the current token (exposes the secret - use sparingly).
    pub fn get_token(&self) -> Option<&str> {
        match self.mode {
            AuthMode::ApiKey => self.api_key.as_ref().map(|s| s.expose_secret().as_ref()),
            AuthMode::OAuth => self
                .access_token
                .as_ref()
                .map(|s| s.expose_secret().as_ref()),
        }
    }

    /// Get the refresh token if available.
    pub fn get_refresh_token(&self) -> Option<&str> {
        self.refresh_token
            .as_ref()
            .map(|s| s.expose_secret().as_ref())
    }

    /// Convert to metadata for storage.
    pub fn to_metadata(&self) -> AuthData {
        AuthData {
            mode: self.mode,
            has_api_key: self.api_key.is_some(),
            has_access_token: self.access_token.is_some(),
            has_refresh_token: self.refresh_token.is_some(),
            expires_at: self.expires_at,
            account_id: self.account_id.clone(),
        }
    }
}

/// Credential store mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CredentialsStoreMode {
    /// Store credentials in the keyring (preferred).
    #[default]
    Keyring,
    /// Store credentials in an encrypted file.
    EncryptedFile,
    /// Legacy: Store credentials in a file (deprecated).
    File,
}

/// Stored secure auth data format for encrypted file storage.
#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct StoredSecureAuth {
    pub mode: AuthMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub access_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
}

/// Legacy auth data format for migration.
#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct LegacyAuthData {
    pub mode: AuthMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub access_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_secure_auth_data_api_key() {
        let data = SecureAuthData::with_api_key("test-key".to_string());
        assert_eq!(data.mode, AuthMode::ApiKey);
        assert_eq!(data.get_token(), Some("test-key"));
    }

    #[test]
    fn test_secure_auth_data_oauth() {
        let data =
            SecureAuthData::with_oauth("access".to_string(), Some("refresh".to_string()), None);
        assert_eq!(data.mode, AuthMode::OAuth);
        assert_eq!(data.get_token(), Some("access"));
        assert_eq!(data.get_refresh_token(), Some("refresh"));
    }

    #[test]
    fn test_auth_data_metadata() {
        let data = SecureAuthData::with_api_key("test".to_string());
        let metadata = data.to_metadata();
        assert!(metadata.has_api_key);
        assert!(!metadata.has_access_token);
    }
}
