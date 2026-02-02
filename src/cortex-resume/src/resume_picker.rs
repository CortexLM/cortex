//! Resume picker UI functionality.

use crate::{Result, SessionStore, SessionSummary};

/// Resume picker for selecting sessions to resume.
pub struct ResumePicker {
    store: SessionStore,
    sessions: Vec<SessionSummary>,
    selected_index: usize,
    filter: Option<String>,
}

impl ResumePicker {
    pub fn new(store: SessionStore) -> Self {
        Self {
            store,
            sessions: Vec::new(),
            selected_index: 0,
            filter: None,
        }
    }

    /// Load sessions.
    pub async fn load(&mut self, include_archived: bool) -> Result<()> {
        self.sessions = self.store.list_sessions(include_archived).await?;
        self.selected_index = 0;
        self.apply_filter();
        Ok(())
    }

    /// Set filter text.
    pub fn set_filter(&mut self, filter: Option<String>) {
        self.filter = filter;
        self.apply_filter();
    }

    /// Apply filter to sessions.
    fn apply_filter(&mut self) {
        if let Some(ref filter) = self.filter {
            let filter_lower = filter.to_lowercase();
            self.sessions.retain(|s| {
                s.title.to_lowercase().contains(&filter_lower)
                    || s.id.to_lowercase().contains(&filter_lower)
                    || s.preview
                        .as_ref()
                        .map(|p| p.to_lowercase().contains(&filter_lower))
                        .unwrap_or(false)
            });
        }
    }

    /// Get filtered sessions.
    pub fn sessions(&self) -> &[SessionSummary] {
        &self.sessions
    }

    /// Get selected session.
    pub fn selected(&self) -> Option<&SessionSummary> {
        self.sessions.get(self.selected_index)
    }

    /// Select next session.
    pub fn select_next(&mut self) {
        if !self.sessions.is_empty() && self.selected_index < self.sessions.len() - 1 {
            self.selected_index += 1;
        }
    }

    /// Select previous session.
    pub fn select_prev(&mut self) {
        if self.selected_index > 0 {
            self.selected_index -= 1;
        }
    }

    /// Select by index.
    pub fn select(&mut self, index: usize) {
        if index < self.sessions.len() {
            self.selected_index = index;
        }
    }

    /// Get selected index.
    pub fn selected_index(&self) -> usize {
        self.selected_index
    }

    /// Check if empty.
    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }

    /// Get session count.
    pub fn len(&self) -> usize {
        self.sessions.len()
    }

    /// Get the store reference.
    pub fn store(&self) -> &SessionStore {
        &self.store
    }

    /// Get mutable store reference.
    pub fn store_mut(&mut self) -> &mut SessionStore {
        &mut self.store
    }
}

/// Format a session summary for display.
pub fn format_session_summary(summary: &SessionSummary, width: usize) -> Vec<String> {
    let mut lines = Vec::new();

    // Title line
    let title = if summary.archived {
        format!("[archived] {}", summary.title)
    } else {
        summary.title.clone()
    };
    lines.push(truncate_string(&title, width));

    // Info line
    let info = format!(
        "{} turns | {} | {}",
        summary.turns,
        format_relative_time(&summary.last_used),
        summary.cwd.display()
    );
    lines.push(truncate_string(&info, width));

    // Preview line
    if let Some(ref preview) = summary.preview {
        lines.push(truncate_string(preview, width));
    }

    lines
}

/// Format time relative to now.
fn format_relative_time(time: &chrono::DateTime<chrono::Utc>) -> String {
    let now = chrono::Utc::now();
    let duration = now.signed_duration_since(*time);

    if duration.num_minutes() < 1 {
        "just now".to_string()
    } else if duration.num_hours() < 1 {
        format!("{}m ago", duration.num_minutes())
    } else if duration.num_days() < 1 {
        format!("{}h ago", duration.num_hours())
    } else if duration.num_days() < 7 {
        format!("{}d ago", duration.num_days())
    } else {
        time.format("%Y-%m-%d").to_string()
    }
}

/// Truncate string to fit width.
fn truncate_string(s: &str, width: usize) -> String {
    if s.len() <= width {
        s.to_string()
    } else if width > 3 {
        format!("{}...", &s[..width - 3])
    } else {
        s.chars().take(width).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_relative_time() {
        let now = chrono::Utc::now();
        assert_eq!(format_relative_time(&now), "just now");

        let hour_ago = now - chrono::Duration::hours(2);
        assert_eq!(format_relative_time(&hour_ago), "2h ago");
    }
}
