//! Color configuration for form modals.

use ratatui::prelude::Color;

/// Colors used by the form modal.
#[derive(Debug, Clone, Copy)]
pub struct FormModalColors {
    pub background: Color,
    pub border: Color,
    pub border_focused: Color,
    pub text: Color,
    pub text_dim: Color,
    pub text_muted: Color,
    pub accent: Color,
    pub surface: Color,
}

impl Default for FormModalColors {
    fn default() -> Self {
        Self {
            background: Color::Rgb(10, 10, 15),
            border: Color::Rgb(60, 60, 70),
            border_focused: Color::Rgb(0, 200, 200),
            text: Color::Rgb(220, 220, 230),
            text_dim: Color::Rgb(150, 150, 160),
            text_muted: Color::Rgb(100, 100, 110),
            accent: Color::Rgb(0, 200, 200),
            surface: Color::Rgb(25, 25, 35),
        }
    }
}
