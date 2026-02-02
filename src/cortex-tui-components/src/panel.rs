//! Panel container component.
//!
//! Panels are resizable containers that can be positioned in different areas.

use crate::borders::{BorderStyle, RoundedBorder};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::Widget;

/// Panel position within a layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PanelPosition {
    /// Main content area
    #[default]
    Center,
    /// Left sidebar
    Left,
    /// Right sidebar
    Right,
    /// Top bar
    Top,
    /// Bottom bar
    Bottom,
}

/// A panel container.
pub struct Panel<'a> {
    title: Option<&'a str>,
    position: PanelPosition,
    border_style: BorderStyle,
    focused: bool,
    size: u16,
}

impl<'a> Panel<'a> {
    /// Create a new panel.
    pub fn new() -> Self {
        Self {
            title: None,
            position: PanelPosition::Center,
            border_style: BorderStyle::Rounded,
            focused: false,
            size: 30,
        }
    }

    /// Set the panel title.
    pub fn title(mut self, title: &'a str) -> Self {
        self.title = Some(title);
        self
    }

    /// Set the panel position.
    pub fn position(mut self, position: PanelPosition) -> Self {
        self.position = position;
        self
    }

    /// Set the border style.
    pub fn border(mut self, style: BorderStyle) -> Self {
        self.border_style = style;
        self
    }

    /// Set whether the panel is focused.
    pub fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }

    /// Set the panel size (width for Left/Right, height for Top/Bottom).
    pub fn size(mut self, size: u16) -> Self {
        self.size = size;
        self
    }

    /// Calculate the panel area within the given bounds.
    pub fn calculate_area(&self, bounds: Rect) -> Rect {
        match self.position {
            PanelPosition::Center => bounds,
            PanelPosition::Left => Rect::new(
                bounds.x,
                bounds.y,
                self.size.min(bounds.width),
                bounds.height,
            ),
            PanelPosition::Right => {
                let width = self.size.min(bounds.width);
                Rect::new(
                    bounds.right().saturating_sub(width),
                    bounds.y,
                    width,
                    bounds.height,
                )
            }
            PanelPosition::Top => Rect::new(
                bounds.x,
                bounds.y,
                bounds.width,
                self.size.min(bounds.height),
            ),
            PanelPosition::Bottom => {
                let height = self.size.min(bounds.height);
                Rect::new(
                    bounds.x,
                    bounds.bottom().saturating_sub(height),
                    bounds.width,
                    height,
                )
            }
        }
    }

    /// Get the inner content area.
    pub fn inner(&self, area: Rect) -> Rect {
        RoundedBorder::new().style(self.border_style).inner(area)
    }
}

impl Default for Panel<'_> {
    fn default() -> Self {
        Self::new()
    }
}

impl Widget for Panel<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let border = RoundedBorder::new()
            .focused(self.focused)
            .style(self.border_style);

        if let Some(title) = self.title {
            border.title(title).render(area, buf);
        } else {
            border.render(area, buf);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_panel_position_left() {
        let panel = Panel::new().position(PanelPosition::Left).size(20);
        let bounds = Rect::new(0, 0, 100, 50);

        let area = panel.calculate_area(bounds);
        assert_eq!(area.x, 0);
        assert_eq!(area.width, 20);
        assert_eq!(area.height, 50);
    }

    #[test]
    fn test_panel_position_right() {
        let panel = Panel::new().position(PanelPosition::Right).size(20);
        let bounds = Rect::new(0, 0, 100, 50);

        let area = panel.calculate_area(bounds);
        assert_eq!(area.x, 80);
        assert_eq!(area.width, 20);
    }

    #[test]
    fn test_panel_builder() {
        let panel = Panel::new()
            .title("Test")
            .position(PanelPosition::Left)
            .focused(true)
            .size(30);

        assert_eq!(panel.title, Some("Test"));
        assert_eq!(panel.position, PanelPosition::Left);
        assert!(panel.focused);
        assert_eq!(panel.size, 30);
    }
}
