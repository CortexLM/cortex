//! String utilities for Cortex.

use unicode_width::UnicodeWidthStr;

/// Truncate a string to a maximum display width.
pub fn truncate_to_width(s: &str, max_width: usize) -> String {
    if s.width() <= max_width {
        return s.to_string();
    }

    let mut result = String::new();
    let mut width = 0;

    for c in s.chars() {
        let char_width = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if width + char_width + 1 > max_width {
            result.push('…');
            break;
        }
        result.push(c);
        width += char_width;
    }

    result
}

/// Count the display width of a string.
pub fn display_width(s: &str) -> usize {
    s.width()
}
