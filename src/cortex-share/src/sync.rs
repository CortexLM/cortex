//! Real-time sync for shared sessions.

use crate::{Result, ShareError, SharedSession, DEFAULT_SHARE_API};
use cortex_common::create_default_client;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tracing::{debug, warn};

/// Sync event types.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SyncEvent {
    /// Session info updated.
    SessionUpdated {
        session_id: String,
        data: serde_json::Value,
    },
    /// Message added/updated.
    MessageUpdated {
        session_id: String,
        message_id: String,
        data: serde_json::Value,
    },
    /// Part added/updated.
    PartUpdated {
        session_id: String,
        message_id: String,
        part_id: String,
        data: serde_json::Value,
    },
}

/// Sync manager for real-time updates.
pub struct ShareSync {
    /// API endpoint.
    api_url: String,
    /// HTTP client.
    client: reqwest::Client,
    /// Pending updates queue.
    pending: tokio::sync::Mutex<Vec<SyncUpdate>>,
}

#[derive(Debug, Clone)]
struct SyncUpdate {
    session_id: String,
    secret: String,
    key: String,
    content: serde_json::Value,
}

impl ShareSync {
    pub fn new() -> Self {
        Self {
            api_url: DEFAULT_SHARE_API.to_string(),
            client: create_default_client().expect("HTTP client"),
            pending: tokio::sync::Mutex::new(Vec::new()),
        }
    }

    pub fn with_api_url(mut self, url: impl Into<String>) -> Self {
        self.api_url = url.into();
        self
    }

    /// Queue a sync update.
    pub async fn queue_update(&self, share: &SharedSession, key: &str, content: serde_json::Value) {
        if !share.sync_enabled {
            return;
        }

        let update = SyncUpdate {
            session_id: share.session_id.clone(),
            secret: share.secret.clone(),
            key: key.to_string(),
            content,
        };

        self.pending.lock().await.push(update);
    }

    /// Sync session info update.
    pub async fn sync_session(&self, share: &SharedSession, data: serde_json::Value) {
        let key = format!("session/info/{}", share.session_id);
        self.queue_update(share, &key, data).await;
    }

    /// Sync message update.
    pub async fn sync_message(
        &self,
        share: &SharedSession,
        message_id: &str,
        data: serde_json::Value,
    ) {
        let key = format!("session/message/{}/{}", share.session_id, message_id);
        self.queue_update(share, &key, data).await;
    }

    /// Sync part update.
    pub async fn sync_part(
        &self,
        share: &SharedSession,
        message_id: &str,
        part_id: &str,
        data: serde_json::Value,
    ) {
        let key = format!(
            "session/part/{}/{}/{}",
            share.session_id, message_id, part_id
        );
        self.queue_update(share, &key, data).await;
    }

    /// Flush pending updates.
    pub async fn flush(&self) -> Result<usize> {
        let updates: Vec<SyncUpdate> = {
            let mut pending = self.pending.lock().await;
            std::mem::take(&mut *pending)
        };

        if updates.is_empty() {
            return Ok(0);
        }

        let count = updates.len();

        for update in updates {
            self.send_update(update).await?;
        }

        debug!("Flushed {} sync updates", count);
        Ok(count)
    }

    /// Send a single update.
    async fn send_update(&self, update: SyncUpdate) -> Result<()> {
        let response = self
            .client
            .post(format!("{}/share_sync", self.api_url))
            .json(&serde_json::json!({
                "sessionID": update.session_id,
                "secret": update.secret,
                "key": update.key,
                "content": update.content,
            }))
            .send()
            .await
            .map_err(|e| ShareError::Network(e.to_string()))?;

        if !response.status().is_success() {
            warn!("Sync update failed: {} {}", response.status(), update.key);
        } else {
            debug!("Synced: {}", update.key);
        }

        Ok(())
    }

    /// Start background sync task.
    pub fn start_background_sync(self) -> (tokio::task::JoinHandle<()>, mpsc::Sender<SyncEvent>) {
        let (tx, mut rx) = mpsc::channel::<SyncEvent>(100);

        let handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));

            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        if let Err(e) = self.flush().await {
                            warn!("Sync flush error: {}", e);
                        }
                    }
                    event = rx.recv() => {
                        if event.is_none() {
                            break;
                        }
                        // Events are queued via queue_update
                    }
                }
            }
        });

        (handle, tx)
    }
}

impl Default for ShareSync {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_queue_update() {
        let sync = ShareSync::new();
        let share = SharedSession::new(
            "test-session".to_string(),
            "https://example.com/share/123".to_string(),
            "secret123".to_string(),
        );

        sync.queue_update(&share, "test/key", serde_json::json!({"foo": "bar"}))
            .await;

        let pending = sync.pending.lock().await;
        assert_eq!(pending.len(), 1);
    }
}
