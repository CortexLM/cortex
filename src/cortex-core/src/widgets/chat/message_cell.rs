//! Single message cell widget.
//!
//! Provides the `MessageCell` widget for rendering individual chat messages.

use super::parsing::parse_markdown_lite;
use super::types::Message;
use super::wrapping::wrap_text;
use crate::animation::Typewriter;
use crate::style::CortexStyle;
use ratatui::prelude::*;
use ratatui::widgets::Widget;

/// The cursor character displayed at the end of streaming text.
const STREAMING_CURSOR: &str = "▌";

/// Widget for rendering a single chat message.
///
/// Handles text wrapping, markdown-lite formatting, and streaming animation.
pub struct MessageCell<'a> {
    message: &'a Message,
    typewriter: Option<&'a Typewriter>,
    show_role_prefix: bool,
    max_width: Option<u16>,
}

impl<'a> MessageCell<'a> {
    /// Creates a new message cell for the given message.
    pub fn new(message: &'a Message) -> Self {
        Self {
            message,
            typewriter: None,
            show_role_prefix: true,
            max_width: None,
        }
    }

    /// Sets the typewriter for streaming animation.
    ///
    /// When provided and the message is streaming, the visible text
    /// will be determined by the typewriter's progress.
    pub fn typewriter(mut self, typewriter: &'a Typewriter) -> Self {
        self.typewriter = Some(typewriter);
        self
    }

    /// Sets whether to show the role prefix (e.g., "You: ", "Assistant: ").
    pub fn show_role_prefix(mut self, show: bool) -> Self {
        self.show_role_prefix = show;
        self
    }

    /// Sets the maximum width for text wrapping.
    pub fn max_width(mut self, width: u16) -> Self {
        self.max_width = Some(width);
        self
    }

    /// Calculates the height needed to render this message.
    pub fn required_height(&self, width: u16) -> u16 {
        let content = self.get_display_content();
        let prefix_len = if self.show_role_prefix {
            self.message.prefix().chars().count()
        } else {
            0
        };

        let effective_width = self.max_width.unwrap_or(width) as usize;
        if effective_width == 0 {
            return 1;
        }

        // Account for prefix on first line
        let first_line_width = effective_width.saturating_sub(prefix_len);
        let wrapped = wrap_text(&content, first_line_width.max(1));

        // If content is short enough to fit with prefix, just count lines
        let lines = wrapped.len();
        lines.max(1) as u16
    }

    /// Gets the content to display, accounting for streaming.
    fn get_display_content(&self) -> String {
        if self.message.is_streaming {
            if let Some(tw) = self.typewriter {
                tw.visible_text().to_string()
            } else {
                self.message.content.clone()
            }
        } else {
            self.message.content.clone()
        }
    }
}

impl<'a> Widget for MessageCell<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        let style = self.message.role.style();
        let content = self.get_display_content();
        let effective_width = self.max_width.unwrap_or(area.width) as usize;

        // Render prefix on first line
        let prefix = if self.show_role_prefix {
            self.message.prefix()
        } else {
            String::new()
        };
        let prefix_len = prefix.chars().count();

        let mut y = area.y;
        let mut x = area.x;

        // Render the prefix with the role's style
        if !prefix.is_empty() {
            for ch in prefix.chars() {
                if x >= area.x + area.width {
                    break;
                }
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_char(ch).set_style(style);
                }
                x += 1;
            }
        }

        // Calculate remaining width for content on first line
        let first_line_width = effective_width.saturating_sub(prefix_len);

        // Parse and render content with markdown-lite
        let segments = parse_markdown_lite(&content, style);

        // Flatten segments into chars with styles for easier line wrapping
        let styled_chars: Vec<(char, Style)> = segments
            .iter()
            .flat_map(|seg| seg.text.chars().map(move |c| (c, seg.style)))
            .collect();

        let mut char_idx = 0;
        let total_chars = styled_chars.len();

        // Render first line (after prefix)
        let mut remaining_width = if first_line_width > 0 {
            first_line_width
        } else {
            effective_width
        };

        while char_idx < total_chars && y < area.y + area.height {
            let (ch, char_style) = styled_chars[char_idx];

            if ch == '\n' {
                // Move to next line
                y += 1;
                x = area.x;
                remaining_width = effective_width;
                char_idx += 1;
                continue;
            }

            if remaining_width == 0 {
                // Move to next line
                y += 1;
                x = area.x;
                remaining_width = effective_width;
                if y >= area.y + area.height {
                    break;
                }
            }

            if x < area.x + area.width {
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_char(ch).set_style(char_style);
                }
                x += 1;
                remaining_width = remaining_width.saturating_sub(1);
            }
            char_idx += 1;
        }

        // Add streaming cursor if applicable
        if self.message.is_streaming {
            let show_cursor = if let Some(tw) = self.typewriter {
                !tw.is_complete()
            } else {
                true
            };

            if show_cursor && x < area.x + area.width && y < area.y + area.height {
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_char(STREAMING_CURSOR.chars().next().expect("cursor char exists"))
                        .set_style(style);
                }
            }
        }

        // Render timestamp if present and there's room
        if let Some(ref ts) = self.message.timestamp {
            let ts_style = CortexStyle::dimmed();
            let ts_len = ts.chars().count();

            // Place timestamp at the end of the last content line
            if y < area.y + area.height {
                let ts_x = area.x + area.width - ts_len as u16 - 1;
                if ts_x > x + 2 {
                    // Ensure some gap
                    let mut tx = ts_x;
                    for ch in ts.chars() {
                        if tx < area.x + area.width {
                            if let Some(cell) = buf.cell_mut((tx, y)) {
                                cell.set_char(ch).set_style(ts_style);
                            }
                            tx += 1;
                        }
                    }
                }
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
    fn test_message_cell_height() {
        let msg = Message::user("Hello");
        let cell = MessageCell::new(&msg);
        let height = cell.required_height(80);
        assert!(height >= 1);
    }

    #[test]
    fn test_message_cell_height_long_message() {
        let long_content = "This is a very long message that should wrap across multiple lines when rendered in a narrow terminal window.";
        let msg = Message::user(long_content);
        let cell = MessageCell::new(&msg);
        let height = cell.required_height(20);
        assert!(height > 1);
    }
}
