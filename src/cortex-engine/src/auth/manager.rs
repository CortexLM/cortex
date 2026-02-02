//! Authentication manager.
//!
//! Provides high-level authentication management including
//! credential retrieval, validation, and refresh.

use std::sync::Arc;

use crate::error::{CortexError, Result};

use super::secure::AuthCredential;
use super::store::CredentialStore;
use super::types::{AuthConfig, CredentialValidation};

/// Authentication manager.
pub struct AuthManager {
    /// Credential store.
    store: Arc<CredentialStore>,
    /// Configuration.
    config: AuthConfig,
}

impl AuthManager {
    /// Create a new auth manager.
    pub fn new(config: AuthConfig) -> Self {
        let store = Arc::new(CredentialStore::new(config.clone()));
        Self { store, config }
    }

    /// Initialize the auth manager.
    pub async fn init(&self) -> Result<()> {
        self.store.load_from_env().await;
        self.store.load().await?;
        Ok(())
    }

    /// Get credentials for a provider.
    pub async fn get_credentials(&self, provider: &str) -> Result<AuthCredential> {
        if let Some(key) = self.store.get_api_key(provider).await {
            return Ok(AuthCredential::ApiKey(key));
        }

        if let Some(token) = self.store.get_oauth_token(provider).await {
            if !token.is_expired() {
                return Ok(AuthCredential::OAuth2(token));
            }

            // Try to refresh if possible
            if token.can_refresh() && self.config.auto_refresh {
                // Would refresh here in real implementation
            }
        }

        Err(CortexError::ApiKeyNotFound {
            provider: provider.to_string(),
        })
    }

    /// Set API key.
    pub async fn set_api_key(&self, provider: &str, key: &str) -> Result<()> {
        self.store.set_api_key(provider, key).await
    }

    /// Validate all credentials.
    pub async fn validate_all(&self) -> Vec<CredentialValidation> {
        let providers = self.store.list_providers().await;
        let mut results = Vec::new();

        for info in providers {
            results.push(self.store.validate(&info.provider).await);
        }

        results
    }

    /// Get credential store reference.
    pub fn store(&self) -> &Arc<CredentialStore> {
        &self.store
    }
}
