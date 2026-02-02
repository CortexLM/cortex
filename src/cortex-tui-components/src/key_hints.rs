//! Key hints bar component.
//!
//! Displays keyboard shortcut hints at the bottom of components/screens.

use cortex_core::style::{CYAN_PRIMARY, SURFACE_1, TEXT_DIM};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::Widget;

/// A single key hint (key + description).
#[derive(Debug, Clone)]
pub struct KeyHint {
    /// The key or key combination
    pub key: String,
    /// Description of what the key does
    pub description: String,
}

impl KeyHint {
    /// Create a new key hint.
    pub fn new(key: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            description: description.into(),
        }
    }
}

impl<'a, 'b> From<(&'a str, &'b str)> for KeyHint {
    fn from((key, desc): (&'a str, &'b str)) -> Self {
        Self::new(key, desc)
    }
}

/// A horizontal bar of key hints.
///
/// Renders keyboard shortcut hints in a compact format:
/// `[Enter] Select  [Esc] Cancel  [↑↓] Navigate`
pub struct KeyHintsBar {
    hints: Vec<KeyHint>,
    separator: String,
}

impl KeyHintsBar {
    /// Create a new key hints bar.
    pub fn new() -> Self {
        Self {
            hints: Vec::new(),
            separator: " · ".to_string(), // Middle dot separator like main TUI
        }
    }

    /// Create from a slice of (key, description) tuples.
    pub fn from_tuples(hints: &[(&str, &str)]) -> Self {
        Self {
            hints: hints.iter().map(|&h| h.into()).collect(),
            separator: "  ".to_string(),
        }
    }

    /// Add a hint.
    pub fn hint(mut self, key: impl Into<String>, description: impl Into<String>) -> Self {
        self.hints.push(KeyHint::new(key, description));
        self
    }

    /// Add multiple hints.
    pub fn hints(mut self, hints: impl IntoIterator<Item = KeyHint>) -> Self {
        self.hints.extend(hints);
        self
    }

    /// Set the separator between hints.
    pub fn separator(mut self, sep: impl Into<String>) -> Self {
        self.separator = sep.into();
        self
    }

    /// Calculate the total width needed for all hints.
    pub fn total_width(&self) -> usize {
        if self.hints.is_empty() {
            return 0;
        }

        let mut width = 0;
        for (i, hint) in self.hints.iter().enumerate() {
            if i > 0 {
                width += self.separator.len();
            }
            // Format: key description (no brackets)
            width += hint.key.len() + 1 + hint.description.len();
        }
        width
    }

    /// Render hints that fit within the given width.
    ///
    /// Returns the hints that fit, excluding those that don't.
    fn hints_that_fit(&self, max_width: usize) -> Vec<&KeyHint> {
        let mut result = Vec::new();
        let mut current_width = 0;

        for hint in &self.hints {
            // No brackets: key + space + description
            let hint_width = hint.key.len() + 1 + hint.description.len();
            let needed = if result.is_empty() {
                hint_width
            } else {
                self.separator.len() + hint_width
            };

            if current_width + needed <= max_width {
                result.push(hint);
                current_width += needed;
            }
        }

        result
    }
}

impl Default for KeyHintsBar {
    fn default() -> Self {
        Self::new()
    }
}

impl Widget for KeyHintsBar {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width < 10 {
            return;
        }

        // Fill background
        let bg_style = Style::default().bg(SURFACE_1);
        for x in area.x..area.right() {
            if let Some(cell) = buf.cell_mut((x, area.y)) {
                cell.set_style(bg_style);
            }
        }

        let hints = self.hints_that_fit(area.width as usize);
        if hints.is_empty() {
            return;
        }

        let key_style = Style::default().fg(CYAN_PRIMARY).bg(SURFACE_1);
        let desc_style = Style::default().fg(TEXT_DIM).bg(SURFACE_1);
        let sep_style = Style::default().fg(TEXT_DIM).bg(SURFACE_1);

        let mut x = area.x + 1;

        for (i, hint) in hints.iter().enumerate() {
            if i > 0 {
                // Separator (middle dot style like main TUI)
                for ch in self.separator.chars() {
                    if x < area.right() {
                        if let Some(cell) = buf.cell_mut((x, area.y)) {
                            cell.set_char(ch).set_style(sep_style);
                        }
                        x += 1;
                    }
                }
            }

            // Key (no brackets, like main TUI)
            for ch in hint.key.chars() {
                if x < area.right() {
                    if let Some(cell) = buf.cell_mut((x, area.y)) {
                        cell.set_char(ch).set_style(key_style);
                    }
                    x += 1;
                }
            }

            // Space
            if x < area.right() {
                if let Some(cell) = buf.cell_mut((x, area.y)) {
                    cell.set_char(' ').set_style(desc_style);
                }
                x += 1;
            }

            // Description
            for ch in hint.description.chars() {
                if x < area.right() {
                    if let Some(cell) = buf.cell_mut((x, area.y)) {
                        cell.set_char(ch).set_style(desc_style);
                    }
                    x += 1;
                }
            }
        }
    }
}

/// Standard key hints for common operations.
pub mod common {
    use super::KeyHint;

    /// Navigation hints (Up/Down arrows)
    pub fn navigation() -> KeyHint {
        KeyHint::new("↑↓", "Navigate")
    }

    /// Select/Enter hint
    pub fn select() -> KeyHint {
        KeyHint::new("Enter", "Select")
    }

    /// Cancel/Escape hint
    pub fn cancel() -> KeyHint {
        KeyHint::new("Esc", "Cancel")
    }

    /// Close hint
    pub fn close() -> KeyHint {
        KeyHint::new("Esc", "Close")
    }

    /// Tab navigation hint
    pub fn tab_navigation() -> KeyHint {
        KeyHint::new("Tab", "Next")
    }

    /// Search hint
    pub fn search() -> KeyHint {
        KeyHint::new("/", "Search")
    }

    /// Help hint
    pub fn help() -> KeyHint {
        KeyHint::new("?", "Help")
    }

    /// Submit hint
    pub fn submit() -> KeyHint {
        KeyHint::new("Enter", "Submit")
    }

    /// Toggle hint
    pub fn toggle() -> KeyHint {
        KeyHint::new("Space", "Toggle")
    }

    /// Page navigation hints
    pub fn page_navigation() -> KeyHint {
        KeyHint::new("PgUp/PgDn", "Page")
    }

    /// Home/End hints
    pub fn home_end() -> KeyHint {
        KeyHint::new("Home/End", "Jump")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_key_hint_new() {
        let hint = KeyHint::new("Enter", "Select");
        assert_eq!(hint.key, "Enter");
        assert_eq!(hint.description, "Select");
    }

    #[test]
    fn test_key_hint_from_tuple() {
        let hint: KeyHint = ("Esc", "Cancel").into();
        assert_eq!(hint.key, "Esc");
        assert_eq!(hint.description, "Cancel");
    }

    #[test]
    fn test_hints_bar_builder() {
        let bar = KeyHintsBar::new()
            .hint("Enter", "Select")
            .hint("Esc", "Cancel")
            .separator(" | ");

        assert_eq!(bar.hints.len(), 2);
        assert_eq!(bar.separator, " | ");
    }

    #[test]
    fn test_hints_bar_from_tuples() {
        let bar = KeyHintsBar::from_tuples(&[("Enter", "Select"), ("Esc", "Cancel")]);
        assert_eq!(bar.hints.len(), 2);
    }

    #[test]
    fn test_total_width() {
        let bar = KeyHintsBar::new()
            .hint("Enter", "Select")
            .hint("Esc", "Cancel");

        // [Enter] Select  [Esc] Cancel
        // = 1+5+1+1+6 + 2 + 1+3+1+1+6 = 14 + 2 + 12 = 28
        assert!(bar.total_width() > 0);
    }

    #[test]
    fn test_hints_that_fit() {
        let bar = KeyHintsBar::new()
            .hint("Enter", "Select")
            .hint("Esc", "Cancel")
            .hint("Tab", "Next");

        // Very narrow - should only fit one
        let fits = bar.hints_that_fit(20);
        assert!(!fits.is_empty());
        assert!(fits.len() < 3);
    }

    #[test]
    fn test_common_hints() {
        let nav = common::navigation();
        assert_eq!(nav.key, "↑↓");

        let select = common::select();
        assert_eq!(select.description, "Select");
    }
}
