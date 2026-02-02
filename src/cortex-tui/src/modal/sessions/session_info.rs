//! Session information for display in the sessions modal.

use std::path::PathBuf;

use chrono::{DateTime, Utc};

/// Information about a session for display in the modal.
#[derive(Debug, Clone)]
pub struct SessionInfo {
    /// Path to the session directory.
    pub path: PathBuf,
    /// Session name/title.
    pub name: String,
    /// Model used in the session.
    pub model: String,
    /// When the session was created.
    pub created_at: DateTime<Utc>,
    /// Number of messages in the session.
    pub message_count: usize,
}

impl SessionInfo {
    /// Creates a new SessionInfo.
    pub fn new(
        path: PathBuf,
        name: impl Into<String>,
        model: impl Into<String>,
        created_at: DateTime<Utc>,
        message_count: usize,
    ) -> Self {
        Self {
            path,
            name: name.into(),
            model: model.into(),
            created_at,
            message_count,
        }
    }

    /// Formats the creation time as a relative string (compact form).
    /// Returns "2h ago", "5d ago", etc.
    pub fn relative_time(&self) -> String {
        let now = Utc::now();
        let diff = now.signed_duration_since(self.created_at);

        if diff.num_seconds() < 60 {
            "now".to_string()
        } else if diff.num_minutes() < 60 {
            format!("{}m ago", diff.num_minutes())
        } else if diff.num_hours() < 24 {
            format!("{}h ago", diff.num_hours())
        } else if diff.num_days() < 7 {
            format!("{}d ago", diff.num_days())
        } else if diff.num_weeks() < 4 {
            format!("{}w ago", diff.num_weeks())
        } else {
            self.created_at.format("%b %d").to_string()
        }
    }

    /// Formats the creation time as a relative string (verbose form).
    pub fn format_time(&self) -> String {
        let now = Utc::now();
        let diff = now.signed_duration_since(self.created_at);

        if diff.num_seconds() < 60 {
            "just now".to_string()
        } else if diff.num_minutes() < 60 {
            let mins = diff.num_minutes();
            if mins == 1 {
                "1 minute ago".to_string()
            } else {
                format!("{} minutes ago", mins)
            }
        } else if diff.num_hours() < 24 {
            let hours = diff.num_hours();
            if hours == 1 {
                "1 hour ago".to_string()
            } else {
                format!("{} hours ago", hours)
            }
        } else if diff.num_days() == 1 {
            "yesterday".to_string()
        } else if diff.num_days() < 7 {
            format!("{} days ago", diff.num_days())
        } else if diff.num_weeks() == 1 {
            "1 week ago".to_string()
        } else if diff.num_weeks() < 4 {
            format!("{} weeks ago", diff.num_weeks())
        } else {
            self.created_at.format("%b %d, %Y").to_string()
        }
    }

    /// Gets a short model name for display.
    pub fn short_model(&self) -> &str {
        // Extract the model name after the last '/'
        self.model.rsplit('/').next().unwrap_or(&self.model)
    }
}
