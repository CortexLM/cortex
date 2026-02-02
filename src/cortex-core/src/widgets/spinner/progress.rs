//! Progress spinner with current/total indication.

use crate::animation::Spinner;
use crate::style::{BLUE, ORANGE, TEXT};
use ratatui::prelude::*;
use ratatui::widgets::Widget;

/// A spinner with progress indication (current/total).
///
/// Displays a spinner with a label and numeric progress counter.
///
/// # Example
/// ```
/// use cortex_engine::animation::Spinner;
/// use cortex_engine::widgets::ProgressSpinner;
///
/// let mut spinner = Spinner::dots();
///
/// let widget = ProgressSpinner::new(&spinner, "Processing files...", 42, 100);
/// ```
///
/// # Output Format
/// ```text
/// * Processing files... (42/100)
/// ```
pub struct ProgressSpinner<'a> {
    spinner: &'a Spinner,
    label: &'a str,
    current: u64,
    total: u64,
}

impl<'a> ProgressSpinner<'a> {
    /// Creates a new progress spinner.
    ///
    /// # Arguments
    /// * `spinner` - Reference to the spinner animation state
    /// * `label` - Label to display after the spinner
    /// * `current` - Current progress value
    /// * `total` - Total progress value
    pub fn new(spinner: &'a Spinner, label: &'a str, current: u64, total: u64) -> Self {
        Self {
            spinner,
            label,
            current,
            total,
        }
    }
}

impl Widget for ProgressSpinner<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        let spinner_char = self.spinner.current();
        let spinner_style = Style::default().fg(ORANGE);
        let label_style = Style::default().fg(TEXT);
        let progress_style = Style::default().fg(BLUE);

        let mut x = area.x;

        // Render spinner character
        buf.set_string(x, area.y, spinner_char, spinner_style);
        x += spinner_char.chars().count() as u16;

        // Render space
        buf.set_string(x, area.y, " ", label_style);
        x += 1;

        // Render label
        let label_display: String = self.label.chars().collect();
        buf.set_string(x, area.y, &label_display, label_style);
        x += label_display.chars().count() as u16;

        // Render space before progress
        buf.set_string(x, area.y, " ", label_style);
        x += 1;

        // Render progress counter
        let progress_str = format!("({}/{})", self.current, self.total);

        // Check if we have enough space
        let remaining_width = area.width.saturating_sub(x - area.x) as usize;
        if progress_str.len() <= remaining_width {
            buf.set_string(x, area.y, &progress_str, progress_style);
        } else if remaining_width > 3 {
            // Truncate with ellipsis if not enough space
            let truncated: String = progress_str.chars().take(remaining_width - 3).collect();
            buf.set_string(x, area.y, &format!("{}...", truncated), progress_style);
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
    fn test_progress_spinner_new() {
        let spinner = Spinner::dots();
        let widget = ProgressSpinner::new(&spinner, "Processing...", 42, 100);
        assert_eq!(widget.label, "Processing...");
        assert_eq!(widget.current, 42);
        assert_eq!(widget.total, 100);
    }

    #[test]
    fn test_progress_spinner_render() {
        let spinner = Spinner::dots();
        let widget = ProgressSpinner::new(&spinner, "Files", 42, 100);

        let mut buf = create_test_buffer(30, 1);
        let area = Rect::new(0, 0, 30, 1);
        widget.render(area, &mut buf);

        // Check that progress is rendered
        let content: String = (0..30)
            .map(|x| buf.get(x, 0).symbol().chars().next().unwrap_or(' '))
            .collect();
        assert!(content.contains("Files"));
        assert!(content.contains("42"));
        assert!(content.contains("100"));
    }
}
