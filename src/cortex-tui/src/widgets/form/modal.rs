//! Form modal widget implementation.

use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};

use super::colors::FormModalColors;
use super::field_kind::FieldKind;
use super::state::FormState;
use crate::ui::text_utils::{MIN_TERMINAL_WIDTH, truncate_with_ellipsis};

/// A modal form widget for rendering form dialogs.
pub struct FormModal<'a> {
    /// The form state to render.
    state: &'a FormState,
    /// Colors for the modal.
    pub colors: FormModalColors,
}

impl<'a> FormModal<'a> {
    /// Creates a new form modal widget.
    pub fn new(state: &'a FormState) -> Self {
        Self {
            state,
            colors: FormModalColors::default(),
        }
    }

    /// Sets custom colors for the modal.
    pub fn colors(mut self, colors: FormModalColors) -> Self {
        self.colors = colors;
        self
    }

    /// Calculates a centered rectangle within the given area.
    pub fn centered_rect(area: Rect, percent_x: u16, percent_y: u16) -> Rect {
        let popup_width = area.width * percent_x / 100;
        let popup_height = area.height * percent_y / 100;
        let x = (area.width.saturating_sub(popup_width)) / 2;
        let y = (area.height.saturating_sub(popup_height)) / 2;
        Rect::new(
            area.x + x,
            area.y + y,
            popup_width.min(area.width),
            popup_height.min(area.height),
        )
    }
}

impl<'a> Widget for FormModal<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Calculate modal area (centered, 60% width, 70% height)
        let modal_area = Self::centered_rect(area, 60, 70);

        // Early return if modal is too narrow or short
        let min_modal_width = MIN_TERMINAL_WIDTH.min(30);
        if modal_area.width < min_modal_width || modal_area.height < 5 {
            return;
        }

        // Clear the background for opaque rendering
        Clear.render(modal_area, buf);

        // Fill with background color
        for y in modal_area.y..modal_area.y + modal_area.height {
            for x in modal_area.x..modal_area.x + modal_area.width {
                if let Some(cell) = buf.cell_mut(Position::new(x, y)) {
                    cell.set_bg(self.colors.background);
                }
            }
        }

        // Draw the border block with title
        let block = Block::default()
            .title(format!(" {} ", self.state.title))
            .title_style(Style::default().fg(self.colors.accent).bold())
            .borders(Borders::ALL)
            .border_style(Style::default().fg(self.colors.border))
            .style(Style::default().bg(self.colors.background));

        let inner_area = block.inner(modal_area);
        block.render(modal_area, buf);

        // Render fields
        let field_height = 2u16; // Label + input per field
        let mut y_offset = 0u16;

        for (idx, field) in self.state.fields.iter().enumerate() {
            if y_offset + field_height > inner_area.height.saturating_sub(2) {
                break; // Leave room for submit button
            }

            let is_focused = idx == self.state.focus_index;
            let label_style = if is_focused {
                Style::default().fg(self.colors.accent).bold()
            } else {
                Style::default().fg(self.colors.text)
            };

            // Render label
            let label_area = Rect::new(
                inner_area.x + 1,
                inner_area.y + y_offset,
                inner_area.width.saturating_sub(2),
                1,
            );

            // Truncate label to fit available width
            let max_label_width = label_area.width as usize;
            let label_text = if field.required {
                let base_label =
                    truncate_with_ellipsis(&field.label, max_label_width.saturating_sub(2));
                format!("{} *", base_label)
            } else {
                truncate_with_ellipsis(&field.label, max_label_width)
            };

            Paragraph::new(label_text)
                .style(label_style)
                .render(label_area, buf);

            y_offset += 1;

            // Render input field
            let input_area = Rect::new(
                inner_area.x + 1,
                inner_area.y + y_offset,
                inner_area.width.saturating_sub(2),
                1,
            );

            let (display_value, input_style) = match &field.kind {
                FieldKind::Text | FieldKind::Number => {
                    let val = if field.value.is_empty() {
                        field.placeholder.clone().unwrap_or_default()
                    } else {
                        field.value.clone()
                    };
                    let style = if field.value.is_empty() && field.placeholder.is_some() {
                        Style::default().fg(self.colors.text_muted)
                    } else {
                        Style::default().fg(self.colors.text)
                    };
                    (val, style)
                }
                FieldKind::Secret => {
                    let val = if field.value.is_empty() {
                        field.placeholder.clone().unwrap_or_default()
                    } else {
                        "*".repeat(field.value.len())
                    };
                    let style = if field.value.is_empty() && field.placeholder.is_some() {
                        Style::default().fg(self.colors.text_muted)
                    } else {
                        Style::default().fg(self.colors.text)
                    };
                    (val, style)
                }
                FieldKind::Toggle => {
                    let val = if field.toggle_state {
                        "[x] ON ".to_string()
                    } else {
                        "[ ] OFF".to_string()
                    };
                    (val, Style::default().fg(self.colors.text))
                }
                FieldKind::Select(options) => {
                    let current = if field.select_index < options.len() {
                        &options[field.select_index]
                    } else {
                        ""
                    };
                    // Truncate option text if needed, leaving space for dropdown indicator
                    let max_opt_width = input_area.width.saturating_sub(3) as usize;
                    let truncated_current = truncate_with_ellipsis(current, max_opt_width);
                    let val = format!("{} v", truncated_current);
                    (val, Style::default().fg(self.colors.text))
                }
            };

            // Truncate display value to fit input area
            let max_value_width = input_area.width as usize;
            let display_value = truncate_with_ellipsis(&display_value, max_value_width);

            // Draw input background
            let input_bg_style = if is_focused {
                Style::default().bg(self.colors.surface)
            } else {
                Style::default().bg(self.colors.background)
            };

            for x in input_area.x..input_area.x + input_area.width {
                if let Some(cell) = buf.cell_mut(Position::new(x, input_area.y)) {
                    cell.set_style(input_bg_style);
                }
            }

            Paragraph::new(display_value)
                .style(input_style.bg(if is_focused {
                    self.colors.surface
                } else {
                    self.colors.background
                }))
                .render(input_area, buf);

            // Show cursor for text fields when focused
            if is_focused {
                match &field.kind {
                    FieldKind::Text | FieldKind::Secret | FieldKind::Number => {
                        let cursor_x = input_area.x + field.cursor_pos as u16;
                        if cursor_x < input_area.x + input_area.width
                            && let Some(cell) = buf.cell_mut(Position::new(cursor_x, input_area.y))
                        {
                            cell.set_style(
                                Style::default()
                                    .bg(self.colors.accent)
                                    .fg(self.colors.background),
                            );
                        }
                    }
                    _ => {}
                }
            }

            y_offset += 1;
        }

        // Render submit button at the bottom
        let submit_y = inner_area.y + inner_area.height.saturating_sub(1);
        let submit_text = "[ Submit ]";
        let submit_x =
            inner_area.x + (inner_area.width.saturating_sub(submit_text.len() as u16)) / 2;

        let submit_style = if self.state.is_submit_focused() {
            Style::default()
                .fg(self.colors.background)
                .bg(self.colors.accent)
                .bold()
        } else {
            Style::default().fg(self.colors.text_dim)
        };

        let submit_area = Rect::new(submit_x, submit_y, submit_text.len() as u16, 1);
        Paragraph::new(submit_text)
            .style(submit_style)
            .render(submit_area, buf);
    }
}
