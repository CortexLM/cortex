//! Dropdown popup component.
//!
//! A compact dropdown menu that appears above/below an anchor point.

use crate::borders::ROUNDED_BORDER;
use crate::scroll::{ScrollState, render_scrollbar};
use cortex_core::style::{CYAN_PRIMARY, SURFACE_1, SURFACE_2, TEXT, TEXT_DIM, TEXT_MUTED};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Borders, Clear, Widget};

/// An item in the dropdown.
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
    /// Create a new dropdown item.
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

    /// Set an icon for this item.
    pub fn with_icon(mut self, icon: char) -> Self {
        self.icon = icon;
        self
    }
}

/// Position of the dropdown relative to its anchor.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DropdownPosition {
    /// Above the anchor (default for autocomplete)
    #[default]
    Above,
    /// Below the anchor
    Below,
}

/// State for a dropdown.
#[derive(Debug, Clone, Default)]
pub struct DropdownState {
    /// Whether the dropdown is visible
    pub visible: bool,
    /// All items
    pub items: Vec<DropdownItem>,
    /// Currently selected index
    pub selected: usize,
    /// Scroll state
    pub scroll: ScrollState,
    /// Maximum visible items
    pub max_visible: usize,
}

impl DropdownState {
    /// Create new dropdown state.
    pub fn new() -> Self {
        Self {
            visible: false,
            items: Vec::new(),
            selected: 0,
            scroll: ScrollState::new(0, 10),
            max_visible: 10,
        }
    }

    /// Show the dropdown with items.
    pub fn show(&mut self, items: Vec<DropdownItem>) {
        self.items = items;
        self.selected = 0;
        self.scroll = ScrollState::new(self.items.len(), self.max_visible);
        self.visible = true;
    }

    /// Hide the dropdown.
    pub fn hide(&mut self) {
        self.visible = false;
        self.items.clear();
        self.selected = 0;
    }

    /// Select the next item.
    pub fn select_next(&mut self) {
        if self.items.is_empty() || self.max_visible == 0 {
            return;
        }
        self.selected = (self.selected + 1) % self.items.len();
        self.scroll.ensure_visible(self.selected);
    }

    /// Select the previous item.
    pub fn select_prev(&mut self) {
        if self.items.is_empty() || self.max_visible == 0 {
            return;
        }
        self.selected = if self.selected == 0 {
            self.items.len() - 1
        } else {
            self.selected - 1
        };
        self.scroll.ensure_visible(self.selected);
    }

    /// Get the selected item.
    pub fn selected_item(&self) -> Option<&DropdownItem> {
        self.items.get(self.selected)
    }

    /// Get the selected value.
    pub fn selected_value(&self) -> Option<&str> {
        self.selected_item().map(|i| i.value.as_str())
    }
}

/// A dropdown popup widget.
pub struct Dropdown<'a> {
    state: &'a DropdownState,
    title: Option<&'a str>,
    position: DropdownPosition,
    max_width: u16,
}

impl<'a> Dropdown<'a> {
    /// Create a new dropdown widget.
    pub fn new(state: &'a DropdownState) -> Self {
        Self {
            state,
            title: None,
            position: DropdownPosition::Above,
            max_width: 50,
        }
    }

    /// Set the dropdown title.
    pub fn title(mut self, title: &'a str) -> Self {
        self.title = Some(title);
        self
    }

    /// Set the position.
    pub fn position(mut self, position: DropdownPosition) -> Self {
        self.position = position;
        self
    }

    /// Set the maximum width.
    pub fn max_width(mut self, width: u16) -> Self {
        self.max_width = width.max(20);
        self
    }

    fn calculate_dimensions(&self) -> (u16, u16) {
        let item_count = self.state.items.len().min(self.state.max_visible) as u16;
        let height = item_count + 2; // +2 for borders

        let content_width = self
            .state
            .items
            .iter()
            .map(|item| {
                let icon_width = if item.icon != '\0' { 2 } else { 0 };
                let label_width = item.label.chars().count();
                let desc_width = if item.description.is_empty() {
                    0
                } else {
                    item.description.chars().count() + 3
                };
                icon_width + label_width + desc_width
            })
            .max()
            .unwrap_or(20) as u16;

        let width = (content_width + 4).max(30).min(self.max_width);
        (width, height)
    }

    fn render_item(&self, item: &DropdownItem, is_selected: bool, area: Rect, buf: &mut Buffer) {
        let bg = if is_selected { SURFACE_2 } else { SURFACE_1 };

        // Clear line
        for x in area.x..area.right() {
            if let Some(cell) = buf.cell_mut((x, area.y)) {
                cell.set_bg(bg);
            }
        }

        let mut x = area.x + 1;

        // Icon
        if item.icon != '\0' {
            if let Some(cell) = buf.cell_mut((x, area.y)) {
                cell.set_char(item.icon)
                    .set_style(Style::default().fg(CYAN_PRIMARY).bg(bg));
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
            if x >= area.right() - 1 {
                break;
            }
            if let Some(cell) = buf.cell_mut((x, area.y)) {
                cell.set_char(ch).set_style(label_style);
            }
            x += 1;
        }

        // Description
        if !item.description.is_empty() && x < area.right() - 5 {
            for ch in " - ".chars() {
                if x >= area.right() - 1 {
                    break;
                }
                if let Some(cell) = buf.cell_mut((x, area.y)) {
                    cell.set_char(ch)
                        .set_style(Style::default().fg(TEXT_MUTED).bg(bg));
                }
                x += 1;
            }

            for ch in item.description.chars() {
                if x >= area.right() - 1 {
                    break;
                }
                if let Some(cell) = buf.cell_mut((x, area.y)) {
                    cell.set_char(ch)
                        .set_style(Style::default().fg(TEXT_DIM).bg(bg));
                }
                x += 1;
            }
        }

        // Selection indicator
        if is_selected && let Some(cell) = buf.cell_mut((area.x, area.y)) {
            cell.set_char('>')
                .set_style(Style::default().fg(CYAN_PRIMARY).bg(bg));
        }
    }
}

impl Widget for Dropdown<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if !self.state.visible || self.state.items.is_empty() {
            return;
        }

        let (width, height) = self.calculate_dimensions();

        // Calculate popup position
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

        // Clear background
        Clear.render(popup_area, buf);

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

        let inner = block.inner(popup_area);
        block.render(popup_area, buf);

        // Render items
        let visible_range = self.state.scroll.visible_range();
        for (i, item_idx) in visible_range.enumerate() {
            let y = inner.y + i as u16;
            if y >= inner.bottom() {
                break;
            }

            if let Some(item) = self.state.items.get(item_idx) {
                let item_width = if self.state.scroll.needs_scrollbar() {
                    inner.width.saturating_sub(1)
                } else {
                    inner.width
                };

                let item_area = Rect::new(inner.x, y, item_width, 1);
                let is_selected = item_idx == self.state.selected;
                self.render_item(item, is_selected, item_area, buf);
            }
        }

        // Scrollbar
        if self.state.scroll.needs_scrollbar() {
            let scrollbar_area =
                Rect::new(inner.right().saturating_sub(1), inner.y, 1, inner.height);
            render_scrollbar(scrollbar_area, buf, &self.state.scroll);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dropdown_item() {
        let item = DropdownItem::new("val", "Label", "Description").with_icon('*');
        assert_eq!(item.value, "val");
        assert_eq!(item.label, "Label");
        assert_eq!(item.icon, '*');
    }

    #[test]
    fn test_dropdown_state() {
        let mut state = DropdownState::new();
        assert!(!state.visible);

        state.show(vec![
            DropdownItem::new("a", "Apple", ""),
            DropdownItem::new("b", "Banana", ""),
            DropdownItem::new("c", "Cherry", ""),
        ]);

        assert!(state.visible);
        assert_eq!(state.selected, 0);
        assert_eq!(state.selected_value(), Some("a"));

        state.select_next();
        assert_eq!(state.selected_value(), Some("b"));

        state.select_prev();
        assert_eq!(state.selected_value(), Some("a"));

        state.hide();
        assert!(!state.visible);
        assert!(state.items.is_empty());
    }

    #[test]
    fn test_dropdown_wrap() {
        let mut state = DropdownState::new();
        state.show(vec![
            DropdownItem::new("a", "A", ""),
            DropdownItem::new("b", "B", ""),
        ]);

        state.select_next();
        state.select_next();
        assert_eq!(state.selected, 0); // Wrapped

        state.select_prev();
        assert_eq!(state.selected, 1); // Wrapped to end
    }
}
