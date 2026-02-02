//! Border styles and utilities.
//!
//! Provides consistent border rendering across all components.

use cortex_core::style::{BORDER, BORDER_FOCUS, CYAN_PRIMARY};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::symbols::border::Set as BorderSet;
use ratatui::widgets::{Block, Borders, Widget};

/// Rounded border character set used throughout Cortex TUI.
pub const ROUNDED_BORDER: BorderSet = BorderSet {
    top_left: "╭",
    top_right: "╮",
    bottom_left: "╰",
    bottom_right: "╯",
    horizontal_top: "─",
    horizontal_bottom: "─",
    vertical_left: "│",
    vertical_right: "│",
};

/// Single-line border character set.
pub const SINGLE_BORDER: BorderSet = BorderSet {
    top_left: "┌",
    top_right: "┐",
    bottom_left: "└",
    bottom_right: "┘",
    horizontal_top: "─",
    horizontal_bottom: "─",
    vertical_left: "│",
    vertical_right: "│",
};

/// Double-line border character set.
pub const DOUBLE_BORDER: BorderSet = BorderSet {
    top_left: "╔",
    top_right: "╗",
    bottom_left: "╚",
    bottom_right: "╝",
    horizontal_top: "═",
    horizontal_bottom: "═",
    vertical_left: "║",
    vertical_right: "║",
};

/// ASCII-only border for maximum compatibility.
pub const ASCII_BORDER: BorderSet = BorderSet {
    top_left: "+",
    top_right: "+",
    bottom_left: "+",
    bottom_right: "+",
    horizontal_top: "-",
    horizontal_bottom: "-",
    vertical_left: "|",
    vertical_right: "|",
};

/// Border style variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BorderStyle {
    /// No border
    None,
    /// Rounded corners (default Cortex style)
    #[default]
    Rounded,
    /// Single line border
    Single,
    /// Double line border
    Double,
    /// ASCII-only for maximum terminal compatibility
    Ascii,
}

impl BorderStyle {
    /// Get the border character set for this style.
    pub fn border_set(&self) -> Option<BorderSet<'_>> {
        match self {
            BorderStyle::None => None,
            BorderStyle::Rounded => Some(ROUNDED_BORDER),
            BorderStyle::Single => Some(SINGLE_BORDER),
            BorderStyle::Double => Some(DOUBLE_BORDER),
            BorderStyle::Ascii => Some(ASCII_BORDER),
        }
    }

    /// Create a ratatui Block with this border style.
    pub fn block(&self, focused: bool) -> Block<'_> {
        let border_color = if focused { BORDER_FOCUS } else { BORDER };

        let mut block = Block::default().border_style(Style::default().fg(border_color));

        if let Some(set) = self.border_set() {
            block = block.borders(Borders::ALL).border_set(set);
        }

        block
    }
}

/// A pre-configured rounded border widget.
///
/// Use this for consistent bordered containers across the TUI.
#[derive(Clone)]
pub struct RoundedBorder<'a> {
    title: Option<&'a str>,
    focused: bool,
    border_style: BorderStyle,
}

impl<'a> RoundedBorder<'a> {
    /// Create a new rounded border.
    pub fn new() -> Self {
        Self {
            title: None,
            focused: false,
            border_style: BorderStyle::Rounded,
        }
    }

    /// Set the border title.
    pub fn title(mut self, title: &'a str) -> Self {
        self.title = Some(title);
        self
    }

    /// Set the focused state.
    pub fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }

    /// Set the border style.
    pub fn style(mut self, style: BorderStyle) -> Self {
        self.border_style = style;
        self
    }

    /// Create a ratatui Block from this configuration.
    pub fn to_block(&self) -> Block<'_> {
        let border_color = if self.focused { BORDER_FOCUS } else { BORDER };
        let title_color = if self.focused { CYAN_PRIMARY } else { BORDER };

        let mut block = Block::default()
            .borders(Borders::ALL)
            .border_set(self.border_style.border_set().unwrap_or(ROUNDED_BORDER))
            .border_style(Style::default().fg(border_color));

        if let Some(title) = self.title {
            block = block
                .title(format!(" {} ", title))
                .title_style(Style::default().fg(title_color));
        }

        block
    }

    /// Calculate the inner area after accounting for borders.
    pub fn inner(&self, area: Rect) -> Rect {
        self.to_block().inner(area)
    }
}

impl Default for RoundedBorder<'_> {
    fn default() -> Self {
        Self::new()
    }
}

impl Widget for RoundedBorder<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        self.to_block().render(area, buf);
    }
}

/// Draw a horizontal separator line.
///
/// # Arguments
/// * `buf` - The buffer to render to
/// * `y` - The y coordinate
/// * `x_start` - Starting x coordinate
/// * `width` - Width of the line
/// * `style` - Style to use for the separator
pub fn draw_horizontal_separator(buf: &mut Buffer, y: u16, x_start: u16, width: u16, style: Style) {
    for x in x_start..x_start.saturating_add(width) {
        if let Some(cell) = buf.cell_mut((x, y)) {
            cell.set_char('─').set_style(style);
        }
    }
}

/// Draw a vertical separator line.
///
/// # Arguments
/// * `buf` - The buffer to render to
/// * `x` - The x coordinate
/// * `y_start` - Starting y coordinate
/// * `height` - Height of the line
/// * `style` - Style to use for the separator
pub fn draw_vertical_separator(buf: &mut Buffer, x: u16, y_start: u16, height: u16, style: Style) {
    for y in y_start..y_start.saturating_add(height) {
        if let Some(cell) = buf.cell_mut((x, y)) {
            cell.set_char('│').set_style(style);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_border_style_set() {
        assert!(BorderStyle::None.border_set().is_none());
        assert!(BorderStyle::Rounded.border_set().is_some());
        assert!(BorderStyle::Single.border_set().is_some());
        assert!(BorderStyle::Double.border_set().is_some());
        assert!(BorderStyle::Ascii.border_set().is_some());
    }

    #[test]
    fn test_rounded_border_builder() {
        let border = RoundedBorder::new()
            .title("Test")
            .focused(true)
            .style(BorderStyle::Single);

        assert_eq!(border.title, Some("Test"));
        assert!(border.focused);
        assert_eq!(border.border_style, BorderStyle::Single);
    }

    #[test]
    fn test_rounded_border_inner() {
        let border = RoundedBorder::new();
        let area = Rect::new(0, 0, 10, 5);
        let inner = border.inner(area);

        // Inner area should be smaller by 1 on each side for borders
        assert_eq!(inner.x, 1);
        assert_eq!(inner.y, 1);
        assert_eq!(inner.width, 8);
        assert_eq!(inner.height, 3);
    }
}
