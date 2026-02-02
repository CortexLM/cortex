//! Adaptive colors system
//!
//! Automatically detects terminal background and adjusts colors for optimal contrast.

use cortex_core::style::ThemeColors;
use ratatui::style::Color;

/// Check if a background color is light (for theme detection)
pub fn is_light(bg: (u8, u8, u8)) -> bool {
    // Use relative luminance formula (ITU-R BT.709)
    let (r, g, b) = bg;
    let luminance =
        0.2126 * (r as f32 / 255.0) + 0.7152 * (g as f32 / 255.0) + 0.0722 * (b as f32 / 255.0);
    luminance > 0.5
}

/// Blend two colors together with alpha
pub fn blend(fg: (u8, u8, u8), bg: (u8, u8, u8), alpha: f32) -> (u8, u8, u8) {
    let alpha = alpha.clamp(0.0, 1.0);
    let inv_alpha = 1.0 - alpha;

    let r = (fg.0 as f32 * alpha + bg.0 as f32 * inv_alpha).round() as u8;
    let g = (fg.1 as f32 * alpha + bg.1 as f32 * inv_alpha).round() as u8;
    let b = (fg.2 as f32 * alpha + bg.2 as f32 * inv_alpha).round() as u8;

    (r, g, b)
}

/// Try to detect terminal background color
///
/// This attempts to query the terminal for its background color using
/// OSC 11 escape sequence. Returns None if detection fails.
pub fn detect_terminal_bg() -> Option<(u8, u8, u8)> {
    // Try environment variables first
    if let Ok(colorfgbg) = std::env::var("COLORFGBG") {
        // Format: "fg;bg" where bg is typically 0 (dark) or 15 (light)
        if let Some(bg_str) = colorfgbg.split(';').next_back()
            && let Ok(bg_num) = bg_str.parse::<u8>()
        {
            return match bg_num {
                0 => Some((0, 0, 0)),        // Black background
                15 => Some((255, 255, 255)), // White background
                _ => None,
            };
        }
    }

    // Check for common terminal theme environment variables
    if let Ok(term_program) = std::env::var("TERM_PROGRAM") {
        // Most modern terminals default to dark theme
        if term_program.contains("iTerm")
            || term_program.contains("Alacritty")
            || term_program.contains("kitty")
            || term_program.contains("WezTerm")
        {
            // Default assumption for modern terminals
            return None; // Let caller use default dark
        }
    }

    // Check COLORTERM for true color support hint
    if std::env::var("COLORTERM").is_ok() {
        // Terminal supports true color, but we can't determine bg without
        // more invasive terminal queries
        return None;
    }

    None
}

/// Adaptive color palette that adjusts to terminal background
#[derive(Debug, Clone)]
pub struct AdaptiveColors {
    /// Brand accent color (green like Amp)
    pub accent: Color,
    /// Primary text color
    pub text: Color,
    /// Secondary/dimmed text color
    pub text_dim: Color,
    /// Very subtle/muted text color
    pub text_muted: Color,
    /// Background color for user messages
    pub user_bg: Color,
    /// Border color for UI elements
    pub border: Color,
    /// Success/confirmation color (green)
    pub success: Color,
    /// Error/danger color (red)
    pub error: Color,
    /// Warning/caution color (amber/yellow)
    pub warning: Color,
    /// Selection highlight color
    pub selection: Color,
}

impl AdaptiveColors {
    /// Create colors by auto-detecting terminal background
    pub fn from_terminal() -> Self {
        match detect_terminal_bg() {
            Some(bg) if is_light(bg) => Self::light_theme(bg),
            Some(bg) => Self::dark_theme(bg),
            None => Self::default_dark(),
        }
    }

    /// Create dark theme colors adapted to the given background
    pub fn dark_theme(bg: (u8, u8, u8)) -> Self {
        // Amp-inspired green accent
        let accent_rgb = (0x00, 0xFF, 0xA3); // #00FFA3

        // Blend colors with background for better integration
        let text_dim_rgb = blend((0x80, 0x80, 0x80), bg, 0.9);
        let text_muted_rgb = blend((0x50, 0x50, 0x50), bg, 0.9);
        let border_rgb = blend((0x40, 0x40, 0x40), bg, 0.9);
        let user_bg_rgb = blend((0x30, 0x30, 0x30), bg, 0.8);
        let selection_rgb = blend(accent_rgb, bg, 0.3);

        Self {
            accent: Color::Rgb(accent_rgb.0, accent_rgb.1, accent_rgb.2),
            text: Color::Rgb(0xE0, 0xE0, 0xE0),
            text_dim: Color::Rgb(text_dim_rgb.0, text_dim_rgb.1, text_dim_rgb.2),
            text_muted: Color::Rgb(text_muted_rgb.0, text_muted_rgb.1, text_muted_rgb.2),
            user_bg: Color::Rgb(user_bg_rgb.0, user_bg_rgb.1, user_bg_rgb.2),
            border: Color::Rgb(border_rgb.0, border_rgb.1, border_rgb.2),
            success: Color::Rgb(0x00, 0xF5, 0xD4), // #00F5D4
            error: Color::Rgb(0xFF, 0x6B, 0x6B),   // #FF6B6B
            warning: Color::Rgb(0xFF, 0xC8, 0x57), // #FFC857
            selection: Color::Rgb(selection_rgb.0, selection_rgb.1, selection_rgb.2),
        }
    }

    /// Create light theme colors adapted to the given background
    pub fn light_theme(bg: (u8, u8, u8)) -> Self {
        // Darker green for light backgrounds
        let accent_rgb = (0x00, 0xA6, 0x6E); // Darker green for contrast

        // Blend colors with background for better integration
        let text_dim_rgb = blend((0x60, 0x60, 0x60), bg, 0.9);
        let text_muted_rgb = blend((0xA0, 0xA0, 0xA0), bg, 0.9);
        let border_rgb = blend((0xC0, 0xC0, 0xC0), bg, 0.9);
        let user_bg_rgb = blend((0xF0, 0xF0, 0xF0), bg, 0.8);
        let selection_rgb = blend(accent_rgb, bg, 0.2);

        Self {
            accent: Color::Rgb(accent_rgb.0, accent_rgb.1, accent_rgb.2),
            text: Color::Rgb(0x1A, 0x1A, 0x1A),
            text_dim: Color::Rgb(text_dim_rgb.0, text_dim_rgb.1, text_dim_rgb.2),
            text_muted: Color::Rgb(text_muted_rgb.0, text_muted_rgb.1, text_muted_rgb.2),
            user_bg: Color::Rgb(user_bg_rgb.0, user_bg_rgb.1, user_bg_rgb.2),
            border: Color::Rgb(border_rgb.0, border_rgb.1, border_rgb.2),
            success: Color::Rgb(0x00, 0x96, 0x7D), // Darker teal for light bg
            error: Color::Rgb(0xD9, 0x3D, 0x3D),   // Darker red for light bg
            warning: Color::Rgb(0xC9, 0x9A, 0x2E), // Darker amber for light bg
            selection: Color::Rgb(selection_rgb.0, selection_rgb.1, selection_rgb.2),
        }
    }

    /// Create default dark theme colors when detection fails
    pub fn default_dark() -> Self {
        // Assume a typical dark terminal background (#1a1a1a or similar)
        let default_bg = (0x1A, 0x1A, 0x1A);

        Self {
            accent: Color::Rgb(0x00, 0xFF, 0xA3),     // #00FFA3 - Amp green
            text: Color::Rgb(0xE0, 0xE0, 0xE0),       // #E0E0E0
            text_dim: Color::Rgb(0x80, 0x80, 0x80),   // #808080
            text_muted: Color::Rgb(0x50, 0x50, 0x50), // #505050
            user_bg: Color::Rgb(0x2A, 0x2A, 0x2A),
            border: Color::Rgb(0x40, 0x40, 0x40),  // #404040
            success: Color::Rgb(0x00, 0xF5, 0xD4), // #00F5D4
            error: Color::Rgb(0xFF, 0x6B, 0x6B),   // #FF6B6B
            warning: Color::Rgb(0xFF, 0xC8, 0x57), // #FFC857
            selection: Color::Rgb(
                blend((0x00, 0xFF, 0xA3), default_bg, 0.3).0,
                blend((0x00, 0xFF, 0xA3), default_bg, 0.3).1,
                blend((0x00, 0xFF, 0xA3), default_bg, 0.3).2,
            ),
        }
    }
}

impl AdaptiveColors {
    /// Create colors from a named theme (dark, light, ocean_dark, monokai)
    pub fn from_theme_name(name: &str) -> Self {
        let theme = ThemeColors::from_name(name);
        Self::from_theme_colors(&theme)
    }

    /// Create AdaptiveColors from a ThemeColors instance
    pub fn from_theme_colors(theme: &ThemeColors) -> Self {
        // Extract background RGB for blending calculations
        let bg = match theme.background {
            Color::Rgb(r, g, b) => (r, g, b),
            _ => (26, 26, 26), // fallback
        };

        let selection_rgb = blend(
            match theme.primary {
                Color::Rgb(r, g, b) => (r, g, b),
                _ => (0, 255, 163),
            },
            bg,
            0.3,
        );

        Self {
            accent: theme.primary,
            text: theme.text,
            text_dim: theme.text_dim,
            text_muted: theme.text_muted,
            user_bg: theme.surface[0],
            border: theme.border,
            success: theme.success,
            error: theme.error,
            warning: theme.warning,
            selection: Color::Rgb(selection_rgb.0, selection_rgb.1, selection_rgb.2),
        }
    }

    /// Get available theme names
    pub fn available_themes() -> &'static [&'static str] {
        ThemeColors::available_themes()
    }
}

impl Default for AdaptiveColors {
    fn default() -> Self {
        Self::from_terminal()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_light() {
        assert!(is_light((255, 255, 255))); // White
        assert!(is_light((200, 200, 200))); // Light gray
        assert!(!is_light((0, 0, 0))); // Black
        assert!(!is_light((30, 30, 30))); // Dark gray
    }

    #[test]
    fn test_blend() {
        // Full alpha = foreground
        assert_eq!(blend((255, 0, 0), (0, 0, 255), 1.0), (255, 0, 0));
        // Zero alpha = background
        assert_eq!(blend((255, 0, 0), (0, 0, 255), 0.0), (0, 0, 255));
        // Half blend
        let result = blend((255, 0, 0), (0, 0, 255), 0.5);
        assert_eq!(result, (128, 0, 128)); // Purple-ish
    }

    #[test]
    fn test_default_dark_colors() {
        let colors = AdaptiveColors::default_dark();
        // Verify accent is Amp green
        assert!(matches!(colors.accent, Color::Rgb(0x00, 0xFF, 0xA3)));
    }

    #[test]
    fn test_dark_theme() {
        let colors = AdaptiveColors::dark_theme((0x1A, 0x1A, 0x1A));
        assert!(matches!(colors.accent, Color::Rgb(0x00, 0xFF, 0xA3)));
    }

    #[test]
    fn test_light_theme() {
        let colors = AdaptiveColors::light_theme((255, 255, 255));
        // Light theme should have darker accent for contrast
        assert!(matches!(colors.accent, Color::Rgb(0x00, 0xA6, 0x6E)));
    }

    #[test]
    fn test_from_theme_name() {
        let dark_colors = AdaptiveColors::from_theme_name("dark");
        // Dark theme should have the primary accent from ThemeColors::dark()
        assert!(matches!(dark_colors.accent, Color::Rgb(0, 255, 163)));

        let light_colors = AdaptiveColors::from_theme_name("light");
        // Light theme should have different accent
        assert!(matches!(light_colors.accent, Color::Rgb(0, 150, 100)));

        let monokai_colors = AdaptiveColors::from_theme_name("monokai");
        // Monokai has green accent
        assert!(matches!(monokai_colors.accent, Color::Rgb(166, 226, 46)));
    }

    #[test]
    fn test_from_theme_colors() {
        use cortex_core::style::ThemeColors;

        let theme = ThemeColors::ocean_dark();
        let colors = AdaptiveColors::from_theme_colors(&theme);

        // Should use theme's primary as accent
        assert_eq!(colors.accent, theme.primary);
        assert_eq!(colors.text, theme.text);
        assert_eq!(colors.text_dim, theme.text_dim);
        assert_eq!(colors.text_muted, theme.text_muted);
        assert_eq!(colors.border, theme.border);
        assert_eq!(colors.success, theme.success);
        assert_eq!(colors.error, theme.error);
        assert_eq!(colors.warning, theme.warning);
    }

    #[test]
    fn test_available_themes() {
        let themes = AdaptiveColors::available_themes();
        assert!(themes.contains(&"dark"));
        assert!(themes.contains(&"light"));
        assert!(themes.contains(&"ocean_dark"));
        assert!(themes.contains(&"monokai"));
    }
}
