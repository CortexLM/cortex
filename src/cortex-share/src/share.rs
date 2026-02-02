//! Session sharing management.

use crate::{Result, ShareError, DEFAULT_SHARE_API};
use chrono::{DateTime, Utc};
use cortex_common::create_default_client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::sync::RwLock;
use tracing::{info, warn};

/// Share mode configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ShareMode {
    /// Manual sharing via commands.
    #[default]
    Manual,
    /// Automatic sharing of new sessions.
    Auto,
    /// Sharing disabled.
    Disabled,
}

/// Information about a shared session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SharedSession {
    /// Session ID.
    pub session_id: String,
    /// Share URL.
    pub url: String,
    /// Secret for updating/deleting.
    pub secret: String,
    /// When the share was created.
    pub created_at: DateTime<Utc>,
    /// Whether sync is enabled.
    pub sync_enabled: bool,
}

impl SharedSession {
    pub fn new(session_id: String, url: String, secret: String) -> Self {
        Self {
            session_id,
            url,
            secret,
            created_at: Utc::now(),
            sync_enabled: true,
        }
    }
}

/// Manager for session sharing.
pub struct ShareManager {
    /// API endpoint.
    api_url: String,
    /// Shared sessions.
    shares: RwLock<HashMap<String, SharedSession>>,
    /// Share mode.
    mode: RwLock<ShareMode>,
    /// HTTP client.
    client: reqwest::Client,
}

impl ShareManager {
    pub fn new() -> Self {
        Self {
            api_url: DEFAULT_SHARE_API.to_string(),
            shares: RwLock::new(HashMap::new()),
            mode: RwLock::new(ShareMode::Manual),
            client: create_default_client().expect("HTTP client"),
        }
    }

    pub fn with_api_url(mut self, url: impl Into<String>) -> Self {
        self.api_url = url.into();
        self
    }

    /// Set share mode.
    pub async fn set_mode(&self, mode: ShareMode) {
        *self.mode.write().await = mode;
    }

    /// Get current share mode.
    pub async fn mode(&self) -> ShareMode {
        *self.mode.read().await
    }

    /// Share a session.
    pub async fn share(&self, session_id: &str) -> Result<SharedSession> {
        let mode = *self.mode.read().await;
        if mode == ShareMode::Disabled {
            return Err(ShareError::ApiError("Sharing is disabled".into()));
        }

        // Check if already shared
        if let Some(share) = self.get_share(session_id).await {
            return Ok(share);
        }

        // Call share API
        let response = self
            .client
            .post(format!("{}/share_create", self.api_url))
            .json(&serde_json::json!({
                "sessionID": session_id
            }))
            .send()
            .await
            .map_err(|e| ShareError::Network(e.to_string()))?;

        if !response.status().is_success() {
            let error = response.text().await.unwrap_or_default();
            return Err(ShareError::ApiError(error));
        }

        #[derive(Deserialize)]
        struct ShareResponse {
            url: String,
            secret: String,
        }

        let share_response: ShareResponse = response
            .json()
            .await
            .map_err(|e| ShareError::ApiError(e.to_string()))?;

        let share = SharedSession::new(
            session_id.to_string(),
            share_response.url,
            share_response.secret,
        );

        self.shares
            .write()
            .await
            .insert(session_id.to_string(), share.clone());

        info!("Shared session {}: {}", session_id, share.url);
        Ok(share)
    }

    /// Unshare a session.
    pub async fn unshare(&self, session_id: &str) -> Result<()> {
        let share = self
            .shares
            .write()
            .await
            .remove(session_id)
            .ok_or(ShareError::NotShared)?;

        // Call unshare API
        let response = self
            .client
            .post(format!("{}/share_delete", self.api_url))
            .json(&serde_json::json!({
                "sessionID": session_id,
                "secret": share.secret
            }))
            .send()
            .await
            .map_err(|e| ShareError::Network(e.to_string()))?;

        if !response.status().is_success() {
            warn!("Failed to delete share from server: {}", response.status());
        }

        info!("Unshared session {}", session_id);
        Ok(())
    }

    /// Get share info for a session.
    pub async fn get_share(&self, session_id: &str) -> Option<SharedSession> {
        self.shares.read().await.get(session_id).cloned()
    }

    /// Check if a session is shared.
    pub async fn is_shared(&self, session_id: &str) -> bool {
        self.shares.read().await.contains_key(session_id)
    }

    /// Get all shared sessions.
    pub async fn list_shares(&self) -> Vec<SharedSession> {
        self.shares.read().await.values().cloned().collect()
    }

    /// Load share info (for persistence).
    pub async fn load_share(&self, share: SharedSession) {
        self.shares
            .write()
            .await
            .insert(share.session_id.clone(), share);
    }

    /// Enable/disable sync for a share.
    pub async fn set_sync(&self, session_id: &str, enabled: bool) -> Result<()> {
        let mut shares = self.shares.write().await;
        if let Some(share) = shares.get_mut(session_id) {
            share.sync_enabled = enabled;
            Ok(())
        } else {
            Err(ShareError::NotShared)
        }
    }
}

impl Default for ShareManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_share_mode() {
        let manager = ShareManager::new();

        assert_eq!(manager.mode().await, ShareMode::Manual);

        manager.set_mode(ShareMode::Auto).await;
        assert_eq!(manager.mode().await, ShareMode::Auto);
    }
}
