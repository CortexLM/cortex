//! Credential storage and caching.
//!
//! Manages secure storage of credentials using OS keychain
//! and provides token caching functionality.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::error::{CortexError, Result};
use crate::secrets::{KeyringSecretStore, SecretStore, SecretType, SecretValue};

use super::secure::{SecureApiKey, SecureOAuth2Token};
use super::types::{
    ApiKeyCredential, AuthConfig, CredentialType, CredentialValidation, OAuth2Token,
    ProviderCredentialInfo,
};
use super::utils::provider_env_var;

/// Keyring account prefix for API keys.
const KEYRING_APIKEY_PREFIX: &str = "apikey-";

/// Secure credential store for managing authentication credentials.
#[derive(Debug)]
pub struct CredentialStore {
    /// Configuration.
    config: AuthConfig,
    /// Keyring store for primary storage.
    keyring_store: KeyringSecretStore,
    /// API key credentials metadata (not the actual keys).
    api_key_metadata: RwLock<HashMap<String, ApiKeyCredential>>,
    /// OAuth token metadata.
    oauth_metadata: RwLock<HashMap<String, OAuth2Token>>,
    /// Token cache.
    token_cache: RwLock<TokenCache>,
}

impl CredentialStore {
    /// Create a new credential store.
    pub fn new(config: AuthConfig) -> Self {
        Self {
            config,
            keyring_store: KeyringSecretStore::new(),
            api_key_metadata: RwLock::new(HashMap::new()),
            oauth_metadata: RwLock::new(HashMap::new()),
            token_cache: RwLock::new(TokenCache::new(100)),
        }
    }

    /// Load credentials metadata from file (not the actual secrets).
    pub async fn load(&self) -> Result<()> {
        let metadata_path = self.config.credentials_path.with_extension("meta.json");

        if !metadata_path.exists() {
            return Ok(());
        }

        let content = tokio::fs::read_to_string(&metadata_path)
            .await
            .map_err(CortexError::Io)?;

        let stored: StoredCredentialsMetadata = serde_json::from_str(&content)
            .map_err(|e| CortexError::config(format!("Invalid credentials metadata: {e}")))?;

        let mut api_keys = self.api_key_metadata.write().await;
        for cred in stored.api_keys {
            api_keys.insert(cred.provider.clone(), cred);
        }

        let mut oauth_tokens = self.oauth_metadata.write().await;
        for token in stored.oauth_tokens {
            if !token.is_expired() {
                oauth_tokens.insert(token.provider.clone(), token);
            }
        }

        Ok(())
    }

    /// Save credentials metadata to file.
    pub async fn save(&self) -> Result<()> {
        let metadata_path = self.config.credentials_path.with_extension("meta.json");

        // Ensure parent directory exists
        if let Some(parent) = metadata_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(CortexError::Io)?;
        }

        let api_keys = self.api_key_metadata.read().await;
        let oauth_tokens = self.oauth_metadata.read().await;

        let stored = StoredCredentialsMetadata {
            api_keys: api_keys.values().cloned().collect(),
            oauth_tokens: oauth_tokens.values().cloned().collect(),
        };

        let content = serde_json::to_string_pretty(&stored)
            .map_err(|e| CortexError::config(format!("Failed to serialize credentials: {e}")))?;

        tokio::fs::write(&metadata_path, content)
            .await
            .map_err(CortexError::Io)?;

        // Set restrictive permissions on Unix
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            std::fs::set_permissions(&metadata_path, perms).map_err(CortexError::Io)?;
        }

        Ok(())
    }

    /// Get API key for a provider (from keyring or environment).
    pub async fn get_api_key(&self, provider: &str) -> Option<SecureApiKey> {
        // First check environment
        let env_var = provider_env_var(provider);
        if let Ok(key) = std::env::var(&env_var) {
            return Some(SecureApiKey::new(provider, key));
        }

        // Then check keyring
        let keyring_key = format!("{}{}", KEYRING_APIKEY_PREFIX, provider);
        if let Ok(Some(secret)) = self.keyring_store.get(&keyring_key).await {
            return Some(SecureApiKey::new(provider, secret.value().to_string()));
        }

        None
    }

    /// Set API key for a provider (stores in keyring).
    pub async fn set_api_key(&self, provider: &str, api_key: &str) -> Result<()> {
        // Store in keyring
        let keyring_key = format!("{}{}", KEYRING_APIKEY_PREFIX, provider);
        let secret = SecretValue::simple(api_key, SecretType::ApiKey);
        self.keyring_store.set(&keyring_key, secret).await?;

        // Update metadata
        let cred = ApiKeyCredential::new(provider, api_key);
        let mut api_keys = self.api_key_metadata.write().await;
        api_keys.insert(provider.to_string(), cred);
        drop(api_keys);

        self.save().await
    }

    /// Remove API key for a provider.
    pub async fn remove_api_key(&self, provider: &str) -> Result<bool> {
        // Remove from keyring
        let keyring_key = format!("{}{}", KEYRING_APIKEY_PREFIX, provider);
        self.keyring_store.delete(&keyring_key).await?;

        // Remove metadata
        let mut api_keys = self.api_key_metadata.write().await;
        let removed = api_keys.remove(provider).is_some();
        drop(api_keys);

        if removed {
            self.save().await?;
        }

        Ok(removed)
    }

    /// List all providers with credentials.
    pub async fn list_providers(&self) -> Vec<ProviderCredentialInfo> {
        let api_keys = self.api_key_metadata.read().await;

        api_keys
            .values()
            .map(|cred| ProviderCredentialInfo {
                provider: cred.provider.clone(),
                key_prefix: cred.key_prefix.clone(),
                from_env: cred.is_from_env(),
                last_used: cred.last_used,
                label: cred.label.clone(),
            })
            .collect()
    }

    /// Get OAuth2 token for a provider.
    pub async fn get_oauth_token(&self, provider: &str) -> Option<SecureOAuth2Token> {
        // Get token from keyring
        let keyring_key = format!("oauth-{}", provider);
        if let Ok(Some(secret)) = self.keyring_store.get(&keyring_key).await {
            // Parse stored token data
            if let Ok(stored) = serde_json::from_str::<StoredOAuthToken>(secret.value()) {
                return Some(SecureOAuth2Token::new(
                    stored.access_token,
                    stored.token_type,
                    stored.expires_at,
                    stored.refresh_token,
                    stored.scopes,
                    provider,
                ));
            }
        }
        None
    }

    /// Set OAuth2 token for a provider.
    pub async fn set_oauth_token(&self, token: &SecureOAuth2Token) -> Result<()> {
        // Store in keyring
        let keyring_key = format!("oauth-{}", token.metadata().provider);
        let stored = StoredOAuthToken {
            access_token: token.access_token().to_string(),
            token_type: token.metadata().token_type.clone(),
            expires_at: token.metadata().expires_at,
            refresh_token: token.refresh_token().map(|s| s.to_string()),
            scopes: token.metadata().scopes.clone(),
        };
        let stored_json = serde_json::to_string(&stored)
            .map_err(|e| CortexError::Internal(format!("Failed to serialize token: {e}")))?;

        let secret = SecretValue::simple(stored_json, SecretType::AccessToken);
        self.keyring_store.set(&keyring_key, secret).await?;

        // Update metadata
        let mut tokens = self.oauth_metadata.write().await;
        tokens.insert(token.metadata().provider.clone(), token.metadata().clone());
        drop(tokens);

        self.save().await
    }

    /// Validate credentials for a provider.
    pub async fn validate(&self, provider: &str) -> CredentialValidation {
        if self.get_api_key(provider).await.is_some() {
            CredentialValidation {
                valid: true,
                provider: provider.to_string(),
                credential_type: CredentialType::ApiKey,
                message: None,
            }
        } else if let Some(token) = self.get_oauth_token(provider).await {
            if token.is_expired() {
                CredentialValidation {
                    valid: false,
                    provider: provider.to_string(),
                    credential_type: CredentialType::OAuth2,
                    message: Some("Token expired".to_string()),
                }
            } else {
                CredentialValidation {
                    valid: true,
                    provider: provider.to_string(),
                    credential_type: CredentialType::OAuth2,
                    message: None,
                }
            }
        } else {
            CredentialValidation {
                valid: false,
                provider: provider.to_string(),
                credential_type: CredentialType::None,
                message: Some("No credentials found".to_string()),
            }
        }
    }

    /// Clear all credentials.
    pub async fn clear(&self) -> Result<()> {
        // Clear keyring entries
        let api_keys = self.api_key_metadata.read().await;
        for provider in api_keys.keys() {
            let keyring_key = format!("{}{}", KEYRING_APIKEY_PREFIX, provider);
            let _ = self.keyring_store.delete(&keyring_key).await;
        }
        drop(api_keys);

        let oauth_tokens = self.oauth_metadata.read().await;
        for provider in oauth_tokens.keys() {
            let keyring_key = format!("oauth-{}", provider);
            let _ = self.keyring_store.delete(&keyring_key).await;
        }
        drop(oauth_tokens);

        // Clear metadata
        self.api_key_metadata.write().await.clear();
        self.oauth_metadata.write().await.clear();
        self.token_cache.write().await.clear();

        self.save().await
    }

    /// Load credentials from environment.
    pub async fn load_from_env(&self) {
        let providers = ["cortex"];

        let mut api_keys = self.api_key_metadata.write().await;
        for provider in providers {
            let env_var = provider_env_var(provider);
            if let Some(cred) = ApiKeyCredential::from_env(provider, &env_var) {
                api_keys.insert(provider.to_string(), cred);
            }
        }
    }
}

/// Stored credentials metadata format (no actual secrets).
#[derive(Debug, Serialize, Deserialize)]
struct StoredCredentialsMetadata {
    api_keys: Vec<ApiKeyCredential>,
    oauth_tokens: Vec<OAuth2Token>,
}

/// Stored OAuth token (for keyring storage).
#[derive(Serialize, Deserialize)]
struct StoredOAuthToken {
    access_token: String,
    token_type: String,
    expires_at: u64,
    refresh_token: Option<String>,
    scopes: Vec<String>,
}

/// Token cache.
#[derive(Debug)]
#[allow(dead_code)]
struct TokenCache {
    /// Cached tokens.
    tokens: HashMap<String, CachedToken>,
    /// Maximum cache size.
    max_size: usize,
}

#[allow(dead_code)]
impl TokenCache {
    fn new(max_size: usize) -> Self {
        Self {
            tokens: HashMap::new(),
            max_size,
        }
    }

    fn get(&self, key: &str) -> Option<&CachedToken> {
        self.tokens.get(key).filter(|t| !t.is_expired())
    }

    fn insert(&mut self, key: String, token: SecretString, ttl: Duration) {
        // Evict if at capacity
        if self.tokens.len() >= self.max_size {
            self.evict_expired();
            if self.tokens.len() >= self.max_size {
                // Remove oldest
                if let Some(oldest) = self
                    .tokens
                    .iter()
                    .min_by_key(|(_, t)| t.created_at)
                    .map(|(k, _)| k.clone())
                {
                    self.tokens.remove(&oldest);
                }
            }
        }

        self.tokens.insert(
            key,
            CachedToken {
                token,
                created_at: Instant::now(),
                ttl,
            },
        );
    }

    fn evict_expired(&mut self) {
        self.tokens.retain(|_, t| !t.is_expired());
    }

    fn clear(&mut self) {
        self.tokens.clear();
    }
}

/// Cached token entry with secure storage.
#[derive(Debug)]
#[allow(dead_code)]
struct CachedToken {
    token: SecretString,
    created_at: Instant,
    ttl: Duration,
}

#[allow(dead_code)]
impl CachedToken {
    fn is_expired(&self) -> bool {
        self.created_at.elapsed() >= self.ttl
    }
}
