//! Utility functions for table rendering.
//!
//! Contains text alignment, truncation, and width calculation helpers.

use unicode_width::UnicodeWidthStr;

use super::types::{Alignment, MIN_COLUMN_WIDTH};

/// Aligns text within a given width.
///
/// # Arguments
/// * `text` - The text to align
/// * `width` - The target width
/// * `alignment` - The alignment type
pub fn align_text(text: &str, width: usize, alignment: Alignment) -> String {
    let text_width = UnicodeWidthStr::width(text);

    if text_width >= width {
        return text.to_string();
    }

    let padding = width - text_width;

    match alignment {
        Alignment::Left => {
            format!("{}{}", text, " ".repeat(padding))
        }
        Alignment::Right => {
            format!("{}{}", " ".repeat(padding), text)
        }
        Alignment::Center => {
            let left_pad = padding / 2;
            let right_pad = padding - left_pad;
            format!("{}{}{}", " ".repeat(left_pad), text, " ".repeat(right_pad))
        }
    }
}

/// Truncates text with ellipsis if it exceeds max_width.
///
/// # Arguments
/// * `text` - The text to potentially truncate
/// * `max_width` - Maximum allowed display width
///
/// # Returns
/// The original text if it fits, or truncated text with "..." appended.
pub fn truncate_with_ellipsis(text: &str, max_width: usize) -> String {
    let text_width = UnicodeWidthStr::width(text);

    if text_width <= max_width {
        return text.to_string();
    }

    if max_width <= 3 {
        // Not enough space for ellipsis, just return dots
        return ".".repeat(max_width);
    }

    // We need to fit text + "..." into max_width
    let target_width = max_width - 3; // Reserve 3 chars for "..."

    let mut result = String::new();
    let mut current_width = 0;

    for ch in text.chars() {
        let ch_width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if current_width + ch_width > target_width {
            break;
        }
        result.push(ch);
        current_width += ch_width;
    }

    result.push_str("...");
    result
}

/// Calculates the width of the longest word in a string.
///
/// Words are split by whitespace. This is used for minimum column width calculation.
pub fn longest_word_width(text: &str) -> usize {
    text.split_whitespace()
        .map(|word| UnicodeWidthStr::width(word))
        .max()
        .unwrap_or(MIN_COLUMN_WIDTH)
        .max(MIN_COLUMN_WIDTH)
}
