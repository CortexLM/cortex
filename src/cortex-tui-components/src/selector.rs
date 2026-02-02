//! Generic selection list component.
//!
//! A reusable component for selecting from a list of items with:
//! - Keyboard navigation (Up/Down, j/k, Home/End)
//! - Keyboard shortcuts
//! - Search/filtering
//! - Scroll support
//! - Current item indicator

use crate::borders::{BorderStyle, RoundedBorder};
use crate::component::{Component, ComponentResult, FocusState};
use crate::scroll::{ScrollState, Scrollable, render_scrollbar};

use cortex_core::style::{CYAN_PRIMARY, SURFACE_0, SURFACE_1, TEXT, TEXT_DIM, TEXT_MUTED};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::Widget;
use unicode_segmentation::UnicodeSegmentation;

/// An item in the selection list.
#[derive(Debug, Clone)]
pub struct SelectItem {
    /// Unique identifier for this item
    pub id: String,
    /// Display name
    pub name: String,
    /// Optional description
    pub description: Option<String>,
    /// Optional keyboard shortcut
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

impl SelectItem {
    /// Create a new selection item.
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            description: None,
            shortcut: None,
            is_current: false,
            is_default: false,
            disabled: false,
            disabled_reason: None,
        }
    }

    /// Add a description.
    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    /// Add a keyboard shortcut.
    pub fn with_shortcut(mut self, shortcut: char) -> Self {
        self.shortcut = Some(shortcut);
        self
    }

    /// Mark as current (selected) item.
    pub fn current(mut self) -> Self {
        self.is_current = true;
        self
    }

    /// Mark as default item.
    pub fn default_item(mut self) -> Self {
        self.is_default = true;
        self
    }

    /// Mark as disabled with optional reason.
    pub fn disabled(mut self, reason: Option<impl Into<String>>) -> Self {
        self.disabled = true;
        self.disabled_reason = reason.map(|r| r.into());
        self
    }
}

/// Result of selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectResult {
    /// An item was selected (returns the item ID)
    Selected(String),
    /// Selection was cancelled
    Cancelled,
    /// Continue displaying (no action taken)
    Continue,
}

/// State for the selector component.
#[derive(Debug, Clone)]
pub struct SelectorState {
    /// All items
    pub items: Vec<SelectItem>,
    /// Currently highlighted index (in filtered list)
    pub selected_idx: usize,
    /// Indices into items that match current filter
    pub filtered_indices: Vec<usize>,
    /// Search query
    pub search_query: String,
    /// Scroll state
    pub scroll: ScrollState,
    /// Maximum visible items
    pub max_visible: usize,
    /// Whether search is enabled
    pub searchable: bool,
}

impl SelectorState {
    /// Create new selector state.
    pub fn new(items: Vec<SelectItem>) -> Self {
        let len = items.len();
        let mut state = Self {
            items,
            selected_idx: 0,
            filtered_indices: (0..len).collect(),
            search_query: String::new(),
            scroll: ScrollState::new(len, 10),
            max_visible: 10,
            searchable: false,
        };

        // Select current item if one exists
        state.select_current_item();
        state
    }

    /// Set maximum visible items.
    pub fn with_max_visible(mut self, max: usize) -> Self {
        self.max_visible = max.max(1);
        self.scroll.set_visible(self.max_visible);
        self
    }

    /// Enable search/filtering.
    pub fn searchable(mut self) -> Self {
        self.searchable = true;
        self
    }

    /// Get the currently highlighted item.
    pub fn selected_item(&self) -> Option<&SelectItem> {
        self.filtered_indices
            .get(self.selected_idx)
            .and_then(|&idx| self.items.get(idx))
    }

    /// Get the ID of the currently highlighted item.
    pub fn selected_id(&self) -> Option<&str> {
        self.selected_item().map(|item| item.id.as_str())
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

    /// Move to first item.
    pub fn move_first(&mut self) {
        self.selected_idx = 0;
        self.scroll.scroll_to_top();
        self.skip_disabled_down();
    }

    /// Move to last item.
    pub fn move_last(&mut self) {
        if !self.filtered_indices.is_empty() {
            self.selected_idx = self.filtered_indices.len() - 1;
            self.scroll.scroll_to_bottom();
            self.skip_disabled_up();
        }
    }

    /// Select the currently highlighted item.
    pub fn select(&self) -> SelectResult {
        if let Some(item) = self.selected_item()
            && !item.disabled
        {
            return SelectResult::Selected(item.id.clone());
        }
        SelectResult::Continue
    }

    /// Try to select by shortcut key.
    pub fn try_shortcut(&mut self, c: char) -> SelectResult {
        let lower_c = c.to_ascii_lowercase();
        for (visible_idx, &actual_idx) in self.filtered_indices.iter().enumerate() {
            if let Some(item) = self.items.get(actual_idx)
                && let Some(shortcut) = item.shortcut
                && shortcut.to_ascii_lowercase() == lower_c
                && !item.disabled
            {
                self.selected_idx = visible_idx;
                return SelectResult::Selected(item.id.clone());
            }
        }
        SelectResult::Continue
    }

    /// Apply search filter.
    pub fn apply_filter(&mut self) {
        let previously_selected = self.filtered_indices.get(self.selected_idx).copied();

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
            if let Some(new_idx) = self
                .filtered_indices
                .iter()
                .position(|&idx| idx == prev_actual)
            {
                self.selected_idx = new_idx;
            } else {
                self.selected_idx = 0;
            }
        } else {
            self.selected_idx = 0;
        }

        self.scroll.set_total(self.filtered_indices.len());
        self.scroll.scroll_to_top();
        self.ensure_visible();
    }

    /// Clear the search query.
    pub fn clear_search(&mut self) {
        self.search_query.clear();
        self.apply_filter();
    }

    /// Handle a search character.
    pub fn search_char(&mut self, c: char) {
        self.search_query.push(c);
        self.apply_filter();
    }

    /// Handle backspace in search.
    pub fn search_backspace(&mut self) {
        let graphemes: Vec<&str> = self.search_query.graphemes(true).collect();
        if !graphemes.is_empty() {
            self.search_query = graphemes[..graphemes.len() - 1].concat();
            self.apply_filter();
        }
    }

    // Private helpers

    fn select_current_item(&mut self) {
        if let Some(idx) = self.filtered_indices.iter().position(|&i| {
            self.items
                .get(i)
                .is_some_and(|item| item.is_current && !item.disabled)
        }) {
            self.selected_idx = idx;
            self.ensure_visible();
        }
    }

    fn ensure_visible(&mut self) {
        self.scroll.ensure_visible(self.selected_idx);
    }

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

impl Scrollable for SelectorState {
    fn scroll_state(&self) -> &ScrollState {
        &self.scroll
    }

    fn scroll_state_mut(&mut self) -> &mut ScrollState {
        &mut self.scroll
    }
}

/// A selection list component.
///
/// # Example
///
/// ```rust,ignore
/// use cortex_tui_components::selector::{Selector, SelectItem, SelectResult};
///
/// let items = vec![
///     SelectItem::new("opt1", "First Option").with_shortcut('1'),
///     SelectItem::new("opt2", "Second Option").current(),
///     SelectItem::new("opt3", "Third Option").disabled(Some("Not available")),
/// ];
///
/// let mut selector = Selector::new(items)
///     .with_title("Select an option")
///     .searchable();
///
/// // Handle input
/// match selector.handle_key(key) {
///     ComponentResult::Done(SelectResult::Selected(id)) => {
///         println!("Selected: {}", id);
///     }
///     ComponentResult::Cancelled => {
///         println!("Cancelled");
///     }
///     _ => {}
/// }
/// ```
pub struct Selector {
    /// Internal state
    pub state: SelectorState,
    /// Optional title
    title: Option<String>,
    /// Border style
    border_style: BorderStyle,
    /// Whether the component has focus
    focused: bool,
}

impl Selector {
    /// Create a new selector with the given items.
    pub fn new(items: Vec<SelectItem>) -> Self {
        Self {
            state: SelectorState::new(items),
            title: None,
            border_style: BorderStyle::Rounded,
            focused: true,
        }
    }

    /// Set a title for the selector.
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Set the border style.
    pub fn with_border(mut self, style: BorderStyle) -> Self {
        self.border_style = style;
        self
    }

    /// Set maximum visible items.
    pub fn with_max_visible(mut self, max: usize) -> Self {
        self.state = self.state.with_max_visible(max);
        self
    }

    /// Enable search/filtering.
    pub fn searchable(mut self) -> Self {
        self.state = self.state.searchable();
        self
    }

    /// Render a single item.
    fn render_item(
        &self,
        item: &SelectItem,
        is_selected: bool,
        x: u16,
        y: u16,
        width: u16,
        buf: &mut Buffer,
    ) {
        // Determine styles
        let (bg, fg, prefix_fg) = if is_selected {
            (CYAN_PRIMARY, SURFACE_0, SURFACE_0)
        } else if item.disabled {
            (SURFACE_0, TEXT_MUTED, TEXT_MUTED)
        } else {
            (SURFACE_0, TEXT, CYAN_PRIMARY)
        };

        // Clear the line
        for col in x..x.saturating_add(width) {
            if let Some(cell) = buf.cell_mut((col, y)) {
                cell.set_bg(bg);
            }
        }

        let mut col = x;

        // Selection prefix
        let prefix = if is_selected { ">" } else { " " };
        if col < x + width {
            buf.set_string(col, y, prefix, Style::default().fg(prefix_fg).bg(bg));
            col += 2;
        }

        // Shortcut in brackets
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
        let max_name_width = (width as usize).saturating_sub((col - x) as usize + 15);
        let truncated_name = truncate(&item.name, max_name_width);
        buf.set_string(col, y, &truncated_name, name_style);
        col += truncated_name.len() as u16;

        // Markers
        let marker = if item.is_current {
            Some("(current)")
        } else if item.is_default {
            Some("(default)")
        } else {
            None
        };

        if let Some(marker_text) = marker {
            col += 1;
            let marker_style = if is_selected {
                Style::default().fg(SURFACE_0).bg(bg)
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

    /// Render the search bar.
    fn render_search(&self, area: Rect, buf: &mut Buffer) {
        // Background
        for x in area.x..area.right() {
            if let Some(cell) = buf.cell_mut((x, area.y)) {
                cell.set_bg(SURFACE_1);
            }
        }

        let x = area.x + 1;

        // Search icon
        buf.set_string(
            x,
            area.y,
            "/",
            Style::default().fg(CYAN_PRIMARY).bg(SURFACE_1),
        );

        // Query or placeholder
        let display_text = if self.state.search_query.is_empty() {
            "type to filter..."
        } else {
            &self.state.search_query
        };

        let text_style = if self.state.search_query.is_empty() {
            Style::default().fg(TEXT_MUTED).bg(SURFACE_1)
        } else {
            Style::default().fg(TEXT).bg(SURFACE_1)
        };

        buf.set_string(x + 2, area.y, display_text, text_style);

        // Cursor
        let cursor_x = x + 2 + self.state.search_query.len() as u16;
        if cursor_x < area.right().saturating_sub(1)
            && let Some(cell) = buf.cell_mut((cursor_x, area.y))
        {
            cell.set_bg(CYAN_PRIMARY).set_fg(SURFACE_0);
        }

        // Result count
        let count_str = format!(
            "{}/{}",
            self.state.filtered_indices.len(),
            self.state.items.len()
        );
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

impl Component for Selector {
    type Output = SelectResult;

    fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.height < 3 || area.width < 10 {
            return;
        }

        // Calculate layout
        let border = RoundedBorder::new()
            .focused(self.focused)
            .style(self.border_style);
        if let Some(title) = &self.title {
            border.clone().title(title).render(area, buf);
        } else {
            border.render(area, buf);
        }

        let inner = RoundedBorder::new().inner(area);

        // Reserve space for search bar if searchable
        let (items_area, search_area) = if self.state.searchable && inner.height > 2 {
            let items_height = inner.height.saturating_sub(1);
            (
                Rect::new(inner.x, inner.y, inner.width, items_height),
                Some(Rect::new(inner.x, inner.y + items_height, inner.width, 1)),
            )
        } else {
            (inner, None)
        };

        // Render items
        let visible_height = items_area.height as usize;
        let start = self.state.scroll.offset();
        let end = (start + visible_height).min(self.state.filtered_indices.len());

        for (row_offset, visible_idx) in (start..end).enumerate() {
            let y = items_area.y + row_offset as u16;
            if y >= items_area.bottom() {
                break;
            }

            if let Some(&actual_idx) = self.state.filtered_indices.get(visible_idx)
                && let Some(item) = self.state.items.get(actual_idx)
            {
                let is_selected = visible_idx == self.state.selected_idx;
                self.render_item(item, is_selected, items_area.x, y, items_area.width, buf);
            }
        }

        // Empty state
        if self.state.filtered_indices.is_empty() {
            let msg = if self.state.search_query.is_empty() {
                "No items"
            } else {
                "No matches"
            };
            let x = items_area.x + (items_area.width.saturating_sub(msg.len() as u16)) / 2;
            let y = items_area.y + items_area.height / 2;
            buf.set_string(x, y, msg, Style::default().fg(TEXT_MUTED));
        }

        // Scrollbar
        if self.state.scroll.needs_scrollbar() {
            let scrollbar_area = Rect::new(
                items_area.right().saturating_sub(1),
                items_area.y,
                1,
                items_area.height,
            );
            render_scrollbar(scrollbar_area, buf, &self.state.scroll);
        }

        // Search bar
        if let Some(search_rect) = search_area {
            self.render_search(search_rect, buf);
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> ComponentResult<Self::Output> {
        match key.code {
            // Navigation with Ctrl (works in both modes)
            KeyCode::Up => {
                self.state.move_up();
                ComponentResult::Handled
            }
            KeyCode::Down => {
                self.state.move_down();
                ComponentResult::Handled
            }
            KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.state.move_up();
                ComponentResult::Handled
            }
            KeyCode::Char('j') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.state.move_down();
                ComponentResult::Handled
            }
            // j/k navigation (only when not searchable)
            KeyCode::Char('k') if !self.state.searchable => {
                self.state.move_up();
                ComponentResult::Handled
            }
            KeyCode::Char('j') if !self.state.searchable => {
                self.state.move_down();
                ComponentResult::Handled
            }
            KeyCode::Home => {
                self.state.move_first();
                ComponentResult::Handled
            }
            KeyCode::End => {
                self.state.move_last();
                ComponentResult::Handled
            }
            KeyCode::PageUp => {
                self.state.page_up();
                ComponentResult::Handled
            }
            KeyCode::PageDown => {
                self.state.page_down();
                ComponentResult::Handled
            }

            // Selection
            KeyCode::Enter => match self.state.select() {
                SelectResult::Selected(id) => ComponentResult::Done(SelectResult::Selected(id)),
                _ => ComponentResult::Handled,
            },

            // Cancel
            KeyCode::Esc => {
                if self.state.searchable && !self.state.search_query.is_empty() {
                    self.state.clear_search();
                    ComponentResult::Handled
                } else {
                    ComponentResult::Done(SelectResult::Cancelled)
                }
            }

            // Clear search
            KeyCode::Char('u') | KeyCode::Char('l')
                if self.state.searchable && key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.state.clear_search();
                ComponentResult::Handled
            }

            // Backspace for search
            KeyCode::Backspace if self.state.searchable => {
                self.state.search_backspace();
                ComponentResult::Handled
            }

            // Character input
            KeyCode::Char(c) => {
                if self.state.searchable {
                    // Try shortcut first if search is empty
                    if self.state.search_query.is_empty()
                        && let SelectResult::Selected(id) = self.state.try_shortcut(c)
                    {
                        return ComponentResult::Done(SelectResult::Selected(id));
                    }
                    // Otherwise add to search
                    self.state.search_char(c);
                    ComponentResult::Handled
                } else {
                    // Try shortcut
                    if let SelectResult::Selected(id) = self.state.try_shortcut(c) {
                        return ComponentResult::Done(SelectResult::Selected(id));
                    }
                    ComponentResult::NotHandled
                }
            }

            _ => ComponentResult::NotHandled,
        }
    }

    fn focus_state(&self) -> FocusState {
        if self.focused {
            FocusState::Focused
        } else {
            FocusState::Unfocused
        }
    }

    fn set_focus(&mut self, focused: bool) {
        self.focused = focused;
    }

    fn key_hints(&self) -> Vec<(&'static str, &'static str)> {
        let mut hints = vec![("↑↓", "Navigate"), ("Enter", "Select"), ("Esc", "Cancel")];
        if self.state.searchable {
            hints.push(("/", "Search"));
        }
        hints
    }
}

/// Truncate a string to fit within the given width, adding "..." if truncated.
fn truncate(s: &str, max_width: usize) -> String {
    if max_width < 4 {
        return s.chars().take(max_width).collect();
    }

    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max_width {
        s.to_string()
    } else {
        let mut result: String = chars[..max_width - 3].iter().collect();
        result.push_str("...");
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_items() -> Vec<SelectItem> {
        vec![
            SelectItem::new("apple", "Apple").with_shortcut('a'),
            SelectItem::new("banana", "Banana")
                .with_shortcut('b')
                .current(),
            SelectItem::new("cherry", "Cherry")
                .with_shortcut('c')
                .default_item(),
            SelectItem::new("date", "Date").disabled(Some("Not available")),
            SelectItem::new("elderberry", "Elderberry"),
        ]
    }

    #[test]
    fn test_select_item_builder() {
        let item = SelectItem::new("id", "Name")
            .with_description("Desc")
            .with_shortcut('n')
            .current()
            .default_item();

        assert_eq!(item.id, "id");
        assert_eq!(item.name, "Name");
        assert_eq!(item.description, Some("Desc".to_string()));
        assert_eq!(item.shortcut, Some('n'));
        assert!(item.is_current);
        assert!(item.is_default);
    }

    #[test]
    fn test_selector_state_selects_current() {
        let state = SelectorState::new(create_test_items());
        // Should select "Banana" which is marked as current
        assert_eq!(state.selected_idx, 1);
        assert_eq!(state.selected_id(), Some("banana"));
    }

    #[test]
    fn test_selector_navigation() {
        let mut state = SelectorState::new(create_test_items());
        state.selected_idx = 0;

        state.move_down();
        assert_eq!(state.selected_idx, 1);

        state.move_down();
        assert_eq!(state.selected_idx, 2);

        // Should skip disabled item (index 3) and go to 4
        state.move_down();
        assert_eq!(state.selected_idx, 4);
    }

    #[test]
    fn test_selector_shortcut() {
        let mut state = SelectorState::new(create_test_items());

        let result = state.try_shortcut('c');
        assert_eq!(result, SelectResult::Selected("cherry".to_string()));
    }

    #[test]
    fn test_selector_search() {
        let mut state = SelectorState::new(create_test_items()).searchable();

        state.search_char('a');
        state.search_char('n');
        state.apply_filter();

        // Should match "Banana"
        assert_eq!(state.filtered_indices.len(), 1);
        assert_eq!(state.selected_id(), Some("banana"));
    }

    #[test]
    fn test_selector_clear_search() {
        let mut state = SelectorState::new(create_test_items()).searchable();

        state.search_char('x');
        state.apply_filter();
        assert!(state.filtered_indices.is_empty());

        state.clear_search();
        assert_eq!(state.filtered_indices.len(), 5);
    }

    #[test]
    fn test_truncate() {
        assert_eq!(truncate("Hello", 10), "Hello");
        assert_eq!(truncate("Hello World", 8), "Hello...");
        assert_eq!(truncate("Hi", 2), "Hi");
    }
}
