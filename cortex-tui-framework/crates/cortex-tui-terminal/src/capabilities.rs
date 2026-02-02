//! Terminal capability detection.
//!
//! Detects supported features based on environment variables, TERM settings,
//! and terminal responses to capability queries.

use std::env;

/// Color support mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ColorMode {
    /// No color support.
    None,
    /// Basic 16 colors.
    Basic,
    /// 256 color palette.
    Extended,
    /// 24-bit true color (16.7M colors).
    #[default]
    TrueColor,
}

/// Unicode width calculation mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum UnicodeMode {
    /// Full Unicode width support (terminal mode 2027).
    #[default]
    Unicode,
    /// Legacy wcwidth() compatibility (for tmux/screen).
    WcWidth,
}

/// Terminal capabilities and feature detection.
///
/// Detects what features the terminal supports by checking environment
/// variables, TERM settings, and optionally querying the terminal directly.
#[derive(Clone, Debug)]
pub struct Capabilities {
    /// Color support level.
    pub color_mode: ColorMode,
    /// Unicode width calculation mode.
    pub unicode_mode: UnicodeMode,
    /// Kitty keyboard protocol support.
    pub kitty_keyboard: bool,
    /// SGR pixel mouse support.
    pub sgr_pixels: bool,
    /// Synchronized output (DEC 2026) support.
    pub synchronized_output: bool,
    /// Bracketed paste mode support.
    pub bracketed_paste: bool,
    /// Focus tracking support.
    pub focus_tracking: bool,
    /// Mouse support available.
    pub mouse: bool,
    /// Alternate screen buffer support.
    pub alternate_screen: bool,
    /// Terminal name/identifier.
    pub term_name: String,
    /// Terminal program (from TERM_PROGRAM).
    pub term_program: Option<String>,
}

impl Default for Capabilities {
    fn default() -> Self {
        Self {
            color_mode: ColorMode::TrueColor,
            unicode_mode: UnicodeMode::Unicode,
            kitty_keyboard: false,
            sgr_pixels: false,
            synchronized_output: true,
            bracketed_paste: true,
            focus_tracking: true,
            mouse: true,
            alternate_screen: true,
            term_name: String::new(),
            term_program: None,
        }
    }
}

impl Capabilities {
    /// Detects terminal capabilities from environment.
    ///
    /// This checks environment variables like TERM, COLORTERM, TERM_PROGRAM,
    /// and TMUX to determine terminal features.
    pub fn detect() -> Self {
        let mut caps = Self::default();

        // Get TERM
        caps.term_name = env::var("TERM").unwrap_or_else(|_| String::from("xterm"));
        caps.term_program = env::var("TERM_PROGRAM").ok();

        // Check color support
        caps.color_mode = Self::detect_color_mode();

        // Check for tmux/screen (affects unicode handling)
        caps.unicode_mode = Self::detect_unicode_mode(&caps.term_name);

        // Check for specific terminal programs
        if let Some(program) = caps.term_program.clone() {
            caps.apply_program_overrides(&program);
        }

        // Check for Kitty
        if env::var("KITTY_WINDOW_ID").is_ok() {
            caps.kitty_keyboard = true;
            caps.sgr_pixels = true;
        }

        // Check for WezTerm
        if env::var("WEZTERM_PANE").is_ok() {
            caps.kitty_keyboard = true;
        }

        // Check for iTerm2
        if env::var("ITERM_SESSION_ID").is_ok() {
            caps.synchronized_output = true;
        }

        // Check for Alacritty
        if env::var("ALACRITTY_SOCKET").is_ok() || env::var("ALACRITTY_LOG").is_ok() {
            caps.synchronized_output = true;
        }

        // Windows Terminal
        if env::var("WT_SESSION").is_ok() {
            caps.color_mode = ColorMode::TrueColor;
            caps.synchronized_output = true;
        }

        caps
    }

    /// Detects color mode from environment variables.
    fn detect_color_mode() -> ColorMode {
        // Check COLORTERM first (most reliable for truecolor)
        if let Ok(colorterm) = env::var("COLORTERM") {
            match colorterm.as_str() {
                "truecolor" | "24bit" => return ColorMode::TrueColor,
                _ => {}
            }
        }

        // Check TERM for color hints
        if let Ok(term) = env::var("TERM") {
            let term_lower = term.to_lowercase();

            // True color terminals
            if term_lower.contains("truecolor")
                || term_lower.contains("24bit")
                || term_lower.contains("direct")
            {
                return ColorMode::TrueColor;
            }

            // 256 color terminals
            if term_lower.contains("256color") || term_lower.contains("256") {
                return ColorMode::Extended;
            }

            // Basic color terminals
            if term_lower.contains("color")
                || term_lower.starts_with("xterm")
                || term_lower.starts_with("screen")
                || term_lower.starts_with("tmux")
                || term_lower.starts_with("rxvt")
                || term_lower.starts_with("linux")
            {
                return ColorMode::Basic;
            }

            // Dumb terminal
            if term_lower == "dumb" {
                return ColorMode::None;
            }
        }

        // Default to truecolor for modern terminals
        ColorMode::TrueColor
    }

    /// Detects unicode mode based on terminal type.
    fn detect_unicode_mode(term: &str) -> UnicodeMode {
        let term_lower = term.to_lowercase();

        // tmux and screen need wcwidth compatibility
        if term_lower.starts_with("tmux") || term_lower.starts_with("screen") {
            return UnicodeMode::WcWidth;
        }

        // Check for TMUX environment variable
        if env::var("TMUX").is_ok() {
            return UnicodeMode::WcWidth;
        }

        UnicodeMode::Unicode
    }

    /// Applies overrides for specific terminal programs.
    fn apply_program_overrides(&mut self, program: &str) {
        match program.to_lowercase().as_str() {
            "vscode" | "code" => {
                // VSCode terminal has limited capabilities
                self.kitty_keyboard = false;
                self.synchronized_output = false;
            }
            "apple_terminal" => {
                // Apple Terminal is limited
                self.kitty_keyboard = false;
                self.color_mode = ColorMode::Extended;
            }
            "iterm.app" => {
                self.synchronized_output = true;
                self.color_mode = ColorMode::TrueColor;
            }
            "mintty" => {
                self.color_mode = ColorMode::TrueColor;
                self.synchronized_output = true;
            }
            "hyper" => {
                self.color_mode = ColorMode::TrueColor;
            }
            _ => {}
        }
    }

    /// Returns whether true color is supported.
    #[inline]
    pub fn has_true_color(&self) -> bool {
        self.color_mode == ColorMode::TrueColor
    }

    /// Returns whether mouse support is available.
    #[inline]
    pub fn has_mouse(&self) -> bool {
        self.mouse
    }

    /// Returns whether synchronized output is supported.
    #[inline]
    pub fn has_sync_output(&self) -> bool {
        self.synchronized_output
    }

    /// Returns whether Kitty keyboard protocol is supported.
    #[inline]
    pub fn has_kitty_keyboard(&self) -> bool {
        self.kitty_keyboard
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_capabilities() {
        let caps = Capabilities::default();
        assert_eq!(caps.color_mode, ColorMode::TrueColor);
        assert!(caps.mouse);
        assert!(caps.alternate_screen);
    }

    #[test]
    fn test_color_mode_ordering() {
        // Just ensure variants exist and are different
        assert_ne!(ColorMode::None, ColorMode::Basic);
        assert_ne!(ColorMode::Basic, ColorMode::Extended);
        assert_ne!(ColorMode::Extended, ColorMode::TrueColor);
    }
}
