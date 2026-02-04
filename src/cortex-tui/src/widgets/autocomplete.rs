//! Autocomplete popup widget for slash commands and mentions.
//!
//! Displays a VS Code-style popup above the input with filtered suggestions
//! for commands (`/`) and mentions (`@`).
//!
//! ## Example
//!
//! ```rust,ignore
//! use cortex_tui::widgets::autocomplete::{AutocompletePopup, AutocompletePosition};
//! use cortex_tui::app::AutocompleteState;
//!
//! let state = AutocompleteState::new();
//! let popup = AutocompletePopup::new(&state)
//!     .position(AutocompletePosition::Above);
//!
//! frame.render_widget(popup, area);
//! ```

use crate::app::{AutocompleteItem, AutocompleteState, AutocompleteTrigger};
use cortex_core::style::{CYAN_PRIMARY, SURFACE_1, SURFACE_2, TEXT, TEXT_DIM, TEXT_MUTED};
use cortex_tui_components::borders::ROUNDED_BORDER;
use ratatui::prelude::*;
use ratatui::widgets::{
    Block, Borders, Clear, Scrollbar, ScrollbarOrientation, ScrollbarState, StatefulWidget, Widget,
};

// ============================================================
// CONSTANTS
// ============================================================

/// Default popup width.
const DEFAULT_WIDTH: u16 = 50;

/// Minimum popup width.
const MIN_WIDTH: u16 = 30;

/// Maximum popup width.
const MAX_WIDTH: u16 = 80;

/// Height per item.
const ITEM_HEIGHT: u16 = 1;

/// Padding inside the popup.
const PADDING: u16 = 1;

// ============================================================
// AUTOCOMPLETE POPUP
// ============================================================

/// Autocomplete popup widget.
///
/// Displays filtered completions in a popup above/below the input.
/// Supports keyboard navigation (Up/Down) and selection (Tab/Enter).
pub struct AutocompletePopup<'a> {
    state: &'a AutocompleteState,
    /// Maximum width of the popup.
    max_width: u16,
}

impl<'a> AutocompletePopup<'a> {
    /// Creates a new autocomplete popup.
    pub fn new(state: &'a AutocompleteState) -> Self {
        Self {
            state,
            max_width: DEFAULT_WIDTH,
        }
    }

    /// Sets the maximum width of the popup.
    pub fn max_width(mut self, width: u16) -> Self {
        self.max_width = width.clamp(MIN_WIDTH, MAX_WIDTH);
        self
    }

    /// Calculates the required popup dimensions.
    fn calculate_dimensions(&self) -> (u16, u16) {
        let item_count = self.state.visible_items().len() as u16;
        let height = item_count * ITEM_HEIGHT + 2; // +2 for borders

        // Calculate width based on visible/filtered items only (not all items)
        // This prevents the popup from being too wide when the filtered list is smaller
        let content_width = self
            .state
            .visible_items()
            .iter()
            .map(|item| {
                let icon_width = if item.icon != '\0' { 2 } else { 0 };
                let label_width = item.label.chars().count();
                let desc_width = if item.description.is_empty() {
                    0
                } else {
                    item.description.chars().count() + 3 // " - " separator
                };
                icon_width + label_width + desc_width
            })
            .max()
            .unwrap_or(20) as u16;

        let width = (content_width + PADDING * 2 + 2) // +2 for borders
            .max(MIN_WIDTH)
            .min(self.max_width);

        (width, height)
    }

    /// Renders a single item.
    fn render_item(
        &self,
        item: &AutocompleteItem,
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

        // Icon
        if item.icon != '\0' {
            let icon_style = Style::default().fg(CYAN_PRIMARY).bg(bg);
            if let Some(cell) = buf.cell_mut((x, area.y)) {
                cell.set_char(item.icon).set_style(icon_style);
            }
            x += 2;
        }

        // Label
        let label_style = if is_selected {
            Style::default()
                .fg(CYAN_PRIMARY)
                .bg(bg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(TEXT).bg(bg)
        };

        for ch in item.label.chars() {
            if x >= area.x + area.width - 1 {
                break;
            }
            if let Some(cell) = buf.cell_mut((x, area.y)) {
                cell.set_char(ch).set_style(label_style);
            }
            x += 1;
        }

        // Description (if there's room)
        if !item.description.is_empty() && x < area.x + area.width - 5 {
            // Add separator
            let sep_style = Style::default().fg(TEXT_MUTED).bg(bg);
            for ch in " - ".chars() {
                if x >= area.x + area.width - 1 {
                    break;
                }
                if let Some(cell) = buf.cell_mut((x, area.y)) {
                    cell.set_char(ch).set_style(sep_style);
                }
                x += 1;
            }

            // Description text
            let desc_style = Style::default().fg(TEXT_DIM).bg(bg);
            for ch in item.description.chars() {
                if x >= area.x + area.width - 1 {
                    break;
                }
                if let Some(cell) = buf.cell_mut((x, area.y)) {
                    cell.set_char(ch).set_style(desc_style);
                }
                x += 1;
            }
        }

        // Selection indicator
        if is_selected {
            let indicator_style = Style::default().fg(CYAN_PRIMARY).bg(bg);
            if let Some(cell) = buf.cell_mut((area.x, area.y)) {
                cell.set_char('>').set_style(indicator_style);
            }
        }
    }

    /// Gets the title based on trigger type.
    fn get_title(&self) -> &'static str {
        match self.state.trigger {
            Some(AutocompleteTrigger::Command) => " Commands ",
            Some(AutocompleteTrigger::Mention) => " Mentions ",
            None => " Autocomplete ",
        }
    }
}

impl Widget for AutocompletePopup<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Don't render if not visible or no items
        if !self.state.visible || self.state.items.is_empty() {
            return;
        }

        let (width, height) = self.calculate_dimensions();

        // Position the popup above the input area if there's room, otherwise below
        // This prevents the popup from going off-screen at the top
        let y = if area.y >= height {
            // Enough room above - position popup above the input
            area.y.saturating_sub(height)
        } else {
            // Not enough room above - position popup below the input
            area.bottom()
        };

        let popup_area = Rect {
            x: area.x,
            y,
            width: width.min(area.width),
            height,
        };

        // Clear the background
        Clear.render(popup_area, buf);

        // Draw border with rounded corners
        let title = self.get_title();
        let block = Block::default()
            .title(title)
            .title_style(
                Style::default()
                    .fg(CYAN_PRIMARY)
                    .add_modifier(Modifier::BOLD),
            )
            .borders(Borders::ALL)
            .border_set(ROUNDED_BORDER)
            .border_style(Style::default().fg(CYAN_PRIMARY));

        let inner = block.inner(popup_area);
        block.render(popup_area, buf);

        // Check if we need scrollbar
        let needs_scrollbar = self.state.items.len() > self.state.max_visible;

        // Render items
        let visible_items = self.state.visible_items();
        for (i, item) in visible_items.iter().enumerate() {
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

            let item_area = Rect {
                x: inner.x,
                y,
                width: item_width,
                height: 1,
            };

            let is_selected = self.state.scroll_offset + i == self.state.selected;
            self.render_item(item, is_selected, item_area, buf);
        }

        // Render scrollbar if needed
        if needs_scrollbar {
            // Create scrollbar state
            // content_length = total items minus visible items (scrollable range)
            // position = scroll_offset for proper thumb position reflecting viewport
            let total_items = self.state.items.len();
            let scrollable_range = total_items.saturating_sub(self.state.max_visible);
            let mut scrollbar_state =
                ScrollbarState::new(scrollable_range).position(self.state.scroll_offset);

            // Define scrollbar area (right side of the inner content area)
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
// MENTION TYPES
// ============================================================

/// Standard mention types for context references.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MentionType {
    /// @file - Add a file to context
    File,
    /// @folder - Add a folder to context
    Folder,
    /// @url - Fetch and add URL content
    Url,
    /// @git - Git repository info
    Git,
    /// @terminal - Recent terminal output
    Terminal,
    /// @problems - LSP diagnostics/problems
    Problems,
    /// @tree - Directory tree structure
    Tree,
}

impl MentionType {
    /// Returns all mention types.
    pub fn all() -> &'static [MentionType] {
        &[
            MentionType::File,
            MentionType::Folder,
            MentionType::Url,
            MentionType::Git,
            MentionType::Terminal,
            MentionType::Problems,
            MentionType::Tree,
        ]
    }

    /// Returns the mention name (without @).
    pub fn name(&self) -> &'static str {
        match self {
            MentionType::File => "file",
            MentionType::Folder => "folder",
            MentionType::Url => "url",
            MentionType::Git => "git",
            MentionType::Terminal => "terminal",
            MentionType::Problems => "problems",
            MentionType::Tree => "tree",
        }
    }

    /// Returns the description.
    pub fn description(&self) -> &'static str {
        match self {
            MentionType::File => "Add a file to context",
            MentionType::Folder => "Add folder contents",
            MentionType::Url => "Fetch URL content",
            MentionType::Git => "Git repository info",
            MentionType::Terminal => "Recent terminal output",
            MentionType::Problems => "LSP diagnostics",
            MentionType::Tree => "Directory tree",
        }
    }

    /// Returns the icon.
    pub fn icon(&self) -> char {
        match self {
            MentionType::File => '#',
            MentionType::Folder => '+',
            MentionType::Url => '@',
            MentionType::Git => '*',
            MentionType::Terminal => '>',
            MentionType::Problems => '!',
            MentionType::Tree => '|',
        }
    }

    /// Converts to an AutocompleteItem.
    pub fn to_item(&self) -> AutocompleteItem {
        AutocompleteItem::new(self.name(), self.name(), self.description())
            .with_icon(self.icon())
            .with_category("mentions".to_string())
    }
}

/// Gets all mention items for autocomplete.
pub fn get_mention_items() -> Vec<AutocompleteItem> {
    MentionType::all().iter().map(|m| m.to_item()).collect()
}

/// Filters mentions by query.
pub fn filter_mentions(query: &str) -> Vec<AutocompleteItem> {
    let query_lower = query.to_lowercase();
    MentionType::all()
        .iter()
        .filter(|m| {
            m.name().to_lowercase().contains(&query_lower)
                || m.description().to_lowercase().contains(&query_lower)
        })
        .map(|m| m.to_item())
        .collect()
}

// ============================================================
// TESTS
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_autocomplete_state_show_hide() {
        let mut state = AutocompleteState::new();
        assert!(!state.visible);

        state.show(AutocompleteTrigger::Command, 0);
        assert!(state.visible);
        assert_eq!(state.trigger, Some(AutocompleteTrigger::Command));

        state.hide();
        assert!(!state.visible);
        assert!(state.trigger.is_none());
    }

    #[test]
    fn test_autocomplete_navigation() {
        let mut state = AutocompleteState::new();
        state.show(AutocompleteTrigger::Command, 0);
        state.set_items(vec![
            AutocompleteItem::new("help", "help", "Show help"),
            AutocompleteItem::new("quit", "quit", "Quit application"),
            AutocompleteItem::new("clear", "clear", "Clear messages"),
        ]);

        assert_eq!(state.selected, 0);

        state.select_next();
        assert_eq!(state.selected, 1);

        state.select_next();
        assert_eq!(state.selected, 2);

        state.select_next();
        assert_eq!(state.selected, 0); // Wraps around

        state.select_prev();
        assert_eq!(state.selected, 2); // Wraps around
    }

    #[test]
    fn test_mention_types() {
        let mentions = MentionType::all();
        assert!(!mentions.is_empty());

        for mention in mentions {
            assert!(!mention.name().is_empty());
            assert!(!mention.description().is_empty());
        }
    }

    #[test]
    fn test_filter_mentions() {
        let results = filter_mentions("file");
        assert!(results.iter().any(|r| r.value == "file"));

        let results = filter_mentions("git");
        assert!(results.iter().any(|r| r.value == "git"));

        let results = filter_mentions("xyz");
        assert!(results.is_empty());
    }

    #[test]
    #[ignore = "TUI behavior differs across platforms"]
    fn test_completion_text() {
        let mut state = AutocompleteState::new();
        state.show(AutocompleteTrigger::Command, 0);
        state.set_items(vec![AutocompleteItem::new("help", "help", "Show help")]);

        let text = state.completion_text();
        assert_eq!(text, Some("/help"));
    }

    #[test]
    fn test_visible_items() {
        let mut state = AutocompleteState::new();
        state.max_visible = 2;
        state.show(AutocompleteTrigger::Command, 0);
        state.set_items(vec![
            AutocompleteItem::new("a", "a", ""),
            AutocompleteItem::new("b", "b", ""),
            AutocompleteItem::new("c", "c", ""),
            AutocompleteItem::new("d", "d", ""),
        ]);

        let visible = state.visible_items();
        assert_eq!(visible.len(), 2);
        assert_eq!(visible[0].value, "a");
        assert_eq!(visible[1].value, "b");

        // Scroll down
        state.select_next();
        state.select_next();
        let visible = state.visible_items();
        assert_eq!(visible[0].value, "b");
        assert_eq!(visible[1].value, "c");
    }
}
