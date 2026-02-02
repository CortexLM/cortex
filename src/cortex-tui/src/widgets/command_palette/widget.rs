//! Command palette widget.
//!
//! The rendering widget for the command palette.

use cortex_core::style::{
    BORDER, BORDER_FOCUS, CYAN_PRIMARY, SKY_BLUE, SURFACE_0, SURFACE_2, TEXT, TEXT_DIM, TEXT_MUTED,
    VOID,
};
use ratatui::prelude::*;
use ratatui::widgets::Widget;

use super::state::CommandPaletteState;
use super::types::PaletteItem;

/// Widget for rendering the command palette.
pub struct CommandPalette<'a> {
    /// Reference to the palette state
    state: &'a CommandPaletteState,
    /// Widget width (0 = auto)
    width: u16,
    /// Widget height (0 = auto)
    height: u16,
}

impl<'a> CommandPalette<'a> {
    /// Creates a new command palette widget.
    pub fn new(state: &'a CommandPaletteState) -> Self {
        Self {
            state,
            width: 0,
            height: 0,
        }
    }

    /// Sets the size of the palette.
    pub fn size(mut self, width: u16, height: u16) -> Self {
        self.width = width;
        self.height = height;
        self
    }

    /// Renders the background.
    fn render_background(&self, area: Rect, buf: &mut Buffer) {
        for y in area.y..area.y + area.height {
            for x in area.x..area.x + area.width {
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_char(' ').set_style(Style::default().bg(SURFACE_0));
                }
            }
        }
    }

    /// Renders the border.
    fn render_border(&self, area: Rect, buf: &mut Buffer) {
        let border_style = Style::default().fg(BORDER_FOCUS);

        // Top border
        if area.width >= 2 {
            buf.set_string(area.x, area.y, "+", border_style);
            for x in (area.x + 1)..(area.x + area.width - 1) {
                buf.set_string(x, area.y, "-", border_style);
            }
            buf.set_string(area.x + area.width - 1, area.y, "+", border_style);
        }

        // Sides
        for y in (area.y + 1)..(area.y + area.height - 1) {
            buf.set_string(area.x, y, "|", border_style);
            if area.width > 1 {
                buf.set_string(area.x + area.width - 1, y, "|", border_style);
            }
        }

        // Bottom border
        if area.height >= 2 {
            buf.set_string(area.x, area.y + area.height - 1, "+", border_style);
            for x in (area.x + 1)..(area.x + area.width - 1) {
                buf.set_string(x, area.y + area.height - 1, "-", border_style);
            }
            buf.set_string(
                area.x + area.width - 1,
                area.y + area.height - 1,
                "+",
                border_style,
            );
        }
    }

    /// Renders the search input area.
    fn render_input(&self, area: Rect, buf: &mut Buffer) {
        if area.height < 3 {
            return;
        }

        let y = area.y + 1;
        let input_width = area.width.saturating_sub(4) as usize;

        // Prompt
        buf.set_string(
            area.x + 2,
            y,
            "> ",
            Style::default().fg(CYAN_PRIMARY).bg(SURFACE_0),
        );

        // Query text (truncated if needed)
        let query_display: String = self
            .state
            .query
            .chars()
            .take(input_width.saturating_sub(12))
            .collect();
        buf.set_string(
            area.x + 4,
            y,
            &query_display,
            Style::default().fg(TEXT).bg(SURFACE_0),
        );

        // Cursor
        let display_cursor_pos = self
            .state
            .query
            .chars()
            .take(self.state.cursor_pos)
            .count()
            .min(input_width.saturating_sub(12));
        let cursor_x = area.x + 4 + display_cursor_pos as u16;
        if cursor_x < area.x + area.width - 2
            && let Some(cell) = buf.cell_mut((cursor_x, y))
        {
            cell.set_style(Style::default().fg(VOID).bg(CYAN_PRIMARY));
        }

        // Hint on right
        let hint = "[Ctrl+P]";
        let hint_x = area.x + area.width.saturating_sub(hint.len() as u16 + 2);
        if hint_x > area.x + 10 {
            buf.set_string(
                hint_x,
                y,
                hint,
                Style::default().fg(TEXT_MUTED).bg(SURFACE_0),
            );
        }

        // Separator line
        let sep_y = area.y + 2;
        if sep_y < area.y + area.height - 1 {
            for x in (area.x + 1)..(area.x + area.width - 1) {
                buf.set_string(x, sep_y, "-", Style::default().fg(BORDER).bg(SURFACE_0));
            }
        }
    }

    /// Renders the list of items.
    fn render_items(&self, area: Rect, buf: &mut Buffer) {
        if area.height < 5 {
            return;
        }

        let start_y = area.y + 3;
        let end_y = area.y + area.height.saturating_sub(2);
        let item_width = area.width.saturating_sub(4);

        if start_y >= end_y {
            return;
        }

        let mut current_y = start_y;
        let mut display_index = 0;
        let mut current_category: Option<&str> = None;

        // First show recent items if query is empty
        if self.state.query.is_empty() && !self.state.recent.is_empty() {
            // Category header
            if current_y < end_y {
                buf.set_string(
                    area.x + 2,
                    current_y,
                    "Recent",
                    Style::default().fg(TEXT_MUTED).bg(SURFACE_0),
                );
                current_y += 1;
            }

            for item in self.state.recent.iter().take(3) {
                if current_y >= end_y {
                    break;
                }

                let is_selected = self.state.selected_index == display_index;
                self.render_item(area.x + 2, current_y, item_width, item, is_selected, buf);
                current_y += 1;
                display_index += 1;
            }

            // Add spacing after recent section
            if current_y < end_y && !self.state.filtered_items.is_empty() {
                current_y += 1;
            }
        }

        // Show filtered items grouped by category
        for (item_idx, _score) in &self.state.filtered_items {
            if current_y >= end_y {
                break;
            }

            let item = &self.state.items[*item_idx];
            let cat = item.category_name();

            // Category header
            if current_category != Some(cat) {
                if current_y + 1 >= end_y {
                    break;
                }

                current_category = Some(cat);
                buf.set_string(
                    area.x + 2,
                    current_y,
                    cat,
                    Style::default().fg(TEXT_MUTED).bg(SURFACE_0),
                );
                current_y += 1;
            }

            if current_y >= end_y {
                break;
            }

            let is_selected = self.state.selected_index == display_index;
            self.render_item(area.x + 2, current_y, item_width, item, is_selected, buf);
            current_y += 1;
            display_index += 1;
        }
    }

    /// Renders a single item.
    fn render_item(
        &self,
        x: u16,
        y: u16,
        width: u16,
        item: &PaletteItem,
        selected: bool,
        buf: &mut Buffer,
    ) {
        let style = if selected {
            Style::default().fg(VOID).bg(CYAN_PRIMARY)
        } else {
            Style::default().fg(TEXT).bg(SURFACE_0)
        };

        // Clear line with background
        for dx in 0..width {
            if let Some(cell) = buf.cell_mut((x + dx, y)) {
                cell.set_char(' ').set_style(style);
            }
        }

        let mut current_x = x + 1;

        // Prefix
        let prefix = item.prefix();
        if !prefix.is_empty() {
            buf.set_string(current_x, y, prefix, style);
            current_x += prefix.len() as u16 + 1;
        }

        // Main text
        let text = item.display_text();
        let max_text_len = (width as usize).saturating_sub((current_x - x) as usize + 15);
        let display_text: String = text.chars().take(max_text_len).collect();
        buf.set_string(current_x, y, &display_text, style);
        current_x += display_text.len() as u16;

        // Detail/description
        if let Some(detail) = item.detail_text() {
            let detail_style = if selected {
                Style::default().fg(SURFACE_2).bg(CYAN_PRIMARY)
            } else {
                Style::default().fg(TEXT_DIM).bg(SURFACE_0)
            };

            let detail_x = current_x + 2;
            let max_detail = (width as usize).saturating_sub((detail_x - x) as usize + 12);
            if max_detail > 3 {
                let truncated: String = detail.chars().take(max_detail).collect();
                buf.set_string(detail_x, y, &truncated, detail_style);
            }
        }

        // Shortcut
        if let Some(shortcut) = item.shortcut() {
            let shortcut_style = if selected {
                Style::default().fg(SKY_BLUE).bg(CYAN_PRIMARY)
            } else {
                Style::default().fg(SKY_BLUE).bg(SURFACE_0)
            };

            let shortcut_x = x + width.saturating_sub(shortcut.len() as u16 + 1);
            if shortcut_x > current_x + 2 {
                buf.set_string(shortcut_x, y, shortcut, shortcut_style);
            }
        }
    }

    /// Renders the footer with keyboard hints.
    fn render_footer(&self, area: Rect, buf: &mut Buffer) {
        if area.height < 2 {
            return;
        }

        let hint_y = area.y + area.height.saturating_sub(2);
        if hint_y <= area.y + 3 {
            return;
        }

        // Clear line
        for dx in 1..(area.width.saturating_sub(1)) {
            if let Some(cell) = buf.cell_mut((area.x + dx, hint_y)) {
                cell.set_char(' ').set_style(Style::default().bg(SURFACE_0));
            }
        }

        let hints = "[Up/Down] Navigate  [Enter] Select  [Esc] Close";
        let hints_len = hints.len() as u16;
        let x = area.x + (area.width.saturating_sub(hints_len)) / 2;

        buf.set_string(
            x,
            hint_y,
            hints,
            Style::default().fg(TEXT_MUTED).bg(SURFACE_0),
        );
    }
}

impl Widget for CommandPalette<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Calculate dimensions
        let max_width = if self.width > 0 {
            self.width
        } else {
            70.min(area.width.saturating_sub(4))
        };

        let max_height = if self.height > 0 {
            self.height
        } else {
            20.min(area.height.saturating_sub(4))
        };

        if max_width < 20 || max_height < 8 {
            return; // Too small to render
        }

        // Center the palette
        let x = area.x + (area.width.saturating_sub(max_width)) / 2;
        let y = area.y + (area.height.saturating_sub(max_height)) / 2;
        let palette_area = Rect::new(x, y, max_width, max_height);

        // Render components
        self.render_background(palette_area, buf);
        self.render_border(palette_area, buf);
        self.render_input(palette_area, buf);
        self.render_items(palette_area, buf);
        self.render_footer(palette_area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_widget_creation() {
        let state = CommandPaletteState::new();
        let widget = CommandPalette::new(&state);
        assert_eq!(widget.width, 0);
        assert_eq!(widget.height, 0);

        let widget = CommandPalette::new(&state).size(80, 24);
        assert_eq!(widget.width, 80);
        assert_eq!(widget.height, 24);
    }
}
