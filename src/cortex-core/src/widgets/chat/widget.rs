//! Chat widget for rendering message lists.
//!
//! Provides the main `ChatWidget` for displaying conversations with support
//! for scrolling, streaming animation, and text selection.

use super::message_cell::MessageCell;
use super::types::Message;
use crate::animation::Typewriter;
use ratatui::prelude::*;
use ratatui::widgets::Widget;

/// Widget for rendering a list of chat messages.
///
/// Supports scrolling, streaming animation, and text selection.
pub struct ChatWidget<'a> {
    messages: &'a [Message],
    scroll_offset: usize,
    typewriter: Option<&'a Typewriter>,
    show_timestamps: bool,
    /// Selection bounds: ((start_col, start_row), (end_col, end_row)) relative to chat area
    selection: Option<((u16, u16), (u16, u16))>,
}

impl<'a> ChatWidget<'a> {
    /// Creates a new chat widget with the given messages.
    pub fn new(messages: &'a [Message]) -> Self {
        Self {
            messages,
            scroll_offset: 0,
            typewriter: None,
            show_timestamps: false,
            selection: None,
        }
    }

    /// Sets the scroll offset (number of messages to skip from the top).
    pub fn scroll_offset(mut self, offset: usize) -> Self {
        self.scroll_offset = offset;
        self
    }

    /// Sets the typewriter for streaming animation.
    ///
    /// The typewriter will be applied to the last message if it's streaming.
    pub fn typewriter(mut self, typewriter: &'a Typewriter) -> Self {
        self.typewriter = Some(typewriter);
        self
    }

    /// Sets whether to show timestamps on messages.
    pub fn show_timestamps(mut self, show: bool) -> Self {
        self.show_timestamps = show;
        self
    }

    /// Sets the text selection bounds for highlighting.
    ///
    /// The bounds are in chat-area-relative coordinates.
    /// Start should be before end in reading order.
    pub fn with_selection(mut self, selection: ((u16, u16), (u16, u16))) -> Self {
        self.selection = Some(selection);
        self
    }

    /// Clears the text selection.
    pub fn clear_selection(mut self) -> Self {
        self.selection = None;
        self
    }

    /// Calculates the total height needed to render all messages.
    pub fn total_height(&self, width: u16) -> u16 {
        self.messages
            .iter()
            .map(|msg| {
                let cell = MessageCell::new(msg);
                cell.required_height(width) + 1 // +1 for spacing between messages
            })
            .sum::<u16>()
            .saturating_sub(1) // Remove trailing space
    }

    /// Returns the number of messages.
    pub fn message_count(&self) -> usize {
        self.messages.len()
    }
}

impl<'a> Widget for ChatWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 || self.messages.is_empty() {
            return;
        }

        let mut y = area.y;
        let message_count = self.messages.len();

        // Skip messages based on scroll offset
        for (idx, message) in self.messages.iter().enumerate().skip(self.scroll_offset) {
            if y >= area.y + area.height {
                break;
            }

            // Determine if this message should use the typewriter
            let is_last = idx == message_count - 1;
            let use_typewriter = is_last && message.is_streaming && self.typewriter.is_some();

            let mut cell = MessageCell::new(message).max_width(area.width);

            if use_typewriter {
                if let Some(tw) = self.typewriter {
                    cell = cell.typewriter(tw);
                }
            }

            // Calculate message height
            let msg_height = cell.required_height(area.width);
            let available_height = area.y + area.height - y;

            // Render the message
            let msg_area = Rect {
                x: area.x,
                y,
                width: area.width,
                height: msg_height.min(available_height),
            };

            cell.render(msg_area, buf);

            // Move to next message position (with spacing)
            y += msg_height + 1;
        }

        // Apply selection highlight if present
        if let Some(((start_col, start_row), (end_col, end_row))) = self.selection {
            apply_selection_highlight(buf, area, start_col, start_row, end_col, end_row);
        }
    }
}

/// Extracts text from the rendered chat area based on selection bounds.
///
/// This function reconstructs the text that would be visible in the selection
/// by examining the buffer contents.
pub fn extract_selected_text(
    buf: &Buffer,
    area: Rect,
    start_col: u16,
    start_row: u16,
    end_col: u16,
    end_row: u16,
) -> String {
    let mut result = String::new();

    for row in start_row..=end_row {
        let screen_y = area.y + row;
        if screen_y >= area.y + area.height {
            break;
        }

        // Determine column range for this row
        let col_start = if row == start_row { start_col } else { 0 };
        let col_end = if row == end_row {
            end_col
        } else {
            area.width.saturating_sub(1)
        };

        let mut line = String::new();
        for col in col_start..=col_end {
            let screen_x = area.x + col;
            if screen_x >= area.x + area.width {
                break;
            }

            if let Some(cell) = buf.cell((screen_x, screen_y)) {
                line.push_str(cell.symbol());
            }
        }

        // Trim trailing whitespace from each line
        let trimmed = line.trim_end();
        result.push_str(trimmed);

        // Add newline between rows (except for last row)
        if row < end_row {
            result.push('\n');
        }
    }

    result
}

/// Applies selection highlight to the buffer.
///
/// Inverts the colors of selected text to show selection.
fn apply_selection_highlight(
    buf: &mut Buffer,
    area: Rect,
    start_col: u16,
    start_row: u16,
    end_col: u16,
    end_row: u16,
) {
    // Selection highlight style (inverted colors)
    let selection_bg = Color::Rgb(60, 100, 140); // Blue-ish highlight

    for row in start_row..=end_row {
        let screen_y = area.y + row;
        if screen_y >= area.y + area.height {
            break;
        }

        // Determine column range for this row
        let col_start = if row == start_row { start_col } else { 0 };
        let col_end = if row == end_row {
            end_col
        } else {
            area.width.saturating_sub(1)
        };

        for col in col_start..=col_end {
            let screen_x = area.x + col;
            if screen_x >= area.x + area.width {
                break;
            }

            if let Some(cell) = buf.cell_mut((screen_x, screen_y)) {
                // Apply selection background
                cell.set_bg(selection_bg);
            }
        }
    }
}

// ============================================================
// TESTS
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chat_widget_creation() {
        let messages = vec![Message::user("Hello"), Message::assistant("Hi there")];
        let widget = ChatWidget::new(&messages);
        assert_eq!(widget.message_count(), 2);
    }
}
