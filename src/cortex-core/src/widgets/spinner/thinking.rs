//! Thinking indicator for AI processing state.

use crate::animation::{Pulse, Spinner};
use crate::style::{CortexStyle, ORANGE};
use ratatui::prelude::*;
use ratatui::widgets::Widget;

/// A pulsing spinner indicator for AI thinking state.
///
/// Combines a spinner with optional pulse animation to create
/// a visually engaging "AI is thinking" indicator. The color
/// pulses through the brain color palette when a pulse is provided.
/// Now includes a "Thinking..." label by default.
///
/// # Example
/// ```
/// use cortex_engine::animation::{Spinner, Pulse};
/// use cortex_engine::widgets::ThinkingIndicator;
///
/// let mut spinner = Spinner::dots();
/// let mut pulse = Pulse::new(1500);
///
/// let widget = ThinkingIndicator::new(&spinner)
///     .with_pulse(&pulse)
///     .label("Processing...");
/// ```
///
/// # Output Format
/// ```text
/// * Thinking...
/// ```
pub struct ThinkingIndicator<'a> {
    spinner: &'a Spinner,
    pulse: Option<&'a Pulse>,
    label: Option<&'a str>,
}

impl<'a> ThinkingIndicator<'a> {
    /// Creates a new thinking indicator.
    ///
    /// # Arguments
    /// * `spinner` - Reference to the spinner animation state
    pub fn new(spinner: &'a Spinner) -> Self {
        Self {
            spinner,
            pulse: None,
            label: None,
        }
    }

    /// Adds a pulse animation for color cycling.
    ///
    /// When a pulse is provided, the spinner color will cycle through
    /// the Ocean/Cyan color palette (CYAN_PRIMARY -> ELECTRIC_BLUE).
    ///
    /// # Arguments
    /// * `pulse` - Reference to the pulse animation state
    pub fn with_pulse(mut self, pulse: &'a Pulse) -> Self {
        self.pulse = Some(pulse);
        self
    }

    /// Sets a custom label (default: "Thinking...").
    ///
    /// # Arguments
    /// * `label` - Custom label text
    pub fn label(mut self, label: &'a str) -> Self {
        self.label = Some(label);
        self
    }
}

impl Widget for ThinkingIndicator<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        let spinner_char = self.spinner.current();

        // Determine the style based on whether we have a pulse
        let style = if let Some(pulse) = self.pulse {
            // Use brain pulse color interpolation
            CortexStyle::brain_pulse(pulse.intensity())
        } else {
            // Default to orange (thinking color)
            Style::default().fg(ORANGE)
        };

        // Render spinner with "Thinking..." label
        let label = self.label.unwrap_or("Thinking...");
        let text = format!("{} {}", spinner_char, label);
        let max_width = area.width as usize;
        let display_text: String = text.chars().take(max_width).collect();

        buf.set_string(area.x, area.y, &display_text, style);
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
    fn test_thinking_indicator_new() {
        let spinner = Spinner::dots();
        let widget = ThinkingIndicator::new(&spinner);
        assert!(widget.pulse.is_none());
    }

    #[test]
    fn test_thinking_indicator_with_pulse() {
        let spinner = Spinner::dots();
        let pulse = Pulse::new(1000);
        let widget = ThinkingIndicator::new(&spinner).with_pulse(&pulse);
        assert!(widget.pulse.is_some());
    }

    #[test]
    fn test_thinking_indicator_render() {
        let spinner = Spinner::dots();
        let widget = ThinkingIndicator::new(&spinner);

        let mut buf = create_test_buffer(10, 1);
        let area = Rect::new(0, 0, 10, 1);
        widget.render(area, &mut buf);

        // Check that spinner char was rendered
        let first_char = buf.get(0, 0).symbol();
        assert!(!first_char.is_empty());
    }

    #[test]
    fn test_thinking_indicator_with_pulse_render() {
        let spinner = Spinner::dots();
        let pulse = Pulse::new(1000);
        let widget = ThinkingIndicator::new(&spinner).with_pulse(&pulse);

        let mut buf = create_test_buffer(20, 1);
        let area = Rect::new(0, 0, 20, 1);
        widget.render(area, &mut buf);

        // Should render without panic and include "Thinking"
        let content: String = (0..20)
            .map(|x| buf.get(x, 0).symbol().chars().next().unwrap_or(' '))
            .collect();
        assert!(content.contains("Thinking"));
    }

    #[test]
    fn test_thinking_indicator_with_label() {
        let spinner = Spinner::dots();
        let widget = ThinkingIndicator::new(&spinner).label("Processing...");
        assert_eq!(widget.label, Some("Processing..."));
    }
}
