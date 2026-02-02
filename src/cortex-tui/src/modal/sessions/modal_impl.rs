//! SessionsModal struct and Modal trait implementation.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::Widget;

use cortex_core::style::{SURFACE_0, TEXT_MUTED};

use crate::widgets::ActionBar;

use super::super::{CancelBehavior, Modal, ModalAction, ModalResult};
use super::rendering::{
    render_confirmation, render_new_session_row, render_search_bar, render_separator,
    render_session_row,
};
use super::session_action::SessionAction;
use super::session_info::SessionInfo;

/// A modal for managing sessions.
pub struct SessionsModal {
    /// Session information list.
    sessions: Vec<SessionInfo>,
    /// Currently selected index (0 = New Session, 1+ = sessions).
    selected_idx: usize,
    /// Scroll offset for long lists.
    scroll_offset: usize,
    /// Maximum visible items.
    max_visible: usize,
    /// Search query for filtering.
    search_query: String,
    /// Filtered session indices.
    filtered_indices: Vec<usize>,
    /// Current action mode (for confirmations).
    action_mode: SessionAction,
}

impl SessionsModal {
    /// Creates a new sessions modal with the given sessions.
    ///
    /// Sessions are automatically sorted by most recent first.
    /// A "New Session" option is added at the top.
    pub fn new(mut sessions: Vec<SessionInfo>) -> Self {
        // Sort by most recent first
        sessions.sort_by(|a, b| b.created_at.cmp(&a.created_at));

        let max_visible = (sessions.len() + 1).clamp(5, 12);
        let filtered_indices = (0..sessions.len()).collect();

        Self {
            sessions,
            selected_idx: 0,
            scroll_offset: 0,
            max_visible,
            search_query: String::new(),
            filtered_indices,
            action_mode: SessionAction::None,
        }
    }

    /// Gets the actual session index from the selected index.
    /// Returns None if "New Session" is selected (idx 0 maps to None).
    fn selected_session_index(&self) -> Option<usize> {
        if self.selected_idx == 0 {
            None
        } else {
            // selected_idx 1 maps to filtered_indices[0], etc.
            self.filtered_indices.get(self.selected_idx - 1).copied()
        }
    }

    /// Gets the currently selected session info.
    fn selected_session(&self) -> Option<&SessionInfo> {
        self.selected_session_index()
            .and_then(|idx| self.sessions.get(idx))
    }

    /// Checks if "New Session" is currently selected.
    fn is_new_session_selected(&self) -> bool {
        self.selected_idx == 0
    }

    /// Total number of items (New Session + filtered sessions).
    fn total_items(&self) -> usize {
        1 + self.filtered_indices.len()
    }

    /// Move selection up.
    fn move_up(&mut self) {
        if self.total_items() == 0 {
            return;
        }
        if self.selected_idx == 0 {
            self.selected_idx = self.total_items() - 1;
        } else {
            self.selected_idx -= 1;
        }
        self.ensure_visible();
    }

    /// Move selection down.
    fn move_down(&mut self) {
        if self.total_items() == 0 {
            return;
        }
        self.selected_idx = (self.selected_idx + 1) % self.total_items();
        self.ensure_visible();
    }

    /// Ensure the selected item is visible.
    fn ensure_visible(&mut self) {
        // Account for search bar (1 line) + separator after New Session (1 line)
        let effective_max = self.max_visible.saturating_sub(1);
        if self.selected_idx < self.scroll_offset {
            self.scroll_offset = self.selected_idx;
        } else if self.selected_idx >= self.scroll_offset + effective_max {
            self.scroll_offset = self.selected_idx.saturating_sub(effective_max - 1);
        }
    }

    /// Apply search filter.
    fn apply_filter(&mut self) {
        if self.search_query.is_empty() {
            self.filtered_indices = (0..self.sessions.len()).collect();
        } else {
            let query_lower = self.search_query.to_lowercase();
            self.filtered_indices = self
                .sessions
                .iter()
                .enumerate()
                .filter(|(_, s)| s.name.to_lowercase().contains(&query_lower))
                .map(|(i, _)| i)
                .collect();
        }
        // Reset selection to New Session
        self.selected_idx = 0;
        self.scroll_offset = 0;
    }

    /// Build contextual action bar.
    fn build_action_bar(&self) -> ActionBar {
        match &self.action_mode {
            SessionAction::None => {
                let mut bar = ActionBar::new().action('n', "New");

                // Only show Delete if a session is selected (not New Session)
                if self.selected_idx > 0 {
                    bar = bar.danger('d', "Delete");
                }

                bar.with_standard_hints()
            }
            SessionAction::Confirm(_) => ActionBar::new()
                .danger('y', "Yes, Delete")
                .secondary('n', "No, Cancel")
                .hint("", ""),
            SessionAction::Delete => {
                // This state shouldn't be rendered, but handle it gracefully
                ActionBar::new().with_standard_hints()
            }
        }
    }

    /// Render the session list.
    fn render_list(&self, area: Rect, buf: &mut Buffer) {
        if area.height < 2 || area.width < 10 {
            return;
        }

        // Clear background
        for y in area.y..area.bottom() {
            for x in area.x..area.right() {
                buf[(x, y)].set_bg(SURFACE_0);
            }
        }

        let mut y = area.y;
        let visible_height = area.height as usize;
        let total = self.total_items();

        // Determine visible range
        let start = self.scroll_offset;
        let end = (start + visible_height).min(total);

        for visible_idx in start..end {
            if y >= area.bottom() {
                break;
            }

            let row_area = Rect::new(area.x, y, area.width, 1);

            if visible_idx == 0 {
                // Render "New Session"
                render_new_session_row(self.selected_idx == 0, row_area, buf);
                y += 1;

                // Render separator if there are sessions and we have room
                if !self.filtered_indices.is_empty() && y < area.bottom() {
                    let sep_area = Rect::new(area.x, y, area.width, 1);
                    render_separator(sep_area, buf);
                    y += 1;
                }
            } else {
                // Render session
                let session_idx = visible_idx - 1;
                if let Some(&actual_idx) = self.filtered_indices.get(session_idx)
                    && let Some(session) = self.sessions.get(actual_idx)
                {
                    let is_selected = self.selected_idx == visible_idx;
                    render_session_row(session, is_selected, row_area, buf);
                    y += 1;
                }
            }
        }

        // Empty state
        if self.filtered_indices.is_empty() && !self.search_query.is_empty() {
            let msg = "No matching sessions";
            let msg_x = area.x + (area.width.saturating_sub(msg.len() as u16)) / 2;
            let msg_y = area.y + 2;
            if msg_y < area.bottom() {
                buf.set_string(msg_x, msg_y, msg, Style::default().fg(TEXT_MUTED));
            }
        }
    }
}

impl Modal for SessionsModal {
    fn title(&self) -> &str {
        "Sessions"
    }

    fn desired_height(&self, max_height: u16, _width: u16) -> u16 {
        // Layout: search bar (1) + New Session (1) + separator (1) + sessions + action bar (1) + padding
        let session_count = self.filtered_indices.len();
        let separator_height = if session_count > 0 { 1 } else { 0 };
        let content_height = 1 + 1 + separator_height + session_count as u16 + 1 + 1;
        let min_height = 6;
        let max_desired = 16;
        content_height
            .clamp(min_height, max_desired)
            .min(max_height)
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.height < 4 || area.width < 20 {
            return;
        }

        // Check if we're in confirmation mode
        if let SessionAction::Confirm(action) = &self.action_mode {
            // Render confirmation with action bar
            let content_area = Rect::new(area.x, area.y, area.width, area.height.saturating_sub(1));
            let action_bar_area = Rect::new(area.x, area.bottom().saturating_sub(1), area.width, 1);

            let session_name = self
                .selected_session()
                .map(|s| s.name.as_str())
                .unwrap_or("Unknown");
            render_confirmation(session_name, action, content_area, buf);

            // Render action bar
            let action_bar = self.build_action_bar();
            (&action_bar).render(action_bar_area, buf);
            return;
        }

        // Layout:
        // [Search bar]        (1 line)
        // [Session list]      (remaining - 1)
        // [Action bar]        (1 line)

        let search_area = Rect::new(area.x, area.y, area.width, 1);
        let action_bar_area = Rect::new(area.x, area.bottom().saturating_sub(1), area.width, 1);
        let list_area = Rect::new(
            area.x,
            area.y + 1,
            area.width,
            area.height.saturating_sub(2),
        );

        // Render search bar
        render_search_bar(&self.search_query, search_area, buf);

        // Render session list
        self.render_list(list_area, buf);

        // Render action bar
        let action_bar = self.build_action_bar();
        (&action_bar).render(action_bar_area, buf);
    }

    fn handle_key(&mut self, key: KeyEvent) -> ModalResult {
        // Handle confirmation mode
        if let SessionAction::Confirm(action) =
            std::mem::replace(&mut self.action_mode, SessionAction::None)
        {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    // Execute the confirmed action
                    if let SessionAction::Delete = *action
                        && let Some(session) = self.selected_session()
                    {
                        return ModalResult::Action(ModalAction::DeleteSession(
                            session.path.clone(),
                        ));
                    }
                    return ModalResult::Continue;
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    self.action_mode = SessionAction::None;
                    return ModalResult::Continue;
                }
                _ => {
                    // Keep confirmation mode active
                    self.action_mode = SessionAction::Confirm(action);
                    return ModalResult::Continue;
                }
            }
        }

        // Normal mode key handling
        match key.code {
            // Navigation
            KeyCode::Up => {
                self.move_up();
            }
            KeyCode::Down => {
                self.move_down();
            }
            KeyCode::Char('k')
                if key
                    .modifiers
                    .contains(crossterm::event::KeyModifiers::CONTROL) =>
            {
                self.move_up();
            }
            KeyCode::Char('j')
                if key
                    .modifiers
                    .contains(crossterm::event::KeyModifiers::CONTROL) =>
            {
                self.move_down();
            }

            // Select/Enter
            KeyCode::Enter => {
                if self.is_new_session_selected() {
                    return ModalResult::Action(ModalAction::NewSession);
                } else if let Some(session) = self.selected_session() {
                    return ModalResult::Action(ModalAction::SelectSession(session.path.clone()));
                }
            }

            // New session shortcut
            KeyCode::Char('n') if self.search_query.is_empty() => {
                return ModalResult::Action(ModalAction::NewSession);
            }

            // Delete session (with confirmation)
            KeyCode::Char('d') if self.search_query.is_empty() => {
                if self.selected_session().is_some() {
                    self.action_mode = SessionAction::Confirm(Box::new(SessionAction::Delete));
                    return ModalResult::Continue;
                }
            }
            KeyCode::Delete => {
                if self.selected_session().is_some() {
                    self.action_mode = SessionAction::Confirm(Box::new(SessionAction::Delete));
                    return ModalResult::Continue;
                }
            }

            // Close modal
            KeyCode::Esc => {
                if !self.search_query.is_empty() {
                    self.search_query.clear();
                    self.apply_filter();
                    return ModalResult::Continue;
                }
                return ModalResult::Close;
            }

            // Clear search (must come before generic Char handler)
            KeyCode::Char('u')
                if key
                    .modifiers
                    .contains(crossterm::event::KeyModifiers::CONTROL) =>
            {
                self.search_query.clear();
                self.apply_filter();
            }

            // Search input
            KeyCode::Char(c) => {
                self.search_query.push(c);
                self.apply_filter();
            }
            KeyCode::Backspace => {
                self.search_query.pop();
                self.apply_filter();
            }

            _ => {}
        }

        ModalResult::Continue
    }

    fn key_hints(&self) -> Vec<(&'static str, &'static str)> {
        // ActionBar handles hints now, but keep this for compatibility
        vec![]
    }

    fn on_cancel(&mut self) -> CancelBehavior {
        if matches!(self.action_mode, SessionAction::Confirm(_)) {
            self.action_mode = SessionAction::None;
            CancelBehavior::Handled
        } else if !self.search_query.is_empty() {
            self.search_query.clear();
            self.apply_filter();
            CancelBehavior::Handled
        } else {
            CancelBehavior::Close
        }
    }

    fn is_searchable(&self) -> bool {
        true
    }

    fn search_placeholder(&self) -> Option<&str> {
        Some("Search sessions...")
    }
}
