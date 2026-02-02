//! Color Scheme for customizable TUI components.
//!
//! Provides a flexible color configuration that can be customized
//! or use sensible defaults from cortex-core.

use cortex_core::style::{
    CYAN_PRIMARY, ERROR, INFO, SUCCESS, SURFACE_0, SURFACE_1, TEXT, TEXT_DIM, TEXT_MUTED, VOID,
    WARNING,
};
use ratatui::style::Color;

/// A flexible color scheme for TUI components.
///
/// All components in cortex-tui-components can be customized with this scheme.
/// Use `Default::default()` for the standard Cortex theme.
#[derive(Debug, Clone, Copy)]
pub struct ColorScheme {
    /// Primary accent color (selection, focus, highlights)
    pub accent: Color,
    /// Normal text color
    pub text: Color,
    /// Secondary/dimmed text color
    pub text_dim: Color,
    /// Muted/disabled text color
    pub text_muted: Color,
    /// Primary background/surface color
    pub surface: Color,
    /// Alternative background color
    pub surface_alt: Color,
    /// Inverted/void color (for text on accent backgrounds)
    pub void: Color,
    /// Success indicator color
    pub success: Color,
    /// Warning indicator color
    pub warning: Color,
    /// Error indicator color
    pub error: Color,
    /// Info indicator color
    pub info: Color,
}

impl Default for ColorScheme {
    fn default() -> Self {
        Self {
            accent: CYAN_PRIMARY,
            text: TEXT,
            text_dim: TEXT_DIM,
            text_muted: TEXT_MUTED,
            surface: SURFACE_0,
            surface_alt: SURFACE_1,
            void: VOID,
            success: SUCCESS,
            warning: WARNING,
            error: ERROR,
            info: INFO,
        }
    }
}

impl ColorScheme {
    /// Creates a new color scheme with all colors specified.
    pub fn new(
        accent: Color,
        text: Color,
        text_dim: Color,
        text_muted: Color,
        surface: Color,
        surface_alt: Color,
        void: Color,
        success: Color,
        warning: Color,
        error: Color,
        info: Color,
    ) -> Self {
        Self {
            accent,
            text,
            text_dim,
            text_muted,
            surface,
            surface_alt,
            void,
            success,
            warning,
            error,
            info,
        }
    }

    /// Creates a color scheme with a custom accent color.
    pub fn with_accent(mut self, accent: Color) -> Self {
        self.accent = accent;
        self
    }

    /// Creates a color scheme with custom text colors.
    pub fn with_text(mut self, text: Color, dim: Color, muted: Color) -> Self {
        self.text = text;
        self.text_dim = dim;
        self.text_muted = muted;
        self
    }

    /// Creates a color scheme with custom surface colors.
    pub fn with_surface(mut self, surface: Color, alt: Color) -> Self {
        self.surface = surface;
        self.surface_alt = alt;
        self
    }

    /// Creates a color scheme with custom status colors.
    pub fn with_status(
        mut self,
        success: Color,
        warning: Color,
        error: Color,
        info: Color,
    ) -> Self {
        self.success = success;
        self.warning = warning;
        self.error = error;
        self.info = info;
        self
    }

    /// Creates a light theme color scheme.
    pub fn light() -> Self {
        Self {
            accent: Color::Rgb(0, 150, 100),
            text: Color::Rgb(30, 30, 30),
            text_dim: Color::Rgb(100, 100, 100),
            text_muted: Color::Rgb(150, 150, 150),
            surface: Color::Rgb(255, 255, 255),
            surface_alt: Color::Rgb(240, 240, 240),
            void: Color::Rgb(255, 255, 255),
            success: Color::Rgb(0, 150, 0),
            warning: Color::Rgb(200, 150, 0),
            error: Color::Rgb(200, 50, 50),
            info: Color::Rgb(50, 100, 200),
        }
    }

    /// Creates a dark theme color scheme (default Cortex theme).
    pub fn dark() -> Self {
        Self::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_scheme() {
        let scheme = ColorScheme::default();
        assert_eq!(scheme.accent, CYAN_PRIMARY);
        assert_eq!(scheme.text, TEXT);
    }

    #[test]
    fn test_with_accent() {
        let scheme = ColorScheme::default().with_accent(Color::Red);
        assert_eq!(scheme.accent, Color::Red);
    }

    #[test]
    fn test_light_theme() {
        let scheme = ColorScheme::light();
        // Light theme should have light surface
        if let Color::Rgb(r, _, _) = scheme.surface {
            assert!(r > 200);
        }
    }
}
