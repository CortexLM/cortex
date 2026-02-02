//! Line utilities for text processing.
//!
//! This module provides functions for working with lines of text,
//! including splitting, iterating, counting, and manipulation.

use crate::measurement::measure_width;
use unicode_segmentation::UnicodeSegmentation;

/// Information about a line in text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineInfo<'a> {
    /// The line content (without the line ending).
    pub content: &'a str,
    /// The byte offset of this line in the original text.
    pub byte_offset: usize,
    /// The line number (0-indexed).
    pub line_number: usize,
    /// The line ending type, if present.
    pub ending: Option<LineEnding>,
}

impl<'a> LineInfo<'a> {
    /// Get the display width of this line.
    #[inline]
    pub fn width(&self) -> usize {
        measure_width(self.content)
    }

    /// Check if this line is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.content.is_empty()
    }

    /// Check if this line is whitespace only.
    #[inline]
    pub fn is_blank(&self) -> bool {
        self.content.chars().all(|c| c.is_whitespace())
    }

    /// Get the byte length of this line (content only, no line ending).
    #[inline]
    pub fn len(&self) -> usize {
        self.content.len()
    }
}

/// Type of line ending.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LineEnding {
    /// Unix line ending: `\n`
    Lf,
    /// Windows line ending: `\r\n`
    CrLf,
    /// Old Mac line ending: `\r`
    Cr,
}

impl LineEnding {
    /// Get the string representation of this line ending.
    #[inline]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Lf => "\n",
            Self::CrLf => "\r\n",
            Self::Cr => "\r",
        }
    }

    /// Get the byte length of this line ending.
    #[inline]
    pub const fn len(&self) -> usize {
        match self {
            Self::Lf => 1,
            Self::CrLf => 2,
            Self::Cr => 1,
        }
    }

    /// Returns false - line endings are never empty.
    /// This is provided to satisfy the len_without_is_empty lint.
    #[inline]
    pub const fn is_empty(&self) -> bool {
        false // Line endings are never empty
    }

    /// Detect the dominant line ending in text.
    ///
    /// Returns `None` if no line endings are found.
    pub fn detect(text: &str) -> Option<Self> {
        let mut lf_count = 0;
        let mut crlf_count = 0;
        let mut cr_count = 0;

        let bytes = text.as_bytes();
        let mut i = 0;

        while i < bytes.len() {
            if bytes[i] == b'\r' {
                if i + 1 < bytes.len() && bytes[i + 1] == b'\n' {
                    crlf_count += 1;
                    i += 2;
                } else {
                    cr_count += 1;
                    i += 1;
                }
            } else if bytes[i] == b'\n' {
                lf_count += 1;
                i += 1;
            } else {
                i += 1;
            }
        }

        if crlf_count >= lf_count && crlf_count >= cr_count && crlf_count > 0 {
            Some(Self::CrLf)
        } else if lf_count >= cr_count && lf_count > 0 {
            Some(Self::Lf)
        } else if cr_count > 0 {
            Some(Self::Cr)
        } else {
            None
        }
    }
}

impl Default for LineEnding {
    /// Default to Unix line ending.
    fn default() -> Self {
        Self::Lf
    }
}

impl std::fmt::Display for LineEnding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Iterator over lines with full information.
pub struct LineIterator<'a> {
    text: &'a str,
    position: usize,
    line_number: usize,
    trailing_empty_returned: bool,
}

impl<'a> LineIterator<'a> {
    /// Create a new line iterator.
    #[inline]
    pub fn new(text: &'a str) -> Self {
        Self {
            text,
            position: 0,
            line_number: 0,
            trailing_empty_returned: false,
        }
    }
}

impl<'a> Iterator for LineIterator<'a> {
    type Item = LineInfo<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        // Already exhausted
        if self.position > self.text.len() {
            return None;
        }

        // At end of text
        if self.position == self.text.len() {
            // Check if text ends with a line ending - return trailing empty line
            if !self.trailing_empty_returned
                && (self.text.ends_with('\n') || self.text.ends_with('\r'))
            {
                self.trailing_empty_returned = true;
                let line_number = self.line_number;
                self.line_number += 1;
                self.position = self.text.len() + 1;
                return Some(LineInfo {
                    content: "",
                    byte_offset: self.text.len(),
                    line_number,
                    ending: None,
                });
            }
            // Empty input case
            if self.text.is_empty() && self.line_number == 0 {
                self.position = 1;
                self.line_number = 1;
                return Some(LineInfo {
                    content: "",
                    byte_offset: 0,
                    line_number: 0,
                    ending: None,
                });
            }
            return None;
        }

        let remaining = &self.text[self.position..];
        let byte_offset = self.position;
        let line_number = self.line_number;

        // Find the next line ending
        let (content, ending, advance) = find_line_end(remaining);

        self.position += advance;
        self.line_number += 1;

        Some(LineInfo {
            content,
            byte_offset,
            line_number,
            ending,
        })
    }
}

impl<'a> std::iter::FusedIterator for LineIterator<'a> {}

/// Find the end of a line and return (content, ending, bytes_to_advance).
fn find_line_end(text: &str) -> (&str, Option<LineEnding>, usize) {
    let bytes = text.as_bytes();

    for (i, &byte) in bytes.iter().enumerate() {
        if byte == b'\r' {
            // Check for CRLF
            if i + 1 < bytes.len() && bytes[i + 1] == b'\n' {
                return (&text[..i], Some(LineEnding::CrLf), i + 2);
            } else {
                return (&text[..i], Some(LineEnding::Cr), i + 1);
            }
        } else if byte == b'\n' {
            return (&text[..i], Some(LineEnding::Lf), i + 1);
        }
    }

    // No line ending found - return entire text
    (text, None, text.len())
}

/// Get an iterator over lines with full information.
///
/// This provides more information than `str::lines()`, including byte offsets
/// and line ending types.
///
/// # Example
///
/// ```
/// use cortex_tui_text::line::lines;
///
/// for line_info in lines("Hello\nWorld") {
///     println!("Line {}: {} (offset {})",
///         line_info.line_number,
///         line_info.content,
///         line_info.byte_offset);
/// }
/// ```
#[inline]
pub fn lines(text: &str) -> LineIterator<'_> {
    LineIterator::new(text)
}

/// Split text into lines, returning just the content (no line endings).
///
/// Unlike `str::lines()`, this preserves a trailing empty line if the text
/// ends with a newline.
///
/// # Example
///
/// ```
/// use cortex_tui_text::line::split_lines;
///
/// let lines = split_lines("a\nb\n");
/// assert_eq!(lines, vec!["a", "b", ""]);
/// ```
pub fn split_lines(text: &str) -> Vec<&str> {
    lines(text).map(|info| info.content).collect()
}

/// Split text into owned line strings.
pub fn split_lines_owned(text: &str) -> Vec<String> {
    lines(text).map(|info| info.content.to_string()).collect()
}

/// Count the number of lines in text.
///
/// An empty string has 1 line. A string ending with a newline has
/// one more line than the number of newlines (the final empty line).
///
/// # Example
///
/// ```
/// use cortex_tui_text::line::line_count;
///
/// assert_eq!(line_count(""), 1);
/// assert_eq!(line_count("Hello"), 1);
/// assert_eq!(line_count("Hello\n"), 2);
/// assert_eq!(line_count("Hello\nWorld"), 2);
/// assert_eq!(line_count("Hello\nWorld\n"), 3);
/// ```
pub fn line_count(text: &str) -> usize {
    if text.is_empty() {
        return 1;
    }

    let mut count = 1;
    let bytes = text.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] == b'\r' {
            count += 1;
            // Skip the \n in CRLF
            if i + 1 < bytes.len() && bytes[i + 1] == b'\n' {
                i += 2;
            } else {
                i += 1;
            }
        } else if bytes[i] == b'\n' {
            count += 1;
            i += 1;
        } else {
            i += 1;
        }
    }

    count
}

/// Get a specific line by number (0-indexed).
///
/// Returns `None` if the line number is out of bounds.
///
/// # Example
///
/// ```
/// use cortex_tui_text::line::get_line;
///
/// assert_eq!(get_line("Hello\nWorld", 0), Some("Hello"));
/// assert_eq!(get_line("Hello\nWorld", 1), Some("World"));
/// assert_eq!(get_line("Hello\nWorld", 2), None);
/// ```
pub fn get_line(text: &str, line_number: usize) -> Option<&str> {
    lines(text)
        .find(|info| info.line_number == line_number)
        .map(|info| info.content)
}

/// Get a range of lines (start..end, 0-indexed).
///
/// Returns lines from `start` (inclusive) to `end` (exclusive).
///
/// # Example
///
/// ```
/// use cortex_tui_text::line::get_line_range;
///
/// let text = "a\nb\nc\nd";
/// assert_eq!(get_line_range(text, 1, 3), vec!["b", "c"]);
/// ```
pub fn get_line_range(text: &str, start: usize, end: usize) -> Vec<&str> {
    lines(text)
        .filter(|info| info.line_number >= start && info.line_number < end)
        .map(|info| info.content)
        .collect()
}

/// Find the line number and column for a byte offset.
///
/// Returns `(line_number, column)` where both are 0-indexed.
/// Column is in graphemes, not bytes.
///
/// Returns `None` if the offset is out of bounds.
///
/// # Example
///
/// ```
/// use cortex_tui_text::line::offset_to_position;
///
/// assert_eq!(offset_to_position("Hello\nWorld", 0), Some((0, 0)));
/// assert_eq!(offset_to_position("Hello\nWorld", 6), Some((1, 0)));
/// assert_eq!(offset_to_position("Hello\nWorld", 8), Some((1, 2)));
/// ```
pub fn offset_to_position(text: &str, offset: usize) -> Option<(usize, usize)> {
    if offset > text.len() {
        return None;
    }

    for info in lines(text) {
        let line_end = info.byte_offset + info.content.len();

        if offset <= line_end {
            // Offset is within this line
            let column_offset = offset.saturating_sub(info.byte_offset);
            let column_text = &info.content[..column_offset.min(info.content.len())];
            let column = column_text.graphemes(true).count();
            return Some((info.line_number, column));
        }
    }

    // Offset is at end of text
    let last_line = line_count(text).saturating_sub(1);
    let last_line_content = get_line(text, last_line).unwrap_or("");
    let column = last_line_content.graphemes(true).count();
    Some((last_line, column))
}

/// Find the byte offset for a line and column position.
///
/// Line and column are 0-indexed. Column is in graphemes.
///
/// Returns `None` if the position is out of bounds.
///
/// # Example
///
/// ```
/// use cortex_tui_text::line::position_to_offset;
///
/// assert_eq!(position_to_offset("Hello\nWorld", 0, 0), Some(0));
/// assert_eq!(position_to_offset("Hello\nWorld", 1, 0), Some(6));
/// assert_eq!(position_to_offset("Hello\nWorld", 1, 2), Some(8));
/// ```
pub fn position_to_offset(text: &str, line: usize, column: usize) -> Option<usize> {
    let line_info = lines(text).find(|info| info.line_number == line)?;

    let mut byte_offset = line_info.byte_offset;

    for (i, grapheme) in line_info.content.graphemes(true).enumerate() {
        if i == column {
            return Some(byte_offset);
        }
        byte_offset += grapheme.len();
    }

    // Column is at end of line
    if column == line_info.content.graphemes(true).count() {
        return Some(byte_offset);
    }

    None
}

/// Join lines with a specific line ending.
///
/// # Example
///
/// ```
/// use cortex_tui_text::line::{join_lines, LineEnding};
///
/// let joined = join_lines(&["Hello", "World"], LineEnding::Lf);
/// assert_eq!(joined, "Hello\nWorld");
/// ```
pub fn join_lines(lines: &[&str], ending: LineEnding) -> String {
    lines.join(ending.as_str())
}

/// Join lines with the default line ending (LF).
#[inline]
pub fn join_lines_lf(lines: &[&str]) -> String {
    lines.join("\n")
}

/// Normalize line endings in text to a specific type.
///
/// # Example
///
/// ```
/// use cortex_tui_text::line::{normalize_line_endings, LineEnding};
///
/// let text = "Hello\r\nWorld\rFoo\nBar";
/// let normalized = normalize_line_endings(text, LineEnding::Lf);
/// assert_eq!(normalized, "Hello\nWorld\nFoo\nBar");
/// ```
pub fn normalize_line_endings(text: &str, ending: LineEnding) -> String {
    let line_vec: Vec<&str> = lines(text).map(|info| info.content).collect();

    // Don't add trailing ending unless original had one
    let has_trailing_ending = text.ends_with('\n') || text.ends_with('\r');

    let mut result = join_lines(&line_vec, ending);

    // Remove the trailing line ending added by join if original didn't have one
    // and the last line was empty
    if has_trailing_ending && !line_vec.is_empty() {
        result.push_str(ending.as_str());
    }

    // Handle case where we have an extra empty string at the end from trailing newline
    if !has_trailing_ending && result.ends_with(ending.as_str()) {
        result.truncate(result.len() - ending.len());
    }

    result
}

/// Indent all lines with the specified prefix.
///
/// # Example
///
/// ```
/// use cortex_tui_text::line::indent;
///
/// let indented = indent("Hello\nWorld", "  ");
/// assert_eq!(indented, "  Hello\n  World");
/// ```
pub fn indent(text: &str, prefix: &str) -> String {
    let mut result = String::with_capacity(text.len() + prefix.len() * line_count(text));
    let mut first = true;

    for info in lines(text) {
        if !first {
            result.push('\n');
        }
        first = false;

        result.push_str(prefix);
        result.push_str(info.content);
    }

    result
}

/// Remove common leading whitespace from all lines.
///
/// # Example
///
/// ```
/// use cortex_tui_text::line::dedent;
///
/// let text = "    Hello\n    World";
/// let dedented = dedent(text);
/// assert_eq!(dedented, "Hello\nWorld");
/// ```
pub fn dedent(text: &str) -> String {
    // Find minimum indentation of non-empty lines
    let min_indent = lines(text)
        .filter(|info| !info.is_blank())
        .map(|info| {
            info.content
                .chars()
                .take_while(|c| c.is_whitespace())
                .count()
        })
        .min()
        .unwrap_or(0);

    if min_indent == 0 {
        return text.to_string();
    }

    // Remove the common indentation
    let mut result = String::with_capacity(text.len());
    let mut first = true;

    for info in lines(text) {
        if !first {
            result.push('\n');
        }
        first = false;

        if info.is_blank() {
            result.push_str(info.content);
        } else {
            // Skip the common indent
            let trimmed: String = info.content.chars().skip(min_indent).collect();
            result.push_str(&trimmed);
        }
    }

    result
}

/// Trim trailing whitespace from each line.
///
/// # Example
///
/// ```
/// use cortex_tui_text::line::trim_trailing_whitespace;
///
/// let text = "Hello   \nWorld  \n";
/// let trimmed = trim_trailing_whitespace(text);
/// assert_eq!(trimmed, "Hello\nWorld\n");
/// ```
pub fn trim_trailing_whitespace(text: &str) -> String {
    let has_trailing_newline = text.ends_with('\n') || text.ends_with('\r');

    let line_vec: Vec<&str> = lines(text).map(|info| info.content.trim_end()).collect();

    let mut result = join_lines_lf(&line_vec);

    if has_trailing_newline && !result.ends_with('\n') {
        result.push('\n');
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_line_ending_detect() {
        assert_eq!(LineEnding::detect("Hello\nWorld"), Some(LineEnding::Lf));
        assert_eq!(LineEnding::detect("Hello\r\nWorld"), Some(LineEnding::CrLf));
        assert_eq!(LineEnding::detect("Hello\rWorld"), Some(LineEnding::Cr));
        assert_eq!(LineEnding::detect("Hello World"), None);
    }

    #[test]
    fn test_line_count() {
        assert_eq!(line_count(""), 1);
        assert_eq!(line_count("Hello"), 1);
        assert_eq!(line_count("Hello\n"), 2);
        assert_eq!(line_count("Hello\nWorld"), 2);
        assert_eq!(line_count("Hello\nWorld\n"), 3);
        assert_eq!(line_count("Hello\r\nWorld\r\n"), 3);
    }

    #[test]
    fn test_split_lines() {
        assert_eq!(split_lines("a\nb"), vec!["a", "b"]);
        assert_eq!(split_lines("a\nb\n"), vec!["a", "b", ""]);
        assert_eq!(split_lines("a\r\nb"), vec!["a", "b"]);
    }

    #[test]
    fn test_get_line() {
        assert_eq!(get_line("Hello\nWorld", 0), Some("Hello"));
        assert_eq!(get_line("Hello\nWorld", 1), Some("World"));
        assert_eq!(get_line("Hello\nWorld", 2), None);
    }

    #[test]
    fn test_get_line_range() {
        let text = "a\nb\nc\nd";
        assert_eq!(get_line_range(text, 0, 2), vec!["a", "b"]);
        assert_eq!(get_line_range(text, 1, 3), vec!["b", "c"]);
    }

    #[test]
    fn test_offset_to_position() {
        assert_eq!(offset_to_position("Hello\nWorld", 0), Some((0, 0)));
        assert_eq!(offset_to_position("Hello\nWorld", 5), Some((0, 5)));
        assert_eq!(offset_to_position("Hello\nWorld", 6), Some((1, 0)));
        assert_eq!(offset_to_position("Hello\nWorld", 11), Some((1, 5)));
    }

    #[test]
    fn test_position_to_offset() {
        assert_eq!(position_to_offset("Hello\nWorld", 0, 0), Some(0));
        assert_eq!(position_to_offset("Hello\nWorld", 0, 5), Some(5));
        assert_eq!(position_to_offset("Hello\nWorld", 1, 0), Some(6));
        assert_eq!(position_to_offset("Hello\nWorld", 1, 5), Some(11));
    }

    #[test]
    fn test_join_lines() {
        assert_eq!(join_lines(&["a", "b", "c"], LineEnding::Lf), "a\nb\nc");
        assert_eq!(join_lines(&["a", "b"], LineEnding::CrLf), "a\r\nb");
    }

    #[test]
    fn test_normalize_line_endings() {
        let text = "a\r\nb\rc\nd";
        let normalized = normalize_line_endings(text, LineEnding::Lf);
        assert_eq!(normalized, "a\nb\nc\nd");
    }

    #[test]
    fn test_indent() {
        assert_eq!(indent("a\nb", "  "), "  a\n  b");
        assert_eq!(indent("Hello", ">>> "), ">>> Hello");
    }

    #[test]
    fn test_dedent() {
        assert_eq!(dedent("    a\n    b"), "a\nb");
        assert_eq!(dedent("  a\n    b"), "a\n  b");
        assert_eq!(dedent("a\nb"), "a\nb");
    }

    #[test]
    fn test_trim_trailing_whitespace() {
        assert_eq!(trim_trailing_whitespace("a  \nb  "), "a\nb");
        assert_eq!(trim_trailing_whitespace("a\nb\n"), "a\nb\n");
    }

    #[test]
    fn test_line_info() {
        let text = "Hello\nWorld";
        let line_infos: Vec<_> = lines(text).collect();

        assert_eq!(line_infos.len(), 2);
        assert_eq!(line_infos[0].content, "Hello");
        assert_eq!(line_infos[0].line_number, 0);
        assert_eq!(line_infos[0].byte_offset, 0);
        assert_eq!(line_infos[0].ending, Some(LineEnding::Lf));

        assert_eq!(line_infos[1].content, "World");
        assert_eq!(line_infos[1].line_number, 1);
        assert_eq!(line_infos[1].byte_offset, 6);
        assert_eq!(line_infos[1].ending, None);
    }

    #[test]
    fn test_line_info_methods() {
        let text = "Hello World";
        let info = lines(text).next().unwrap();

        assert_eq!(info.width(), 11);
        assert!(!info.is_empty());
        assert!(!info.is_blank());
        assert_eq!(info.len(), 11);
    }
}
