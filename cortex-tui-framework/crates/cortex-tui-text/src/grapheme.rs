//! Grapheme cluster utilities for Unicode text processing.
//!
//! This module provides functions for iterating over grapheme clusters and
//! calculating their display widths, handling wide characters (CJK),
//! zero-width characters, and emoji sequences properly.

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthChar;

/// Result of iterating over a grapheme with its display properties.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphemeInfo<'a> {
    /// The grapheme cluster string slice.
    pub grapheme: &'a str,
    /// The byte offset in the original string.
    pub byte_offset: usize,
    /// The display width of this grapheme in terminal columns.
    pub width: usize,
}

/// Iterator over graphemes with their display information.
pub struct GraphemeIterator<'a> {
    grapheme_indices: std::iter::Peekable<unicode_segmentation::GraphemeIndices<'a>>,
}

impl<'a> GraphemeIterator<'a> {
    /// Creates a new grapheme iterator over the given text.
    #[inline]
    pub fn new(text: &'a str) -> Self {
        Self {
            grapheme_indices: text.grapheme_indices(true).peekable(),
        }
    }
}

impl<'a> Iterator for GraphemeIterator<'a> {
    type Item = GraphemeInfo<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let (byte_offset, grapheme) = self.grapheme_indices.next()?;
        let width = grapheme_display_width(grapheme);

        Some(GraphemeInfo {
            grapheme,
            byte_offset,
            width,
        })
    }
}

/// Returns an iterator over grapheme clusters with their display information.
///
/// This provides both the grapheme string slice and its calculated display width,
/// useful for text layout and rendering.
///
/// # Example
///
/// ```
/// use cortex_tui_text::grapheme::graphemes;
///
/// for info in graphemes("Hello 世界") {
///     println!("{}: width {}", info.grapheme, info.width);
/// }
/// ```
#[inline]
pub fn graphemes(text: &str) -> GraphemeIterator<'_> {
    GraphemeIterator::new(text)
}

/// Returns an iterator over grapheme clusters with their widths as tuples.
///
/// This is a convenience function for simple iteration.
///
/// # Example
///
/// ```
/// use cortex_tui_text::grapheme::graphemes_with_widths;
///
/// let widths: Vec<_> = graphemes_with_widths("日本語").collect();
/// assert_eq!(widths, vec![("日", 2), ("本", 2), ("語", 2)]);
/// ```
#[inline]
pub fn graphemes_with_widths(text: &str) -> impl Iterator<Item = (&str, usize)> {
    text.graphemes(true).map(|g| (g, grapheme_display_width(g)))
}

/// Calculate the display width of a single grapheme cluster.
///
/// This function handles:
/// - Regular ASCII characters (width 1)
/// - Wide characters like CJK ideographs (width 2)
/// - Zero-width characters (combining marks, ZWJ, etc.)
/// - Emoji sequences (typically width 2)
/// - Tab characters (configurable, defaults to width 1 for single-char measurement)
///
/// # Example
///
/// ```
/// use cortex_tui_text::grapheme::grapheme_display_width;
///
/// assert_eq!(grapheme_display_width("a"), 1);
/// assert_eq!(grapheme_display_width("中"), 2);
/// assert_eq!(grapheme_display_width("👨‍👩‍👧"), 2); // Family emoji (ZWJ sequence)
/// ```
pub fn grapheme_display_width(grapheme: &str) -> usize {
    if grapheme.is_empty() {
        return 0;
    }

    // Handle tab specially - width is context-dependent, treat as 1 for measurement
    if grapheme == "\t" {
        return 1;
    }

    // Handle newlines and other control characters
    if grapheme == "\n" || grapheme == "\r" || grapheme == "\r\n" {
        return 0;
    }

    // Check if this is an emoji sequence (contains ZWJ or variation selectors)
    if is_emoji_sequence(grapheme) {
        return 2;
    }

    // For multi-codepoint graphemes, take the maximum width of component characters
    // This handles combining characters correctly (the base char determines width)
    grapheme
        .chars()
        .filter_map(|c| {
            // Filter out zero-width characters for width calculation
            if is_zero_width_char(c) {
                None
            } else {
                c.width()
            }
        })
        .max()
        .unwrap_or(0)
}

/// Calculate grapheme display width with custom tab width.
///
/// # Arguments
///
/// * `grapheme` - The grapheme cluster to measure
/// * `tab_width` - The display width to use for tab characters
///
/// # Example
///
/// ```
/// use cortex_tui_text::grapheme::grapheme_display_width_with_tab;
///
/// assert_eq!(grapheme_display_width_with_tab("\t", 4), 4);
/// assert_eq!(grapheme_display_width_with_tab("a", 4), 1);
/// ```
#[inline]
pub fn grapheme_display_width_with_tab(grapheme: &str, tab_width: usize) -> usize {
    if grapheme == "\t" {
        tab_width
    } else {
        grapheme_display_width(grapheme)
    }
}

/// Check if a character is a zero-width character.
///
/// Zero-width characters include:
/// - Zero Width Space (U+200B)
/// - Zero Width Non-Joiner (U+200C)
/// - Zero Width Joiner (U+200D)
/// - Combining marks (general category Mn, Mc, Me)
/// - Variation selectors
/// - Other format characters
#[inline]
pub fn is_zero_width_char(c: char) -> bool {
    matches!(
        c,
        // Zero-width characters
        '\u{200B}' // Zero Width Space
        | '\u{200C}' // Zero Width Non-Joiner
        | '\u{200D}' // Zero Width Joiner
        | '\u{FEFF}' // Zero Width No-Break Space (BOM)
        // Variation selectors
        | '\u{FE00}'..='\u{FE0F}' // Variation Selectors
        | '\u{E0100}'..='\u{E01EF}' // Variation Selectors Supplement
        // Combining marks - these are typically rendered with their base character
        | '\u{0300}'..='\u{036F}' // Combining Diacritical Marks
        | '\u{1AB0}'..='\u{1AFF}' // Combining Diacritical Marks Extended
        | '\u{1DC0}'..='\u{1DFF}' // Combining Diacritical Marks Supplement
        | '\u{20D0}'..='\u{20FF}' // Combining Diacritical Marks for Symbols
        | '\u{FE20}'..='\u{FE2F}' // Combining Half Marks
    ) || c.width() == Some(0)
}

/// Check if a grapheme is an emoji sequence.
///
/// This detects:
/// - ZWJ sequences (family emoji, profession emoji, etc.)
/// - Emoji with variation selectors (VS16 for emoji presentation)
/// - Flag sequences (regional indicators)
fn is_emoji_sequence(grapheme: &str) -> bool {
    let chars: Vec<char> = grapheme.chars().collect();

    if chars.is_empty() {
        return false;
    }

    // Check for ZWJ (Zero Width Joiner) sequences
    if chars.contains(&'\u{200D}') {
        return true;
    }

    // Check for VS16 (emoji presentation selector)
    if chars.contains(&'\u{FE0F}') {
        return true;
    }

    // Check for regional indicator sequences (flags)
    if chars.len() >= 2 && chars.iter().all(|&c| is_regional_indicator(c)) {
        return true;
    }

    // Check for keycap sequences (digit + VS16 + combining enclosing keycap)
    if chars.len() >= 2 && chars.contains(&'\u{20E3}') {
        return true;
    }

    // Check if first character is an emoji base
    is_emoji_base(chars[0]) && chars.len() > 1
}

/// Check if a character is a regional indicator (used in flag sequences).
#[inline]
fn is_regional_indicator(c: char) -> bool {
    ('\u{1F1E6}'..='\u{1F1FF}').contains(&c)
}

/// Check if a character is an emoji base character.
///
/// This is a simplified check that covers common emoji ranges.
fn is_emoji_base(c: char) -> bool {
    matches!(
        c as u32,
        // Miscellaneous Symbols and Pictographs
        0x1F300..=0x1F5FF
        // Emoticons
        | 0x1F600..=0x1F64F
        // Transport and Map Symbols
        | 0x1F680..=0x1F6FF
        // Symbols and Pictographs Extended-A
        | 0x1FA00..=0x1FA6F
        // Symbols and Pictographs Extended-B
        | 0x1FA70..=0x1FAFF
        // Supplemental Symbols and Pictographs
        | 0x1F900..=0x1F9FF
        // Miscellaneous Symbols
        | 0x2600..=0x26FF
        // Dingbats
        | 0x2700..=0x27BF
        // Regional Indicators
        | 0x1F1E0..=0x1F1FF
    )
}

/// Check if a character is a wide (double-width) character.
///
/// Wide characters include:
/// - CJK Ideographs
/// - CJK Compatibility Ideographs
/// - Fullwidth forms
/// - Various CJK symbols
///
/// # Example
///
/// ```
/// use cortex_tui_text::grapheme::is_wide_char;
///
/// assert!(is_wide_char('中'));
/// assert!(is_wide_char('あ'));
/// assert!(!is_wide_char('a'));
/// ```
#[inline]
pub fn is_wide_char(c: char) -> bool {
    c.width() == Some(2)
}

/// Count the number of grapheme clusters in a string.
///
/// This is more accurate than `str::chars().count()` for user-perceived
/// character counts.
///
/// # Example
///
/// ```
/// use cortex_tui_text::grapheme::grapheme_count;
///
/// // "é" can be 1 or 2 codepoints depending on normalization
/// assert_eq!(grapheme_count("café"), 4);
/// // Family emoji is one grapheme
/// assert_eq!(grapheme_count("👨‍👩‍👧"), 1);
/// ```
#[inline]
pub fn grapheme_count(text: &str) -> usize {
    text.graphemes(true).count()
}

/// Get the grapheme cluster at a specific grapheme index.
///
/// Returns `None` if the index is out of bounds.
///
/// # Example
///
/// ```
/// use cortex_tui_text::grapheme::grapheme_at;
///
/// assert_eq!(grapheme_at("Hello", 0), Some("H"));
/// assert_eq!(grapheme_at("Hello", 4), Some("o"));
/// assert_eq!(grapheme_at("Hello", 5), None);
/// ```
pub fn grapheme_at(text: &str, index: usize) -> Option<&str> {
    text.graphemes(true).nth(index)
}

/// Find the byte offset for a grapheme index.
///
/// Returns `None` if the index is out of bounds.
///
/// # Example
///
/// ```
/// use cortex_tui_text::grapheme::grapheme_byte_offset;
///
/// assert_eq!(grapheme_byte_offset("Hello", 0), Some(0));
/// assert_eq!(grapheme_byte_offset("Hello", 1), Some(1));
/// assert_eq!(grapheme_byte_offset("日本語", 1), Some(3)); // Each CJK char is 3 bytes
/// ```
pub fn grapheme_byte_offset(text: &str, grapheme_index: usize) -> Option<usize> {
    text.grapheme_indices(true)
        .nth(grapheme_index)
        .map(|(offset, _)| offset)
}

/// Slice a string by grapheme indices.
///
/// Returns the substring from `start` (inclusive) to `end` (exclusive) grapheme indices.
/// Returns `None` if indices are out of bounds or `start > end`.
///
/// # Example
///
/// ```
/// use cortex_tui_text::grapheme::grapheme_slice;
///
/// assert_eq!(grapheme_slice("Hello World", 0, 5), Some("Hello"));
/// assert_eq!(grapheme_slice("日本語", 0, 2), Some("日本"));
/// ```
pub fn grapheme_slice(text: &str, start: usize, end: usize) -> Option<&str> {
    if start > end {
        return None;
    }

    let graphemes: Vec<&str> = text.graphemes(true).collect();

    if end > graphemes.len() {
        return None;
    }

    let start_byte = if start == 0 {
        0
    } else {
        graphemes[..start].iter().map(|g| g.len()).sum()
    };

    let end_byte = graphemes[..end].iter().map(|g| g.len()).sum();

    Some(&text[start_byte..end_byte])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ascii_width() {
        assert_eq!(grapheme_display_width("a"), 1);
        assert_eq!(grapheme_display_width("Z"), 1);
        assert_eq!(grapheme_display_width(" "), 1);
    }

    #[test]
    fn test_cjk_width() {
        assert_eq!(grapheme_display_width("中"), 2);
        assert_eq!(grapheme_display_width("日"), 2);
        assert_eq!(grapheme_display_width("あ"), 2);
        assert_eq!(grapheme_display_width("ア"), 2);
    }

    #[test]
    fn test_zero_width() {
        assert_eq!(grapheme_display_width("\n"), 0);
        assert_eq!(grapheme_display_width("\r"), 0);
    }

    #[test]
    fn test_tab_width() {
        assert_eq!(grapheme_display_width("\t"), 1);
        assert_eq!(grapheme_display_width_with_tab("\t", 4), 4);
        assert_eq!(grapheme_display_width_with_tab("\t", 8), 8);
    }

    #[test]
    fn test_emoji() {
        // Basic emoji should be width 2
        assert_eq!(grapheme_display_width("😀"), 2);
    }

    #[test]
    fn test_grapheme_count() {
        assert_eq!(grapheme_count("Hello"), 5);
        assert_eq!(grapheme_count("日本語"), 3);
        assert_eq!(grapheme_count(""), 0);
    }

    #[test]
    fn test_grapheme_at() {
        assert_eq!(grapheme_at("Hello", 0), Some("H"));
        assert_eq!(grapheme_at("Hello", 4), Some("o"));
        assert_eq!(grapheme_at("Hello", 5), None);
        assert_eq!(grapheme_at("日本語", 1), Some("本"));
    }

    #[test]
    fn test_grapheme_slice() {
        assert_eq!(grapheme_slice("Hello World", 0, 5), Some("Hello"));
        assert_eq!(grapheme_slice("日本語", 0, 2), Some("日本"));
        assert_eq!(grapheme_slice("Hello", 5, 3), None); // Invalid range
        assert_eq!(grapheme_slice("Hello", 0, 10), None); // Out of bounds
    }

    #[test]
    fn test_grapheme_iterator() {
        let text = "Hi世";
        let infos: Vec<_> = graphemes(text).collect();

        assert_eq!(infos.len(), 3);
        assert_eq!(infos[0].grapheme, "H");
        assert_eq!(infos[0].width, 1);
        assert_eq!(infos[0].byte_offset, 0);

        assert_eq!(infos[1].grapheme, "i");
        assert_eq!(infos[1].width, 1);
        assert_eq!(infos[1].byte_offset, 1);

        assert_eq!(infos[2].grapheme, "世");
        assert_eq!(infos[2].width, 2);
        assert_eq!(infos[2].byte_offset, 2);
    }

    #[test]
    fn test_is_wide_char() {
        assert!(is_wide_char('中'));
        assert!(is_wide_char('あ'));
        assert!(!is_wide_char('a'));
        assert!(!is_wide_char(' '));
    }

    #[test]
    fn test_grapheme_byte_offset() {
        assert_eq!(grapheme_byte_offset("Hello", 0), Some(0));
        assert_eq!(grapheme_byte_offset("Hello", 3), Some(3));
        assert_eq!(grapheme_byte_offset("日本語", 0), Some(0));
        assert_eq!(grapheme_byte_offset("日本語", 1), Some(3));
        assert_eq!(grapheme_byte_offset("日本語", 2), Some(6));
    }
}
