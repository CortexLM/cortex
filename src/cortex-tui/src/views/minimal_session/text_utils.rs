//! Text utility functions for minimal session view.

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Wraps text to fit within a maximum width (measured in visual columns).
///
/// This function performs word wrapping, breaking lines at word boundaries
/// when possible. Very long words that exceed the width are broken mid-word.
/// Uses unicode-width for proper handling of CJK characters and emoji.
pub fn wrap_text(text: &str, max_width: usize) -> Vec<String> {
    if max_width == 0 {
        return vec![text.to_string()];
    }

    let mut result = Vec::new();

    for line in text.lines() {
        if line.is_empty() {
            result.push(String::new());
            continue;
        }

        // Check if line fits as-is (fast path)
        let line_width = UnicodeWidthStr::width(line);
        if line_width <= max_width {
            result.push(line.to_string());
            continue;
        }

        // Word wrap the line
        let mut current_line = String::new();
        let mut current_width = 0;

        for word in line.split_whitespace() {
            let word_width = UnicodeWidthStr::width(word);

            if current_line.is_empty() {
                // First word on line
                if word_width <= max_width {
                    current_line = word.to_string();
                    current_width = word_width;
                } else {
                    // Word is too long - break it by visual width
                    split_long_word_into_lines(word, max_width, &mut result);
                }
            } else if current_width + 1 + word_width <= max_width {
                // Word fits on current line (1 for space)
                current_line.push(' ');
                current_line.push_str(word);
                current_width += 1 + word_width;
            } else {
                // Word doesn't fit - start new line
                result.push(current_line);

                if word_width <= max_width {
                    current_line = word.to_string();
                    current_width = word_width;
                } else {
                    // Word is too long - break it by visual width
                    current_line = String::new();
                    current_width = 0;
                    let last_chunk = split_long_word_returning_last(word, max_width, &mut result);
                    if let Some((chunk, width)) = last_chunk {
                        current_line = chunk;
                        current_width = width;
                    }
                }
            }
        }

        if !current_line.is_empty() {
            result.push(current_line);
        }
    }

    // Ensure we have at least one line (for empty input)
    if result.is_empty() {
        result.push(String::new());
    }

    result
}

/// Splits a long word into lines by visual width, pushing all lines to result.
fn split_long_word_into_lines(word: &str, max_width: usize, result: &mut Vec<String>) {
    let mut current_chunk = String::new();
    let mut current_width = 0;

    for ch in word.chars() {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);

        if current_width + ch_width > max_width && !current_chunk.is_empty() {
            result.push(current_chunk);
            current_chunk = String::new();
            current_width = 0;
        }

        current_chunk.push(ch);
        current_width += ch_width;
    }

    if !current_chunk.is_empty() {
        result.push(current_chunk);
    }
}

/// Splits a long word into lines by visual width, returning the last partial chunk.
/// Returns Some((last_chunk, width)) if there's a partial chunk, None otherwise.
fn split_long_word_returning_last(
    word: &str,
    max_width: usize,
    result: &mut Vec<String>,
) -> Option<(String, usize)> {
    let mut current_chunk = String::new();
    let mut current_width = 0;

    for ch in word.chars() {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);

        if current_width + ch_width > max_width && !current_chunk.is_empty() {
            result.push(current_chunk);
            current_chunk = String::new();
            current_width = 0;
        }

        current_chunk.push(ch);
        current_width += ch_width;
    }

    if !current_chunk.is_empty() {
        // Return the last chunk instead of pushing it
        if current_width == max_width {
            // If it's exactly max_width, push it and return None
            result.push(current_chunk);
            None
        } else {
            Some((current_chunk, current_width))
        }
    } else {
        None
    }
}
