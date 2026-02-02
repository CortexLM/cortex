//! Loading spinner component.

use cortex_core::style::CYAN_PRIMARY;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::Widget;

/// Spinner animation styles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SpinnerStyle {
    /// Breathing pattern used in streaming (default in Cortex TUI)
    #[default]
    Breathing,
    /// Half-circle rotation for tool execution
    HalfCircle,
    /// Classic braille dots
    Dots,
    /// Simple line rotation
    Line,
    /// Braille pattern
    Braille,
    /// Block pattern
    Blocks,
}

impl SpinnerStyle {
    /// Get the frames for this spinner style.
    pub fn frames(&self) -> &[&'static str] {
        match self {
            // Breathing pattern: · → ✢ → ✻ → ✽ (ping-pong for fluid effect)
            SpinnerStyle::Breathing => &["·", "✢", "✻", "✽", "✻", "✢"],
            // Half-circle rotation for tools
            SpinnerStyle::HalfCircle => &["◐", "◑", "◒", "◓"],
            SpinnerStyle::Dots => &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"],
            SpinnerStyle::Line => &["|", "/", "-", "\\"],
            SpinnerStyle::Braille => &["⣾", "⣽", "⣻", "⢿", "⡿", "⣟", "⣯", "⣷"],
            SpinnerStyle::Blocks => &["▖", "▘", "▝", "▗"],
        }
    }
}

/// A loading spinner widget.
pub struct LoadingSpinner {
    /// Current frame index
    pub frame: usize,
    /// Spinner style
    pub style: SpinnerStyle,
    /// Optional label text
    pub label: Option<String>,
}

impl LoadingSpinner {
    /// Create a new spinner.
    pub fn new() -> Self {
        Self {
            frame: 0,
            style: SpinnerStyle::default(),
            label: None,
        }
    }

    /// Set the spinner style.
    pub fn with_style(mut self, style: SpinnerStyle) -> Self {
        self.style = style;
        self
    }

    /// Set a label.
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Advance to the next frame.
    pub fn tick(&mut self) {
        let frames = self.style.frames();
        self.frame = (self.frame + 1) % frames.len();
    }

    /// Get the current frame character.
    pub fn current_frame(&self) -> &'static str {
        let frames = self.style.frames();
        frames[self.frame % frames.len()]
    }
}

impl Default for LoadingSpinner {
    fn default() -> Self {
        Self::new()
    }
}

impl Widget for &LoadingSpinner {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width < 2 {
            return;
        }

        let spinner_style = Style::default().fg(CYAN_PRIMARY);
        buf.set_string(area.x, area.y, self.current_frame(), spinner_style);

        if let Some(label) = &self.label
            && area.width > 3
        {
            buf.set_string(area.x + 2, area.y, label, Style::default());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spinner_tick() {
        let mut spinner = LoadingSpinner::new();
        let first_frame = spinner.current_frame();

        spinner.tick();
        let second_frame = spinner.current_frame();

        assert_ne!(first_frame, second_frame);
    }

    #[test]
    fn test_spinner_styles() {
        assert!(!SpinnerStyle::Dots.frames().is_empty());
        assert!(!SpinnerStyle::Line.frames().is_empty());
        assert!(!SpinnerStyle::Braille.frames().is_empty());
        assert!(!SpinnerStyle::Blocks.frames().is_empty());
    }
}
