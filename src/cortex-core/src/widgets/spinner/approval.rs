//! Approval indicator for awaiting user confirmation.

use crate::animation::{Pulse, Spinner, interpolate_color};
use crate::style::{CYAN_PRIMARY, ELECTRIC_BLUE, TEXT};
use ratatui::prelude::*;
use ratatui::widgets::Widget;

/// A pulsing indicator for awaiting user approval.
///
/// Displays a slow-pulsing spinner with "Awaiting approval..." text,
/// ideal for showing that user input is required.
///
/// # Example
/// ```
/// use cortex_engine::widgets::ApprovalIndicator;
///
/// let mut indicator = ApprovalIndicator::new();
/// // Later: indicator.tick();
/// ```
///
/// # Output Format
/// ```text
/// * Awaiting approval...
/// ```
pub struct ApprovalIndicator {
    spinner: Spinner,
    pulse: Pulse,
    message: String,
}

impl ApprovalIndicator {
    /// Creates a new approval indicator with default message.
    pub fn new() -> Self {
        Self {
            spinner: Spinner::approval(),
            pulse: Pulse::new(2000), // Slow pulse for attention
            message: "Awaiting approval...".to_string(),
        }
    }

    /// Creates an approval indicator with a custom message.
    pub fn with_message(message: &str) -> Self {
        Self {
            spinner: Spinner::approval(),
            pulse: Pulse::new(2000),
            message: message.to_string(),
        }
    }

    /// Advances the animation.
    pub fn tick(&mut self) {
        self.spinner.tick();
        self.pulse.tick();
    }

    /// Returns the current message.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Sets a new message.
    pub fn set_message(&mut self, message: &str) {
        self.message = message.to_string();
    }
}

impl Default for ApprovalIndicator {
    fn default() -> Self {
        Self::new()
    }
}

impl Widget for &ApprovalIndicator {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        let spinner_char = self.spinner.frame();
        // Use pulsing color - brighter at high intensity for attention
        let pulse_color = interpolate_color(CYAN_PRIMARY, ELECTRIC_BLUE, self.pulse.intensity());
        let spinner_style = Style::default().fg(pulse_color);
        let text_style = Style::default().fg(TEXT);

        let mut x = area.x;

        // Render spinner character
        buf.set_string(x, area.y, spinner_char, spinner_style);
        x += spinner_char.chars().count() as u16;

        // Render space and message
        let message_text = format!(" {}", self.message);
        let remaining_width = area.width.saturating_sub(x - area.x) as usize;
        let display_text: String = message_text.chars().take(remaining_width).collect();
        buf.set_string(x, area.y, &display_text, text_style);
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
    fn test_approval_indicator_new() {
        let indicator = ApprovalIndicator::new();
        assert_eq!(indicator.message(), "Awaiting approval...");
    }

    #[test]
    fn test_approval_indicator_with_message() {
        let indicator = ApprovalIndicator::with_message("Please confirm");
        assert_eq!(indicator.message(), "Please confirm");
    }

    #[test]
    fn test_approval_indicator_set_message() {
        let mut indicator = ApprovalIndicator::new();
        indicator.set_message("New message");
        assert_eq!(indicator.message(), "New message");
    }

    #[test]
    fn test_approval_indicator_render() {
        let indicator = ApprovalIndicator::new();

        let mut buf = create_test_buffer(50, 1);
        let area = Rect::new(0, 0, 50, 1);
        (&indicator).render(area, &mut buf);

        // Check that content includes the message
        let content: String = (0..50)
            .map(|x| buf.get(x, 0).symbol().chars().next().unwrap_or(' '))
            .collect();
        assert!(content.contains("Awaiting"));
    }
}
