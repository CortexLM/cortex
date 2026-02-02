//! Selection List Widget
//!
//! A reusable generic selection list component.
//! Supports navigation, shortcuts, search filtering, and various display modes.
//!
//! ## Usage
//!
//! ```ignore
//! use cortex_tui::widgets::{SelectionList, SelectionItem, SelectionResult};
//! use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
//!
//! let items = vec![
//!     SelectionItem::new("Option 1").with_shortcut('1'),
//!     SelectionItem::new("Option 2").with_shortcut('2').with_current(true),
//!     SelectionItem::new("Option 3").with_description("A longer description"),
//! ];
//!
//! let mut list = SelectionList::new(items)
//!     .with_searchable(true)
//!     .with_max_visible(10);
//!
//! // Handle key events
//! let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
//! match list.handle_key(key) {
//!     SelectionResult::Selected(idx) => println!("Selected item {}", idx),
//!     SelectionResult::Cancelled => println!("Cancelled"),
//!     SelectionResult::None => {} // Continue waiting for input
//! }
//!
//! // Render in a ratatui frame
//! frame.render_widget(&list, area);
//! ```

use cortex_core::style::{CYAN_PRIMARY, SURFACE_0, SURFACE_1, TEXT, TEXT_DIM, TEXT_MUTED, VOID};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Scrollbar, ScrollbarOrientation, ScrollbarState, StatefulWidget, Widget};
use unicode_segmentation::UnicodeSegmentation;

use crate::ui::text_utils::{MIN_TERMINAL_WIDTH, truncate_with_ellipsis};

// ============================================================
// SELECTION ITEM
// ============================================================

/// A single item in the selection list.
#[derive(Debug, Clone)]
pub struct SelectionItem {
    /// Display name of the item
    pub name: String,
    /// Optional description shown below the name
    pub description: Option<String>,
    /// Optional keyboard shortcut character
    pub shortcut: Option<char>,
    /// Whether this is the currently active item
    pub is_current: bool,
    /// Whether this is the default item
    pub is_default: bool,
    /// Whether the item is disabled
    pub disabled: bool,
    /// Reason why the item is disabled
    pub disabled_reason: Option<String>,
}

impl SelectionItem {
    /// Create a new selection item with the given name.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: None,
            shortcut: None,
            is_current: false,
            is_default: false,
            disabled: false,
            disabled_reason: None,
        }
    }

    /// Set a description for this item.
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Set a keyboard shortcut for this item.
    pub fn with_shortcut(mut self, shortcut: char) -> Self {
        self.shortcut = Some(shortcut);
        self
    }

    /// Mark this item as the current selection.
    pub fn with_current(mut self, current: bool) -> Self {
        self.is_current = current;
        self
    }

    /// Mark this item as the default option.
    pub fn with_default(mut self, default: bool) -> Self {
        self.is_default = default;
        self
    }

    /// Disable this item with an optional reason.
    pub fn with_disabled(mut self, disabled: bool, reason: Option<String>) -> Self {
        self.disabled = disabled;
        self.disabled_reason = reason;
        self
    }
}

// ============================================================
// SELECTION RESULT
// ============================================================

/// Result of handling a key event in the selection list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectionResult {
    /// No selection made yet, continue waiting
    None,
    /// Item at the given index was selected (index in original items list)
    Selected(usize),
    /// User cancelled the selection (pressed Escape)
    Cancelled,
}

// ============================================================
// SELECTION LIST
// ============================================================

/// A generic selection list widget with navigation, shortcuts, and search.
#[derive(Debug, Clone)]
pub struct SelectionList {
    /// All items in the list
    items: Vec<SelectionItem>,
    /// Currently highlighted index (in filtered list)
    selected_idx: usize,
    /// Scroll offset for long lists
    scroll_offset: usize,
    /// Maximum number of visible items
    max_visible: usize,
    /// Whether search/filtering is enabled
    is_searchable: bool,
    /// Current search query
    search_query: String,
    /// Indices into `items` that match the current filter
    filtered_indices: Vec<usize>,
}

impl SelectionList {
    /// Create a new selection list with the given items.
    pub fn new(items: Vec<SelectionItem>) -> Self {
        let len = items.len();
        let mut list = Self {
            items,
            selected_idx: 0,
            scroll_offset: 0,
            max_visible: 10,
            is_searchable: false,
            search_query: String::new(),
            filtered_indices: (0..len).collect(),
        };

        // Select the current item if one exists
        if let Some(idx) = list.filtered_indices.iter().position(|&i| {
            list.items
                .get(i)
                .is_some_and(|item| item.is_current && !item.disabled)
        }) {
            list.selected_idx = idx;
            list.ensure_visible();
        }

        list
    }

    /// Enable or disable search filtering.
    pub fn with_searchable(mut self, searchable: bool) -> Self {
        self.is_searchable = searchable;
        self
    }

    /// Set the maximum number of visible items before scrolling.
    pub fn with_max_visible(mut self, max: usize) -> Self {
        self.max_visible = max.max(1);
        self.ensure_visible();
        self
    }

    /// Handle a key event and return the result.
    pub fn handle_key(&mut self, key: KeyEvent) -> SelectionResult {
        match key.code {
            // Navigation with Ctrl modifier (works in both modes)
            KeyCode::Up => {
                self.move_up();
                SelectionResult::None
            }
            KeyCode::Down => {
                self.move_down();
                SelectionResult::None
            }
            KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.move_up();
                SelectionResult::None
            }
            KeyCode::Char('j') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.move_down();
                SelectionResult::None
            }
            // j/k navigation without modifier (only when not searchable)
            KeyCode::Char('k') if !self.is_searchable => {
                self.move_up();
                SelectionResult::None
            }
            KeyCode::Char('j') if !self.is_searchable => {
                self.move_down();
                SelectionResult::None
            }

            // Selection
            KeyCode::Enter => self.select_current(),

            // Cancellation
            KeyCode::Esc => SelectionResult::Cancelled,

            // Clear search with Ctrl+U or Ctrl+L (must come before general Char handler)
            KeyCode::Char('u') | KeyCode::Char('l')
                if self.is_searchable && key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.search_query.clear();
                self.apply_filter();
                SelectionResult::None
            }

            // Backspace for search
            // Uses grapheme-aware deletion for proper Unicode/emoji support
            KeyCode::Backspace if self.is_searchable => {
                let graphemes: Vec<&str> = self.search_query.graphemes(true).collect();
                if !graphemes.is_empty() {
                    self.search_query = graphemes[..graphemes.len() - 1].concat();
                }
                self.apply_filter();
                SelectionResult::None
            }

            // Search input (when searchable)
            KeyCode::Char(c) if self.is_searchable => {
                // Check if it's a shortcut first (only if no search query)
                if self.search_query.is_empty()
                    && let Some(result) = self.try_shortcut(c)
                {
                    return result;
                }
                // Otherwise add to search
                self.search_query.push(c);
                self.apply_filter();
                SelectionResult::None
            }

            // Shortcut keys (when not searchable)
            KeyCode::Char(c) if !self.is_searchable => {
                if let Some(result) = self.try_shortcut(c) {
                    return result;
                }
                SelectionResult::None
            }

            _ => SelectionResult::None,
        }
    }

    /// Get the actual index (in original items list) of the currently selected item.
    pub fn selected_index(&self) -> Option<usize> {
        self.filtered_indices.get(self.selected_idx).copied()
    }

    /// Get the currently selected item.
    pub fn selected_item(&self) -> Option<&SelectionItem> {
        self.selected_index().and_then(|idx| self.items.get(idx))
    }

    /// Move selection up.
    pub fn move_up(&mut self) {
        if self.filtered_indices.is_empty() {
            return;
        }

        let len = self.filtered_indices.len();
        self.selected_idx = if self.selected_idx == 0 {
            len - 1
        } else {
            self.selected_idx - 1
        };
        self.ensure_visible();
        self.skip_disabled_up();
    }

    /// Move selection down.
    pub fn move_down(&mut self) {
        if self.filtered_indices.is_empty() {
            return;
        }

        let len = self.filtered_indices.len();
        self.selected_idx = (self.selected_idx + 1) % len;
        self.ensure_visible();
        self.skip_disabled_down();
    }

    /// Select the current item.
    pub fn select_current(&mut self) -> SelectionResult {
        if let Some(actual_idx) = self.selected_index()
            && let Some(item) = self.items.get(actual_idx)
            && !item.disabled
        {
            return SelectionResult::Selected(actual_idx);
        }
        SelectionResult::None
    }

    /// Get the current search query.
    pub fn search_query(&self) -> &str {
        &self.search_query
    }

    /// Get the number of filtered items.
    pub fn filtered_len(&self) -> usize {
        self.filtered_indices.len()
    }

    /// Get the total number of items.
    pub fn total_len(&self) -> usize {
        self.items.len()
    }

    // --------------------------------------------------------
    // Private helpers
    // --------------------------------------------------------

    /// Try to select an item by its shortcut key.
    fn try_shortcut(&mut self, c: char) -> Option<SelectionResult> {
        let lower_c = c.to_ascii_lowercase();
        for (visible_idx, &actual_idx) in self.filtered_indices.iter().enumerate() {
            if let Some(item) = self.items.get(actual_idx)
                && let Some(shortcut) = item.shortcut
                && shortcut.to_ascii_lowercase() == lower_c
                && !item.disabled
            {
                self.selected_idx = visible_idx;
                return Some(SelectionResult::Selected(actual_idx));
            }
        }
        None
    }

    /// Apply the current search filter.
    fn apply_filter(&mut self) {
        let previously_selected = self.selected_index();

        if self.search_query.is_empty() {
            self.filtered_indices = (0..self.items.len()).collect();
        } else {
            let query_lower = self.search_query.to_lowercase();
            self.filtered_indices = self
                .items
                .iter()
                .enumerate()
                .filter(|(_, item)| item.name.to_lowercase().contains(&query_lower))
                .map(|(i, _)| i)
                .collect();
        }

        // Try to preserve selection
        if let Some(prev_actual) = previously_selected {
            if let Some(new_visible_idx) = self
                .filtered_indices
                .iter()
                .position(|&idx| idx == prev_actual)
            {
                self.selected_idx = new_visible_idx;
            } else {
                self.selected_idx = 0;
            }
        } else {
            self.selected_idx = 0;
        }

        self.scroll_offset = 0;
        self.ensure_visible();
    }

    /// Ensure the selected item is visible within the scroll window.
    fn ensure_visible(&mut self) {
        if self.selected_idx < self.scroll_offset {
            self.scroll_offset = self.selected_idx;
        } else if self.selected_idx >= self.scroll_offset + self.max_visible {
            self.scroll_offset = self.selected_idx.saturating_sub(self.max_visible - 1);
        }
    }

    /// Skip disabled items when moving up.
    fn skip_disabled_up(&mut self) {
        let len = self.filtered_indices.len();
        for _ in 0..len {
            if let Some(&actual_idx) = self.filtered_indices.get(self.selected_idx) {
                if self.items.get(actual_idx).is_some_and(|item| item.disabled) {
                    self.selected_idx = if self.selected_idx == 0 {
                        len - 1
                    } else {
                        self.selected_idx - 1
                    };
                } else {
                    break;
                }
            } else {
                break;
            }
        }
    }

    /// Skip disabled items when moving down.
    fn skip_disabled_down(&mut self) {
        let len = self.filtered_indices.len();
        for _ in 0..len {
            if let Some(&actual_idx) = self.filtered_indices.get(self.selected_idx) {
                if self.items.get(actual_idx).is_some_and(|item| item.disabled) {
                    self.selected_idx = (self.selected_idx + 1) % len;
                } else {
                    break;
                }
            } else {
                break;
            }
        }
    }
}

// ============================================================
// WIDGET IMPLEMENTATION
// ============================================================

impl Widget for &SelectionList {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Early return for terminals that are too narrow or short
        let min_width = MIN_TERMINAL_WIDTH.min(20);
        if area.height < 1 || area.width < min_width {
            return;
        }

        // Calculate layout: items area and optional search bar
        let (items_area, search_area) = if self.is_searchable && area.height > 2 {
            let items_height = area.height.saturating_sub(1);
            (
                Rect::new(area.x, area.y, area.width, items_height),
                Some(Rect::new(area.x, area.y + items_height, area.width, 1)),
            )
        } else {
            (area, None)
        };

        // Render items
        self.render_items(items_area, buf);

        // Render search bar if searchable
        if let Some(search_rect) = search_area {
            self.render_search_bar(search_rect, buf);
        }
    }
}

impl SelectionList {
    /// Render the list items.
    fn render_items(&self, area: Rect, buf: &mut Buffer) {
        let visible_height = area.height as usize;
        let start = self.scroll_offset;
        let end = (start + visible_height).min(self.filtered_indices.len());

        for (row_offset, visible_idx) in (start..end).enumerate() {
            let y = area.y + row_offset as u16;
            if y >= area.bottom() {
                break;
            }

            let Some(&actual_idx) = self.filtered_indices.get(visible_idx) else {
                continue;
            };
            let Some(item) = self.items.get(actual_idx) else {
                continue;
            };

            let is_selected = visible_idx == self.selected_idx;
            self.render_item(item, is_selected, area.x, y, area.width, buf);
        }

        // Show scroll indicators if needed
        if self.filtered_indices.len() > visible_height {
            self.render_scroll_indicators(area, buf);
        }

        // Empty state
        if self.filtered_indices.is_empty() {
            let msg = if self.search_query.is_empty() {
                "No items"
            } else {
                "No matches"
            };
            let x = area.x + (area.width.saturating_sub(msg.len() as u16)) / 2;
            let y = area.y + area.height / 2;
            buf.set_string(x, y, msg, Style::default().fg(TEXT_MUTED));
        }
    }

    /// Render a single item.
    fn render_item(
        &self,
        item: &SelectionItem,
        is_selected: bool,
        x: u16,
        y: u16,
        width: u16,
        buf: &mut Buffer,
    ) {
        // Determine styles based on selection and disabled state
        let (bg, fg, prefix_fg) = if is_selected {
            (CYAN_PRIMARY, VOID, VOID)
        } else if item.disabled {
            (SURFACE_0, TEXT_MUTED, TEXT_MUTED)
        } else {
            (SURFACE_0, TEXT, CYAN_PRIMARY)
        };

        // Clear the line
        for col in x..x.saturating_add(width) {
            buf[(col, y)].set_bg(bg);
        }

        let mut col = x;

        // Selection prefix: ">" for selected, " " for others
        let prefix = if is_selected { ">" } else { " " };
        buf.set_string(col, y, prefix, Style::default().fg(prefix_fg).bg(bg));
        col += 2;

        // Shortcut in brackets if present
        if let Some(shortcut) = item.shortcut {
            let shortcut_str = format!("[{}] ", shortcut);
            let shortcut_style = if is_selected || item.disabled {
                Style::default().fg(fg).bg(bg)
            } else {
                Style::default().fg(TEXT_DIM).bg(bg)
            };
            buf.set_string(col, y, &shortcut_str, shortcut_style);
            col += shortcut_str.len() as u16;
        }

        // Item name
        let name_style = if item.disabled {
            Style::default().fg(fg).bg(bg).add_modifier(Modifier::DIM)
        } else {
            Style::default().fg(fg).bg(bg)
        };
        // Calculate available width for name (leave space for markers and disabled reason)
        let used_cols = (col - x) as usize;
        let max_name_width = (width as usize).saturating_sub(used_cols + 15);
        if max_name_width < 4 {
            return; // Too narrow to render this item meaningfully
        }
        let truncated_name = truncate_with_ellipsis(&item.name, max_name_width);
        buf.set_string(col, y, &truncated_name, name_style);
        col += truncated_name.chars().count() as u16;

        // Markers: (current) or (default)
        let marker = if item.is_current {
            Some("(current)")
        } else if item.is_default {
            Some("(default)")
        } else {
            None
        };

        if let Some(marker_text) = marker {
            col += 1; // space before marker
            let marker_style = if is_selected {
                Style::default().fg(VOID).bg(bg)
            } else {
                Style::default().fg(TEXT_DIM).bg(bg)
            };
            buf.set_string(col, y, marker_text, marker_style);
        }

        // Disabled reason on the right
        if item.disabled
            && let Some(reason) = &item.disabled_reason
        {
            let reason_str = format!(" {}", reason);
            let reason_x = x + width - reason_str.len() as u16 - 1;
            if reason_x > col + 2 {
                buf.set_string(
                    reason_x,
                    y,
                    &reason_str,
                    Style::default()
                        .fg(TEXT_MUTED)
                        .bg(bg)
                        .add_modifier(Modifier::ITALIC),
                );
            }
        }
    }

    /// Render scrollbar on the right side.
    fn render_scroll_indicators(&self, area: Rect, buf: &mut Buffer) {
        // Create scrollbar state
        // content_length = total filtered items minus visible items (scrollable range)
        // position = scroll_offset for proper thumb position reflecting viewport
        let total_items = self.filtered_indices.len();
        let visible_height = area.height as usize;
        let scrollable_range = total_items.saturating_sub(visible_height);
        let mut scrollbar_state =
            ScrollbarState::new(scrollable_range).position(self.scroll_offset);

        // Define scrollbar area (right side)
        let scrollbar_area = Rect {
            x: area.right().saturating_sub(1),
            y: area.y,
            width: 1,
            height: area.height,
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

    /// Render the search bar.
    fn render_search_bar(&self, area: Rect, buf: &mut Buffer) {
        // Background
        for x in area.x..area.right() {
            buf[(x, area.y)].set_bg(SURFACE_1);
        }

        let x = area.x + 1;

        // Search icon
        buf.set_string(
            x,
            area.y,
            "/",
            Style::default().fg(CYAN_PRIMARY).bg(SURFACE_1),
        );

        // Search query or placeholder (truncate to fit available width)
        let max_query_width = (area.width as usize).saturating_sub(15);
        let display_text = if self.search_query.is_empty() {
            if max_query_width >= 17 {
                "type to filter...".to_string()
            } else if max_query_width >= 6 {
                "filter".to_string()
            } else {
                String::new()
            }
        } else {
            truncate_with_ellipsis(&self.search_query, max_query_width)
        };

        let text_style = if self.search_query.is_empty() {
            Style::default().fg(TEXT_MUTED).bg(SURFACE_1)
        } else {
            Style::default().fg(TEXT).bg(SURFACE_1)
        };

        buf.set_string(x + 2, area.y, &display_text, text_style);

        // Cursor
        let cursor_x = x + 2 + self.search_query.len() as u16;
        if cursor_x < area.right().saturating_sub(1) {
            buf[(cursor_x, area.y)].set_bg(CYAN_PRIMARY);
            buf[(cursor_x, area.y)].set_fg(VOID);
        }

        // Result count on the right
        let count_str = format!("{}/{}", self.filtered_indices.len(), self.items.len());
        let count_x = area.right().saturating_sub(count_str.len() as u16 + 1);
        if count_x > cursor_x + 2 {
            buf.set_string(
                count_x,
                area.y,
                &count_str,
                Style::default().fg(TEXT_DIM).bg(SURFACE_1),
            );
        }
    }
}

// ============================================================
// TESTS
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_items() -> Vec<SelectionItem> {
        vec![
            SelectionItem::new("Apple").with_shortcut('a'),
            SelectionItem::new("Banana")
                .with_shortcut('b')
                .with_current(true),
            SelectionItem::new("Cherry")
                .with_shortcut('c')
                .with_default(true),
            SelectionItem::new("Date").with_disabled(true, Some("Not available".to_string())),
            SelectionItem::new("Elderberry").with_description("A tasty berry"),
        ]
    }

    #[test]
    fn test_new_selects_current() {
        let list = SelectionList::new(create_test_items());
        // Should select "Banana" which is marked as current
        assert_eq!(list.selected_idx, 1);
        assert_eq!(list.selected_index(), Some(1));
    }

    #[test]
    fn test_navigation() {
        let mut list = SelectionList::new(create_test_items());
        list.selected_idx = 0;

        list.move_down();
        assert_eq!(list.selected_idx, 1);

        list.move_down();
        assert_eq!(list.selected_idx, 2);

        // Should skip disabled item (index 3) and go to 4
        list.move_down();
        assert_eq!(list.selected_idx, 4);

        list.move_up();
        // Should skip disabled item (index 3) and go to 2
        assert_eq!(list.selected_idx, 2);
    }

    #[test]
    fn test_wrap_around() {
        let mut list = SelectionList::new(create_test_items());
        list.selected_idx = 4;

        list.move_down();
        assert_eq!(list.selected_idx, 0); // Wraps to start
    }

    #[test]
    fn test_shortcut_selection() {
        let mut list = SelectionList::new(create_test_items());

        let key = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE);
        let result = list.handle_key(key);

        assert_eq!(result, SelectionResult::Selected(2)); // Cherry is at index 2
    }

    #[test]
    fn test_disabled_shortcut_ignored() {
        let mut list = SelectionList::new(create_test_items());
        // Add a shortcut to the disabled item
        list.items[3].shortcut = Some('d');

        let key = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE);
        let result = list.handle_key(key);

        // Should not select the disabled item
        assert_eq!(result, SelectionResult::None);
    }

    #[test]
    fn test_escape_cancels() {
        let mut list = SelectionList::new(create_test_items());

        let key = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        let result = list.handle_key(key);

        assert_eq!(result, SelectionResult::Cancelled);
    }

    #[test]
    fn test_enter_selects() {
        let mut list = SelectionList::new(create_test_items());
        list.selected_idx = 2;

        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        let result = list.handle_key(key);

        assert_eq!(result, SelectionResult::Selected(2));
    }

    #[test]
    fn test_enter_on_disabled_does_nothing() {
        let mut list = SelectionList::new(create_test_items());
        // Force select disabled item
        list.selected_idx = 3;
        list.filtered_indices = (0..list.items.len()).collect();

        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        let result = list.handle_key(key);

        assert_eq!(result, SelectionResult::None);
    }

    #[test]
    fn test_search_filtering() {
        let mut list = SelectionList::new(create_test_items()).with_searchable(true);

        // Type "an" to filter
        list.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        list.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));

        // Should filter to "Banana"
        assert_eq!(list.filtered_len(), 1);
        assert_eq!(list.selected_index(), Some(1)); // Banana
    }

    #[test]
    fn test_search_clear() {
        let mut list = SelectionList::new(create_test_items()).with_searchable(true);

        // Type something (use 'x' which is not a shortcut for any item)
        list.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        assert!(!list.search_query.is_empty());
        assert_eq!(list.search_query, "x");

        // Clear with Ctrl+U
        list.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        assert!(list.search_query.is_empty());
        assert_eq!(list.filtered_len(), 5);
    }

    #[test]
    fn test_max_visible() {
        let items: Vec<SelectionItem> = (0..20)
            .map(|i| SelectionItem::new(format!("Item {}", i)))
            .collect();

        let mut list = SelectionList::new(items).with_max_visible(5);
        list.selected_idx = 0;

        // Navigate down beyond visible range
        for _ in 0..7 {
            list.move_down();
        }

        // Scroll offset should have moved
        assert!(list.scroll_offset > 0);
        assert_eq!(list.selected_idx, 7);
    }

    #[test]
    fn test_selected_item() {
        let list = SelectionList::new(create_test_items());
        let item = list.selected_item().unwrap();
        assert_eq!(item.name, "Banana");
        assert!(item.is_current);
    }

    #[test]
    fn test_jk_navigation_non_searchable() {
        let mut list = SelectionList::new(create_test_items());
        list.selected_idx = 0;

        list.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
        assert_eq!(list.selected_idx, 1);

        list.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
        assert_eq!(list.selected_idx, 0);
    }

    #[test]
    fn test_ctrl_jk_navigation_searchable() {
        let mut list = SelectionList::new(create_test_items()).with_searchable(true);
        list.selected_idx = 0;

        // Plain j/k should type characters when searchable
        list.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
        assert_eq!(list.search_query, "j");

        list.search_query.clear();
        list.apply_filter();

        // Ctrl+j should navigate
        list.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL));
        assert_eq!(list.selected_idx, 1);

        list.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL));
        assert_eq!(list.selected_idx, 0);
    }
}
