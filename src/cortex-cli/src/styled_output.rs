//! Styled CLI output with theme-aware colors.
//!
//! Provides human-friendly, colorful messages for CLI operations that
//! automatically adapt to the terminal's color capabilities and respect
//! the NO_COLOR environment variable.
//!
//! # Examples
//!
//! ```
//! use cortex_cli::styled_output::{print_success, print_error, print_warning, print_info};
//!
//! print_success("Operation completed successfully");
//! print_error("Failed to connect to server");
//! print_warning("Configuration file not found, using defaults");
//! print_info("Processing 42 files...");
//! ```

use std::io::{IsTerminal, Write};

/// Check if colors should be disabled based on NO_COLOR env var.
fn colors_disabled() -> bool {
    std::env::var("NO_COLOR")
        .map(|v| !v.is_empty() && v != "0" && v.to_lowercase() != "false")
        .unwrap_or(false)
}

/// Check if the output is a terminal (TTY).
fn is_terminal_output(stderr: bool) -> bool {
    if stderr {
        std::io::stderr().is_terminal()
    } else {
        std::io::stdout().is_terminal()
    }
}

/// ANSI color codes for light theme (bright/light terminal backgrounds).
mod light_theme {
    pub const SUCCESS: &str = "\x1b[38;2;0;150;125m"; // #00967D - Teal for contrast
    pub const ERROR: &str = "\x1b[38;2;217;61;61m"; // #D93D3D - Darker red for contrast
    pub const WARNING: &str = "\x1b[38;2;201;154;46m"; // #C99A2E - Darker amber for contrast
    pub const INFO: &str = "\x1b[38;2;0;100;160m"; // Dark blue for contrast
    pub const DIM: &str = "\x1b[38;2;100;100;100m"; // Gray for muted text
    pub const BOLD: &str = "\x1b[1m";
    pub const RESET: &str = "\x1b[0m";
}

/// ANSI color codes for dark theme (dark terminal backgrounds).
mod dark_theme {
    pub const SUCCESS: &str = "\x1b[38;2;0;245;212m"; // Bright cyan-green (#00F5D4)
    pub const ERROR: &str = "\x1b[38;2;255;107;107m"; // Coral red (#FF6B6B)
    pub const WARNING: &str = "\x1b[38;2;255;200;87m"; // Golden amber (#FFC857)
    pub const INFO: &str = "\x1b[38;2;72;202;228m"; // Light blue (#48CAE4)
    pub const DIM: &str = "\x1b[38;2;130;154;177m"; // Dim text (#829AB1)
    pub const BOLD: &str = "\x1b[1m";
    pub const RESET: &str = "\x1b[0m";
}

/// Detect if the terminal has a light background.
///
/// Checks environment variables like COLORFGBG to determine theme.
/// Returns false (dark theme) by default if detection fails.
fn is_light_theme() -> bool {
    // Check COLORFGBG first (format: "fg;bg" where bg is 0 for black, 15 for white)
    if let Ok(colorfgbg) = std::env::var("COLORFGBG")
        && let Some(bg_str) = colorfgbg.split(';').next_back()
        && let Ok(bg_num) = bg_str.parse::<u8>()
    {
        return bg_num >= 7; // 7+ typically indicates light background
    }

    // Check for common light theme indicators in ITERM_PROFILE, TERM_PROFILE, etc.
    if let Ok(profile) = std::env::var("ITERM_PROFILE") {
        let profile_lower = profile.to_lowercase();
        if profile_lower.contains("light") || profile_lower.contains("solarized light") {
            return true;
        }
    }

    // Default to dark theme (most common)
    false
}

/// Get the appropriate color codes based on terminal theme.
fn get_theme_colors() -> (
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
) {
    if is_light_theme() {
        (
            light_theme::SUCCESS,
            light_theme::ERROR,
            light_theme::WARNING,
            light_theme::INFO,
            light_theme::DIM,
            light_theme::BOLD,
            light_theme::RESET,
        )
    } else {
        (
            dark_theme::SUCCESS,
            dark_theme::ERROR,
            dark_theme::WARNING,
            dark_theme::INFO,
            dark_theme::DIM,
            dark_theme::BOLD,
            dark_theme::RESET,
        )
    }
}

/// Message type for styled output.
#[derive(Debug, Clone, Copy)]
pub enum MessageType {
    /// Success message (green checkmark)
    Success,
    /// Error message (red X)
    Error,
    /// Warning message (yellow/amber warning sign)
    Warning,
    /// Info message (blue info icon)
    Info,
    /// Neutral/dimmed message
    Dim,
}

impl MessageType {
    /// Get the icon for this message type.
    fn icon(&self) -> &'static str {
        match self {
            MessageType::Success => "[OK]",
            MessageType::Error => "[ERROR]",
            MessageType::Warning => "[WARN]",
            MessageType::Info => "[INFO]",
            MessageType::Dim => "-",
        }
    }

    /// Get the color code for this message type.
    fn color(&self) -> &'static str {
        let (success, error, warning, info, dim, _, _) = get_theme_colors();
        match self {
            MessageType::Success => success,
            MessageType::Error => error,
            MessageType::Warning => warning,
            MessageType::Info => info,
            MessageType::Dim => dim,
        }
    }
}

/// Internal function to print a styled message.
fn print_styled_internal(msg_type: MessageType, message: &str, to_stderr: bool, bold: bool) {
    let use_colors = !colors_disabled() && is_terminal_output(to_stderr);
    let (_, _, _, _, _, bold_code, reset) = get_theme_colors();
    let color = msg_type.color();
    let icon = msg_type.icon();

    let formatted = if use_colors {
        if bold {
            format!("{}{} {}{}", color, icon, message, reset)
        } else {
            format!("{}{}{} {}", color, bold_code, icon, message)
                .replace(bold_code, "") // Remove bold for non-bold messages
                + reset
        }
    } else {
        format!("{} {}", icon, message)
    };

    if use_colors {
        if to_stderr {
            let _ = write!(std::io::stderr(), "{}{} {}{}", color, icon, message, reset);
            let _ = writeln!(std::io::stderr());
        } else {
            let _ = write!(std::io::stdout(), "{}{} {}{}", color, icon, message, reset);
            let _ = writeln!(std::io::stdout());
        }
    } else if to_stderr {
        eprintln!("{}", formatted);
    } else {
        println!("{}", formatted);
    }
}

// ============================================================
// PUBLIC API - Simple functions for common use cases
// ============================================================

/// Print a success message to stderr.
///
/// Displays a green checkmark followed by the message.
/// Respects NO_COLOR and terminal theme.
///
/// # Example
/// ```
/// use cortex_cli::styled_output::print_success;
/// print_success("Operation completed successfully");
/// // Output: ✓ Operation completed successfully (in green)
/// ```
pub fn print_success(message: &str) {
    print_styled_internal(MessageType::Success, message, true, false);
}

/// Print an error message to stderr.
///
/// Displays a red X followed by the message.
/// Respects NO_COLOR and terminal theme.
///
/// # Example
/// ```
/// use cortex_cli::styled_output::print_error;
/// print_error("Failed to connect to server");
/// // Output: ✗ Failed to connect to server (in red)
/// ```
pub fn print_error(message: &str) {
    print_styled_internal(MessageType::Error, message, true, false);
}

/// Print a warning message to stderr.
///
/// Displays an amber/yellow warning sign followed by the message.
/// Respects NO_COLOR and terminal theme.
///
/// # Example
/// ```
/// use cortex_cli::styled_output::print_warning;
/// print_warning("Configuration file not found, using defaults");
/// // Output: [WARN] Configuration file not found, using defaults (in amber)
/// ```
pub fn print_warning(message: &str) {
    print_styled_internal(MessageType::Warning, message, true, false);
}

/// Print an info message to stderr.
///
/// Displays a blue info icon followed by the message.
/// Respects NO_COLOR and terminal theme.
///
/// # Example
/// ```
/// use cortex_cli::styled_output::print_info;
/// print_info("Processing 42 files...");
/// // Output: [INFO] Processing 42 files... (in blue)
/// ```
pub fn print_info(message: &str) {
    print_styled_internal(MessageType::Info, message, true, false);
}

/// Print a dimmed/muted message to stderr.
///
/// Displays a gray bullet followed by the message.
/// Useful for secondary information or notes.
///
/// # Example
/// ```
/// use cortex_cli::styled_output::print_dim;
/// print_dim("Note: Using default configuration");
/// // Output: · Note: Using default configuration (in gray)
/// ```
pub fn print_dim(message: &str) {
    print_styled_internal(MessageType::Dim, message, true, false);
}

// ============================================================
// STDOUT VARIANTS - For output that should go to stdout
// ============================================================

/// Print a success message to stdout.
pub fn println_success(message: &str) {
    print_styled_internal(MessageType::Success, message, false, false);
}

/// Print an error message to stdout.
pub fn println_error(message: &str) {
    print_styled_internal(MessageType::Error, message, false, false);
}

/// Print a warning message to stdout.
pub fn println_warning(message: &str) {
    print_styled_internal(MessageType::Warning, message, false, false);
}

/// Print an info message to stdout.
pub fn println_info(message: &str) {
    print_styled_internal(MessageType::Info, message, false, false);
}

/// Print a dimmed message to stdout.
pub fn println_dim(message: &str) {
    print_styled_internal(MessageType::Dim, message, false, false);
}

// ============================================================
// FORMAT FUNCTIONS - Return formatted strings without printing
// ============================================================

/// Format a success message (returns the formatted string).
pub fn format_success(message: &str) -> String {
    format_styled(MessageType::Success, message)
}

/// Format an error message (returns the formatted string).
pub fn format_error(message: &str) -> String {
    format_styled(MessageType::Error, message)
}

/// Format a warning message (returns the formatted string).
pub fn format_warning(message: &str) -> String {
    format_styled(MessageType::Warning, message)
}

/// Format an info message (returns the formatted string).
pub fn format_info(message: &str) -> String {
    format_styled(MessageType::Info, message)
}

/// Format a dimmed message (returns the formatted string).
pub fn format_dim(message: &str) -> String {
    format_styled(MessageType::Dim, message)
}

/// Format a styled message and return the string.
fn format_styled(msg_type: MessageType, message: &str) -> String {
    let use_colors = !colors_disabled() && is_terminal_output(true);
    let (_, _, _, _, _, _, reset) = get_theme_colors();
    let color = msg_type.color();
    let icon = msg_type.icon();

    if use_colors {
        format!("{}{} {}{}", color, icon, message, reset)
    } else {
        format!("{} {}", icon, message)
    }
}

// ============================================================
// STYLED LABEL - For inline colored labels
// ============================================================

/// Return a styled label string for inline use.
///
/// # Example
/// ```
/// use cortex_cli::styled_output::{styled_label, MessageType};
/// println!("Status: {}", styled_label(MessageType::Success, "PASSED"));
/// // Output: Status: ✓ PASSED (with PASSED in green)
/// ```
pub fn styled_label(msg_type: MessageType, label: &str) -> String {
    let use_colors = !colors_disabled() && is_terminal_output(true);
    let (_, _, _, _, _, _, reset) = get_theme_colors();
    let color = msg_type.color();

    if use_colors {
        format!("{}{}{}", color, label, reset)
    } else {
        label.to_string()
    }
}

/// Return a styled icon string without the message.
pub fn styled_icon(msg_type: MessageType) -> String {
    let use_colors = !colors_disabled() && is_terminal_output(true);
    let (_, _, _, _, _, _, reset) = get_theme_colors();
    let color = msg_type.color();
    let icon = msg_type.icon();

    if use_colors {
        format!("{}{}{}", color, icon, reset)
    } else {
        icon.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    #[test]
    fn test_message_type_icons() {
        assert_eq!(MessageType::Success.icon(), "[OK]");
        assert_eq!(MessageType::Error.icon(), "[ERROR]");
        assert_eq!(MessageType::Warning.icon(), "[WARN]");
        assert_eq!(MessageType::Info.icon(), "[INFO]");
        assert_eq!(MessageType::Dim.icon(), "-");
    }

    #[test]
    #[serial]
    fn test_format_styled_no_color() {
        // When colors are disabled, should just return icon + message
        // SAFETY: These tests run serially and we restore env vars immediately
        unsafe { std::env::set_var("NO_COLOR", "1") };
        let result = format_styled(MessageType::Success, "test message");
        assert!(result.contains("[OK]"));
        assert!(result.contains("test message"));
        unsafe { std::env::remove_var("NO_COLOR") };
    }

    #[test]
    #[serial]
    fn test_colors_disabled() {
        // SAFETY: These tests run serially and we restore env vars immediately
        unsafe { std::env::set_var("NO_COLOR", "1") };
        assert!(colors_disabled());
        unsafe { std::env::remove_var("NO_COLOR") };

        unsafe { std::env::set_var("NO_COLOR", "true") };
        assert!(colors_disabled());
        unsafe { std::env::remove_var("NO_COLOR") };

        unsafe { std::env::set_var("NO_COLOR", "0") };
        assert!(!colors_disabled());
        unsafe { std::env::remove_var("NO_COLOR") };

        unsafe { std::env::set_var("NO_COLOR", "false") };
        assert!(!colors_disabled());
        unsafe { std::env::remove_var("NO_COLOR") };

        unsafe { std::env::set_var("NO_COLOR", "") };
        assert!(!colors_disabled());
        unsafe { std::env::remove_var("NO_COLOR") };
    }
}
