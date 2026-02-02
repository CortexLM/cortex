//! Backtrack overlay widget for session rewind.
//!
//! Displays a modal overlay showing the conversation history,
//! allowing users to navigate and select a point to rewind to.
//!
//! ## Example
//!
//! ```rust,ignore
//! use cortex_tui::widgets::backtrack_overlay::BacktrackOverlay;
//! use cortex_tui::backtrack::{BacktrackState, MessageSnapshot, MessageRole};
//!
//! let state = BacktrackState::new();
//! let overlay = BacktrackOverlay::new(&state);
//! frame.render_widget(overlay, area);
//! ```

use crate::backtrack::{BacktrackState, MessageRole, MessageSnapshot};
use cortex_core::style::{
    CYAN_PRIMARY, PINK, PURPLE, SURFACE_1, SURFACE_2, TEXT, TEXT_DIM, TEXT_MUTED,
};
use cortex_tui_components::borders::ROUNDED_BORDER;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, Padding, Paragraph, Widget};

// ============================================================
// CONSTANTS
// ============================================================

/// Overlay width as percentage of screen.
const OVERLAY_WIDTH_PERCENT: u16 = 80;

/// Overlay height as percentage of screen.
const OVERLAY_HEIGHT_PERCENT: u16 = 60;

/// Maximum number of messages to show in the list.
const _MAX_VISIBLE_MESSAGES: usize = 20;

/// Maximum content preview length per message.
const _MAX_CONTENT_PREVIEW: usize = 80;

// ============================================================
// BACKTRACK OVERLAY
// ============================================================

/// Backtrack overlay widget for navigating conversation history.
///
/// Renders a centered modal showing user messages that can be
/// selected for rewinding the conversation.
pub struct BacktrackOverlay<'a> {
    /// Reference to the backtrack state.
    state: &'a BacktrackState,
}

impl<'a> BacktrackOverlay<'a> {
    /// Creates a new backtrack overlay.
    pub fn new(state: &'a BacktrackState) -> Self {
        Self { state }
    }

    /// Calculate the centered overlay area.
    fn calculate_area(&self, screen: Rect) -> Rect {
        let width = (screen.width * OVERLAY_WIDTH_PERCENT / 100).clamp(40, 100);
        let height = (screen.height * OVERLAY_HEIGHT_PERCENT / 100).clamp(10, 30);

        let x = (screen.width.saturating_sub(width)) / 2;
        let y = (screen.height.saturating_sub(height)) / 2;

        Rect::new(x, y, width, height)
    }

    /// Truncate content for preview.
    fn truncate_content(content: &str, max_len: usize) -> String {
        let content = content.replace('\n', " ").trim().to_string();
        if content.len() <= max_len {
            content
        } else {
            format!("{}...", &content[..max_len.saturating_sub(3)])
        }
    }

    /// Get the role icon/prefix.
    fn role_icon(role: MessageRole) -> &'static str {
        match role {
            MessageRole::User => "[User]",
            MessageRole::Assistant => "[AI]",
            MessageRole::System => "[System]",
        }
    }

    /// Render a single message item.
    fn render_message(
        &self,
        snapshot: &MessageSnapshot,
        index: usize,
        is_selected: bool,
        area: Rect,
        buf: &mut Buffer,
    ) {
        // Background
        let bg = if is_selected { SURFACE_2 } else { SURFACE_1 };
        for x in area.x..area.x + area.width {
            if let Some(cell) = buf.cell_mut((x, area.y)) {
                cell.set_bg(bg);
            }
        }

        let mut x = area.x + 1;

        // Selection indicator
        if is_selected {
            let indicator_style = Style::default().fg(CYAN_PRIMARY).bg(bg).bold();
            if let Some(cell) = buf.cell_mut((x, area.y)) {
                cell.set_char('>').set_style(indicator_style);
            }
        }
        x += 2;

        // Index number
        let index_str = format!("{}.", index + 1);
        let index_style = Style::default().fg(TEXT_DIM).bg(bg);
        for ch in index_str.chars() {
            if x >= area.x + area.width - 1 {
                break;
            }
            if let Some(cell) = buf.cell_mut((x, area.y)) {
                cell.set_char(ch).set_style(index_style);
            }
            x += 1;
        }
        x += 1;

        // Role icon
        let icon = Self::role_icon(snapshot.role);
        let icon_style = Style::default()
            .fg(if snapshot.role == MessageRole::User {
                PINK
            } else {
                PURPLE
            })
            .bg(bg);
        for ch in icon.chars() {
            if x >= area.x + area.width - 1 {
                break;
            }
            if let Some(cell) = buf.cell_mut((x, area.y)) {
                cell.set_char(ch).set_style(icon_style);
            }
            x += ch.len_utf8() as u16;
        }
        x += 1;

        // Content preview
        let max_content_width = (area.width as usize).saturating_sub((x - area.x) as usize + 12);
        let content_preview = Self::truncate_content(&snapshot.content, max_content_width);
        let content_style = if is_selected {
            Style::default().fg(TEXT).bg(bg).bold()
        } else {
            Style::default().fg(TEXT).bg(bg)
        };

        for ch in content_preview.chars() {
            if x >= area.x + area.width - 10 {
                break;
            }
            if let Some(cell) = buf.cell_mut((x, area.y)) {
                cell.set_char(ch).set_style(content_style);
            }
            x += 1;
        }

        // Timestamp (right-aligned)
        let time_str = snapshot.timestamp.format("%H:%M:%S").to_string();
        let time_style = Style::default().fg(TEXT_MUTED).bg(bg);
        let time_x = area.x + area.width - time_str.len() as u16 - 1;
        for (i, ch) in time_str.chars().enumerate() {
            if let Some(cell) = buf.cell_mut((time_x + i as u16, area.y)) {
                cell.set_char(ch).set_style(time_style);
            }
        }
    }
}

impl Widget for BacktrackOverlay<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Don't render if overlay is not active
        if !self.state.overlay_preview_active() {
            return;
        }

        // Calculate overlay position
        let overlay_area = self.calculate_area(area);

        // Clear the background (semi-transparent effect)
        Clear.render(overlay_area, buf);

        // Draw the main block
        let title = " Rewind Session ";
        let footer = " ←/→: navigate | Enter: confirm | f: fork | Esc: cancel ";

        let block = Block::default()
            .title(title)
            .title_style(Style::default().fg(CYAN_PRIMARY).bold())
            .title_bottom(Line::from(footer).centered())
            .borders(Borders::ALL)
            .border_set(ROUNDED_BORDER)
            .border_style(Style::default().fg(CYAN_PRIMARY))
            .padding(Padding::horizontal(1));

        let inner = block.inner(overlay_area);
        block.render(overlay_area, buf);

        // Get user messages only
        let user_messages: Vec<(usize, &MessageSnapshot)> = self
            .state
            .message_snapshots
            .iter()
            .enumerate()
            .filter(|(_, s)| s.role == MessageRole::User)
            .collect();

        if user_messages.is_empty() {
            // Show "no messages" text
            let text = Paragraph::new("No messages to rewind to")
                .style(Style::default().fg(TEXT_MUTED))
                .alignment(Alignment::Center);
            text.render(inner, buf);
            return;
        }

        // Calculate visible range (scroll if needed)
        let total = user_messages.len();
        let selected_idx = self.state.nth_user_message.saturating_sub(1);
        let max_visible = inner.height as usize;

        // Calculate scroll offset to keep selection visible
        let scroll_offset = if selected_idx >= max_visible {
            selected_idx - max_visible + 1
        } else {
            0
        };

        // Render messages
        for (display_idx, (original_idx, snapshot)) in user_messages
            .iter()
            .skip(scroll_offset)
            .take(max_visible)
            .enumerate()
        {
            let y = inner.y + display_idx as u16;
            if y >= inner.y + inner.height {
                break;
            }

            let item_area = Rect::new(inner.x, y, inner.width, 1);
            let user_msg_index = user_messages
                .iter()
                .position(|(i, _)| *i == *original_idx)
                .unwrap_or(0);
            let is_selected = user_msg_index + 1 == self.state.nth_user_message;

            self.render_message(snapshot, user_msg_index, is_selected, item_area, buf);
        }

        // Scroll indicators
        if scroll_offset > 0 {
            // Show "more above" indicator
            let style = Style::default().fg(TEXT_MUTED);
            if let Some(cell) =
                buf.cell_mut((overlay_area.x + overlay_area.width - 3, overlay_area.y))
            {
                cell.set_char('▲').set_style(style);
            }
        }

        if scroll_offset + max_visible < total {
            // Show "more below" indicator
            let style = Style::default().fg(TEXT_MUTED);
            if let Some(cell) = buf.cell_mut((
                overlay_area.x + overlay_area.width - 3,
                overlay_area.y + overlay_area.height - 1,
            )) {
                cell.set_char('▼').set_style(style);
            }
        }

        // Selection info at the bottom
        let info_text = format!("{} of {} messages", self.state.nth_user_message, total);
        let info_x = inner.x + (inner.width.saturating_sub(info_text.len() as u16)) / 2;
        let info_style = Style::default().fg(TEXT_DIM);

        // Only show if there's room
        if inner.height > user_messages.len().min(max_visible) as u16 + 1 {
            let info_y = inner.y + inner.height - 1;
            for (i, ch) in info_text.chars().enumerate() {
                if let Some(cell) = buf.cell_mut((info_x + i as u16, info_y)) {
                    cell.set_char(ch).set_style(info_style);
                }
            }
        }
    }
}

// ============================================================
// TESTS
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_content() {
        let short = "Hello";
        assert_eq!(BacktrackOverlay::truncate_content(short, 10), "Hello");

        let long = "This is a very long message that should be truncated";
        let truncated = BacktrackOverlay::truncate_content(long, 20);
        assert!(truncated.len() <= 20);
        assert!(truncated.ends_with("..."));
    }

    #[test]
    fn test_role_icon() {
        assert_eq!(BacktrackOverlay::role_icon(MessageRole::User), "[User]");
        assert_eq!(BacktrackOverlay::role_icon(MessageRole::Assistant), "[AI]");
        assert_eq!(BacktrackOverlay::role_icon(MessageRole::System), "[System]");
    }

    #[test]
    fn test_calculate_area() {
        let state = BacktrackState::new();
        let overlay = BacktrackOverlay::new(&state);

        let screen = Rect::new(0, 0, 100, 50);
        let area = overlay.calculate_area(screen);

        // Should be centered
        assert!(area.x > 0);
        assert!(area.y > 0);
        assert!(area.x + area.width < screen.width || area.width == screen.width);
        assert!(area.y + area.height < screen.height || area.height == screen.height);
    }
}
