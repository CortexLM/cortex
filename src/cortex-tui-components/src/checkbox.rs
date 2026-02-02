//! Checkbox component for multi-selection.

use cortex_core::style::{CYAN_PRIMARY, TEXT, TEXT_DIM};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::Widget;

/// A checkbox item.
#[derive(Debug, Clone)]
pub struct CheckboxItem {
    /// Unique ID
    pub id: String,
    /// Display label
    pub label: String,
    /// Whether checked
    pub checked: bool,
    /// Whether disabled
    pub disabled: bool,
}

impl CheckboxItem {
    /// Create a new checkbox item.
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            checked: false,
            disabled: false,
        }
    }

    /// Set as checked.
    pub fn checked(mut self) -> Self {
        self.checked = true;
        self
    }

    /// Set as disabled.
    pub fn disabled(mut self) -> Self {
        self.disabled = true;
        self
    }
}

/// A group of checkboxes.
pub struct CheckboxGroup {
    /// Items
    pub items: Vec<CheckboxItem>,
    /// Currently focused index
    pub focused: usize,
}

impl CheckboxGroup {
    /// Create a new checkbox group.
    pub fn new(items: Vec<CheckboxItem>) -> Self {
        Self { items, focused: 0 }
    }

    /// Toggle the focused item.
    pub fn toggle(&mut self) {
        if let Some(item) = self.items.get_mut(self.focused)
            && !item.disabled
        {
            item.checked = !item.checked;
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

    /// Get checked IDs.
    pub fn checked_ids(&self) -> Vec<&str> {
        self.items
            .iter()
            .filter(|i| i.checked)
            .map(|i| i.id.as_str())
            .collect()
    }
}

impl Widget for &CheckboxGroup {
    fn render(self, area: Rect, buf: &mut Buffer) {
        for (i, item) in self.items.iter().enumerate() {
            let y = area.y + i as u16;
            if y >= area.bottom() {
                break;
            }

            let is_focused = i == self.focused;
            let checkbox = if item.checked { "[✓]" } else { "[ ]" };

            let style = if item.disabled {
                Style::default().fg(TEXT_DIM)
            } else if is_focused {
                Style::default().fg(CYAN_PRIMARY)
            } else {
                Style::default().fg(TEXT)
            };

            let prefix = if is_focused { "> " } else { "  " };
            let text = format!("{}{} {}", prefix, checkbox, item.label);

            buf.set_string(area.x, y, &text, style);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_checkbox_toggle() {
        let mut group = CheckboxGroup::new(vec![
            CheckboxItem::new("a", "Option A"),
            CheckboxItem::new("b", "Option B").checked(),
        ]);

        assert!(!group.items[0].checked);
        assert!(group.items[1].checked);

        group.toggle();
        assert!(group.items[0].checked);

        assert_eq!(group.checked_ids(), vec!["a", "b"]);
    }
}
