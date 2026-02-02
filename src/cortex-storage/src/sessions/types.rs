//! Session data types and structures.
//!
//! Contains the core data structures for session storage:
//! - `StoredSession` - Session metadata
//! - `ShareInfo` - Session sharing information
//! - `StoredMessage` - Message history entry
//! - `StoredToolCall` - Tool call record
//! - `SessionSummary` - Lightweight session listing

use std::time::Duration;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Session metadata stored on disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredSession {
    /// Unique session identifier.
    pub id: String,
    /// Model used for this session.
    pub model: String,
    /// Working directory.
    pub cwd: String,
    /// Creation timestamp (Unix seconds).
    pub created_at: i64,
    /// Last update timestamp (Unix seconds).
    pub updated_at: i64,
    /// Session title (auto-generated from first message or user-set).
    #[serde(default)]
    pub title: Option<String>,
    /// Whether this session is marked as favorite.
    #[serde(default)]
    pub is_favorite: bool,
    /// Tags for session organization.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Sharing information.
    #[serde(default)]
    pub share_info: Option<ShareInfo>,
}

/// Information about a shared session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShareInfo {
    /// Unique share token.
    pub token: String,
    /// Generated share URL.
    pub url: String,
    /// When the share was created.
    pub created_at: i64,
    /// When the share expires (None = never).
    pub expires_at: Option<i64>,
}

impl ShareInfo {
    /// Create a new share info with optional expiration.
    pub fn new(token: String, url: String, expires_in: Option<Duration>) -> Self {
        let now = Utc::now().timestamp();
        Self {
            token,
            url,
            created_at: now,
            expires_at: expires_in.map(|d| now + d.as_secs() as i64),
        }
    }

    /// Check if the share is still valid (not expired).
    pub fn is_valid(&self) -> bool {
        match self.expires_at {
            Some(expires) => Utc::now().timestamp() < expires,
            None => true, // No expiration means always valid
        }
    }
}

impl StoredSession {
    /// Create a new session with generated ID.
    pub fn new(model: impl Into<String>, cwd: impl Into<String>) -> Self {
        let now = Utc::now().timestamp();
        Self {
            id: Uuid::new_v4().to_string(),
            model: model.into(),
            cwd: cwd.into(),
            created_at: now,
            updated_at: now,
            title: None,
            is_favorite: false,
            tags: Vec::new(),
            share_info: None,
        }
    }

    /// Create a new session with a specific ID.
    pub fn with_id(
        id: impl Into<String>,
        model: impl Into<String>,
        cwd: impl Into<String>,
    ) -> Self {
        let now = Utc::now().timestamp();
        Self {
            id: id.into(),
            model: model.into(),
            cwd: cwd.into(),
            created_at: now,
            updated_at: now,
            title: None,
            is_favorite: false,
            tags: Vec::new(),
            share_info: None,
        }
    }

    /// Update the timestamp to now.
    pub fn touch(&mut self) {
        self.updated_at = Utc::now().timestamp();
    }

    /// Toggle the favorite status.
    pub fn toggle_favorite(&mut self) -> bool {
        self.is_favorite = !self.is_favorite;
        self.touch();
        self.is_favorite
    }

    /// Set the favorite status explicitly.
    pub fn set_favorite(&mut self, favorite: bool) {
        self.is_favorite = favorite;
        self.touch();
    }

    /// Add a tag to the session.
    pub fn add_tag(&mut self, tag: impl Into<String>) {
        let tag = tag.into();
        if !self.tags.contains(&tag) {
            self.tags.push(tag);
            self.touch();
        }
    }

    /// Remove a tag from the session.
    pub fn remove_tag(&mut self, tag: &str) -> bool {
        if let Some(pos) = self.tags.iter().position(|t| t == tag) {
            self.tags.remove(pos);
            self.touch();
            true
        } else {
            false
        }
    }

    /// Check if the session has a specific tag.
    pub fn has_tag(&self, tag: &str) -> bool {
        self.tags.iter().any(|t| t == tag)
    }

    /// Set share information for the session.
    pub fn set_share(&mut self, token: String, url: String, expires_in: Option<Duration>) {
        self.share_info = Some(ShareInfo::new(token, url, expires_in));
        self.touch();
    }

    /// Remove share information.
    pub fn unshare(&mut self) {
        self.share_info = None;
        self.touch();
    }

    /// Check if the session has a valid share.
    pub fn has_valid_share(&self) -> bool {
        self.share_info.as_ref().is_some_and(|s| s.is_valid())
    }

    /// Get the share URL if valid.
    pub fn share_url(&self) -> Option<&str> {
        self.share_info
            .as_ref()
            .filter(|s| s.is_valid())
            .map(|s| s.url.as_str())
    }
}

/// A message stored in session history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredMessage {
    /// Unique message identifier.
    pub id: String,
    /// Role: "user" or "assistant".
    pub role: String,
    /// Message content.
    pub content: String,
    /// Timestamp (Unix seconds).
    pub timestamp: i64,
    /// Tool calls made during this message.
    #[serde(default)]
    pub tool_calls: Vec<StoredToolCall>,
}

impl StoredMessage {
    /// Create a new user message.
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            role: "user".to_string(),
            content: content.into(),
            timestamp: Utc::now().timestamp(),
            tool_calls: Vec::new(),
        }
    }

    /// Create a new assistant message.
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            role: "assistant".to_string(),
            content: content.into(),
            timestamp: Utc::now().timestamp(),
            tool_calls: Vec::new(),
        }
    }

    /// Add a tool call to this message.
    pub fn with_tool_call(mut self, tool_call: StoredToolCall) -> Self {
        self.tool_calls.push(tool_call);
        self
    }
}

/// A tool call record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredToolCall {
    /// Tool call identifier.
    pub id: String,
    /// Tool name.
    pub name: String,
    /// Tool input (JSON).
    pub input: serde_json::Value,
    /// Tool output (if completed).
    #[serde(default)]
    pub output: Option<String>,
    /// Whether the tool call succeeded.
    #[serde(default)]
    pub success: bool,
    /// Duration in milliseconds.
    #[serde(default)]
    pub duration_ms: Option<u64>,
}

/// Session summary for listing (lighter than full session).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    pub id: String,
    pub title: Option<String>,
    pub model: String,
    pub cwd: String,
    pub created_at: i64,
    pub updated_at: i64,
    /// Whether this session is marked as favorite.
    #[serde(default)]
    pub is_favorite: bool,
    /// Tags for session organization.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Whether the session has a valid share link.
    #[serde(default)]
    pub is_shared: bool,
}

impl From<StoredSession> for SessionSummary {
    fn from(session: StoredSession) -> Self {
        let is_shared = session.has_valid_share();
        Self {
            id: session.id,
            title: session.title,
            model: session.model,
            cwd: session.cwd,
            created_at: session.created_at,
            updated_at: session.updated_at,
            is_favorite: session.is_favorite,
            tags: session.tags,
            is_shared,
        }
    }
}
