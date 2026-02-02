//! Resume Picker - Session selection at startup.
//!
//! Displays a list of recent sessions with preview information,
//! allowing users to Resume, Fork, or start a New session.

use crate::interactive::state::{InteractiveAction, InteractiveItem, InteractiveState};
use crate::session::SessionSummary;
use chrono::{Duration, Utc};

/// Build an interactive state for the resume picker (startup session selection).
///
/// This picker is displayed at startup when `--resume` flag is used or
/// when configured to show on launch.
///
/// Shortcuts:
/// - Enter: Resume selected session
/// - F: Fork selected session (create copy)
/// - N: Start new session
/// - Esc: Start new session (same as N)
pub fn build_resume_picker(sessions: &[SessionSummary], show_archived: bool) -> InteractiveState {
    let mut items: Vec<InteractiveItem> = Vec::new();

    // Add "New Session" option at the top
    items.push(
        InteractiveItem::new("__new__", "New Session")
            .with_icon('+')
            .with_description("Start a fresh conversation")
            .with_shortcut('n'),
    );

    // Add separator if there are sessions
    if !sessions.is_empty() {
        items.push(
            InteractiveItem::new("__sep_recent__", "─── Recent Sessions ───")
                .with_icon(' ')
                .as_separator(),
        );
    }

    // Filter and format sessions
    let filtered_sessions: Vec<_> = sessions
        .iter()
        .filter(|s| show_archived || !s.archived)
        .take(15) // Limit to 15 most recent
        .collect();

    for session in filtered_sessions {
        let time_ago = format_time_ago(session.updated_at);

        // Format title with fallback
        let title = if session.title.is_empty() {
            "Untitled session"
        } else {
            &session.title
        };

        // Truncate title if too long
        let display_title = if title.len() > 40 {
            format!("{}...", &title[..37])
        } else {
            title.to_string()
        };

        // Format description with metadata
        let description = format!(
            "{} • {} messages • {}",
            time_ago, session.message_count, session.model
        );

        // Icon based on recency
        let icon = if is_recent(&session.updated_at, 2) {
            '*' // Very recent (< 2 hours)
        } else if is_recent(&session.updated_at, 24) {
            '+' // Today
        } else if is_recent(&session.updated_at, 168) {
            '-' // This week
        } else {
            '.' // Older
        };

        items.push(
            InteractiveItem::new(&session.id, &display_title)
                .with_icon(icon)
                .with_description(description)
                .with_metadata(format!("ID: {}", short_id(&session.id))),
        );
    }

    // Create state with custom hints
    InteractiveState::new(
        "Resume Session".to_string(),
        items,
        InteractiveAction::ResumeSession,
    )
    .with_search()
    .with_max_visible(12)
    .with_hints(vec![
        ("Enter".to_string(), "Resume".to_string()),
        ("f".to_string(), "Fork".to_string()),
        ("n".to_string(), "New".to_string()),
        ("Esc".to_string(), "Cancel".to_string()),
    ])
}

/// Format a timestamp as "X ago" human-readable string.
fn format_time_ago(timestamp: chrono::DateTime<Utc>) -> String {
    let now = Utc::now();
    let diff = now.signed_duration_since(timestamp);

    if diff < Duration::minutes(1) {
        "just now".to_string()
    } else if diff < Duration::hours(1) {
        let mins = diff.num_minutes();
        format!("{}m ago", mins)
    } else if diff < Duration::hours(24) {
        let hours = diff.num_hours();
        format!("{}h ago", hours)
    } else if diff < Duration::days(7) {
        let days = diff.num_days();
        if days == 1 {
            "yesterday".to_string()
        } else {
            format!("{}d ago", days)
        }
    } else if diff < Duration::days(30) {
        let weeks = diff.num_weeks();
        format!("{}w ago", weeks)
    } else {
        timestamp.format("%b %d").to_string()
    }
}

/// Check if timestamp is within X hours of now.
fn is_recent(timestamp: &chrono::DateTime<Utc>, hours: i64) -> bool {
    let now = Utc::now();
    let diff = now.signed_duration_since(*timestamp);
    diff < Duration::hours(hours)
}

/// Get short ID (first 8 characters).
fn short_id(id: &str) -> &str {
    if id.len() >= 8 { &id[..8] } else { id }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn create_test_session(id: &str, title: &str, hours_ago: i64) -> SessionSummary {
        SessionSummary {
            id: id.to_string(),
            title: title.to_string(),
            message_count: 5,
            created_at: Utc::now() - Duration::hours(hours_ago),
            updated_at: Utc::now() - Duration::hours(hours_ago),
            archived: false,
            provider: "anthropic".to_string(),
            model: "claude-3".to_string(),
        }
    }

    #[test]
    fn test_build_resume_picker_empty() {
        let state = build_resume_picker(&[], false);
        // Should have "New Session" option only
        assert_eq!(state.items.len(), 1);
        assert_eq!(state.items[0].id, "__new__");
    }

    #[test]
    fn test_build_resume_picker_with_sessions() {
        let sessions = vec![
            create_test_session("abc123", "Fix auth bug", 1),
            create_test_session("def456", "Add user profile", 24),
            create_test_session("ghi789", "Refactor database", 72),
        ];
        let state = build_resume_picker(&sessions, false);

        // New Session + separator + 3 sessions = 5 items
        assert_eq!(state.items.len(), 5);
        assert_eq!(state.items[0].id, "__new__");
        assert!(state.items[1].is_separator);
        assert_eq!(state.items[2].id, "abc123");
    }

    #[test]
    fn test_format_time_ago() {
        let now = Utc::now();

        assert_eq!(format_time_ago(now), "just now");
        assert_eq!(format_time_ago(now - Duration::minutes(30)), "30m ago");
        assert_eq!(format_time_ago(now - Duration::hours(2)), "2h ago");
        assert_eq!(format_time_ago(now - Duration::days(1)), "yesterday");
        assert_eq!(format_time_ago(now - Duration::days(3)), "3d ago");
    }

    #[test]
    fn test_is_recent() {
        let now = Utc::now();

        assert!(is_recent(&now, 1));
        assert!(is_recent(&(now - Duration::minutes(30)), 1));
        assert!(!is_recent(&(now - Duration::hours(2)), 1));
    }
}
