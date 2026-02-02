//! Session sharing API for cortex-app-server.
//!
//! Provides endpoints for sharing sessions via temporary links with expiration and view limits.

use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    Extension, Json, Router,
    extract::{Path, State},
    routing::{delete, get, post},
};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::auth::AuthResult;
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::storage::StoredMessage;

/// Create share routes.
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/share", post(create_share))
        .route("/share/:token", get(get_shared_session))
        .route("/share/:token", delete(revoke_share))
        .route("/share/:token/stats", get(get_share_stats))
}

/// Shared session manager.
#[derive(Debug, Default)]
pub struct ShareManager {
    /// Active shares by token.
    shares: RwLock<HashMap<String, SharedSessionData>>,
}

impl ShareManager {
    /// Create a new share manager.
    pub fn new() -> Self {
        Self {
            shares: RwLock::new(HashMap::new()),
        }
    }

    /// Create a new share.
    pub async fn create(
        &self,
        session_id: String,
        user_id: Option<String>,
        title: Option<String>,
        messages: Vec<StoredMessage>,
        expires_in_secs: u64,
        max_views: Option<u32>,
    ) -> SharedSessionData {
        let token = generate_share_token();
        let created_at = chrono::Utc::now().timestamp();
        let expires_at = created_at + expires_in_secs as i64;

        let share = SharedSessionData {
            token: token.clone(),
            session_id,
            user_id,
            title,
            messages,
            created_at,
            expires_at,
            view_count: 0,
            max_views,
        };

        let mut shares = self.shares.write().await;
        shares.insert(token, share.clone());

        share
    }

    /// Get a share by token.
    pub async fn get(&self, token: &str) -> Option<SharedSessionData> {
        let shares = self.shares.read().await;
        shares.get(token).cloned()
    }

    /// Increment view count for a share.
    pub async fn increment_view(&self, token: &str) -> Option<u32> {
        let mut shares = self.shares.write().await;
        if let Some(share) = shares.get_mut(token) {
            share.view_count += 1;
            Some(share.view_count)
        } else {
            None
        }
    }

    /// Delete a share.
    pub async fn delete(&self, token: &str) -> bool {
        let mut shares = self.shares.write().await;
        shares.remove(token).is_some()
    }

    /// Check if a share is owned by a user.
    pub async fn is_owned_by(&self, token: &str, user_id: &str) -> bool {
        let shares = self.shares.read().await;
        shares
            .get(token)
            .map(|s| s.user_id.as_deref() == Some(user_id))
            .unwrap_or(false)
    }

    /// Cleanup expired shares.
    pub async fn cleanup_expired(&self) -> usize {
        let now = chrono::Utc::now().timestamp();
        let mut shares = self.shares.write().await;
        let initial_count = shares.len();
        shares.retain(|_, share| share.expires_at > now);
        initial_count - shares.len()
    }

    /// Get all shares for a user.
    pub async fn get_user_shares(&self, user_id: &str) -> Vec<SharedSessionData> {
        let shares = self.shares.read().await;
        shares
            .values()
            .filter(|s| s.user_id.as_deref() == Some(user_id))
            .cloned()
            .collect()
    }

    /// Count active shares.
    pub async fn count(&self) -> usize {
        let shares = self.shares.read().await;
        shares.len()
    }
}

/// Data for a shared session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SharedSessionData {
    /// Unique share token.
    pub token: String,
    /// Original session ID.
    pub session_id: String,
    /// User ID who created the share.
    pub user_id: Option<String>,
    /// Session title.
    pub title: Option<String>,
    /// Snapshot of messages at share time.
    pub messages: Vec<StoredMessage>,
    /// When the share was created (Unix timestamp).
    pub created_at: i64,
    /// When the share expires (Unix timestamp).
    pub expires_at: i64,
    /// Number of times the share has been viewed.
    pub view_count: u32,
    /// Maximum number of views allowed (None = unlimited).
    pub max_views: Option<u32>,
}

/// Generate a unique share token.
fn generate_share_token() -> String {
    // Generate a URL-safe token using UUID v4
    let id = Uuid::new_v4();
    // Use base64url encoding without padding for shorter, URL-safe tokens
    base64::Engine::encode(
        &base64::engine::general_purpose::URL_SAFE_NO_PAD,
        id.as_bytes(),
    )
}

// ============================================================================
// Request/Response Types
// ============================================================================

/// Create share request.
#[derive(Debug, Deserialize)]
pub struct CreateShareRequest {
    /// Session ID to share.
    pub session_id: String,
    /// Expiration time in seconds (default: 7 days).
    #[serde(default = "default_expires_in")]
    pub expires_in: u64,
    /// Maximum number of views (optional).
    pub max_views: Option<u32>,
}

fn default_expires_in() -> u64 {
    7 * 24 * 60 * 60 // 7 days in seconds
}

/// Create share response.
#[derive(Debug, Serialize)]
pub struct CreateShareResponse {
    /// Share token.
    pub token: String,
    /// Full share URL.
    pub url: String,
    /// Expiration timestamp (ISO 8601).
    pub expires_at: String,
}

/// Shared session response.
#[derive(Debug, Serialize)]
pub struct SharedSessionResponse {
    /// Original session ID.
    pub id: String,
    /// Session title.
    pub title: Option<String>,
    /// When the session was created.
    pub created_at: String,
    /// When the share expires.
    pub expires_at: String,
    /// Messages in the session.
    pub messages: Vec<SharedMessage>,
    /// Number of times the share has been viewed.
    pub view_count: u32,
}

/// Message in a shared session.
#[derive(Debug, Serialize)]
pub struct SharedMessage {
    /// Message ID.
    pub id: String,
    /// Role (user/assistant/system).
    pub role: String,
    /// Message content.
    pub content: String,
    /// Timestamp (ISO 8601).
    pub timestamp: String,
    /// Tool calls (if any).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<SharedToolCall>,
}

/// Tool call in a shared message.
#[derive(Debug, Serialize)]
pub struct SharedToolCall {
    /// Tool call ID.
    pub id: String,
    /// Tool name.
    pub name: String,
    /// Tool input.
    pub input: serde_json::Value,
    /// Tool output.
    pub output: Option<String>,
    /// Whether the call succeeded.
    pub success: bool,
}

/// Share statistics response.
#[derive(Debug, Serialize)]
pub struct ShareStatsResponse {
    /// Share token.
    pub token: String,
    /// Number of views.
    pub view_count: u32,
    /// Maximum views allowed.
    pub max_views: Option<u32>,
    /// Whether the share is expired.
    pub expired: bool,
    /// When the share was created.
    pub created_at: String,
    /// When the share expires.
    pub expires_at: String,
    /// Remaining views (if max_views is set).
    pub remaining_views: Option<u32>,
}

// ============================================================================
// Handlers
// ============================================================================

/// Create a share link for a session.
async fn create_share(
    State(state): State<Arc<AppState>>,
    auth: Option<Extension<AuthResult>>,
    Json(req): Json<CreateShareRequest>,
) -> AppResult<Json<CreateShareResponse>> {
    // Validate expiration time (max 30 days)
    let expires_in = req.expires_in.min(30 * 24 * 60 * 60);

    // Extract user_id from authentication context if available
    let user_id = auth
        .as_ref()
        .and_then(|Extension(auth_result)| auth_result.user_id().map(String::from));

    // Load the session messages
    let messages = state
        .cli_sessions
        .storage()
        .read_history(&req.session_id)
        .map_err(|e| AppError::NotFound(format!("Session not found: {}", e)))?;

    // Get session metadata for title
    let session = state
        .cli_sessions
        .storage()
        .load_session(&req.session_id)
        .ok();
    let title = session.as_ref().and_then(|s| s.title.clone());

    // Create the share with user_id from auth context
    let share = state
        .share_manager
        .create(
            req.session_id.clone(),
            user_id,
            title,
            messages,
            expires_in,
            req.max_views,
        )
        .await;

    // Generate the share URL
    let base_url = std::env::var("APP_URL").unwrap_or_else(|_| "http://localhost:8080".to_string());
    let url = format!("{}/share/{}", base_url, share.token);

    let expires_at = chrono::DateTime::from_timestamp(share.expires_at, 0)
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_default();

    Ok(Json(CreateShareResponse {
        token: share.token,
        url,
        expires_at,
    }))
}

/// Get a shared session.
async fn get_shared_session(
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
) -> AppResult<Json<SharedSessionResponse>> {
    let share = state
        .share_manager
        .get(&token)
        .await
        .ok_or_else(|| AppError::NotFound("Share not found".to_string()))?;

    // Check if expired
    let now = chrono::Utc::now().timestamp();
    if share.expires_at <= now {
        return Err(AppError::Gone("Share has expired".to_string()));
    }

    // Check if max views exceeded
    if let Some(max) = share.max_views
        && share.view_count >= max
    {
        return Err(AppError::Gone("Share view limit reached".to_string()));
    }

    // Increment view count
    state.share_manager.increment_view(&token).await;

    // Convert messages
    let messages: Vec<SharedMessage> = share
        .messages
        .iter()
        .map(|m| SharedMessage {
            id: m.id.clone(),
            role: m.role.clone(),
            content: m.content.clone(),
            timestamp: chrono::DateTime::from_timestamp(m.timestamp, 0)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default(),
            tool_calls: m
                .tool_calls
                .iter()
                .map(|tc| SharedToolCall {
                    id: tc.id.clone(),
                    name: tc.name.clone(),
                    input: tc.input.clone(),
                    output: tc.output.clone(),
                    success: tc.success,
                })
                .collect(),
        })
        .collect();

    let created_at = chrono::DateTime::from_timestamp(share.created_at, 0)
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_default();
    let expires_at = chrono::DateTime::from_timestamp(share.expires_at, 0)
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_default();

    Ok(Json(SharedSessionResponse {
        id: share.session_id,
        title: share.title,
        created_at,
        expires_at,
        messages,
        view_count: share.view_count + 1, // Include this view
    }))
}

/// Revoke a share.
///
/// If authentication is enabled, only the owner can revoke a share.
/// If no owner is set (anonymous share), anyone can revoke it.
async fn revoke_share(
    State(state): State<Arc<AppState>>,
    auth: Option<Extension<AuthResult>>,
    Path(token): Path<String>,
) -> AppResult<Json<serde_json::Value>> {
    // Get the share to verify ownership
    let share = state
        .share_manager
        .get(&token)
        .await
        .ok_or_else(|| AppError::NotFound("Share not found".to_string()))?;

    // Extract user_id from authentication context
    let user_id = auth
        .as_ref()
        .and_then(|Extension(auth_result)| auth_result.user_id().map(String::from));

    // Verify ownership: if the share has an owner, the requester must be that owner
    if let Some(ref share_owner) = share.user_id {
        match user_id {
            Some(ref req_user) if req_user == share_owner => {
                // Owner is revoking their own share - allowed
            }
            Some(_) => {
                // Different user trying to revoke - forbidden
                return Err(AppError::Authorization(
                    "You can only revoke your own shares".to_string(),
                ));
            }
            None => {
                // No auth but share has owner - forbidden
                return Err(AppError::Authorization(
                    "Authentication required to revoke this share".to_string(),
                ));
            }
        }
    }
    // If share has no owner (anonymous), anyone can revoke it

    // Delete the share
    state.share_manager.delete(&token).await;

    Ok(Json(serde_json::json!({ "deleted": true })))
}

/// Get share statistics.
async fn get_share_stats(
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
) -> AppResult<Json<ShareStatsResponse>> {
    let share = state
        .share_manager
        .get(&token)
        .await
        .ok_or_else(|| AppError::NotFound("Share not found".to_string()))?;

    let now = chrono::Utc::now().timestamp();
    let expired = share.expires_at <= now
        || share
            .max_views
            .map(|max| share.view_count >= max)
            .unwrap_or(false);

    let remaining_views = share
        .max_views
        .map(|max| max.saturating_sub(share.view_count));

    let created_at = chrono::DateTime::from_timestamp(share.created_at, 0)
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_default();
    let expires_at = chrono::DateTime::from_timestamp(share.expires_at, 0)
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_default();

    Ok(Json(ShareStatsResponse {
        token: share.token,
        view_count: share.view_count,
        max_views: share.max_views,
        expired,
        created_at,
        expires_at,
        remaining_views,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_share_token() {
        let token = generate_share_token();
        assert!(!token.is_empty());
        // Token should be URL-safe
        assert!(
            token
                .chars()
                .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
        );
    }

    #[tokio::test]
    async fn test_share_manager() {
        let manager = ShareManager::new();

        // Create a share
        let share = manager
            .create(
                "session-123".to_string(),
                Some("user-456".to_string()),
                Some("Test Session".to_string()),
                vec![],
                3600, // 1 hour
                Some(10),
            )
            .await;

        assert!(!share.token.is_empty());
        assert_eq!(share.session_id, "session-123");
        assert_eq!(share.view_count, 0);

        // Get the share
        let retrieved = manager.get(&share.token).await;
        assert!(retrieved.is_some());

        // Increment view count
        let new_count = manager.increment_view(&share.token).await;
        assert_eq!(new_count, Some(1));

        // Verify ownership
        assert!(manager.is_owned_by(&share.token, "user-456").await);
        assert!(!manager.is_owned_by(&share.token, "other-user").await);

        // Delete the share
        assert!(manager.delete(&share.token).await);
        assert!(manager.get(&share.token).await.is_none());
    }

    #[tokio::test]
    async fn test_cleanup_expired() {
        let manager = ShareManager::new();

        // Create an expired share (expires_in = 0 means immediate expiration)
        let _share = manager
            .create(
                "session-expired".to_string(),
                None,
                None,
                vec![],
                0, // Already expired
                None,
            )
            .await;

        // Wait a moment for expiration
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Cleanup should remove it
        let removed = manager.cleanup_expired().await;
        assert_eq!(removed, 1);
        assert_eq!(manager.count().await, 0);
    }
}
