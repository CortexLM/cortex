//! Session metadata types.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Session metadata.
///
/// Unknown fields from newer versions are preserved in `extra_fields`
/// to prevent data loss during forward/backward version compatibility (#2176).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMeta {
    /// Session ID.
    pub id: String,
    /// Session title (usually first message or auto-generated).
    pub title: Option<String>,
    /// Working directory.
    pub cwd: PathBuf,
    /// When the session was created.
    pub created_at: DateTime<Utc>,
    /// When the session was last updated.
    pub updated_at: DateTime<Utc>,
    /// Number of turns in the session.
    pub turn_count: usize,
    /// Total tokens used.
    pub total_tokens: u64,
    /// Session source (cli, vscode, etc.).
    pub source: SessionSource,
    /// Whether the session is archived.
    pub archived: bool,
    /// Model used.
    pub model: Option<String>,
    /// Git branch (if in a git repo).
    pub git_branch: Option<String>,
    /// Preserve unknown fields from newer versions to prevent data loss.
    /// When a session is created by a newer version of cortex with additional
    /// fields, those fields are captured here and will be preserved when
    /// the session is saved by an older version.
    #[serde(flatten)]
    pub extra_fields: HashMap<String, serde_json::Value>,
}

/// Source of a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum SessionSource {
    Cli,
    VSCode,
    Api,
    Exec,
    #[default]
    Unknown,
}

/// Summary of a session for display.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    /// Session ID.
    pub id: String,
    /// Display title.
    pub title: String,
    /// When last used.
    pub last_used: DateTime<Utc>,
    /// Working directory.
    pub cwd: PathBuf,
    /// Turn count.
    pub turns: usize,
    /// Whether archived.
    pub archived: bool,
    /// First message preview.
    pub preview: Option<String>,
}

impl SessionMeta {
    pub fn new(id: impl Into<String>, cwd: impl Into<PathBuf>) -> Self {
        let now = Utc::now();
        Self {
            id: id.into(),
            title: None,
            cwd: cwd.into(),
            created_at: now,
            updated_at: now,
            turn_count: 0,
            total_tokens: 0,
            source: SessionSource::Cli,
            archived: false,
            model: None,
            git_branch: None,
            extra_fields: HashMap::new(),
        }
    }

    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn with_source(mut self, source: SessionSource) -> Self {
        self.source = source;
        self
    }

    pub fn to_summary(&self, preview: Option<String>) -> SessionSummary {
        SessionSummary {
            id: self.id.clone(),
            title: self
                .title
                .clone()
                .unwrap_or_else(|| format!("Session {}", &self.id[..8.min(self.id.len())])),
            last_used: self.updated_at,
            cwd: self.cwd.clone(),
            turns: self.turn_count,
            archived: self.archived,
            preview,
        }
    }

    pub fn increment_turn(&mut self) {
        self.turn_count += 1;
        self.updated_at = Utc::now();
    }

    pub fn add_tokens(&mut self, tokens: u64) {
        self.total_tokens += tokens;
        self.updated_at = Utc::now();
    }
}
