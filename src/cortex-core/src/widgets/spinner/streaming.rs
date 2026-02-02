//! Streaming indicator for AI response streaming with token count.

use crate::animation::{Pulse, Spinner, TokenCounter, interpolate_color};
use crate::style::{CYAN_PRIMARY, ELECTRIC_BLUE, TEXT, TEXT_DIM};
use ratatui::prelude::*;
use ratatui::widgets::Widget;

/// A wave animation indicator for streaming responses with token count.
///
/// Displays a flowing wave animation with optional token count display,
/// ideal for showing active streaming from an AI model.
///
/// # Example
/// ```
/// use cortex_engine::widgets::StreamingIndicator;
///
/// let mut indicator = StreamingIndicator::new();
/// indicator.add_tokens(100);
/// // Later: indicator.tick();
/// ```
///
/// # Output Format
/// ```text
/// * Streaming... (1.2k tokens)
/// ```
pub struct StreamingIndicator {
    spinner: Spinner,
    token_counter: TokenCounter,
    pulse: Pulse,
}

impl StreamingIndicator {
    /// Creates a new streaming indicator.
    pub fn new() -> Self {
        Self {
            spinner: Spinner::streaming(),
            token_counter: TokenCounter::new(),
            pulse: Pulse::new(1500),
        }
    }

    /// Creates a streaming indicator with a max token limit.
    pub fn with_max_tokens(max: u64) -> Self {
        Self {
            spinner: Spinner::streaming(),
            token_counter: TokenCounter::new().with_max(max),
            pulse: Pulse::new(1500),
        }
    }

    /// Advances the animation.
    pub fn tick(&mut self) {
        self.spinner.tick();
        self.pulse.tick();
    }

    /// Adds tokens to the counter.
    pub fn add_tokens(&mut self, count: u64) {
        self.token_counter.add_output(count);
    }

    /// Returns the total token count.
    pub fn token_count(&self) -> u64 {
        self.token_counter.total()
    }

    /// Resets the token counter.
    pub fn reset(&mut self) {
        self.token_counter.reset();
    }
}

impl Default for StreamingIndicator {
    fn default() -> Self {
        Self::new()
    }
}

impl Widget for &StreamingIndicator {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        let spinner_char = self.spinner.frame();
        // Use pulsing color from Ocean/Cyan palette
        let pulse_color = interpolate_color(CYAN_PRIMARY, ELECTRIC_BLUE, self.pulse.intensity());
        let spinner_style = Style::default().fg(pulse_color);
        let text_style = Style::default().fg(TEXT);
        let token_style = Style::default().fg(TEXT_DIM);

        let mut x = area.x;

        // Render spinner character
        buf.set_string(x, area.y, spinner_char, spinner_style);
        x += spinner_char.chars().count() as u16;

        // Render " Streaming..."
        let streaming_text = " Streaming...";
        buf.set_string(x, area.y, streaming_text, text_style);
        x += streaming_text.len() as u16;

        // Render token count if any tokens
        if self.token_counter.total() > 0 {
            let token_str = format!(" ({})", self.token_counter.render());
            let remaining_width = area.width.saturating_sub(x - area.x) as usize;
            if token_str.len() <= remaining_width {
                buf.set_string(x, area.y, &token_str, token_style);
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
    fn test_streaming_indicator_new() {
        let indicator = StreamingIndicator::new();
        assert_eq!(indicator.token_count(), 0);
    }

    #[test]
    fn test_streaming_indicator_add_tokens() {
        let mut indicator = StreamingIndicator::new();
        indicator.add_tokens(100);
        assert_eq!(indicator.token_count(), 100);
        indicator.add_tokens(50);
        assert_eq!(indicator.token_count(), 150);
    }

    #[test]
    fn test_streaming_indicator_render() {
        let mut indicator = StreamingIndicator::new();
        indicator.add_tokens(1500);

        let mut buf = create_test_buffer(50, 1);
        let area = Rect::new(0, 0, 50, 1);
        (&indicator).render(area, &mut buf);

        // Check that content includes "Streaming"
        let content: String = (0..50)
            .map(|x| buf.get(x, 0).symbol().chars().next().unwrap_or(' '))
            .collect();
        assert!(content.contains("Streaming"));
    }
}
