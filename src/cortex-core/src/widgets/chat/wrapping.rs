//! Text wrapping utilities.
//!
//! Provides functions for wrapping text to fit within specified widths.
//! Uses unicode-width for proper handling of CJK characters and emoji.

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Wraps text to fit within the specified width (measured in visual columns).
pub fn wrap_text(text: &str, max_width: usize) -> Vec<String> {
    if max_width == 0 {
        return vec![text.to_string()];
    }

    let mut lines = Vec::new();
    for line in text.lines() {
        if line.is_empty() {
            lines.push(String::new());
            continue;
        }

        let mut current_line = String::new();
        let mut current_width = 0;

        for word in line.split_whitespace() {
            let word_width = UnicodeWidthStr::width(word);

            if current_width == 0 {
                // First word on the line
                if word_width > max_width {
                    // Word is longer than max width, split it by visual width
                    let mut remaining = word;
                    while !remaining.is_empty() {
                        let (chunk, rest) = split_at_visual_width(remaining, max_width);
                        lines.push(chunk.to_string());
                        remaining = rest;
                    }
                } else {
                    current_line = word.to_string();
                    current_width = word_width;
                }
            } else if current_width + 1 + word_width <= max_width {
                // Word fits on current line (1 for space)
                current_line.push(' ');
                current_line.push_str(word);
                current_width += 1 + word_width;
            } else {
                // Word doesn't fit, start new line
                lines.push(std::mem::take(&mut current_line));
                if word_width > max_width {
                    // Word is longer than max width, split it by visual width
                    let mut remaining = word;
                    while !remaining.is_empty() {
                        let (chunk, rest) = split_at_visual_width(remaining, max_width);
                        if rest.is_empty() {
                            current_line = chunk.to_string();
                            current_width = UnicodeWidthStr::width(chunk);
                        } else {
                            lines.push(chunk.to_string());
                        }
                        remaining = rest;
                    }
                } else {
                    current_line = word.to_string();
                    current_width = word_width;
                }
            }
        }

        if !current_line.is_empty() {
            lines.push(current_line);
        }
    }

    if lines.is_empty() {
        lines.push(String::new());
    }

    lines
}

/// Splits a string at a visual width boundary, returning (prefix, suffix).
///
/// Uses unicode-width to properly handle multi-width characters like CJK
/// characters (2 columns) and emoji.
pub fn split_at_visual_width(s: &str, max_width: usize) -> (&str, &str) {
    if max_width == 0 {
        return ("", s);
    }

    let mut visual_width = 0;
    let mut byte_idx = s.len();

    for (idx, ch) in s.char_indices() {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if visual_width + ch_width > max_width {
            byte_idx = idx;
            break;
        }
        visual_width += ch_width;
    }

    s.split_at(byte_idx)
}

/// Splits a string at a character boundary, returning (prefix, suffix).
///
/// Note: This function splits by character count, not visual width.
/// For visual width-aware splitting, use `split_at_visual_width` instead.
#[allow(dead_code)]
pub fn split_at_char_boundary(s: &str, max_chars: usize) -> (&str, &str) {
    if max_chars == 0 {
        return ("", s);
    }

    let mut char_count = 0;
    let mut byte_idx = 0;

    for (idx, _) in s.char_indices() {
        if char_count >= max_chars {
            byte_idx = idx;
            break;
        }
        char_count += 1;
        byte_idx = s.len();
    }

    s.split_at(byte_idx)
}

// ============================================================
// TESTS
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wrap_text_simple() {
        let text = "Hello world";
        let wrapped = wrap_text(text, 20);
        assert_eq!(wrapped, vec!["Hello world"]);
    }

    #[test]
    fn test_wrap_text_multiple_lines() {
        let text = "Hello world this is a test";
        let wrapped = wrap_text(text, 12);
        assert_eq!(wrapped.len(), 3);
        assert_eq!(wrapped[0], "Hello world");
        assert_eq!(wrapped[1], "this is a");
        assert_eq!(wrapped[2], "test");
    }

    #[test]
    fn test_wrap_text_long_word() {
        let text = "supercalifragilisticexpialidocious";
        let wrapped = wrap_text(text, 10);
        assert!(wrapped.len() > 1);
        for line in &wrapped {
            // Visual width should be <= max_width
            assert!(UnicodeWidthStr::width(line.as_str()) <= 10);
        }
    }

    #[test]
    fn test_wrap_text_empty() {
        let text = "";
        let wrapped = wrap_text(text, 10);
        assert_eq!(wrapped, vec![""]);
    }

    #[test]
    fn test_wrap_text_newlines() {
        let text = "Line 1\nLine 2\nLine 3";
        let wrapped = wrap_text(text, 20);
        assert_eq!(wrapped.len(), 3);
        assert_eq!(wrapped[0], "Line 1");
        assert_eq!(wrapped[1], "Line 2");
        assert_eq!(wrapped[2], "Line 3");
    }

    #[test]
    fn test_split_at_visual_width() {
        // ASCII - 1 column per char
        let (a, b) = split_at_visual_width("hello", 3);
        assert_eq!(a, "hel");
        assert_eq!(b, "lo");

        let (a, b) = split_at_visual_width("hello", 10);
        assert_eq!(a, "hello");
        assert_eq!(b, "");

        // CJK characters - 2 columns per char
        // "日本語" = 6 visual columns (3 chars * 2 columns each)
        let (a, b) = split_at_visual_width("日本語", 4);
        assert_eq!(a, "日本"); // 4 visual columns
        assert_eq!(b, "語");

        let (a, b) = split_at_visual_width("日本語", 2);
        assert_eq!(a, "日"); // 2 visual columns
        assert_eq!(b, "本語");
    }

    #[test]
    fn test_split_at_char_boundary() {
        let (a, b) = split_at_char_boundary("hello", 3);
        assert_eq!(a, "hel");
        assert_eq!(b, "lo");

        let (a, b) = split_at_char_boundary("hello", 10);
        assert_eq!(a, "hello");
        assert_eq!(b, "");

        // Character-based split: 2 chars regardless of visual width
        let (a, b) = split_at_char_boundary("日本語", 2);
        assert_eq!(a, "日本");
        assert_eq!(b, "語");
    }

    #[test]
    fn test_wrap_cjk_text() {
        // "日本語テスト" = 12 visual columns
        let text = "日本語テスト";
        let wrapped = wrap_text(text, 6);
        assert_eq!(wrapped.len(), 2);
        assert_eq!(wrapped[0], "日本語"); // 6 visual columns
        assert_eq!(wrapped[1], "テスト"); // 6 visual columns
    }
}
