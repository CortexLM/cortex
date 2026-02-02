//! Radio button component for single selection.

use cortex_core::style::{CYAN_PRIMARY, TEXT, TEXT_DIM};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::Widget;

/// A radio button item.
#[derive(Debug, Clone)]
pub struct RadioItem {
    /// Unique ID
    pub id: String,
    /// Display label
    pub label: String,
    /// Whether disabled
    pub disabled: bool,
}

impl RadioItem {
    /// Create a new radio item.
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            disabled: false,
        }
    }

    /// Set as disabled.
    pub fn disabled(mut self) -> Self {
        self.disabled = true;
        self
    }
}

/// A group of radio buttons.
pub struct RadioGroup {
    /// Items
    pub items: Vec<RadioItem>,
    /// Currently selected index
    pub selected: usize,
    /// Currently focused index
    pub focused: usize,
}

impl RadioGroup {
    /// Create a new radio group.
    pub fn new(items: Vec<RadioItem>) -> Self {
        Self {
            items,
            selected: 0,
            focused: 0,
        }
    }

    /// Select the focused item.
    pub fn select(&mut self) {
        if let Some(item) = self.items.get(self.focused)
            && !item.disabled
        {
            self.selected = self.focused;
        }
    }

    /// Move focus up.
    pub fn focus_prev(&mut self) {
        if self.focused > 0 {
            self.focused -= 1;
        }
    }

    /// Move focus down.
    pub fn focus_next(&mut self) {
        if self.focused + 1 < self.items.len() {
            self.focused += 1;
        }
    }

    /// Get the selected ID.
    pub fn selected_id(&self) -> Option<&str> {
        self.items.get(self.selected).map(|i| i.id.as_str())
    }
}

impl Widget for &RadioGroup {
    fn render(self, area: Rect, buf: &mut Buffer) {
        for (i, item) in self.items.iter().enumerate() {
            let y = area.y + i as u16;
            if y >= area.bottom() {
                break;
            }

            let is_focused = i == self.focused;
            let is_selected = i == self.selected;
            let radio = if is_selected { "(*)" } else { "( )" };

            let style = if item.disabled {
                Style::default().fg(TEXT_DIM)
            } else if is_focused {
                Style::default().fg(CYAN_PRIMARY)
            } else {
                Style::default().fg(TEXT)
            };

            let prefix = if is_focused { "> " } else { "  " };
            let text = format!("{}{} {}", prefix, radio, item.label);

            buf.set_string(area.x, y, &text, style);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_radio_select() {
        let mut group = RadioGroup::new(vec![
            RadioItem::new("a", "Option A"),
            RadioItem::new("b", "Option B"),
        ]);

        assert_eq!(group.selected, 0);
        assert_eq!(group.selected_id(), Some("a"));

        group.focus_next();
        group.select();

        assert_eq!(group.selected, 1);
        assert_eq!(group.selected_id(), Some("b"));
    }
}
