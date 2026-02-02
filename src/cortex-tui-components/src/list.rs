//! Scrollable list component.

use crate::scroll::{ScrollState, Scrollable, render_scrollbar};
use cortex_core::style::{CYAN_PRIMARY, SURFACE_0, TEXT, TEXT_DIM};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::Widget;

/// An item in a list.
#[derive(Debug, Clone)]
pub struct ListItem {
    /// Primary text
    pub text: String,
    /// Secondary text (optional)
    pub secondary: Option<String>,
}

impl ListItem {
    /// Create a new list item.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            secondary: None,
        }
    }

    /// Add secondary text.
    pub fn with_secondary(mut self, text: impl Into<String>) -> Self {
        self.secondary = Some(text.into());
        self
    }
}

/// A scrollable list widget.
pub struct ScrollableList {
    /// Items in the list
    pub items: Vec<ListItem>,
    /// Currently selected index
    pub selected: Option<usize>,
    /// Scroll state
    pub scroll: ScrollState,
}

impl ScrollableList {
    /// Create a new scrollable list.
    pub fn new(items: Vec<ListItem>) -> Self {
        let len = items.len();
        Self {
            items,
            selected: if len > 0 { Some(0) } else { None },
            scroll: ScrollState::new(len, 10),
        }
    }

    /// Set the visible height.
    pub fn with_visible_height(mut self, height: usize) -> Self {
        self.scroll.set_visible(height);
        self
    }

    /// Select the next item.
    pub fn select_next(&mut self) {
        if let Some(idx) = self.selected
            && idx + 1 < self.items.len()
        {
            self.selected = Some(idx + 1);
            self.scroll.ensure_visible(idx + 1);
        }
    }

    /// Select the previous item.
    pub fn select_prev(&mut self) {
        if let Some(idx) = self.selected
            && idx > 0
        {
            self.selected = Some(idx - 1);
            self.scroll.ensure_visible(idx - 1);
        }
    }

    /// Get the selected item.
    pub fn selected_item(&self) -> Option<&ListItem> {
        self.selected.and_then(|idx| self.items.get(idx))
    }
}

impl Scrollable for ScrollableList {
    fn scroll_state(&self) -> &ScrollState {
        &self.scroll
    }

    fn scroll_state_mut(&mut self) -> &mut ScrollState {
        &mut self.scroll
    }
}

impl Widget for &ScrollableList {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width < 5 {
            return;
        }

        let needs_scrollbar = self.scroll.needs_scrollbar();
        let content_width = if needs_scrollbar {
            area.width.saturating_sub(1)
        } else {
            area.width
        };

        for (i, idx) in self.scroll.visible_range().enumerate() {
            let y = area.y + i as u16;
            if y >= area.bottom() {
                break;
            }

            if let Some(item) = self.items.get(idx) {
                let is_selected = self.selected == Some(idx);

                let (bg, fg) = if is_selected {
                    (CYAN_PRIMARY, SURFACE_0)
                } else {
                    (SURFACE_0, TEXT)
                };

                // Clear line
                for x in area.x..area.x + content_width {
                    if let Some(cell) = buf.cell_mut((x, y)) {
                        cell.set_bg(bg);
                    }
                }

                // Prefix
                let prefix = if is_selected { "> " } else { "  " };
                buf.set_string(area.x, y, prefix, Style::default().fg(fg).bg(bg));

                // Text
                buf.set_string(area.x + 2, y, &item.text, Style::default().fg(fg).bg(bg));

                // Secondary
                if let Some(secondary) = &item.secondary {
                    let sec_x = area.x + 2 + item.text.len() as u16 + 1;
                    if sec_x < area.x + content_width {
                        let sec_style = if is_selected {
                            Style::default().fg(SURFACE_0).bg(bg)
                        } else {
                            Style::default().fg(TEXT_DIM).bg(bg)
                        };
                        buf.set_string(sec_x, y, secondary, sec_style);
                    }
                }
            }
        }

        // Scrollbar
        if needs_scrollbar {
            let scrollbar_area = Rect::new(area.right().saturating_sub(1), area.y, 1, area.height);
            render_scrollbar(scrollbar_area, buf, &self.scroll);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_list_navigation() {
        let mut list = ScrollableList::new(vec![
            ListItem::new("Item 1"),
            ListItem::new("Item 2"),
            ListItem::new("Item 3"),
        ]);

        assert_eq!(list.selected, Some(0));

        list.select_next();
        assert_eq!(list.selected, Some(1));

        list.select_prev();
        assert_eq!(list.selected, Some(0));
    }

    #[test]
    fn test_list_item_secondary() {
        let item = ListItem::new("Primary").with_secondary("Secondary");
        assert_eq!(item.secondary, Some("Secondary".to_string()));
    }
}
