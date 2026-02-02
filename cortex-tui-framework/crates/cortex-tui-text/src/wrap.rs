//! Text wrapping utilities for terminal display.
//!
//! This module provides functions for wrapping text to fit within
//! specified widths, with support for different wrapping strategies.

use crate::grapheme::grapheme_display_width;
use unicode_segmentation::UnicodeSegmentation;

/// Text wrapping mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WrapMode {
    /// No wrapping - lines extend beyond the width.
    None,
    /// Wrap at character/grapheme boundaries.
    /// Best for code, logs, or when preserving exact character positions matters.
    Char,
    /// Wrap at word boundaries when possible, falling back to character wrap.
    /// Best for prose and natural language text.
    #[default]
    Word,
}

/// Options for text wrapping.
#[derive(Debug, Clone)]
pub struct WrapOptions {
    /// Maximum width in columns.
    pub width: usize,
    /// Wrapping mode.
    pub mode: WrapMode,
    /// Tab width for expansion.
    pub tab_width: usize,
    /// Whether to preserve leading whitespace on wrapped lines.
    pub preserve_leading_whitespace: bool,
}

impl Default for WrapOptions {
    fn default() -> Self {
        Self {
            width: 80,
            mode: WrapMode::Word,
            tab_width: 4,
            preserve_leading_whitespace: false,
        }
    }
}

impl WrapOptions {
    /// Create wrap options with specified width.
    pub fn new(width: usize) -> Self {
        Self {
            width,
            ..Default::default()
        }
    }

    /// Set the wrap mode.
    pub fn with_mode(mut self, mode: WrapMode) -> Self {
        self.mode = mode;
        self
    }

    /// Set the tab width.
    pub fn with_tab_width(mut self, tab_width: usize) -> Self {
        self.tab_width = tab_width;
        self
    }

    /// Set whether to preserve leading whitespace.
    pub fn with_preserve_leading_whitespace(mut self, preserve: bool) -> Self {
        self.preserve_leading_whitespace = preserve;
        self
    }
}

/// Wrap text to fit within the specified width.
///
/// # Arguments
///
/// * `text` - The text to wrap
/// * `width` - Maximum width in columns
/// * `mode` - Wrapping mode
///
/// # Returns
///
/// A vector of lines, each fitting within the specified width.
///
/// # Example
///
/// ```
/// use cortex_tui_text::wrap::{wrap_text, WrapMode};
///
/// let lines = wrap_text("Hello World", 6, WrapMode::Word);
/// assert_eq!(lines, vec!["Hello", "World"]);
///
/// let lines = wrap_text("Hello World", 5, WrapMode::Char);
/// assert_eq!(lines, vec!["Hello", " Worl", "d"]);
/// ```
pub fn wrap_text(text: &str, width: usize, mode: WrapMode) -> Vec<String> {
    wrap_text_with_options(
        text,
        WrapOptions {
            width,
            mode,
            ..Default::default()
        },
    )
}

/// Wrap text with full options.
///
/// # Example
///
/// ```
/// use cortex_tui_text::wrap::{wrap_text_with_options, WrapOptions, WrapMode};
///
/// let opts = WrapOptions::new(20).with_mode(WrapMode::Word);
/// let lines = wrap_text_with_options("Hello World", opts);
/// ```
pub fn wrap_text_with_options(text: &str, options: WrapOptions) -> Vec<String> {
    if options.width == 0 || options.mode == WrapMode::None {
        // No wrapping - just split by newlines
        return text.lines().map(String::from).collect();
    }

    let mut result = Vec::new();

    // Process each line separately (preserving explicit line breaks)
    for line in text.split('\n') {
        if line.is_empty() {
            result.push(String::new());
            continue;
        }

        let wrapped = match options.mode {
            WrapMode::None => vec![line.to_string()],
            WrapMode::Char => wrap_line_char(line, options.width, options.tab_width),
            WrapMode::Word => wrap_line_word(line, options.width, options.tab_width),
        };

        result.extend(wrapped);
    }

    // Handle trailing newline
    if text.ends_with('\n') && !result.last().is_none_or(|s| s.is_empty()) {
        result.push(String::new());
    }

    if result.is_empty() {
        result.push(String::new());
    }

    result
}

/// Wrap a single line at character boundaries.
fn wrap_line_char(line: &str, width: usize, tab_width: usize) -> Vec<String> {
    let mut result = Vec::new();
    let mut current_line = String::new();
    let mut current_width = 0;

    for grapheme in line.graphemes(true) {
        let g_width = if grapheme == "\t" {
            tab_width
        } else {
            grapheme_display_width(grapheme)
        };

        // Handle the case where a single grapheme is wider than the width
        if g_width > width {
            // Push current line if not empty
            if !current_line.is_empty() {
                result.push(current_line);
                current_line = String::new();
            }
            // Add the wide grapheme on its own line
            result.push(grapheme.to_string());
            current_width = 0;
            continue;
        }

        if current_width + g_width > width {
            // Start a new line
            result.push(current_line);
            current_line = String::new();
            current_width = 0;
        }

        // Expand tabs to spaces
        if grapheme == "\t" {
            for _ in 0..tab_width {
                current_line.push(' ');
            }
        } else {
            current_line.push_str(grapheme);
        }
        current_width += g_width;
    }

    if !current_line.is_empty() {
        result.push(current_line);
    }

    if result.is_empty() {
        result.push(String::new());
    }

    result
}

/// Wrap a single line at word boundaries.
fn wrap_line_word(line: &str, width: usize, tab_width: usize) -> Vec<String> {
    let mut result = Vec::new();
    let mut current_line = String::new();
    let mut current_width = 0;

    // Split into word chunks (preserving whitespace)
    let words = split_into_words(line);

    for word in words {
        let word_width = measure_word_width(&word, tab_width);

        // If word alone is wider than width, need to break it
        if word_width > width {
            // Flush current line if not empty
            if !current_line.is_empty() {
                result.push(current_line.trim_end().to_string());
                current_line = String::new();
                current_width = 0;
            }

            // Break the long word
            let broken = break_long_word(&word, width, tab_width);
            let broken_len = broken.len();
            for (i, part) in broken.into_iter().enumerate() {
                if i < broken_len - 1 {
                    result.push(part);
                } else {
                    // Last part becomes the start of current line
                    current_width = measure_word_width(&part, tab_width);
                    current_line = part;
                }
            }
            continue;
        }

        // Check if word fits on current line
        if current_width + word_width > width {
            // Start a new line
            if !current_line.is_empty() {
                result.push(current_line.trim_end().to_string());
            }
            current_line = String::new();
            current_width = 0;

            // Skip leading whitespace for new line
            let trimmed = word.trim_start();
            if !trimmed.is_empty() {
                current_line.push_str(trimmed);
                current_width = measure_word_width(trimmed, tab_width);
            }
        } else {
            current_line.push_str(&word);
            current_width += word_width;
        }
    }

    if !current_line.is_empty() {
        result.push(current_line.trim_end().to_string());
    }

    if result.is_empty() {
        result.push(String::new());
    }

    result
}

/// Split text into word chunks.
/// Each word includes its trailing whitespace.
fn split_into_words(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut was_whitespace = true; // Start as true so leading ws forms its own chunk

    for grapheme in text.graphemes(true) {
        let is_ws = is_word_break(grapheme);

        if is_ws {
            // Whitespace: add to current word
            current.push_str(grapheme);
            was_whitespace = true;
        } else {
            // Non-whitespace
            if was_whitespace && !current.is_empty() {
                // We're starting a new word after whitespace
                words.push(current);
                current = String::new();
            }
            current.push_str(grapheme);
            was_whitespace = false;
        }
    }

    if !current.is_empty() {
        words.push(current);
    }

    words
}

/// Measure the display width of a word.
fn measure_word_width(word: &str, tab_width: usize) -> usize {
    word.graphemes(true)
        .map(|g| {
            if g == "\t" {
                tab_width
            } else {
                grapheme_display_width(g)
            }
        })
        .sum()
}

/// Break a long word that doesn't fit in width.
fn break_long_word(word: &str, width: usize, tab_width: usize) -> Vec<String> {
    let mut result = Vec::new();
    let mut current = String::new();
    let mut current_width = 0;

    for grapheme in word.graphemes(true) {
        let g_width = if grapheme == "\t" {
            tab_width
        } else {
            grapheme_display_width(grapheme)
        };

        if current_width + g_width > width && !current.is_empty() {
            result.push(current);
            current = String::new();
            current_width = 0;
        }

        if grapheme == "\t" {
            for _ in 0..tab_width {
                current.push(' ');
            }
        } else {
            current.push_str(grapheme);
        }
        current_width += g_width;
    }

    if !current.is_empty() {
        result.push(current);
    }

    result
}

/// Check if a grapheme is a word break point.
///
/// This follows Unicode word boundary rules for common cases.
fn is_word_break(grapheme: &str) -> bool {
    // Get the first (and usually only) character
    let c = match grapheme.chars().next() {
        Some(c) => c,
        None => return false,
    };

    // Standard ASCII whitespace
    if c.is_whitespace() {
        // But not NBSP and similar non-breaking spaces
        return !matches!(
            c,
            '\u{00A0}' // NBSP
            | '\u{202F}' // Narrow NBSP  
            | '\u{2007}' // Figure Space (non-breaking)
            | '\u{2060}' // Word Joiner
        );
    }

    false
}

/// Check if a character is a potential word break point (for CJK text).
///
/// CJK characters can break anywhere, so we treat them as word boundaries.
#[allow(dead_code)]
fn is_cjk_char(c: char) -> bool {
    matches!(c as u32,
        // CJK Unified Ideographs
        0x4E00..=0x9FFF |
        // CJK Extension A
        0x3400..=0x4DBF |
        // CJK Extension B-F (surrogate pairs in UTF-16)
        0x20000..=0x2A6DF |
        0x2A700..=0x2B73F |
        0x2B740..=0x2B81F |
        0x2B820..=0x2CEAF |
        0x2CEB0..=0x2EBEF |
        // CJK Compatibility Ideographs
        0xF900..=0xFAFF |
        // Hiragana
        0x3040..=0x309F |
        // Katakana
        0x30A0..=0x30FF |
        // Hangul Syllables
        0xAC00..=0xD7AF
    )
}

/// Iterator that yields wrapped lines.
pub struct WrapIterator<'a> {
    remaining: &'a str,
    options: WrapOptions,
}

impl<'a> WrapIterator<'a> {
    /// Create a new wrap iterator.
    pub fn new(text: &'a str, options: WrapOptions) -> Self {
        Self {
            remaining: text,
            options,
        }
    }
}

impl<'a> Iterator for WrapIterator<'a> {
    type Item = String;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining.is_empty() {
            return None;
        }

        // Find the next line break
        let line_end = self.remaining.find('\n').unwrap_or(self.remaining.len());
        let line = &self.remaining[..line_end];

        // Update remaining
        self.remaining = if line_end < self.remaining.len() {
            &self.remaining[line_end + 1..]
        } else {
            ""
        };

        if self.options.mode == WrapMode::None || self.options.width == 0 {
            return Some(line.to_string());
        }

        // For wrapped lines, we need to handle this differently
        // This simple implementation returns the first wrapped segment
        let wrapped = match self.options.mode {
            WrapMode::Char => wrap_line_char(line, self.options.width, self.options.tab_width),
            WrapMode::Word => wrap_line_word(line, self.options.width, self.options.tab_width),
            WrapMode::None => vec![line.to_string()],
        };

        wrapped.into_iter().next()
    }
}

/// Create an iterator that yields wrapped lines.
///
/// # Example
///
/// ```
/// use cortex_tui_text::wrap::{wrap_iter, WrapOptions};
///
/// let options = WrapOptions::new(10);
/// for line in wrap_iter("Hello World, this is a test", options) {
///     println!("{}", line);
/// }
/// ```
pub fn wrap_iter(text: &str, options: WrapOptions) -> impl Iterator<Item = String> + '_ {
    wrap_text_with_options(text, options).into_iter()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wrap_mode_default() {
        assert_eq!(WrapMode::default(), WrapMode::Word);
    }

    #[test]
    fn test_wrap_none() {
        let lines = wrap_text("Hello World", 5, WrapMode::None);
        assert_eq!(lines, vec!["Hello World"]);
    }

    #[test]
    fn test_wrap_char() {
        let lines = wrap_text("Hello World", 5, WrapMode::Char);
        assert_eq!(lines, vec!["Hello", " Worl", "d"]);
    }

    #[test]
    fn test_wrap_word() {
        let lines = wrap_text("Hello World", 6, WrapMode::Word);
        assert_eq!(lines, vec!["Hello", "World"]);
    }

    #[test]
    fn test_wrap_word_long() {
        let lines = wrap_text("Hello Beautiful World", 10, WrapMode::Word);
        assert_eq!(lines, vec!["Hello", "Beautiful", "World"]);
    }

    #[test]
    fn test_wrap_preserves_newlines() {
        let lines = wrap_text("Hello\nWorld", 20, WrapMode::Word);
        assert_eq!(lines, vec!["Hello", "World"]);
    }

    #[test]
    fn test_wrap_empty() {
        let lines = wrap_text("", 10, WrapMode::Word);
        assert_eq!(lines, vec![""]);
    }

    #[test]
    fn test_wrap_unicode() {
        let lines = wrap_text("日本語テスト", 6, WrapMode::Char);
        // Each CJK char is width 2, so 3 chars per line
        assert_eq!(lines, vec!["日本語", "テスト"]);
    }

    #[test]
    fn test_wrap_long_word() {
        let lines = wrap_text("Supercalifragilisticexpialidocious", 10, WrapMode::Word);
        assert_eq!(lines.len(), 4);
        for line in &lines {
            let width: usize = line.graphemes(true).map(grapheme_display_width).sum();
            assert!(width <= 10);
        }
    }

    #[test]
    fn test_wrap_mixed_content() {
        let lines = wrap_text("Hello 世界", 8, WrapMode::Word);
        // "Hello " is 6 width, "世界" is 4 width
        // Together they're 10, which exceeds 8
        assert_eq!(lines, vec!["Hello", "世界"]);
    }

    #[test]
    fn test_wrap_options() {
        let opts = WrapOptions::new(10)
            .with_mode(WrapMode::Char)
            .with_tab_width(8);
        assert_eq!(opts.width, 10);
        assert_eq!(opts.mode, WrapMode::Char);
        assert_eq!(opts.tab_width, 8);
    }

    #[test]
    fn test_split_into_words() {
        let words = split_into_words("Hello World");
        assert_eq!(words, vec!["Hello ", "World"]);

        let words = split_into_words("  Hello  World  ");
        // Leading whitespace forms its own chunk, then "Hello  ", then "World  "
        assert_eq!(words, vec!["  ", "Hello  ", "World  "]);
    }

    #[test]
    fn test_is_word_break() {
        assert!(is_word_break(" "));
        assert!(is_word_break("\t"));
        assert!(!is_word_break("a"));
        assert!(!is_word_break("\u{00A0}")); // NBSP should not break
    }

    #[test]
    fn test_wrap_with_tabs() {
        let opts = WrapOptions::new(10)
            .with_tab_width(4)
            .with_mode(WrapMode::Char);
        let lines = wrap_text_with_options("a\tb", opts);
        // "a" + 4 spaces + "b" = 6 width, fits in 10
        assert_eq!(lines.len(), 1);
        // In char mode, tab is expanded to spaces
        assert!(lines[0].contains("    ") || lines[0].contains('\t'));
    }
}
