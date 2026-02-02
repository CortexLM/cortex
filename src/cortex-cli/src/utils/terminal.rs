//! Terminal utilities for the Cortex CLI.
//!
//! Provides color detection, terminal capability checks, ANSI formatting,
//! and safe output functions that handle broken pipes gracefully.
//!
//! This module centralizes all terminal-related functionality to avoid
//! duplication across command modules.

use std::io::{self, IsTerminal, Write};

/// Terminal colors for CLI output.
///
/// Provides cross-platform terminal color codes with automatic detection
/// of terminal capabilities and respect for the NO_COLOR standard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TermColor {
    /// Red color (errors, dangerous operations)
    Red,
    /// Green color (success, safe operations)
    Green,
    /// Yellow color (warnings, caution)
    Yellow,
    /// Blue color (info, file operations)
    Blue,
    /// Magenta color (highlights, special items)
    Magenta,
    /// Cyan color (accents, secondary info)
    Cyan,
    /// White color (neutral, default text)
    White,
    /// Reset to default terminal color
    Default,
}

impl TermColor {
    /// Get the ANSI escape code for this color.
    #[inline]
    pub fn ansi_code(&self) -> &'static str {
        match self {
            TermColor::Red => "\x1b[1;31m",
            TermColor::Green => "\x1b[1;32m",
            TermColor::Yellow => "\x1b[1;33m",
            TermColor::Blue => "\x1b[1;34m",
            TermColor::Magenta => "\x1b[1;35m",
            TermColor::Cyan => "\x1b[1;36m",
            TermColor::White => "\x1b[1;37m",
            TermColor::Default => "\x1b[0m",
        }
    }

    /// Get ANSI code only if colors should be shown (respects NO_COLOR and TTY).
    #[inline]
    pub fn code_if_tty(&self) -> &'static str {
        if should_use_colors() {
            self.ansi_code()
        } else {
            ""
        }
    }

    /// Get ANSI code only if colors should be shown on stderr.
    #[inline]
    pub fn code_if_tty_stderr(&self) -> &'static str {
        if should_use_colors_stderr() {
            self.ansi_code()
        } else {
            ""
        }
    }

    /// Format a string with this color, respecting terminal settings.
    #[inline]
    pub fn format(&self, text: &str) -> String {
        if should_use_colors() {
            format!(
                "{}{}{}",
                self.ansi_code(),
                text,
                TermColor::Default.ansi_code()
            )
        } else {
            text.to_string()
        }
    }

    /// Format a string with this color for stderr output.
    #[inline]
    pub fn format_stderr(&self, text: &str) -> String {
        if should_use_colors_stderr() {
            format!(
                "{}{}{}",
                self.ansi_code(),
                text,
                TermColor::Default.ansi_code()
            )
        } else {
            text.to_string()
        }
    }
}

/// Check if colors should be disabled based on NO_COLOR env var.
///
/// Follows the NO_COLOR standard: https://no-color.org/
#[inline]
pub fn colors_disabled() -> bool {
    std::env::var("NO_COLOR")
        .map(|v| !v.is_empty() && v != "0" && v.to_lowercase() != "false")
        .unwrap_or(false)
}

/// Check if the output is a terminal (TTY).
///
/// # Arguments
/// * `use_stderr` - If true, check stderr; otherwise check stdout
#[inline]
pub fn is_terminal_output(use_stderr: bool) -> bool {
    if use_stderr {
        std::io::stderr().is_terminal()
    } else {
        std::io::stdout().is_terminal()
    }
}

/// Check if colors should be used in output.
///
/// This checks both:
/// 1. NO_COLOR environment variable (https://no-color.org/)
/// 2. Whether stdout is a terminal
#[inline]
pub fn should_use_colors() -> bool {
    !colors_disabled() && is_terminal_output(false)
}

/// Check if colors should be used for stderr output.
#[inline]
pub fn should_use_colors_stderr() -> bool {
    !colors_disabled() && is_terminal_output(true)
}

/// Detect if the terminal has a light background.
///
/// Checks environment variables like COLORFGBG to determine theme.
/// Returns false (dark theme) by default if detection fails.
pub fn is_light_theme() -> bool {
    // Check COLORFGBG first (format: "fg;bg" where bg is 0 for black, 15 for white)
    if let Ok(colorfgbg) = std::env::var("COLORFGBG")
        && let Some(bg_str) = colorfgbg.split(';').next_back()
        && let Ok(bg_num) = bg_str.parse::<u8>()
    {
        return bg_num >= 7; // 7+ typically indicates light background
    }

    // Check for common light theme indicators in ITERM_PROFILE
    if let Ok(profile) = std::env::var("ITERM_PROFILE") {
        let profile_lower = profile.to_lowercase();
        if profile_lower.contains("light") || profile_lower.contains("solarized light") {
            return true;
        }
    }

    // Default to dark theme (most common)
    false
}

/// ANSI escape codes to reset terminal state.
pub mod reset {
    /// Show cursor
    pub const SHOW_CURSOR: &str = "\x1b[?25h";
    /// Disable mouse tracking modes
    pub const DISABLE_MOUSE: &str = "\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l";
    /// Disable bracketed paste mode
    pub const DISABLE_BRACKETED_PASTE: &str = "\x1b[?2004l";
    /// Reset all colors and styles
    pub const RESET_STYLES: &str = "\x1b[0m";
}

/// Restore terminal state to default.
///
/// This should be called on cleanup (Ctrl+C, panic, etc.) to ensure
/// the terminal is left in a usable state.
pub fn restore_terminal() {
    use std::io::Write;

    // Show cursor
    eprint!("{}", reset::SHOW_CURSOR);
    // Disable mouse tracking
    eprint!("{}", reset::DISABLE_MOUSE);
    // Disable bracketed paste
    eprint!("{}", reset::DISABLE_BRACKETED_PASTE);
    // Reset styles
    eprint!("{}", reset::RESET_STYLES);
    // Move to new line
    eprintln!();
    // Flush stderr
    let _ = std::io::stderr().flush();
}

/// Format a duration in human-readable form.
///
/// # Arguments
/// * `secs` - Duration in seconds
///
/// # Returns
/// A human-readable string like "2m 30s" or "1h 5m".
pub fn format_duration(secs: u64) -> String {
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        let mins = secs / 60;
        let remaining = secs % 60;
        if remaining > 0 {
            format!("{}m {}s", mins, remaining)
        } else {
            format!("{}m", mins)
        }
    } else {
        let hours = secs / 3600;
        let mins = (secs % 3600) / 60;
        if mins > 0 {
            format!("{}h {}m", hours, mins)
        } else {
            format!("{}h", hours)
        }
    }
}

/// Format a byte size in human-readable form.
///
/// # Arguments
/// * `bytes` - Size in bytes
///
/// # Returns
/// A human-readable string like "1.5 MB" or "256 KB".
pub fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} bytes", bytes)
    }
}

// ============================================================================
// Safe Output Functions (Handle broken pipe gracefully - Issue #1989)
// ============================================================================

/// Safely prints to stdout, ignoring broken pipe errors.
///
/// This prevents crashes when output is piped to commands like `head`
/// that close their input stream early.
///
/// Returns `Ok(true)` if printed successfully, `Ok(false)` if pipe was closed.
#[inline]
pub fn safe_print(s: &str) -> io::Result<bool> {
    match write!(io::stdout(), "{}", s) {
        Ok(()) => match io::stdout().flush() {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Ok(false),
            Err(e) => Err(e),
        },
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Ok(false),
        Err(e) => Err(e),
    }
}

/// Safely prints a line to stdout, ignoring broken pipe errors.
///
/// This prevents crashes when output is piped to commands like `head`
/// that close their input stream early.
///
/// Returns `Ok(true)` if printed successfully, `Ok(false)` if pipe was closed.
#[inline]
pub fn safe_println(s: &str) -> io::Result<bool> {
    match writeln!(io::stdout(), "{}", s) {
        Ok(()) => match io::stdout().flush() {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Ok(false),
            Err(e) => Err(e),
        },
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Ok(false),
        Err(e) => Err(e),
    }
}

/// Safely prints an empty line to stdout, ignoring broken pipe errors.
#[inline]
pub fn safe_println_empty() -> io::Result<bool> {
    match writeln!(io::stdout()) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Ok(false),
        Err(e) => Err(e),
    }
}

/// Safely prints to stderr, ignoring broken pipe errors.
#[inline]
pub fn safe_eprint(s: &str) -> io::Result<bool> {
    match write!(io::stderr(), "{}", s) {
        Ok(()) => match io::stderr().flush() {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Ok(false),
            Err(e) => Err(e),
        },
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Ok(false),
        Err(e) => Err(e),
    }
}

/// Safely prints a line to stderr, ignoring broken pipe errors.
#[inline]
pub fn safe_eprintln(s: &str) -> io::Result<bool> {
    match writeln!(io::stderr(), "{}", s) {
        Ok(()) => match io::stderr().flush() {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Ok(false),
            Err(e) => Err(e),
        },
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Ok(false),
        Err(e) => Err(e),
    }
}

// ============================================================================
// Tool Display Helpers (Centralized from run_cmd and exec_cmd)
// ============================================================================

/// Tool display information for formatted output.
#[derive(Debug, Clone, Copy)]
pub struct ToolDisplay {
    /// Display name for the tool
    pub name: &'static str,
    /// Color to use for this tool type
    pub color: TermColor,
}

/// Get standardized display information for a tool by name.
///
/// Returns consistent color coding and display names for tool calls
/// across all CLI commands.
pub fn get_tool_display(tool_name: &str) -> ToolDisplay {
    match tool_name.to_lowercase().as_str() {
        "todowrite" | "todoread" => ToolDisplay {
            name: "Todo",
            color: TermColor::Yellow,
        },
        "bash" | "execute" => ToolDisplay {
            name: "Bash",
            color: TermColor::Red,
        },
        "edit" | "multiedit" => ToolDisplay {
            name: "Edit",
            color: TermColor::Green,
        },
        "glob" => ToolDisplay {
            name: "Glob",
            color: TermColor::Blue,
        },
        "grep" => ToolDisplay {
            name: "Grep",
            color: TermColor::Blue,
        },
        "list" | "ls" => ToolDisplay {
            name: "List",
            color: TermColor::Blue,
        },
        "read" => ToolDisplay {
            name: "Read",
            color: TermColor::Cyan,
        },
        "write" | "create" => ToolDisplay {
            name: "Write",
            color: TermColor::Green,
        },
        "websearch" | "fetchurl" => ToolDisplay {
            name: "Search",
            color: TermColor::Magenta,
        },
        _ => ToolDisplay {
            name: "Tool",
            color: TermColor::White,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_term_color_codes() {
        assert_eq!(TermColor::Red.ansi_code(), "\x1b[1;31m");
        assert_eq!(TermColor::Default.ansi_code(), "\x1b[0m");
    }

    #[test]
    fn test_format_duration() {
        assert_eq!(format_duration(30), "30s");
        assert_eq!(format_duration(60), "1m");
        assert_eq!(format_duration(90), "1m 30s");
        assert_eq!(format_duration(3600), "1h");
        assert_eq!(format_duration(3660), "1h 1m");
    }

    #[test]
    fn test_format_size() {
        assert_eq!(format_size(500), "500 bytes");
        assert_eq!(format_size(1024), "1.0 KB");
        assert_eq!(format_size(1536), "1.5 KB");
        assert_eq!(format_size(1024 * 1024), "1.00 MB");
    }
}
