//! Text measurement utilities for calculating display dimensions.
//!
//! This module provides functions for measuring text width, height,
//! and truncating text to fit within specified bounds.

use crate::grapheme::{grapheme_display_width, grapheme_display_width_with_tab, graphemes};
use crate::wrap::{wrap_text, WrapMode};
use unicode_segmentation::UnicodeSegmentation;

/// Result of finding a wrap position in text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WrapResult {
    /// Byte offset in the original string where wrapping should occur.
    pub byte_offset: usize,
    /// Number of grapheme clusters before the wrap point.
    pub grapheme_count: usize,
    /// Number of display columns used by the text before wrap.
    pub columns_used: usize,
}

/// Measure the display width of a string in terminal columns.
///
/// This function properly handles:
/// - ASCII characters (width 1)
/// - Wide characters like CJK (width 2)
/// - Zero-width characters
/// - Emoji and emoji sequences
///
/// # Example
///
/// ```
/// use cortex_tui_text::measurement::measure_width;
///
/// assert_eq!(measure_width("Hello"), 5);
/// assert_eq!(measure_width("日本語"), 6); // 3 chars × 2 width
/// assert_eq!(measure_width("Hi世界"), 6); // 2 + 2×2
/// ```
pub fn measure_width(text: &str) -> usize {
    // Fast path for ASCII-only text
    if text.is_ascii() {
        return text.chars().filter(|&c| c != '\n' && c != '\r').count();
    }

    graphemes(text).map(|info| info.width).sum()
}

/// Measure text width with custom tab width.
///
/// # Arguments
///
/// * `text` - The text to measure
/// * `tab_width` - The display width for tab characters
///
/// # Example
///
/// ```
/// use cortex_tui_text::measurement::measure_width_with_tab;
///
/// assert_eq!(measure_width_with_tab("a\tb", 4), 6); // 1 + 4 + 1
/// assert_eq!(measure_width_with_tab("a\tb", 8), 10); // 1 + 8 + 1
/// ```
pub fn measure_width_with_tab(text: &str, tab_width: usize) -> usize {
    text.graphemes(true)
        .map(|g| grapheme_display_width_with_tab(g, tab_width))
        .sum()
}

/// Measure the height of text when rendered with the given maximum width and wrap mode.
///
/// Returns the number of lines the text would occupy.
///
/// # Arguments
///
/// * `text` - The text to measure
/// * `max_width` - Maximum width in columns (0 means no wrapping limit)
/// * `wrap_mode` - How to wrap text
///
/// # Example
///
/// ```
/// use cortex_tui_text::measurement::measure_height;
/// use cortex_tui_text::wrap::WrapMode;
///
/// // Single line that fits
/// assert_eq!(measure_height("Hello", 10, WrapMode::Word), 1);
///
/// // Line that wraps
/// assert_eq!(measure_height("Hello World", 6, WrapMode::Word), 2);
///
/// // Multiple lines
/// assert_eq!(measure_height("Line 1\nLine 2", 20, WrapMode::Word), 2);
/// ```
pub fn measure_height(text: &str, max_width: usize, wrap_mode: WrapMode) -> usize {
    if text.is_empty() {
        return 1; // Empty text still takes one line
    }

    if max_width == 0 || wrap_mode == WrapMode::None {
        // Just count newlines
        return text.lines().count().max(1);
    }

    wrap_text(text, max_width, wrap_mode).len().max(1)
}

/// Find the position where text should wrap to fit within max_columns.
///
/// Returns information about how many graphemes fit and at what byte offset
/// the wrap should occur.
///
/// # Arguments
///
/// * `text` - The text to analyze (should be a single line, no newlines)
/// * `max_columns` - Maximum display width in columns
///
/// # Example
///
/// ```
/// use cortex_tui_text::measurement::find_wrap_position;
///
/// let result = find_wrap_position("Hello World", 5);
/// assert_eq!(result.columns_used, 5);
/// assert_eq!(result.grapheme_count, 5);
/// ```
pub fn find_wrap_position(text: &str, max_columns: usize) -> WrapResult {
    find_wrap_position_with_tab(text, max_columns, 1)
}

/// Find wrap position with custom tab width.
///
/// # Arguments
///
/// * `text` - The text to analyze
/// * `max_columns` - Maximum display width
/// * `tab_width` - Display width for tab characters
pub fn find_wrap_position_with_tab(text: &str, max_columns: usize, tab_width: usize) -> WrapResult {
    if max_columns == 0 {
        return WrapResult {
            byte_offset: 0,
            grapheme_count: 0,
            columns_used: 0,
        };
    }

    let mut columns = 0;
    let mut grapheme_count = 0;
    let mut byte_offset = 0;

    for (idx, grapheme) in text.grapheme_indices(true) {
        let width = grapheme_display_width_with_tab(grapheme, tab_width);

        if columns + width > max_columns {
            break;
        }

        columns += width;
        grapheme_count += 1;
        byte_offset = idx + grapheme.len();
    }

    WrapResult {
        byte_offset,
        grapheme_count,
        columns_used: columns,
    }
}

/// Truncation style for text that exceeds maximum width.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TruncationStyle {
    /// Add ellipsis at the end: "Hello Wo…"
    #[default]
    End,
    /// Add ellipsis at the start: "…lo World"
    Start,
    /// Add ellipsis in the middle: "Hel…rld"
    Middle,
}

/// Options for text truncation.
#[derive(Debug, Clone)]
pub struct TruncationOptions {
    /// Maximum width in columns.
    pub max_width: usize,
    /// Where to place the ellipsis.
    pub style: TruncationStyle,
    /// The ellipsis string to use (default: "…").
    pub ellipsis: String,
}

impl Default for TruncationOptions {
    fn default() -> Self {
        Self {
            max_width: 0,
            style: TruncationStyle::End,
            ellipsis: "…".to_string(),
        }
    }
}

impl TruncationOptions {
    /// Create new truncation options with the given max width.
    pub fn new(max_width: usize) -> Self {
        Self {
            max_width,
            ..Default::default()
        }
    }

    /// Set the truncation style.
    pub fn with_style(mut self, style: TruncationStyle) -> Self {
        self.style = style;
        self
    }

    /// Set a custom ellipsis string.
    pub fn with_ellipsis(mut self, ellipsis: impl Into<String>) -> Self {
        self.ellipsis = ellipsis.into();
        self
    }
}

/// Truncate text to fit within the specified width, adding an ellipsis.
///
/// # Arguments
///
/// * `text` - The text to truncate
/// * `max_width` - Maximum display width in columns
///
/// # Example
///
/// ```
/// use cortex_tui_text::measurement::truncate_to_width;
///
/// assert_eq!(truncate_to_width("Hello World", 8), "Hello W…");
/// assert_eq!(truncate_to_width("Short", 10), "Short");
/// ```
pub fn truncate_to_width(text: &str, max_width: usize) -> String {
    truncate_with_options(text, TruncationOptions::new(max_width))
}

/// Truncate text with custom options.
///
/// # Example
///
/// ```
/// use cortex_tui_text::measurement::{truncate_with_options, TruncationOptions, TruncationStyle};
///
/// let opts = TruncationOptions::new(10).with_style(TruncationStyle::Middle);
/// assert_eq!(truncate_with_options("Hello World!", opts), "Hell…orld!");
/// ```
pub fn truncate_with_options(text: &str, options: TruncationOptions) -> String {
    let text_width = measure_width(text);

    if text_width <= options.max_width {
        return text.to_string();
    }

    let ellipsis_width = measure_width(&options.ellipsis);

    // Need at least space for the ellipsis
    if options.max_width < ellipsis_width {
        // Return as much of ellipsis as fits, or empty
        return truncate_end_simple(
            &options.ellipsis,
            options.max_width.saturating_sub(ellipsis_width),
        );
    }

    let available = options.max_width.saturating_sub(ellipsis_width);

    match options.style {
        TruncationStyle::End => truncate_end(text, available, &options.ellipsis),
        TruncationStyle::Start => truncate_start(text, available, &options.ellipsis),
        TruncationStyle::Middle => truncate_middle(text, available, &options.ellipsis),
    }
}

/// Simple truncation without ellipsis.
fn truncate_end_simple(text: &str, max_width: usize) -> String {
    let mut result = String::new();
    let mut width = 0;

    for grapheme in text.graphemes(true) {
        let g_width = grapheme_display_width(grapheme);
        if width + g_width > max_width {
            break;
        }
        result.push_str(grapheme);
        width += g_width;
    }

    result
}

/// Truncate at the end, adding ellipsis.
fn truncate_end(text: &str, available: usize, ellipsis: &str) -> String {
    let mut result = String::new();
    let mut width = 0;

    for grapheme in text.graphemes(true) {
        let g_width = grapheme_display_width(grapheme);
        if width + g_width > available {
            break;
        }
        result.push_str(grapheme);
        width += g_width;
    }

    result.push_str(ellipsis);
    result
}

/// Truncate at the start, adding ellipsis.
fn truncate_start(text: &str, available: usize, ellipsis: &str) -> String {
    let graphemes: Vec<&str> = text.graphemes(true).collect();
    let mut result = String::new();
    let mut width = 0;

    // Iterate from end
    for grapheme in graphemes.into_iter().rev() {
        let g_width = grapheme_display_width(grapheme);
        if width + g_width > available {
            break;
        }
        result = format!("{}{}", grapheme, result);
        width += g_width;
    }

    format!("{}{}", ellipsis, result)
}

/// Truncate in the middle, adding ellipsis.
fn truncate_middle(text: &str, available: usize, ellipsis: &str) -> String {
    if available == 0 {
        return ellipsis.to_string();
    }

    let first_half = available / 2;
    let second_half = available - first_half;

    let graphemes: Vec<&str> = text.graphemes(true).collect();

    // Take from start
    let mut start = String::new();
    let mut start_width = 0;
    for g in &graphemes {
        let w = grapheme_display_width(g);
        if start_width + w > first_half {
            break;
        }
        start.push_str(g);
        start_width += w;
    }

    // Take from end
    let mut end_parts: Vec<&str> = Vec::new();
    let mut end_width = 0;
    for g in graphemes.iter().rev() {
        let w = grapheme_display_width(g);
        if end_width + w > second_half {
            break;
        }
        end_parts.push(g);
        end_width += w;
    }

    // Reverse end_parts to get correct order
    end_parts.reverse();
    let end: String = end_parts.into_iter().collect();

    format!("{}{}{}", start, ellipsis, end)
}

/// Check if text fits within the given width.
///
/// # Example
///
/// ```
/// use cortex_tui_text::measurement::fits_in_width;
///
/// assert!(fits_in_width("Hello", 10));
/// assert!(fits_in_width("Hello", 5));
/// assert!(!fits_in_width("Hello", 4));
/// ```
#[inline]
pub fn fits_in_width(text: &str, max_width: usize) -> bool {
    measure_width(text) <= max_width
}

/// Calculate the width of the widest line in multi-line text.
///
/// # Example
///
/// ```
/// use cortex_tui_text::measurement::max_line_width;
///
/// assert_eq!(max_line_width("Hello\nWorld!"), 6); // "World!" is widest
/// assert_eq!(max_line_width("Short\nVery long line"), 14);
/// ```
pub fn max_line_width(text: &str) -> usize {
    text.lines().map(measure_width).max().unwrap_or(0)
}

/// Calculate text dimensions (width, height) with wrapping.
///
/// Returns a tuple of (max_width, line_count).
///
/// # Arguments
///
/// * `text` - The text to measure
/// * `max_width` - Maximum width for wrapping (0 for no wrapping)
/// * `wrap_mode` - How to wrap text
///
/// # Example
///
/// ```
/// use cortex_tui_text::measurement::measure_dimensions;
/// use cortex_tui_text::wrap::WrapMode;
///
/// let (width, height) = measure_dimensions("Hello World", 0, WrapMode::None);
/// assert_eq!(width, 11);
/// assert_eq!(height, 1);
/// ```
pub fn measure_dimensions(text: &str, max_width: usize, wrap_mode: WrapMode) -> (usize, usize) {
    if text.is_empty() {
        return (0, 1);
    }

    if max_width == 0 || wrap_mode == WrapMode::None {
        let width = max_line_width(text);
        let height = text.lines().count().max(1);
        return (width, height);
    }

    let wrapped = wrap_text(text, max_width, wrap_mode);
    let width = wrapped
        .iter()
        .map(|line| measure_width(line))
        .max()
        .unwrap_or(0);
    let height = wrapped.len().max(1);

    (width, height)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_measure_width_ascii() {
        assert_eq!(measure_width("Hello"), 5);
        assert_eq!(measure_width(""), 0);
        assert_eq!(measure_width(" "), 1);
    }

    #[test]
    fn test_measure_width_unicode() {
        assert_eq!(measure_width("日本語"), 6);
        assert_eq!(measure_width("Hi世界"), 6);
        assert_eq!(measure_width("café"), 4);
    }

    #[test]
    fn test_measure_width_with_tab() {
        assert_eq!(measure_width_with_tab("a\tb", 4), 6);
        assert_eq!(measure_width_with_tab("a\tb", 8), 10);
        assert_eq!(measure_width_with_tab("\t\t", 4), 8);
    }

    #[test]
    fn test_measure_height() {
        assert_eq!(measure_height("Hello", 10, WrapMode::Word), 1);
        assert_eq!(measure_height("Hello\nWorld", 10, WrapMode::Word), 2);
        assert_eq!(measure_height("", 10, WrapMode::Word), 1);
    }

    #[test]
    fn test_find_wrap_position() {
        let result = find_wrap_position("Hello World", 5);
        assert_eq!(result.columns_used, 5);
        assert_eq!(result.grapheme_count, 5);
        assert_eq!(result.byte_offset, 5);
    }

    #[test]
    fn test_find_wrap_position_unicode() {
        let result = find_wrap_position("日本語", 4);
        assert_eq!(result.columns_used, 4);
        assert_eq!(result.grapheme_count, 2);
        assert_eq!(result.byte_offset, 6); // 2 CJK chars × 3 bytes each
    }

    #[test]
    fn test_truncate_to_width() {
        assert_eq!(truncate_to_width("Hello World", 8), "Hello W…");
        assert_eq!(truncate_to_width("Short", 10), "Short");
        assert_eq!(truncate_to_width("Hello", 5), "Hello");
    }

    #[test]
    fn test_truncate_start() {
        let opts = TruncationOptions::new(8).with_style(TruncationStyle::Start);
        // max_width=8, ellipsis=1, available=7 chars from end
        // "Hello World" -> "…o World" (7 chars + ellipsis)
        assert_eq!(truncate_with_options("Hello World", opts), "…o World");
    }

    #[test]
    fn test_truncate_middle() {
        let opts = TruncationOptions::new(10).with_style(TruncationStyle::Middle);
        // max_width=10, ellipsis=1, available=9
        // first_half=4, second_half=5
        // "Hello World!" -> "Hell" + "…" + "orld!" (4+1+5=10)
        assert_eq!(truncate_with_options("Hello World!", opts), "Hell…orld!");
    }

    #[test]
    fn test_fits_in_width() {
        assert!(fits_in_width("Hello", 10));
        assert!(fits_in_width("Hello", 5));
        assert!(!fits_in_width("Hello", 4));
    }

    #[test]
    fn test_max_line_width() {
        assert_eq!(max_line_width("Hello\nWorld!"), 6);
        assert_eq!(max_line_width("Short\nVery long line"), 14);
        assert_eq!(max_line_width(""), 0);
    }

    #[test]
    fn test_measure_dimensions() {
        let (w, h) = measure_dimensions("Hello World", 0, WrapMode::None);
        assert_eq!(w, 11);
        assert_eq!(h, 1);

        let (w, h) = measure_dimensions("Hi\nWorld", 0, WrapMode::None);
        assert_eq!(w, 5);
        assert_eq!(h, 2);
    }
}
