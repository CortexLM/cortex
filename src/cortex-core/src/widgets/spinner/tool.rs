//! Tool execution indicator with tool name and elapsed time.

use crate::animation::{ElapsedTimer, Spinner};
use crate::style::{CYAN_PRIMARY, TEXT, TEXT_MUTED};
use ratatui::prelude::*;
use ratatui::widgets::Widget;

/// A spinner indicator for tool execution with tool name and elapsed time.
///
/// Displays a fast-spinning indicator with the tool name and elapsed time,
/// ideal for showing active tool execution.
///
/// # Example
/// ```
/// use cortex_engine::widgets::ToolIndicator;
///
/// let indicator = ToolIndicator::new("file_search");
/// // Later: indicator.tick();
/// ```
///
/// # Output Format
/// ```text
/// * Running file_search... (1.2s)
/// ```
pub struct ToolIndicator {
    spinner: Spinner,
    tool_name: String,
    timer: ElapsedTimer,
}

impl ToolIndicator {
    /// Creates a new tool indicator with the specified tool name.
    ///
    /// # Arguments
    /// * `tool_name` - Name of the tool being executed
    pub fn new(tool_name: &str) -> Self {
        Self {
            spinner: Spinner::tool(),
            tool_name: tool_name.to_string(),
            timer: ElapsedTimer::new(),
        }
    }

    /// Advances the spinner animation.
    pub fn tick(&mut self) {
        self.spinner.tick();
    }

    /// Resets the timer.
    pub fn reset_timer(&mut self) {
        self.timer.reset();
    }

    /// Returns the elapsed time.
    pub fn elapsed(&self) -> std::time::Duration {
        self.timer.elapsed()
    }

    /// Returns the tool name.
    pub fn tool_name(&self) -> &str {
        &self.tool_name
    }
}

impl Widget for &ToolIndicator {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        let spinner_char = self.spinner.frame();
        let spinner_style = Style::default().fg(CYAN_PRIMARY);
        let text_style = Style::default().fg(TEXT);
        let timer_style = Style::default().fg(TEXT_MUTED);

        let mut x = area.x;

        // Render spinner character
        buf.set_string(x, area.y, spinner_char, spinner_style);
        x += spinner_char.chars().count() as u16;

        // Render " Running "
        let running_text = " Running ";
        buf.set_string(x, area.y, running_text, text_style);
        x += running_text.len() as u16;

        // Render tool name
        buf.set_string(x, area.y, &self.tool_name, spinner_style);
        x += self.tool_name.chars().count() as u16;

        // Render "..."
        buf.set_string(x, area.y, "... ", text_style);
        x += 4;

        // Render timer
        let timer_str = self.timer.render_bracketed();
        let remaining_width = area.width.saturating_sub(x - area.x) as usize;
        if timer_str.len() <= remaining_width {
            buf.set_string(x, area.y, &timer_str, timer_style);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;

    fn create_test_buffer(width: u16, height: u16) -> Buffer {
        Buffer::empty(Rect::new(0, 0, width, height))
    }

    #[test]
    fn test_tool_indicator_new() {
        let indicator = ToolIndicator::new("file_search");
        assert_eq!(indicator.tool_name(), "file_search");
    }

    #[test]
    fn test_tool_indicator_tick() {
        let mut indicator = ToolIndicator::new("bash");
        indicator.tick();
        // Should not panic
    }

    #[test]
    fn test_tool_indicator_render() {
        let indicator = ToolIndicator::new("read_file");

        let mut buf = create_test_buffer(50, 1);
        let area = Rect::new(0, 0, 50, 1);
        (&indicator).render(area, &mut buf);

        // Check that content includes "Running" and tool name
        let content: String = (0..50)
            .map(|x| buf.get(x, 0).symbol().chars().next().unwrap_or(' '))
            .collect();
        assert!(content.contains("Running"));
        assert!(content.contains("read_file"));
    }
}
