//! Sessions Card
//!
//! A card for managing sessions - listing, resuming, forking, and deleting.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::Widget;

use cortex_core::style::{CYAN_PRIMARY, SURFACE_0, TEXT, TEXT_DIM, YELLOW};

use crate::widgets::{SelectionItem, SelectionList, SelectionResult};

use super::{CancellationEvent, CardAction, CardResult, CardView};

// ============================================================
// SESSION INFO
// ============================================================

/// Information about a session for display in the card.
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

    /// Formats the creation time as a relative string.
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

// ============================================================
// SESSION ACTION
// ============================================================

/// Actions that can be performed on a session.
#[derive(Debug, Clone, PartialEq, Eq)]
enum SessionAction {
    /// Resume the selected session.
    Resume,
    /// Fork the selected session.
    Fork,
    /// Delete the selected session.
    Delete,
    /// Confirm a dangerous action.
    Confirm(Box<SessionAction>),
}

// ============================================================
// SESSIONS CARD
// ============================================================

/// A card for managing sessions.
pub struct SessionsCard {
    /// Session information list.
    sessions: Vec<SessionInfo>,
    /// Selection list widget.
    list: SelectionList,
    /// Current action mode (for confirmations).
    action_mode: Option<SessionAction>,
}

impl SessionsCard {
    /// Creates a new sessions card with the given sessions.
    ///
    /// Sessions are automatically sorted by most recent first.
    pub fn new(mut sessions: Vec<SessionInfo>) -> Self {
        // Sort by most recent first
        sessions.sort_by(|a, b| b.created_at.cmp(&a.created_at));

        // Create selection items from sessions
        let items: Vec<SelectionItem> = sessions
            .iter()
            .map(|session| {
                let description = format!(
                    "{} | {} | {} messages",
                    session.short_model(),
                    session.format_time(),
                    session.message_count
                );
                SelectionItem::new(&session.name).with_description(description)
            })
            .collect();

        let max_visible = sessions.len().clamp(5, 15);
        let list = SelectionList::new(items)
            .with_searchable(true)
            .with_max_visible(max_visible);

        Self {
            sessions,
            list,
            action_mode: None,
        }
    }

    /// Gets the currently selected session info.
    fn selected_session(&self) -> Option<&SessionInfo> {
        self.list
            .selected_index()
            .and_then(|idx| self.sessions.get(idx))
    }

    /// Handles confirming an action.
    fn handle_confirm(&mut self, action: SessionAction) -> CardResult {
        match action {
            SessionAction::Resume => {
                if let Some(session) = self.selected_session() {
                    return CardResult::Action(CardAction::SelectSession(session.path.clone()));
                }
            }
            SessionAction::Fork => {
                if let Some(session) = self.selected_session() {
                    // Fork is implemented as a custom action with the session path
                    return CardResult::Action(CardAction::Custom(format!(
                        "fork:{}",
                        session.path.display()
                    )));
                }
            }
            SessionAction::Delete => {
                if let Some(session) = self.selected_session() {
                    return CardResult::Action(CardAction::Custom(format!(
                        "delete:{}",
                        session.path.display()
                    )));
                }
            }
            SessionAction::Confirm(inner) => {
                return self.handle_confirm(*inner);
            }
        }
        CardResult::Continue
    }
}

impl CardView for SessionsCard {
    fn title(&self) -> &str {
        "Sessions"
    }

    fn desired_height(&self, max_height: u16, _width: u16) -> u16 {
        // Base height: sessions + header + search bar + padding
        let content_height = self.sessions.len() as u16 + 3;
        let min_height = 5;
        let max_desired = 15;
        content_height
            .clamp(min_height, max_desired)
            .min(max_height)
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.height < 2 || area.width < 10 {
            return;
        }

        // Check if we're in confirmation mode
        if let Some(SessionAction::Confirm(action)) = &self.action_mode {
            self.render_confirmation(area, buf, action);
            return;
        }

        // Render the selection list
        (&self.list).render(area, buf);
    }

    fn handle_key(&mut self, key: KeyEvent) -> CardResult {
        // Handle confirmation mode
        if let Some(SessionAction::Confirm(action)) = self.action_mode.take() {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    return self.handle_confirm(*action);
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    self.action_mode = None;
                    return CardResult::Continue;
                }
                _ => {
                    // Keep confirmation mode active
                    self.action_mode = Some(SessionAction::Confirm(action));
                    return CardResult::Continue;
                }
            }
        }

        // Normal mode key handling
        match key.code {
            // Resume session
            KeyCode::Enter | KeyCode::Char('r') => {
                if self.selected_session().is_some() {
                    return self.handle_confirm(SessionAction::Resume);
                }
            }
            // Fork session
            KeyCode::Char('f') => {
                if self.selected_session().is_some() {
                    return self.handle_confirm(SessionAction::Fork);
                }
            }
            // Delete session (with confirmation)
            KeyCode::Char('d') | KeyCode::Delete => {
                if self.selected_session().is_some() {
                    self.action_mode =
                        Some(SessionAction::Confirm(Box::new(SessionAction::Delete)));
                    return CardResult::Continue;
                }
            }
            // Close card
            KeyCode::Esc => {
                return CardResult::Close;
            }
            // Delegate to selection list
            _ => {
                match self.list.handle_key(key) {
                    SelectionResult::Selected(_) => {
                        // Enter was pressed, resume the session
                        return self.handle_confirm(SessionAction::Resume);
                    }
                    SelectionResult::Cancelled => {
                        return CardResult::Close;
                    }
                    SelectionResult::None => {}
                }
            }
        }

        CardResult::Continue
    }

    fn key_hints(&self) -> Vec<(&'static str, &'static str)> {
        if self.action_mode.is_some() {
            vec![("y", "confirm"), ("n", "cancel")]
        } else {
            vec![
                ("Enter", "resume"),
                ("f", "fork"),
                ("d", "delete"),
                ("Esc", "close"),
            ]
        }
    }

    fn on_cancel(&mut self) -> CancellationEvent {
        if self.action_mode.is_some() {
            self.action_mode = None;
            CancellationEvent::Handled
        } else {
            CancellationEvent::NotHandled
        }
    }

    fn is_complete(&self) -> bool {
        false
    }

    fn is_searchable(&self) -> bool {
        true
    }

    fn search_placeholder(&self) -> Option<&str> {
        Some("Search sessions...")
    }
}

impl SessionsCard {
    /// Renders the confirmation dialog.
    fn render_confirmation(&self, area: Rect, buf: &mut Buffer, action: &SessionAction) {
        let session_name = self
            .selected_session()
            .map(|s| s.name.as_str())
            .unwrap_or("Unknown");

        let (title, message, warning) = match action {
            SessionAction::Delete => (
                "Delete Session",
                format!("Delete session \"{}\"?", session_name),
                Some("This action cannot be undone."),
            ),
            SessionAction::Fork => (
                "Fork Session",
                format!("Fork session \"{}\"?", session_name),
                None,
            ),
            _ => ("Confirm", "Proceed?".to_string(), None),
        };

        // Background
        for y in area.y..area.bottom() {
            for x in area.x..area.right() {
                buf[(x, y)].set_bg(SURFACE_0);
            }
        }

        // Title
        let title_style = Style::default()
            .fg(CYAN_PRIMARY)
            .add_modifier(Modifier::BOLD);
        let title_x = area.x + (area.width.saturating_sub(title.len() as u16)) / 2;
        buf.set_string(title_x, area.y + 1, title, title_style);

        // Message
        let msg_style = Style::default().fg(TEXT);
        let msg_x = area.x + (area.width.saturating_sub(message.len() as u16)) / 2;
        buf.set_string(msg_x, area.y + 3, &message, msg_style);

        // Warning (if any)
        if let Some(warn) = warning {
            let warn_style = Style::default().fg(YELLOW).add_modifier(Modifier::ITALIC);
            let warn_x = area.x + (area.width.saturating_sub(warn.len() as u16)) / 2;
            buf.set_string(warn_x, area.y + 4, warn, warn_style);
        }

        // Prompt
        let prompt = "[y] Yes  [n] No";
        let prompt_style = Style::default().fg(TEXT_DIM);
        let prompt_x = area.x + (area.width.saturating_sub(prompt.len() as u16)) / 2;
        let prompt_y = area.bottom().saturating_sub(2);
        buf.set_string(prompt_x, prompt_y, prompt, prompt_style);
    }
}

// ============================================================
// TESTS
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn create_test_sessions() -> Vec<SessionInfo> {
        let now = Utc::now();
        vec![
            SessionInfo::new(
                PathBuf::from("/sessions/session1"),
                "First Session",
                "anthropic/claude-sonnet-4-20250514",
                now - Duration::hours(2),
                5,
            ),
            SessionInfo::new(
                PathBuf::from("/sessions/session2"),
                "Second Session",
                "openai/gpt-4",
                now - Duration::days(1),
                10,
            ),
            SessionInfo::new(
                PathBuf::from("/sessions/session3"),
                "Third Session",
                "anthropic/claude-opus-4-20250514",
                now - Duration::minutes(30),
                3,
            ),
        ]
    }

    #[test]
    fn test_sessions_card_creation() {
        let sessions = create_test_sessions();
        let card = SessionsCard::new(sessions);

        assert_eq!(card.title(), "Sessions");
        assert_eq!(card.sessions.len(), 3);
        // Sessions should be sorted by most recent first
        assert_eq!(card.sessions[0].name, "Third Session");
        assert_eq!(card.sessions[1].name, "First Session");
        assert_eq!(card.sessions[2].name, "Second Session");
    }

    #[test]
    fn test_session_info_format_time() {
        let now = Utc::now();

        let recent = SessionInfo::new(
            PathBuf::from("/test"),
            "Test",
            "model",
            now - Duration::seconds(30),
            0,
        );
        assert_eq!(recent.format_time(), "just now");

        let minutes_ago = SessionInfo::new(
            PathBuf::from("/test"),
            "Test",
            "model",
            now - Duration::minutes(5),
            0,
        );
        assert_eq!(minutes_ago.format_time(), "5 minutes ago");

        let hours_ago = SessionInfo::new(
            PathBuf::from("/test"),
            "Test",
            "model",
            now - Duration::hours(3),
            0,
        );
        assert_eq!(hours_ago.format_time(), "3 hours ago");

        let yesterday = SessionInfo::new(
            PathBuf::from("/test"),
            "Test",
            "model",
            now - Duration::days(1),
            0,
        );
        assert_eq!(yesterday.format_time(), "yesterday");
    }

    #[test]
    fn test_short_model() {
        let session = SessionInfo::new(
            PathBuf::from("/test"),
            "Test",
            "anthropic/claude-sonnet-4-20250514",
            Utc::now(),
            0,
        );
        assert_eq!(session.short_model(), "claude-sonnet-4-20250514");

        let simple = SessionInfo::new(PathBuf::from("/test"), "Test", "gpt-4", Utc::now(), 0);
        assert_eq!(simple.short_model(), "gpt-4");
    }

    #[test]
    fn test_key_hints() {
        let sessions = create_test_sessions();
        let card = SessionsCard::new(sessions);

        let hints = card.key_hints();
        assert!(hints.iter().any(|(k, _)| *k == "Enter"));
        assert!(hints.iter().any(|(k, _)| *k == "f"));
        assert!(hints.iter().any(|(k, _)| *k == "d"));
        assert!(hints.iter().any(|(k, _)| *k == "Esc"));
    }

    #[test]
    fn test_is_searchable() {
        let sessions = create_test_sessions();
        let card = SessionsCard::new(sessions);
        assert!(card.is_searchable());
    }

    #[test]
    fn test_desired_height() {
        let sessions = create_test_sessions();
        let card = SessionsCard::new(sessions);

        let height = card.desired_height(20, 80);
        assert!(height >= 5);
        assert!(height <= 15);
    }

    #[test]
    fn test_empty_sessions() {
        let card = SessionsCard::new(vec![]);
        assert_eq!(card.sessions.len(), 0);
        assert_eq!(card.desired_height(20, 80), 5); // Minimum height
    }
}
