//! Inline popup component.
//!
//! Popups that appear inline (like autocomplete) rather than as modal overlays.

use crate::borders::ROUNDED_BORDER;
use cortex_core::style::CYAN_PRIMARY;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Borders, Clear, Widget};

/// Position of the popup relative to its anchor.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PopupPosition {
    /// Above the anchor
    #[default]
    Above,
    /// Below the anchor
    Below,
    /// To the left of the anchor
    Left,
    /// To the right of the anchor
    Right,
}

/// An inline popup widget.
///
/// Unlike modals, popups are positioned relative to an anchor point
/// and don't necessarily capture all input.
pub struct Popup<'a> {
    title: Option<&'a str>,
    position: PopupPosition,
    width: u16,
    height: u16,
}

impl<'a> Popup<'a> {
    /// Create a new popup.
    pub fn new(width: u16, height: u16) -> Self {
        Self {
            title: None,
            position: PopupPosition::Above,
            width,
            height,
        }
    }

    /// Set the popup title.
    pub fn title(mut self, title: &'a str) -> Self {
        self.title = Some(title);
        self
    }

    /// Set the position.
    pub fn position(mut self, position: PopupPosition) -> Self {
        self.position = position;
        self
    }

    /// Calculate the popup area given an anchor point.
    ///
    /// # Arguments
    /// * `anchor` - The anchor point (x, y)
    /// * `bounds` - The maximum bounds for the popup
    pub fn calculate_area(&self, anchor: (u16, u16), bounds: Rect) -> Rect {
        let (anchor_x, anchor_y) = anchor;

        let (x, y) = match self.position {
            PopupPosition::Above => {
                let y = anchor_y.saturating_sub(self.height);
                (anchor_x, y)
            }
            PopupPosition::Below => (anchor_x, anchor_y + 1),
            PopupPosition::Left => {
                let x = anchor_x.saturating_sub(self.width);
                (x, anchor_y)
            }
            PopupPosition::Right => (anchor_x + 1, anchor_y),
        };

        // Clamp to bounds
        let x = x
            .max(bounds.x)
            .min(bounds.right().saturating_sub(self.width));
        let y = y
            .max(bounds.y)
            .min(bounds.bottom().saturating_sub(self.height));
        let width = self.width.min(bounds.width);
        let height = self.height.min(bounds.height);

        Rect::new(x, y, width, height)
    }

    /// Render the popup at the given area.
    pub fn render_at(&self, area: Rect, buf: &mut Buffer) {
        if area.height < 2 || area.width < 5 {
            return;
        }

        // Clear background
        Clear.render(area, buf);

        // Border
        let mut block = Block::default()
            .borders(Borders::ALL)
            .border_set(ROUNDED_BORDER)
            .border_style(Style::default().fg(CYAN_PRIMARY));

        if let Some(title) = self.title {
            block = block.title(format!(" {} ", title)).title_style(
                Style::default()
                    .fg(CYAN_PRIMARY)
                    .add_modifier(Modifier::BOLD),
            );
        }

        block.render(area, buf);
    }
}

impl Widget for Popup<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        self.render_at(area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_popup_position_above() {
        let popup = Popup::new(20, 5).position(PopupPosition::Above);
        let bounds = Rect::new(0, 0, 100, 50);

        let area = popup.calculate_area((10, 20), bounds);
        assert_eq!(area.y, 15); // 20 - 5
        assert_eq!(area.x, 10);
    }

    #[test]
    fn test_popup_position_below() {
        let popup = Popup::new(20, 5).position(PopupPosition::Below);
        let bounds = Rect::new(0, 0, 100, 50);

        let area = popup.calculate_area((10, 20), bounds);
        assert_eq!(area.y, 21); // 20 + 1
    }

    #[test]
    fn test_popup_bounds_clamping() {
        let popup = Popup::new(20, 5).position(PopupPosition::Above);
        let bounds = Rect::new(0, 0, 100, 50);

        // Anchor near top - should clamp
        let area = popup.calculate_area((10, 2), bounds);
        assert_eq!(area.y, 0); // Can't go negative
    }
}
