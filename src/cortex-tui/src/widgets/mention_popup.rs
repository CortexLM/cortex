//! File mention popup widget.
//!
//! Displays a popup with fuzzy file search results when the user
//! types `@` followed by a query in the input.
//!
//! ## Example
//!
//! ```rust,ignore
//! use cortex_tui::widgets::mention_popup::MentionPopup;
//! use cortex_tui::mentions::FileMentionState;
//!
//! let state = FileMentionState::new();
//! let popup = MentionPopup::new(&state);
//! frame.render_widget(popup, area);
//! ```

use crate::mentions::FileMentionState;
use cortex_core::style::{CYAN_PRIMARY, PINK, SURFACE_1, SURFACE_2, TEXT, TEXT_MUTED};
use cortex_tui_components::borders::ROUNDED_BORDER;
use ratatui::prelude::*;
use ratatui::widgets::{
    Block, Borders, Clear, Scrollbar, ScrollbarOrientation, ScrollbarState, StatefulWidget, Widget,
};
use std::path::Path;

// ============================================================
// CONSTANTS
// ============================================================

/// Default popup width.
const DEFAULT_WIDTH: u16 = 60;

/// Minimum popup width.
const MIN_WIDTH: u16 = 30;

/// Maximum popup height (number of items).
const MAX_HEIGHT: u16 = 12;

/// Padding inside the popup.
const PADDING: u16 = 1;

// ============================================================
// MENTION POPUP
// ============================================================

/// Popup widget for displaying file search results.
///
/// Renders above/below the input area with matching files
/// and allows keyboard navigation.
pub struct MentionPopup<'a> {
    /// Reference to the mention state.
    state: &'a FileMentionState,

    /// Maximum width of the popup.
    max_width: u16,

    /// Position above the cursor (true) or below (false).
    above: bool,
}

impl<'a> MentionPopup<'a> {
    /// Creates a new mention popup.
    pub fn new(state: &'a FileMentionState) -> Self {
        Self {
            state,
            max_width: DEFAULT_WIDTH,
            above: true,
        }
    }

    /// Sets the maximum width of the popup.
    pub fn max_width(mut self, width: u16) -> Self {
        self.max_width = width.max(MIN_WIDTH);
        self
    }

    /// Sets whether the popup appears above (true) or below (false) the cursor.
    pub fn above(mut self, above: bool) -> Self {
        self.above = above;
        self
    }

    /// Calculates the popup dimensions.
    fn calculate_dimensions(&self, area: Rect) -> (u16, u16) {
        let item_count = self.state.visible_results().len() as u16;
        let height = (item_count + 2).min(MAX_HEIGHT + 2); // +2 for borders

        // Calculate width based on content (use chars().count() for Unicode support)
        let content_width = self
            .state
            .results()
            .iter()
            .map(|p| p.to_string_lossy().chars().count())
            .max()
            .unwrap_or(20) as u16;

        let width = (content_width + PADDING * 2 + 4) // +4 for icon and borders
            .max(MIN_WIDTH)
            .min(self.max_width)
            .min(area.width);

        (width, height)
    }

    /// Gets the file icon based on extension.
    fn file_icon(path: &Path) -> &'static str {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

        match ext {
            "rs" => "R",
            "py" => "P",
            "js" | "jsx" => "J",
            "ts" | "tsx" => "T",
            "go" => "G",
            "java" => "J",
            "c" | "cpp" | "h" | "hpp" => "C",
            "rb" => "R",
            "md" => "M",
            "json" => "J",
            "toml" | "yaml" | "yml" => "C",
            "html" => "H",
            "css" | "scss" | "sass" => "S",
            _ => "-",
        }
    }

    /// Renders a single file item.
    fn render_item(
        &self,
        path: &Path,
        _index: usize,
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
            let style = Style::default().fg(CYAN_PRIMARY).bg(bg).bold();
            if let Some(cell) = buf.cell_mut((x, area.y)) {
                cell.set_char('>').set_style(style);
            }
        }
        x += 2;

        // File icon
        let icon = Self::file_icon(path);
        let icon_style = Style::default().fg(PINK).bg(bg);
        for ch in icon.chars() {
            if x >= area.x + area.width - 1 {
                break;
            }
            if let Some(cell) = buf.cell_mut((x, area.y)) {
                cell.set_char(ch).set_style(icon_style);
            }
            x += 1;
        }
        x += 1;

        // File path
        let path_str = path.to_string_lossy();
        let style = if is_selected {
            Style::default().fg(TEXT).bg(bg).bold()
        } else {
            Style::default().fg(TEXT).bg(bg)
        };

        for ch in path_str.chars() {
            if x >= area.x + area.width - 1 {
                break;
            }
            if let Some(cell) = buf.cell_mut((x, area.y)) {
                cell.set_char(ch).set_style(style);
            }
            x += 1;
        }
    }
}

impl Widget for MentionPopup<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Don't render if not active or no results
        if !self.state.is_active() || self.state.results().is_empty() {
            return;
        }

        let (width, height) = self.calculate_dimensions(area);

        // Position the popup - check if it fits above, otherwise render below
        let fits_above = area.y >= height;
        let render_above = self.above && fits_above;

        let popup_area = if render_above {
            Rect::new(area.x, area.y.saturating_sub(height), width, height)
        } else {
            Rect::new(
                area.x,
                area.y + 1,
                width,
                height.min(area.height.saturating_sub(1)),
            )
        };

        // Clear the background
        Clear.render(popup_area, buf);

        // Draw the border
        let query = self.state.query();
        let title = if query.is_empty() {
            " @files ".to_string()
        } else {
            format!(" @{} ", query)
        };

        let block = Block::default()
            .title(title)
            .title_style(Style::default().fg(CYAN_PRIMARY).bold())
            .borders(Borders::ALL)
            .border_set(ROUNDED_BORDER)
            .border_style(Style::default().fg(CYAN_PRIMARY));

        let inner = block.inner(popup_area);
        block.render(popup_area, buf);

        // Check if we need scrollbar
        let total_results = self.state.results().len();
        let needs_scrollbar = self.state.has_more_above() || self.state.has_more_below();

        // Render items
        let visible = self.state.visible_results();
        for (i, path) in visible.iter().enumerate() {
            let y = inner.y + i as u16;
            if y >= inner.y + inner.height {
                break;
            }

            // Reserve space for scrollbar on the right if needed
            let item_width = if needs_scrollbar {
                inner.width.saturating_sub(1)
            } else {
                inner.width
            };

            let item_area = Rect::new(inner.x, y, item_width, 1);
            let is_selected = i == self.state.selected_visible();

            self.render_item(path, i, is_selected, item_area, buf);
        }

        // Render scrollbar if needed
        if needs_scrollbar {
            // Create scrollbar state
            // content_length = total items minus visible items (scrollable range)
            // position = scroll_offset for proper thumb position reflecting viewport
            let scrollable_range = total_results.saturating_sub(self.state.max_visible());
            let mut scrollbar_state =
                ScrollbarState::new(scrollable_range).position(self.state.scroll_offset());

            // Define scrollbar area (right side of inner area)
            let scrollbar_area = Rect {
                x: inner.right().saturating_sub(1),
                y: inner.y,
                width: 1,
                height: inner.height,
            };

            // Render scrollbar
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None)
                .track_symbol(Some("│"))
                .track_style(Style::default().fg(SURFACE_1))
                .thumb_symbol("█")
                .thumb_style(Style::default().fg(TEXT_MUTED))
                .render(scrollbar_area, buf, &mut scrollbar_state);
        }
    }
}

// ============================================================
// TESTS
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_file_icon() {
        assert_eq!(MentionPopup::file_icon(Path::new("lib.rs")), "R");
        assert_eq!(MentionPopup::file_icon(Path::new("main.py")), "P");
        assert_eq!(MentionPopup::file_icon(Path::new("index.js")), "J");
        assert_eq!(MentionPopup::file_icon(Path::new("unknown.xyz")), "-");
    }

    #[test]
    fn test_calculate_dimensions() {
        let mut state = FileMentionState::new();
        state.set_results(vec![
            PathBuf::from("short.rs"),
            PathBuf::from("very_long_filename_here.rs"),
        ]);

        let popup = MentionPopup::new(&state);
        let area = Rect::new(0, 0, 100, 50);
        let (width, height) = popup.calculate_dimensions(area);

        assert!(width >= MIN_WIDTH);
        assert!(height >= 2); // At least borders
    }
}
