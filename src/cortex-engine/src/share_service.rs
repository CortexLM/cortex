//! Share service for generating public URLs for session transcripts.

use crate::client::Message;
use crate::error::Result;
use cortex_share::ShareManager;
use tracing::info;

/// Service for managing session sharing.
pub struct ShareService {
    manager: ShareManager,
}

impl ShareService {
    /// Create a new share service.
    pub fn new() -> Self {
        Self {
            manager: ShareManager::new(),
        }
    }

    /// Share a session and return the public URL.
    pub async fn share(&self, session_id: &str, _messages: &[Message]) -> Result<String> {
        info!("Sharing session {}", session_id);

        let share = self
            .manager
            .share(session_id)
            .await
            .map_err(|e| crate::error::CortexError::Internal(e.to_string()))?;

        // In a more complete implementation, we might want to upload the
        // current transcript to the shared session if it's a new share.

        Ok(share.url)
    }

    /// Unshare a session.
    pub async fn unshare(&self, session_id: &str) -> Result<()> {
        info!("Unsharing session {}", session_id);

        self.manager
            .unshare(session_id)
            .await
            .map_err(|e| crate::error::CortexError::Internal(e.to_string()))?;

        Ok(())
    }

    /// Check if a session is shared.
    pub async fn is_shared(&self, session_id: &str) -> bool {
        self.manager.is_shared(session_id).await
    }

    /// Get share URL if already shared.
    pub async fn get_share_url(&self, session_id: &str) -> Option<String> {
        self.manager.get_share(session_id).await.map(|s| s.url)
    }
}

impl Default for ShareService {
    fn default() -> Self {
        Self::new()
    }
}
