//! Scrollable Dropdown Widget
//!
//! A centralized reusable dropdown component with proper scrollbar support.
//! This widget can be used by autocomplete, selection lists, and other dropdown menus.
//!
//! ## Features
//!
//! - Visual scrollbar for lists that exceed visible height
//! - Customizable styling (colors, borders)
//! - Support for both above and below positioning
//! - Scroll position tracking indicators
//!
//! ## Example
//!
//! ```rust,ignore
//! use cortex_tui::widgets::scrollable_dropdown::{ScrollableDropdown, DropdownItem};
//!
//! let items = vec![
//!     DropdownItem::new("item1", "Option 1", "Description"),
//!     DropdownItem::new("item2", "Option 2", "Another desc"),
//! ];
//!
//! let dropdown = ScrollableDropdown::new(&items)
//!     .with_selected(0)
//!     .with_scroll_offset(0)
//!     .with_max_visible(10)
//!     .with_title(" Commands ");
//!
//! frame.render_widget(dropdown, area);
//! ```

use cortex_core::style::{CYAN_PRIMARY, SURFACE_1, SURFACE_2, TEXT, TEXT_DIM, TEXT_MUTED};
use cortex_tui_components::borders::ROUNDED_BORDER;
use ratatui::prelude::*;
use ratatui::widgets::{
    Block, Borders, Clear, Scrollbar, ScrollbarOrientation, ScrollbarState, StatefulWidget, Widget,
};

// ============================================================
// DROPDOWN ITEM
// ============================================================

/// A generic item for the scrollable dropdown.
#[derive(Debug, Clone)]
pub struct DropdownItem {
    /// Unique value/identifier
    pub value: String,
    /// Display label
    pub label: String,
    /// Optional description
    pub description: String,
    /// Optional icon character
    pub icon: char,
}

impl DropdownItem {
    /// Creates a new dropdown item.
    pub fn new(
        value: impl Into<String>,
        label: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
            description: description.into(),
            icon: '\0',
        }
    }

    /// Sets the icon for this item.
    pub fn with_icon(mut self, icon: char) -> Self {
        self.icon = icon;
        self
    }
}

// ============================================================
// DROPDOWN POSITION
// ============================================================

/// Position of the dropdown relative to its anchor point.
#[derive(Debug, Clone, Copy, Default)]
pub enum DropdownPosition {
    /// Dropdown appears above the anchor (default for command palettes)
    #[default]
    Above,
    /// Dropdown appears below the anchor
    Below,
}

// ============================================================
// SCROLLBAR STYLE
// ============================================================

/// Style configuration for the scrollbar.
#[derive(Debug, Clone)]
pub struct ScrollbarStyle {
    /// Track character (background)
    pub track_symbol: &'static str,
    /// Thumb character (the movable part)
    pub thumb_symbol: &'static str,
    /// Track color
    pub track_color: Color,
    /// Thumb color
    pub thumb_color: Color,
}

impl Default for ScrollbarStyle {
    fn default() -> Self {
        Self {
            track_symbol: "│",
            thumb_symbol: "█",
            track_color: SURFACE_1,
            thumb_color: TEXT_MUTED,
        }
    }
}

// ============================================================
// SCROLLABLE DROPDOWN
// ============================================================

/// A reusable scrollable dropdown widget.
///
/// This component provides a consistent scrolling experience across
/// all dropdown menus in the TUI, including:
/// - Autocomplete popups
/// - Selection lists
/// - Command palettes
pub struct ScrollableDropdown<'a> {
    /// Items to display
    items: &'a [DropdownItem],
    /// Currently selected index
    selected: usize,
    /// Scroll offset (first visible item index)
    scroll_offset: usize,
    /// Maximum number of visible items
    max_visible: usize,
    /// Title for the dropdown border
    title: Option<&'a str>,
    /// Position relative to anchor
    position: DropdownPosition,
    /// Border color
    border_color: Color,
    /// Scrollbar style
    scrollbar_style: ScrollbarStyle,
    /// Whether to show the scrollbar
    show_scrollbar: bool,
}

impl<'a> ScrollableDropdown<'a> {
    /// Creates a new scrollable dropdown.
    pub fn new(items: &'a [DropdownItem]) -> Self {
        Self {
            items,
            selected: 0,
            scroll_offset: 0,
            max_visible: 10,
            title: None,
            position: DropdownPosition::Above,
            border_color: CYAN_PRIMARY,
            scrollbar_style: ScrollbarStyle::default(),
            show_scrollbar: true,
        }
    }

    /// Sets the currently selected item index.
    pub fn with_selected(mut self, selected: usize) -> Self {
        self.selected = selected;
        self
    }

    /// Sets the scroll offset.
    pub fn with_scroll_offset(mut self, offset: usize) -> Self {
        self.scroll_offset = offset;
        self
    }

    /// Sets the maximum number of visible items.
    pub fn with_max_visible(mut self, max: usize) -> Self {
        self.max_visible = max.max(1);
        self
    }

    /// Sets the dropdown title.
    pub fn with_title(mut self, title: &'a str) -> Self {
        self.title = Some(title);
        self
    }

    /// Sets the dropdown position.
    pub fn with_position(mut self, position: DropdownPosition) -> Self {
        self.position = position;
        self
    }

    /// Sets the border color.
    pub fn with_border_color(mut self, color: Color) -> Self {
        self.border_color = color;
        self
    }

    /// Sets the scrollbar style.
    pub fn with_scrollbar_style(mut self, style: ScrollbarStyle) -> Self {
        self.scrollbar_style = style;
        self
    }

    /// Sets whether to show the scrollbar.
    pub fn with_scrollbar(mut self, show: bool) -> Self {
        self.show_scrollbar = show;
        self
    }

    /// Calculates the required popup dimensions.
    fn calculate_dimensions(&self) -> (u16, u16) {
        let item_count = self.visible_item_count() as u16;
        let height = item_count + 2; // +2 for borders

        // Calculate width based on content
        let content_width = self
            .items
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

        // Account for scrollbar space if needed
        let scrollbar_width = if self.needs_scrollbar() { 2 } else { 0 };
        let width = (content_width + 4 + scrollbar_width).clamp(30, 80); // padding + borders + scrollbar

        (width, height)
    }

    /// Returns the number of visible items.
    fn visible_item_count(&self) -> usize {
        self.items.len().min(self.max_visible)
    }

    /// Returns whether scrollbar is needed.
    fn needs_scrollbar(&self) -> bool {
        self.items.len() > self.max_visible
    }

    /// Returns visible items slice.
    fn visible_items(&self) -> &[DropdownItem] {
        if self.max_visible == 0 || self.items.is_empty() {
            return &[];
        }
        let start = self.scroll_offset.min(self.items.len());
        let end = (start + self.max_visible).min(self.items.len());
        self.items.get(start..end).unwrap_or(&[])
    }

    /// Renders a single item.
    fn render_item(&self, item: &DropdownItem, is_selected: bool, area: Rect, buf: &mut Buffer) {
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

    /// Renders the scrollbar if needed.
    fn render_scrollbar(&self, area: Rect, buf: &mut Buffer) {
        if !self.show_scrollbar || !self.needs_scrollbar() {
            return;
        }

        // Create scrollbar state
        // content_length = total items minus visible items (scrollable range)
        // position = scroll_offset for proper thumb position reflecting viewport
        let total_items = self.items.len();
        let scrollable_range = total_items.saturating_sub(self.max_visible);
        let mut scrollbar_state =
            ScrollbarState::new(scrollable_range).position(self.scroll_offset);

        // Define scrollbar area (right side of the inner content area)
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
            .track_symbol(Some(self.scrollbar_style.track_symbol))
            .track_style(Style::default().fg(self.scrollbar_style.track_color))
            .thumb_symbol(self.scrollbar_style.thumb_symbol)
            .thumb_style(Style::default().fg(self.scrollbar_style.thumb_color))
            .render(scrollbar_area, buf, &mut scrollbar_state);
    }
}

impl Widget for ScrollableDropdown<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Don't render if no items
        if self.items.is_empty() {
            return;
        }

        let (width, height) = self.calculate_dimensions();

        // Position the popup based on setting
        let popup_area = match self.position {
            DropdownPosition::Above => Rect {
                x: area.x,
                y: area.y.saturating_sub(height),
                width: width.min(area.width),
                height,
            },
            DropdownPosition::Below => Rect {
                x: area.x,
                y: area.y,
                width: width.min(area.width),
                height: height.min(area.height),
            },
        };

        // Clear the background
        Clear.render(popup_area, buf);

        // Draw border with rounded corners
        let mut block = Block::default()
            .borders(Borders::ALL)
            .border_set(ROUNDED_BORDER)
            .border_style(Style::default().fg(self.border_color));

        if let Some(title) = self.title {
            block = block.title(title).title_style(
                Style::default()
                    .fg(self.border_color)
                    .add_modifier(Modifier::BOLD),
            );
        }

        let inner = block.inner(popup_area);
        block.render(popup_area, buf);

        // Render items
        let visible_items = self.visible_items();
        for (i, item) in visible_items.iter().enumerate() {
            let y = inner.y + i as u16;
            if y >= inner.y + inner.height {
                break;
            }

            // Reserve space for scrollbar on the right
            let item_width = if self.needs_scrollbar() {
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

            let is_selected = self.scroll_offset + i == self.selected;
            self.render_item(item, is_selected, item_area, buf);
        }

        // Render scrollbar
        self.render_scrollbar(inner, buf);
    }
}

// ============================================================
// HELPER FUNCTIONS FOR DROPDOWN STATE MANAGEMENT
// ============================================================

/// Calculates the new scroll offset after selection change.
///
/// This ensures the selected item is always visible within the viewport.
pub fn calculate_scroll_offset(
    selected: usize,
    current_offset: usize,
    max_visible: usize,
    total_items: usize,
) -> usize {
    if max_visible == 0 || total_items <= max_visible {
        return 0;
    }

    if selected < current_offset {
        // Selected item is above visible area - scroll up
        selected
    } else if selected >= current_offset + max_visible {
        // Selected item is below visible area - scroll down
        selected.saturating_sub(max_visible.saturating_sub(1))
    } else {
        // Selected item is visible - no change needed
        current_offset
    }
}

/// Moves selection up with wrap-around support.
pub fn select_prev(
    selected: usize,
    scroll_offset: usize,
    max_visible: usize,
    total_items: usize,
) -> (usize, usize) {
    if total_items == 0 || max_visible == 0 {
        return (0, 0);
    }

    let new_selected = if selected == 0 {
        total_items - 1
    } else {
        selected - 1
    };

    let new_offset = if new_selected < scroll_offset {
        new_selected
    } else if selected == 0 && total_items > max_visible {
        // Wrapped to end - show last items
        total_items.saturating_sub(max_visible)
    } else {
        scroll_offset
    };

    (new_selected, new_offset)
}

/// Moves selection down with wrap-around support.
pub fn select_next(
    selected: usize,
    scroll_offset: usize,
    max_visible: usize,
    total_items: usize,
) -> (usize, usize) {
    if total_items == 0 || max_visible == 0 {
        return (0, 0);
    }

    let new_selected = (selected + 1) % total_items;

    let new_offset = if new_selected == 0 {
        // Wrapped to start
        0
    } else if new_selected >= scroll_offset + max_visible {
        // Need to scroll down - use saturating_sub to prevent underflow
        new_selected.saturating_sub(max_visible.saturating_sub(1))
    } else {
        scroll_offset
    };

    (new_selected, new_offset)
}

// ============================================================
// TESTS
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_items(count: usize) -> Vec<DropdownItem> {
        (0..count)
            .map(|i| DropdownItem::new(format!("item{}", i), format!("Item {}", i), "Description"))
            .collect()
    }

    #[test]
    fn test_dropdown_item_creation() {
        let item = DropdownItem::new("test", "Test Label", "Test Description").with_icon('*');

        assert_eq!(item.value, "test");
        assert_eq!(item.label, "Test Label");
        assert_eq!(item.description, "Test Description");
        assert_eq!(item.icon, '*');
    }

    #[test]
    fn test_calculate_scroll_offset() {
        // Item in view - no change
        assert_eq!(calculate_scroll_offset(5, 0, 10, 20), 0);
        assert_eq!(calculate_scroll_offset(5, 5, 10, 20), 5);

        // Item above view - scroll up
        assert_eq!(calculate_scroll_offset(2, 5, 10, 20), 2);

        // Item below view - scroll down
        assert_eq!(calculate_scroll_offset(15, 5, 10, 20), 6);

        // All items fit - always 0
        assert_eq!(calculate_scroll_offset(5, 0, 20, 10), 0);
    }

    #[test]
    fn test_select_prev() {
        // Normal decrement
        let (sel, off) = select_prev(5, 0, 10, 20);
        assert_eq!(sel, 4);
        assert_eq!(off, 0);

        // Wrap to end
        let (sel, off) = select_prev(0, 0, 10, 20);
        assert_eq!(sel, 19);
        assert_eq!(off, 10); // Should show last 10 items

        // Scroll up when needed
        let (sel, off) = select_prev(5, 5, 10, 20);
        assert_eq!(sel, 4);
        assert_eq!(off, 4);
    }

    #[test]
    fn test_select_next() {
        // Normal increment
        let (sel, off) = select_next(5, 0, 10, 20);
        assert_eq!(sel, 6);
        assert_eq!(off, 0);

        // Wrap to start
        let (sel, off) = select_next(19, 10, 10, 20);
        assert_eq!(sel, 0);
        assert_eq!(off, 0);

        // Scroll down when needed
        let (sel, off) = select_next(9, 0, 10, 20);
        assert_eq!(sel, 10);
        assert_eq!(off, 1);
    }

    #[test]
    fn test_needs_scrollbar() {
        let items = create_test_items(15);
        let dropdown = ScrollableDropdown::new(&items).with_max_visible(10);
        assert!(dropdown.needs_scrollbar());

        let items = create_test_items(5);
        let dropdown = ScrollableDropdown::new(&items).with_max_visible(10);
        assert!(!dropdown.needs_scrollbar());
    }

    #[test]
    fn test_visible_items() {
        let items = create_test_items(20);
        let dropdown = ScrollableDropdown::new(&items)
            .with_max_visible(10)
            .with_scroll_offset(5);

        let visible = dropdown.visible_items();
        assert_eq!(visible.len(), 10);
        assert_eq!(visible[0].value, "item5");
        assert_eq!(visible[9].value, "item14");
    }

    #[test]
    fn test_visible_items_at_end() {
        let items = create_test_items(20);
        let dropdown = ScrollableDropdown::new(&items)
            .with_max_visible(10)
            .with_scroll_offset(15);

        let visible = dropdown.visible_items();
        assert_eq!(visible.len(), 5); // Only 5 items left (15-19)
        assert_eq!(visible[0].value, "item15");
        assert_eq!(visible[4].value, "item19");
    }

    #[test]
    fn test_empty_dropdown() {
        let items: Vec<DropdownItem> = vec![];
        let dropdown = ScrollableDropdown::new(&items);
        assert!(!dropdown.needs_scrollbar());
        assert_eq!(dropdown.visible_items().len(), 0);
    }
}
