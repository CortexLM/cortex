//! Renderer for interactive selection in the input area.

use super::state::{InlineFormState, InteractiveItem, InteractiveState};
use cortex_core::style::{CYAN_PRIMARY, SUCCESS, SURFACE_1, TEXT, TEXT_DIM, TEXT_MUTED};
use cortex_tui_components::borders::ROUNDED_BORDER;
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Widget},
};

/// Widget for rendering the interactive selection list.
pub struct InteractiveWidget<'a> {
    state: &'a InteractiveState,
}

impl<'a> InteractiveWidget<'a> {
    /// Create a new interactive widget.
    pub fn new(state: &'a InteractiveState) -> Self {
        Self { state }
    }

    /// Calculate click zones for the interactive list.
    /// Call this after rendering to populate state.click_zones.
    pub fn calculate_click_zones(state: &mut InteractiveState, area: Rect) {
        state.click_zones.clear();
        state.tab_click_zones.clear();

        // Calculate tab click zones (on title line)
        if !state.tabs.is_empty() {
            let title = format!(" {} ", state.title);
            let title_y = area.y + 1;
            let tabs_x = area.x + 2 + title.len() as u16 + 2;
            let mut x = tabs_x;
            for (i, tab) in state.tabs.iter().enumerate() {
                let tab_text = format!(" {} ", tab.label);
                let tab_width = tab_text.len() as u16;
                let tab_rect = Rect::new(x, title_y, tab_width, 1);
                state.tab_click_zones.push((tab_rect, i));
                x += tab_width + 2;
            }
        }

        // If inline form is active, no item click zones
        if state.is_form_active() {
            return;
        }

        // Calculate the inner area (same logic as render)
        // Inner area starts after top border (1) + title line (1) = 2
        let inner = Rect::new(
            area.x,
            area.y + 2,
            area.width,
            area.height.saturating_sub(2),
        );

        if inner.height < 3 {
            return;
        }

        // Layout: search (optional) + items + hints
        let search_height = if state.searchable { 1 } else { 0 };
        let hints_height = 1;
        let items_height = inner.height.saturating_sub(search_height + hints_height);

        let items_y = inner.y + search_height;
        let items_area = Rect::new(inner.x, items_y, inner.width, items_height);

        // Register click zones for visible items
        // We need to collect indices first to avoid borrow conflicts
        let start = state.scroll_offset;
        let visible_count = state.filtered_indices.len();
        let end = (start + items_area.height as usize).min(visible_count);

        for i in 0..(end - start) {
            let y = items_area.y + i as u16;
            if y >= items_area.y + items_area.height {
                break;
            }

            let filtered_idx = start + i;
            let item_rect = Rect::new(items_area.x, y, items_area.width, 1);
            state.click_zones.push((item_rect, filtered_idx));
        }
    }

    /// Calculate the required height for this widget.
    pub fn required_height(&self) -> u16 {
        // If inline form is active, calculate form height
        if let Some(ref form) = self.state.inline_form {
            let fields_count = form.fields.len() as u16;
            let header_height = 1; // Title
            let hints_height = 1;
            let border_height = 2;
            // Each field takes 2 lines (label + input)
            return (fields_count * 2) + header_height + hints_height + border_height;
        }

        let items_count = self
            .state
            .filtered_indices
            .len()
            .min(self.state.max_visible);
        let header_height = 1; // Title
        let search_height = if self.state.searchable { 1 } else { 0 };
        let hints_height = 1;
        let border_height = 2;

        (items_count as u16) + header_height + search_height + hints_height + border_height
    }
}

impl<'a> Widget for InteractiveWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Clear the area first
        Clear.render(area, buf);

        // If inline form is active, render the form instead
        if let Some(ref form) = self.state.inline_form {
            self.render_form(form, area, buf);
            return;
        }

        // Draw only top border line
        let border_style = Style::default().fg(CYAN_PRIMARY);
        let border_line = "─".repeat(area.width as usize);
        buf.set_string(area.x, area.y, &border_line, border_style);

        // Render title as normal text on the line below
        let title_y = area.y + 1;
        let title = format!(" {} ", self.state.title);
        buf.set_string(
            area.x + 1,
            title_y,
            &title,
            Style::default()
                .fg(CYAN_PRIMARY)
                .add_modifier(Modifier::BOLD),
        );

        // Render tabs if present (on the same line as title, after it)
        let header_height = 2;
        if !self.state.tabs.is_empty() {
            let tabs_x = area.x + 2 + title.len() as u16 + 2;
            let mut x = tabs_x;
            for (i, tab) in self.state.tabs.iter().enumerate() {
                let is_active = i == self.state.active_tab;
                let is_hovered = self.state.hovered_tab == Some(i);
                let tab_text = format!(" {} ", tab.label);
                let style = if is_active {
                    Style::default()
                        .fg(Color::Black)
                        .bg(CYAN_PRIMARY)
                        .add_modifier(Modifier::BOLD)
                } else if is_hovered {
                    Style::default().fg(CYAN_PRIMARY)
                } else {
                    Style::default().fg(TEXT_DIM)
                };
                buf.set_string(x, title_y, &tab_text, style);
                x += tab_text.len() as u16 + 2;
            }
        }

        // Inner area starts after top border + title line
        let inner = Rect::new(
            area.x,
            area.y + header_height,
            area.width,
            area.height.saturating_sub(header_height),
        );

        if inner.height < 3 {
            return;
        }

        // Layout: search (optional) + items + hints
        let mut constraints = Vec::new();
        if self.state.searchable {
            constraints.push(Constraint::Length(1)); // Search bar
        }
        constraints.push(Constraint::Min(1)); // Items
        constraints.push(Constraint::Length(1)); // Hints

        let chunks = Layout::vertical(constraints).split(inner);
        let mut chunk_idx = 0;

        // Render search bar if enabled
        if self.state.searchable {
            let search_area = chunks[chunk_idx];
            chunk_idx += 1;

            let search_text = if self.state.search_query.is_empty() {
                Span::styled("Type to search...", Style::default().fg(TEXT_MUTED))
            } else {
                Span::styled(
                    format!("Search: {}_", self.state.search_query),
                    Style::default().fg(TEXT),
                )
            };

            let search_line = Line::from(vec![
                Span::styled(" ", Style::default().fg(TEXT_DIM)),
                search_text,
            ]);

            Paragraph::new(search_line).render(search_area, buf);
        }

        // Render items
        let items_area = chunks[chunk_idx];
        chunk_idx += 1;

        self.render_items(items_area, buf);

        // Render hints
        let hints_area = chunks[chunk_idx];
        self.render_hints(hints_area, buf);
    }
}

impl<'a> InteractiveWidget<'a> {
    /// Render the list items.
    fn render_items(&self, area: Rect, buf: &mut Buffer) {
        let visible_items = self.state.visible_items();
        let start = self.state.scroll_offset;
        let end = (start + area.height as usize).min(visible_items.len());

        for (i, (real_idx, item)) in visible_items
            .iter()
            .skip(start)
            .take(end - start)
            .enumerate()
        {
            let y = area.y + i as u16;
            if y >= area.y + area.height {
                break;
            }

            let filtered_idx = start + i;
            let is_selected = self.state.selected == filtered_idx;
            let is_hovered = self.state.hovered == Some(filtered_idx);
            let is_checked = self.state.is_checked(*real_idx);

            self.render_item(
                Rect::new(area.x, y, area.width, 1),
                buf,
                item,
                is_selected,
                is_hovered,
                is_checked,
            );
        }

        // Show scroll indicators if needed
        if start > 0 {
            buf.set_string(
                area.x + area.width.saturating_sub(3),
                area.y,
                "▲",
                Style::default().fg(TEXT_MUTED),
            );
        }
        if end < visible_items.len() {
            buf.set_string(
                area.x + area.width.saturating_sub(3),
                area.y + area.height.saturating_sub(1),
                "▼",
                Style::default().fg(TEXT_MUTED),
            );
        }
    }

    /// Render a single item.
    fn render_item(
        &self,
        area: Rect,
        buf: &mut Buffer,
        item: &InteractiveItem,
        is_selected: bool,
        is_hovered: bool,
        is_checked: bool,
    ) {
        // Show hover highlight with subtle background
        let (fg, bg) = if item.disabled {
            (TEXT_MUTED, Color::Reset)
        } else if is_selected {
            (CYAN_PRIMARY, Color::Reset)
        } else if is_hovered {
            // Subtle hover highlight - use dim background
            (TEXT, Color::Rgb(40, 44, 52))
        } else {
            (TEXT, Color::Reset)
        };

        // Apply background for hover effect
        if is_hovered && !item.disabled && !item.is_separator {
            for dx in 0..area.width {
                if let Some(cell) = buf.cell_mut((area.x + dx, area.y)) {
                    cell.set_bg(bg);
                }
            }
        }

        let mut x = area.x + 1;

        // Selection indicator (not shown for separators)
        let indicator = if is_selected && !item.is_separator {
            ">"
        } else {
            " "
        };
        buf.set_string(
            x,
            area.y,
            indicator,
            Style::default()
                .fg(CYAN_PRIMARY)
                .add_modifier(Modifier::BOLD),
        );
        x += 2;

        // Checkbox (multi-select)
        if self.state.multi_select {
            let checkbox = if is_checked { "[x]" } else { "[ ]" };
            let checkbox_style = if is_checked {
                Style::default().fg(SUCCESS)
            } else {
                Style::default().fg(TEXT_DIM)
            };
            buf.set_string(x, area.y, checkbox, checkbox_style);
            x += 4;
        }

        // Icon
        if let Some(icon) = item.icon {
            buf.set_string(x, area.y, icon.to_string(), Style::default().fg(fg));
            x += 2;
        }

        // Shortcut - hidden (shortcuts still work via keyboard)

        // Label - bold for separators (category headers)
        let label_style = if item.is_separator {
            Style::default()
                .fg(CYAN_PRIMARY)
                .add_modifier(Modifier::BOLD)
        } else if is_selected {
            Style::default().fg(fg).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(fg)
        };

        let max_label_len = (area.width as usize).saturating_sub((x - area.x) as usize + 2);
        let label = if item.label.len() > max_label_len {
            format!("{}...", &item.label[..max_label_len.saturating_sub(3)])
        } else {
            item.label.clone()
        };
        buf.set_string(x, area.y, &label, label_style);
        x += label.len() as u16;

        // Description (if room)
        if let Some(ref desc) = item.description {
            let desc_x = x + 2;
            let remaining = (area.x + area.width).saturating_sub(desc_x);
            if remaining > 10 {
                let desc_text = if desc.len() > remaining as usize {
                    format!("({}...)", &desc[..remaining as usize - 5])
                } else {
                    format!("({})", desc)
                };
                buf.set_string(desc_x, area.y, &desc_text, Style::default().fg(TEXT_DIM));
            }
        }
    }

    /// Render the key hints at the bottom.
    fn render_hints(&self, area: Rect, buf: &mut Buffer) {
        let mut hints = vec![("↑↓", "navigate"), ("Enter", "select")];

        if self.state.multi_select {
            hints.insert(1, ("Space", "toggle"));
        }

        if self.state.searchable {
            hints.push(("Type", "search"));
        }

        hints.push(("Esc", "cancel"));

        // Use standard TEXT_DIM color for hints
        let hint_color = TEXT_DIM;

        let mut spans = Vec::new();
        for (i, (key, action)) in hints.iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled("  ", Style::default()));
            }
            spans.push(Span::styled(
                format!("[{}]", key),
                Style::default().fg(hint_color),
            ));
            spans.push(Span::styled(
                format!(" {}", action),
                Style::default().fg(hint_color),
            ));
        }

        let hints_line = Line::from(spans);
        Paragraph::new(hints_line).render(area, buf);
    }

    /// Render inline form for configuration within the panel.
    fn render_form(&self, form: &InlineFormState, area: Rect, buf: &mut Buffer) {
        // Draw border with form title
        let block = Block::default()
            .borders(Borders::ALL)
            .border_set(ROUNDED_BORDER)
            .border_style(Style::default().fg(CYAN_PRIMARY))
            .title(Span::styled(
                format!(" {} ", form.title),
                Style::default()
                    .fg(CYAN_PRIMARY)
                    .add_modifier(Modifier::BOLD),
            ));

        let inner = block.inner(area);
        block.render(area, buf);

        if inner.height < 3 {
            return;
        }

        // Calculate field layout: each field takes 1 line (label: value)
        let fields_count = form.fields.len();
        let mut constraints: Vec<Constraint> =
            form.fields.iter().map(|_| Constraint::Length(1)).collect();
        constraints.push(Constraint::Min(0)); // Spacer
        constraints.push(Constraint::Length(1)); // Hints

        let chunks = Layout::vertical(constraints).split(inner);

        // Render each field
        for (i, field) in form.fields.iter().enumerate() {
            if i >= chunks.len().saturating_sub(2) {
                break;
            }
            let field_area = chunks[i];
            let is_focused = i == form.focused_field;

            self.render_form_field(field_area, buf, field, is_focused);
        }

        // Render form hints
        let hints_area = chunks[fields_count + 1];
        self.render_form_hints(hints_area, buf);
    }

    /// Render a single form field.
    fn render_form_field(
        &self,
        area: Rect,
        buf: &mut Buffer,
        field: &super::state::InlineFormField,
        is_focused: bool,
    ) {
        let x = area.x + 1;

        // Label
        let label_style = if is_focused {
            Style::default()
                .fg(CYAN_PRIMARY)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(TEXT_DIM)
        };

        let required_marker = if field.required { "*" } else { "" };
        let label = format!("{}{}:", field.label, required_marker);
        buf.set_string(x, area.y, &label, label_style);

        // Value or placeholder
        let value_x = x + label.len() as u16 + 1;
        let remaining_width = area.width.saturating_sub(value_x - area.x + 1);

        if field.value.is_empty() && !is_focused {
            // Show placeholder
            let placeholder = if field.placeholder.len() > remaining_width as usize {
                format!("{}...", &field.placeholder[..remaining_width as usize - 3])
            } else {
                field.placeholder.clone()
            };
            buf.set_string(
                value_x,
                area.y,
                &placeholder,
                Style::default().fg(TEXT_MUTED),
            );
        } else {
            // Show value with cursor if focused
            let display_value = if field.value.len() > remaining_width as usize - 1 {
                format!(
                    "...{}",
                    &field.value[field.value.len() - (remaining_width as usize - 4)..]
                )
            } else {
                field.value.clone()
            };

            let value_style = if is_focused {
                Style::default().fg(TEXT).bg(SURFACE_1)
            } else {
                Style::default().fg(TEXT)
            };

            // Draw input background if focused
            if is_focused {
                for xi in value_x..(value_x + remaining_width) {
                    buf[(xi, area.y)].set_bg(SURFACE_1);
                }
            }

            buf.set_string(value_x, area.y, &display_value, value_style);

            // Draw cursor
            if is_focused {
                let cursor_x = value_x + display_value.len() as u16;
                if cursor_x < area.x + area.width - 1 {
                    buf[(cursor_x, area.y)].set_char('_');
                    buf[(cursor_x, area.y)].set_fg(CYAN_PRIMARY);
                }
            }
        }
    }

    /// Render hints for the form.
    fn render_form_hints(&self, area: Rect, buf: &mut Buffer) {
        let hints = [("Tab", "next"), ("Enter", "submit"), ("Esc", "cancel")];

        // Use standard TEXT_DIM color for hints
        let hint_color = TEXT_DIM;

        let mut spans = Vec::new();
        for (i, (key, action)) in hints.iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled("  ", Style::default()));
            }
            spans.push(Span::styled(
                format!("[{}]", key),
                Style::default().fg(hint_color),
            ));
            spans.push(Span::styled(
                format!(" {}", action),
                Style::default().fg(hint_color),
            ));
        }

        let hints_line = Line::from(spans);
        Paragraph::new(hints_line).render(area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interactive::state::InteractiveAction;

    #[test]
    fn test_required_height() {
        let items = vec![
            InteractiveItem::new("1", "Item 1"),
            InteractiveItem::new("2", "Item 2"),
            InteractiveItem::new("3", "Item 3"),
        ];
        let state = InteractiveState::new("Test", items, InteractiveAction::Custom("test".into()));
        let widget = InteractiveWidget::new(&state);

        // 3 items + 1 title + 1 hints + 2 border = 7
        assert_eq!(widget.required_height(), 7);
    }

    #[test]
    fn test_required_height_with_search() {
        let items = vec![
            InteractiveItem::new("1", "Item 1"),
            InteractiveItem::new("2", "Item 2"),
        ];
        let state = InteractiveState::new("Test", items, InteractiveAction::Custom("test".into()))
            .with_search();
        let widget = InteractiveWidget::new(&state);

        // 2 items + 1 title + 1 search + 1 hints + 2 border = 7
        assert_eq!(widget.required_height(), 7);
    }
}
