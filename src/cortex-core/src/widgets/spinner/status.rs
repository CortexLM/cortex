//! Status spinner with status text and elapsed time display.

use crate::animation::Spinner;
use crate::style::{ORANGE, TEXT, TEXT_DIM, TEXT_MUTED};
use ratatui::prelude::*;
use ratatui::widgets::Widget;
use std::time::Instant;

/// A spinner with status text and optional elapsed time display.
///
/// Renders a spinner with a main status message, optional detail line,
/// and optional elapsed time counter.
///
/// # Example
/// ```
/// use cortex_engine::animation::Spinner;
/// use cortex_engine::widgets::StatusSpinner;
/// use std::time::Instant;
///
/// let mut spinner = Spinner::dots();
/// let start = Instant::now();
///
/// let widget = StatusSpinner::new(&spinner, "Loading model...")
///     .detail("Connecting to API")
///     .with_timer(start);
/// ```
///
/// # Output Format
/// ```text
/// * Loading model...          [2.3s]
///   Connecting to API
/// ```
pub struct StatusSpinner<'a> {
    spinner: &'a Spinner,
    status: &'a str,
    detail: Option<&'a str>,
    start_time: Option<Instant>,
}

impl<'a> StatusSpinner<'a> {
    /// Creates a new status spinner.
    ///
    /// # Arguments
    /// * `spinner` - Reference to the spinner animation state
    /// * `status` - Main status message to display
    pub fn new(spinner: &'a Spinner, status: &'a str) -> Self {
        Self {
            spinner,
            status,
            detail: None,
            start_time: None,
        }
    }

    /// Sets the detail text to display below the status.
    ///
    /// # Arguments
    /// * `detail` - Secondary detail text
    pub fn detail(mut self, detail: &'a str) -> Self {
        self.detail = Some(detail);
        self
    }

    /// Enables the elapsed time display.
    ///
    /// # Arguments
    /// * `start` - The instant when the operation started
    pub fn with_timer(mut self, start: Instant) -> Self {
        self.start_time = Some(start);
        self
    }
}

impl Widget for StatusSpinner<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        let spinner_char = self.spinner.current();
        let spinner_style = Style::default().fg(ORANGE);
        let status_style = Style::default().fg(TEXT);
        let timer_style = Style::default().fg(TEXT_MUTED);
        let detail_style = Style::default().fg(TEXT_DIM);

        // Calculate timer string if we have a start time
        let timer_str = self.start_time.map(|start| {
            let elapsed = start.elapsed();
            format!("[{:.1}s]", elapsed.as_secs_f64())
        });

        // Calculate the timer width for right-alignment
        let timer_width = timer_str.as_ref().map(|s| s.len()).unwrap_or(0);

        // First line: "{spinner} {status}          [{time}]"
        let mut x = area.x;

        // Render spinner character
        buf.set_string(x, area.y, spinner_char, spinner_style);
        x += spinner_char.chars().count() as u16;

        // Render space
        buf.set_string(x, area.y, " ", status_style);
        x += 1;

        // Calculate available width for status
        let available_for_status = if timer_width > 0 {
            area.width
                .saturating_sub(x - area.x + timer_width as u16 + 1)
        } else {
            area.width.saturating_sub(x - area.x)
        };

        // Render status (truncate if necessary)
        let status_display: String = self
            .status
            .chars()
            .take(available_for_status as usize)
            .collect();
        buf.set_string(x, area.y, &status_display, status_style);

        // Render timer at the right edge
        if let Some(ref timer) = timer_str {
            let timer_x = area.x + area.width - timer.len() as u16;
            buf.set_string(timer_x, area.y, timer, timer_style);
        }

        // Second line: detail (indented to align with status)
        if let Some(detail) = self.detail {
            if area.height > 1 {
                // Indent with 2 spaces (to align with status after spinner + space)
                let indent = "  ";
                let detail_x = area.x;
                let available_width = area.width as usize;

                let detail_display: String = format!("{}{}", indent, detail)
                    .chars()
                    .take(available_width)
                    .collect();

                buf.set_string(detail_x, area.y + 1, &detail_display, detail_style);
            }
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
    fn test_status_spinner_new() {
        let spinner = Spinner::dots();
        let widget = StatusSpinner::new(&spinner, "Loading...");
        assert_eq!(widget.status, "Loading...");
        assert!(widget.detail.is_none());
        assert!(widget.start_time.is_none());
    }

    #[test]
    fn test_status_spinner_with_detail() {
        let spinner = Spinner::dots();
        let widget = StatusSpinner::new(&spinner, "Loading...").detail("Please wait");
        assert_eq!(widget.detail, Some("Please wait"));
    }

    #[test]
    fn test_status_spinner_with_timer() {
        let spinner = Spinner::dots();
        let start = Instant::now();
        let widget = StatusSpinner::new(&spinner, "Loading...").with_timer(start);
        assert!(widget.start_time.is_some());
    }

    #[test]
    fn test_status_spinner_render() {
        let spinner = Spinner::dots();
        let widget = StatusSpinner::new(&spinner, "Loading...").detail("Detail text");

        let mut buf = create_test_buffer(40, 2);
        let area = Rect::new(0, 0, 40, 2);
        widget.render(area, &mut buf);

        // Check first line contains status
        let line1: String = (0..40)
            .map(|x| buf.get(x, 0).symbol().chars().next().unwrap_or(' '))
            .collect();
        assert!(line1.contains("Loading"));

        // Check second line contains detail
        let line2: String = (0..40)
            .map(|x| buf.get(x, 1).symbol().chars().next().unwrap_or(' '))
            .collect();
        assert!(line2.contains("Detail"));
    }
}
