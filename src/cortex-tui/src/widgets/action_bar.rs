//! Action Bar Widget
//!
//! A reusable widget for displaying action buttons at the bottom of modals.
//! Provides visual styling for primary, danger, and secondary actions.

use crate::ui::text_utils::MIN_TERMINAL_WIDTH;
use cortex_core::style::{CYAN_PRIMARY, ERROR, TEXT, TEXT_DIM, TEXT_MUTED};
use ratatui::{buffer::Buffer, layout::Rect, style::Style, widgets::Widget};

/// Style for action items
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionStyle {
    /// Primary action (cyan) - main actions like Add, Select
    Primary,
    /// Danger action (red) - destructive actions like Delete
    Danger,
    /// Secondary action (dim) - less important actions
    Secondary,
    /// Disabled action (very dim) - unavailable actions
    Disabled,
}

impl ActionStyle {
    fn key_style(&self) -> Style {
        match self {
            ActionStyle::Primary => Style::default().fg(CYAN_PRIMARY),
            ActionStyle::Danger => Style::default().fg(ERROR),
            ActionStyle::Secondary => Style::default().fg(TEXT_DIM),
            ActionStyle::Disabled => Style::default().fg(TEXT_MUTED),
        }
    }

    fn label_style(&self) -> Style {
        match self {
            ActionStyle::Primary => Style::default().fg(TEXT),
            ActionStyle::Danger => Style::default().fg(TEXT),
            ActionStyle::Secondary => Style::default().fg(TEXT_DIM),
            ActionStyle::Disabled => Style::default().fg(TEXT_MUTED),
        }
    }
}

/// A single action item in the action bar
#[derive(Debug, Clone)]
pub struct ActionItem {
    /// Key to trigger the action (e.g., 'a' for Add)
    pub key: char,
    /// Label to display (e.g., "Add")
    pub label: String,
    /// Visual style
    pub style: ActionStyle,
    /// Whether the action is currently visible
    pub visible: bool,
}

impl ActionItem {
    pub fn new(key: char, label: impl Into<String>) -> Self {
        Self {
            key,
            label: label.into(),
            style: ActionStyle::Primary,
            visible: true,
        }
    }

    pub fn danger(key: char, label: impl Into<String>) -> Self {
        Self {
            key,
            label: label.into(),
            style: ActionStyle::Danger,
            visible: true,
        }
    }

    pub fn secondary(key: char, label: impl Into<String>) -> Self {
        Self {
            key,
            label: label.into(),
            style: ActionStyle::Secondary,
            visible: true,
        }
    }

    pub fn with_style(mut self, style: ActionStyle) -> Self {
        self.style = style;
        self
    }

    pub fn with_visible(mut self, visible: bool) -> Self {
        self.visible = visible;
        self
    }
}

/// Navigation hint (right side of action bar)
#[derive(Debug, Clone)]
pub struct NavHint {
    pub key: String,
    pub description: String,
}

impl NavHint {
    pub fn new(key: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            description: description.into(),
        }
    }
}

/// Action bar widget for modal footers
#[derive(Debug, Clone, Default)]
pub struct ActionBar {
    /// Action items (left side)
    actions: Vec<ActionItem>,
    /// Navigation hints (right side)
    nav_hints: Vec<NavHint>,
}

impl ActionBar {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add an action item
    pub fn action(mut self, key: char, label: impl Into<String>) -> Self {
        self.actions.push(ActionItem::new(key, label));
        self
    }

    /// Add a danger action item
    pub fn danger(mut self, key: char, label: impl Into<String>) -> Self {
        self.actions.push(ActionItem::danger(key, label));
        self
    }

    /// Add a secondary action item
    pub fn secondary(mut self, key: char, label: impl Into<String>) -> Self {
        self.actions.push(ActionItem::secondary(key, label));
        self
    }

    /// Add a custom action item
    pub fn with_action(mut self, item: ActionItem) -> Self {
        self.actions.push(item);
        self
    }

    /// Add a navigation hint
    pub fn hint(mut self, key: impl Into<String>, description: impl Into<String>) -> Self {
        self.nav_hints.push(NavHint::new(key, description));
        self
    }

    /// Standard navigation hints (Up/Dn Enter Esc)
    pub fn with_standard_hints(self) -> Self {
        self.hint("Up/Dn", "nav")
            .hint("Enter", "select")
            .hint("Esc", "close")
    }

    /// Set enabled state for an action by key
    pub fn set_enabled(&mut self, key: char, enabled: bool) {
        for action in &mut self.actions {
            if action.key == key {
                action.style = if enabled {
                    ActionStyle::Primary
                } else {
                    ActionStyle::Disabled
                };
            }
        }
    }

    /// Update an action's label
    pub fn set_label(&mut self, key: char, label: impl Into<String>) {
        let label = label.into();
        for action in &mut self.actions {
            if action.key == key {
                action.label = label.clone();
            }
        }
    }

    /// Calculate total width needed for all actions
    fn calculate_actions_width(&self) -> usize {
        let visible_actions: Vec<_> = self.actions.iter().filter(|a| a.visible).collect();
        if visible_actions.is_empty() {
            return 0;
        }
        let mut width = 0;
        for (i, action) in visible_actions.iter().enumerate() {
            if i > 0 {
                width += 2; // "  " separator
            }
            width += 3 + action.label.len() + 1; // "[K] Label"
        }
        width
    }

    /// Calculate total width needed for all hints
    fn calculate_hints_width(&self, abbreviated: bool) -> usize {
        if self.nav_hints.is_empty() {
            return 0;
        }
        let mut width = 0;
        for (i, hint) in self.nav_hints.iter().enumerate() {
            if i > 0 {
                width += 2; // "  " separator
            }
            width += hint.key.len();
            if !hint.description.is_empty() {
                let desc = if abbreviated {
                    abbreviate_nav_hint(&hint.description)
                } else {
                    &hint.description
                };
                width += 1 + desc.len(); // " desc"
            }
        }
        width
    }

    /// Render the action bar directly to the buffer
    fn render_to_buffer(&self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 {
            return;
        }

        // Handle narrow terminals
        if area.width < MIN_TERMINAL_WIDTH {
            // Just show keys in minimal format: "[A] [D]"
            let x = area.x + 1;
            let y = area.y;
            let mut col = x;

            for action in self.actions.iter().filter(|a| a.visible) {
                if col > x {
                    col += 1;
                }
                if col + 3 > area.right() {
                    break;
                }
                let key_str = format!("[{}]", action.key.to_ascii_uppercase());
                buf.set_string(col, y, &key_str, action.style.key_style());
                col += key_str.len() as u16;
            }
            return;
        }

        let available_width = area.width as usize - 2; // padding
        let actions_width = self.calculate_actions_width();
        let hints_width = self.calculate_hints_width(false);
        let hints_abbreviated_width = self.calculate_hints_width(true);
        let spacer_width = if !self.actions.is_empty() && !self.nav_hints.is_empty() {
            3
        } else {
            0
        };

        // Determine what to show based on available width
        let (show_hints, abbreviate_hints) =
            if actions_width + spacer_width + hints_width <= available_width {
                (true, false) // Show everything
            } else if actions_width + spacer_width + hints_abbreviated_width <= available_width {
                (true, true) // Show abbreviated hints
            } else {
                (false, false) // Hide hints or truncate actions
            };

        let x = area.x + 1; // Small padding
        let y = area.y;
        let mut col = x;

        // Left side: actions
        for (i, action) in self.actions.iter().filter(|a| a.visible).enumerate() {
            if i > 0 {
                if col + 2 >= area.right() {
                    break;
                }
                buf.set_string(col, y, "  ", Style::default());
                col += 2;
            }

            // [K] Label format
            let key_str = format!("[{}]", action.key.to_ascii_uppercase());
            if col + key_str.len() as u16 >= area.right() {
                break;
            }
            buf.set_string(col, y, &key_str, action.style.key_style());
            col += key_str.len() as u16;

            let label_str = format!(" {}", action.label);
            let remaining = (area.right() - col) as usize;
            if remaining > 0 {
                let truncated = if label_str.len() > remaining {
                    format!("{}...", &label_str[..remaining.saturating_sub(3)])
                } else {
                    label_str
                };
                buf.set_string(col, y, &truncated, action.style.label_style());
                col += truncated.len() as u16;
            }
        }

        // Spacer between actions and hints
        if show_hints
            && !self.actions.is_empty()
            && !self.nav_hints.is_empty()
            && col + 3 < area.right()
        {
            buf.set_string(col, y, "   ", Style::default());
            col += 3;
        }

        // Right side: navigation hints
        if show_hints {
            for (i, hint) in self.nav_hints.iter().enumerate() {
                if i > 0 {
                    if col + 2 >= area.right() {
                        break;
                    }
                    buf.set_string(col, y, "  ", Style::default());
                    col += 2;
                }
                if col + hint.key.len() as u16 >= area.right() {
                    break;
                }
                buf.set_string(col, y, &hint.key, Style::default().fg(TEXT_DIM));
                col += hint.key.len() as u16;

                if !hint.description.is_empty() {
                    let desc = if abbreviate_hints {
                        abbreviate_nav_hint(&hint.description)
                    } else {
                        &hint.description
                    };
                    let desc_str = format!(" {}", desc);
                    let remaining = (area.right() - col) as usize;
                    if remaining > 0 {
                        let truncated = if desc_str.len() > remaining {
                            &desc_str[..remaining]
                        } else {
                            &desc_str
                        };
                        buf.set_string(col, y, truncated, Style::default().fg(TEXT_MUTED));
                        col += truncated.len() as u16;
                    }
                }
            }
        }
    }
}

/// Abbreviate navigation hint descriptions
fn abbreviate_nav_hint(description: &str) -> &str {
    match description {
        "navigate" | "nav" => "nav",
        "select" | "sel" => "sel",
        "cancel" => "esc",
        "close" | "cls" => "cls",
        _ => description,
    }
}

impl Widget for &ActionBar {
    fn render(self, area: Rect, buf: &mut Buffer) {
        self.render_to_buffer(area, buf);
    }
}

impl Widget for ActionBar {
    fn render(self, area: Rect, buf: &mut Buffer) {
        (&self).render(area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_action_bar_creation() {
        let bar = ActionBar::new()
            .action('a', "Add")
            .danger('d', "Delete")
            .secondary('r', "Refresh")
            .with_standard_hints();

        assert_eq!(bar.actions.len(), 3);
        assert_eq!(bar.nav_hints.len(), 3);
    }

    #[test]
    fn test_action_styles() {
        let bar = ActionBar::new().action('a', "Add").danger('d', "Delete");

        assert_eq!(bar.actions[0].style, ActionStyle::Primary);
        assert_eq!(bar.actions[1].style, ActionStyle::Danger);
    }

    #[test]
    fn test_set_enabled() {
        let mut bar = ActionBar::new().action('a', "Add");
        bar.set_enabled('a', false);
        assert_eq!(bar.actions[0].style, ActionStyle::Disabled);
    }
}
