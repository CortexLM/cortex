//! Builder for session selection.

use crate::interactive::state::{InteractiveAction, InteractiveItem, InteractiveState};
use crate::session::SessionSummary;

/// Build an interactive state for session selection.
pub fn build_sessions_selector(
    sessions: &[SessionSummary],
    current_session_id: Option<&str>,
) -> InteractiveState {
    let mut items: Vec<InteractiveItem> = sessions
        .iter()
        .take(20) // Limit to 20 most recent
        .map(|session| {
            let is_current = current_session_id
                .map(|id| id == session.id)
                .unwrap_or(false);

            let title = if session.title.is_empty() {
                "Untitled"
            } else {
                &session.title
            };

            // Format the date
            let date = session.updated_at.format("%m/%d %H:%M").to_string();

            let description = format!("{} - {} msgs", date, session.message_count);

            // Short ID for display
            let short_id = if session.id.len() >= 8 {
                &session.id[..8]
            } else {
                &session.id
            };

            let icon = if is_current { '>' } else { '-' };

            InteractiveItem::new(&session.id, title)
                .with_icon(icon)
                .with_description(description)
                .with_current(is_current)
                .with_metadata(short_id.to_string())
        })
        .collect();

    // Add "New Session" option at the top
    items.insert(
        0,
        InteractiveItem::new("__new__", "+ New Session")
            .with_icon('+')
            .with_description("Start a fresh conversation")
            .with_shortcut('n'),
    );

    let title = if sessions.is_empty() {
        "Sessions".to_string()
    } else {
        format!("Sessions ({})", sessions.len())
    };

    InteractiveState::new(title, items, InteractiveAction::SelectSession)
        .with_search()
        .with_max_visible(12)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn create_test_session(id: &str, title: &str) -> SessionSummary {
        SessionSummary {
            id: id.to_string(),
            title: title.to_string(),
            message_count: 5,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            archived: false,
            provider: "test".to_string(),
            model: "test-model".to_string(),
        }
    }

    #[test]
    fn test_build_sessions_selector_empty() {
        let state = build_sessions_selector(&[], None);
        // Should have "New Session" option
        assert_eq!(state.items.len(), 1);
        assert_eq!(state.items[0].id, "__new__");
    }

    #[test]
    fn test_build_sessions_selector_with_sessions() {
        let sessions = vec![
            create_test_session("abc123", "Test Session 1"),
            create_test_session("def456", "Test Session 2"),
        ];
        let state = build_sessions_selector(&sessions, Some("abc123"));
        // New Session + 2 sessions
        assert_eq!(state.items.len(), 3);
        // Second item should be current
        assert!(state.items[1].is_current);
    }
}
