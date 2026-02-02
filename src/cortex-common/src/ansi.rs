//! ANSI escape code handling utilities.
//!
//! Provides functions to detect terminal capabilities and strip ANSI codes
//! when output is piped or redirected (#2811).

use std::io::IsTerminal;

/// Check if stdout should output colors/ANSI codes.
///
/// Returns false when:
/// - stdout is not a terminal (piped/redirected)
/// - NO_COLOR environment variable is set
///
/// This implements the NO_COLOR standard: https://no-color.org/
pub fn should_colorize() -> bool {
    // Check NO_COLOR environment variable first (https://no-color.org/)
    if std::env::var("NO_COLOR").is_ok() {
        return false;
    }

    // Check if stdout is a terminal
    std::io::stdout().is_terminal()
}

/// Check if stderr should output colors/ANSI codes.
pub fn should_colorize_stderr() -> bool {
    // Check NO_COLOR environment variable first
    if std::env::var("NO_COLOR").is_ok() {
        return false;
    }

    // Check if stderr is a terminal
    std::io::stderr().is_terminal()
}

/// Strip ANSI escape codes from a string.
///
/// This removes all ANSI escape sequences including:
/// - Color codes (\x1b[...m)
/// - Cursor movement (\x1b[...H, \x1b[...A, etc.)
/// - Screen clearing (\x1b[2J, etc.)
/// - Other CSI sequences
pub fn strip_ansi_codes(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // Skip the escape sequence
            if let Some(&next) = chars.peek() {
                if next == '[' {
                    chars.next(); // consume '['
                    // Skip until we hit a letter (the terminator)
                    while let Some(&c) = chars.peek() {
                        chars.next();
                        if c.is_ascii_alphabetic() {
                            break;
                        }
                    }
                    continue;
                } else if next == ']' {
                    // OSC sequence (e.g., terminal title)
                    chars.next(); // consume ']'
                    // Skip until BEL (\x07) or ST (\x1b\\)
                    while let Some(c) = chars.next() {
                        if c == '\x07' {
                            break;
                        }
                        if c == '\x1b' && chars.peek() == Some(&'\\') {
                            chars.next();
                            break;
                        }
                    }
                    continue;
                }
            }
        }
        result.push(c);
    }

    result
}

/// Conditionally wrap a string with ANSI color codes.
///
/// Returns the colored string if colors are enabled, otherwise returns the plain string.
pub fn maybe_color(text: &str, color_code: &str, reset: &str) -> String {
    if should_colorize() {
        format!("{}{}{}", color_code, text, reset)
    } else {
        text.to_string()
    }
}

/// Conditionally wrap a string with ANSI color codes for stderr.
pub fn maybe_color_stderr(text: &str, color_code: &str, reset: &str) -> String {
    if should_colorize_stderr() {
        format!("{}{}{}", color_code, text, reset)
    } else {
        text.to_string()
    }
}

/// ANSI color codes for common colors.
pub mod colors {
    pub const RESET: &str = "\x1b[0m";
    pub const BOLD: &str = "\x1b[1m";
    pub const DIM: &str = "\x1b[2m";

    pub const RED: &str = "\x1b[31m";
    pub const GREEN: &str = "\x1b[32m";
    pub const YELLOW: &str = "\x1b[33m";
    pub const BLUE: &str = "\x1b[34m";
    pub const MAGENTA: &str = "\x1b[35m";
    pub const CYAN: &str = "\x1b[36m";
    pub const WHITE: &str = "\x1b[37m";

    pub const BOLD_RED: &str = "\x1b[1;31m";
    pub const BOLD_GREEN: &str = "\x1b[1;32m";
    pub const BOLD_YELLOW: &str = "\x1b[1;33m";
    pub const BOLD_BLUE: &str = "\x1b[1;34m";
    pub const BOLD_MAGENTA: &str = "\x1b[1;35m";
    pub const BOLD_CYAN: &str = "\x1b[1;36m";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_ansi_codes_basic() {
        let colored = "\x1b[31mRed\x1b[0m Normal";
        assert_eq!(strip_ansi_codes(colored), "Red Normal");
    }

    #[test]
    fn test_strip_ansi_codes_complex() {
        let colored = "\x1b[1;32mBold Green\x1b[0m \x1b[33mYellow\x1b[0m";
        assert_eq!(strip_ansi_codes(colored), "Bold Green Yellow");
    }

    #[test]
    fn test_strip_ansi_codes_cursor() {
        let with_cursor = "Hello\x1b[2JWorld\x1b[H";
        assert_eq!(strip_ansi_codes(with_cursor), "HelloWorld");
    }

    #[test]
    fn test_strip_ansi_codes_no_codes() {
        let plain = "Hello World";
        assert_eq!(strip_ansi_codes(plain), "Hello World");
    }

    #[test]
    fn test_strip_ansi_codes_empty() {
        assert_eq!(strip_ansi_codes(""), "");
    }

    #[test]
    fn test_strip_ansi_codes_only_codes() {
        let only_codes = "\x1b[31m\x1b[0m\x1b[32m\x1b[0m";
        assert_eq!(strip_ansi_codes(only_codes), "");
    }
}
