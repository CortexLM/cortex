//! Highlighted span types for syntax highlighting output.

use cortex_tui_text::{Span, Style, StyledText};
use std::ops::Range;

/// A byte range within the source text.
pub type ByteRange = Range<usize>;

/// A highlighted span of source code.
///
/// Represents a contiguous region of text with associated syntax highlighting style.
#[derive(Debug, Clone, PartialEq)]
pub struct HighlightSpan {
    /// The text content of this span.
    pub text: String,
    /// The style to apply (based on syntax highlighting).
    pub style: Style,
    /// The byte range in the original source text.
    pub range: ByteRange,
    /// The syntax capture group name (e.g., "keyword", "string", "comment").
    pub capture: Option<String>,
}

impl HighlightSpan {
    /// Creates a new highlight span.
    #[inline]
    pub fn new(text: impl Into<String>, style: Style, range: ByteRange) -> Self {
        Self {
            text: text.into(),
            style,
            range,
            capture: None,
        }
    }

    /// Creates a highlight span with a capture group name.
    #[inline]
    pub fn with_capture(
        text: impl Into<String>,
        style: Style,
        range: ByteRange,
        capture: impl Into<String>,
    ) -> Self {
        Self {
            text: text.into(),
            style,
            range,
            capture: Some(capture.into()),
        }
    }

    /// Creates a plain (unstyled) highlight span.
    #[inline]
    pub fn plain(text: impl Into<String>, range: ByteRange) -> Self {
        Self {
            text: text.into(),
            style: Style::new(),
            range,
            capture: None,
        }
    }

    /// Returns the length of the span in bytes.
    #[inline]
    pub fn len(&self) -> usize {
        self.text.len()
    }

    /// Returns true if the span is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Converts this highlight span to a cortex-tui-text Span.
    #[inline]
    pub fn to_span(&self) -> Span<'static> {
        Span::styled(self.text.clone(), self.style)
    }
}

impl From<HighlightSpan> for Span<'static> {
    fn from(span: HighlightSpan) -> Self {
        Span::styled(span.text, span.style)
    }
}

/// A collection of highlighted spans representing syntax-highlighted code.
#[derive(Debug, Clone, Default)]
pub struct HighlightedText {
    /// The highlighted spans.
    spans: Vec<HighlightSpan>,
    /// The original source text.
    source: String,
}

impl HighlightedText {
    /// Creates a new empty highlighted text.
    #[inline]
    pub fn new() -> Self {
        Self {
            spans: Vec::new(),
            source: String::new(),
        }
    }

    /// Creates highlighted text from source and spans.
    pub fn from_parts(source: impl Into<String>, spans: Vec<HighlightSpan>) -> Self {
        Self {
            spans,
            source: source.into(),
        }
    }

    /// Returns the original source text.
    #[inline]
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Returns the highlighted spans.
    #[inline]
    pub fn spans(&self) -> &[HighlightSpan] {
        &self.spans
    }

    /// Returns mutable access to the spans.
    #[inline]
    pub fn spans_mut(&mut self) -> &mut Vec<HighlightSpan> {
        &mut self.spans
    }

    /// Returns true if there are no spans.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.spans.is_empty()
    }

    /// Returns the number of spans.
    #[inline]
    pub fn len(&self) -> usize {
        self.spans.len()
    }

    /// Adds a span.
    #[inline]
    pub fn push(&mut self, span: HighlightSpan) {
        self.spans.push(span);
    }

    /// Converts this highlighted text to styled text.
    pub fn to_styled_text(&self) -> StyledText<'static> {
        self.spans.iter().map(|span| span.to_span()).collect()
    }

    /// Returns an iterator over the spans.
    #[inline]
    pub fn iter(&self) -> impl Iterator<Item = &HighlightSpan> {
        self.spans.iter()
    }
}

impl From<HighlightedText> for StyledText<'static> {
    fn from(highlighted: HighlightedText) -> Self {
        highlighted.to_styled_text()
    }
}

impl FromIterator<HighlightSpan> for HighlightedText {
    fn from_iter<T: IntoIterator<Item = HighlightSpan>>(iter: T) -> Self {
        Self {
            spans: iter.into_iter().collect(),
            source: String::new(),
        }
    }
}

/// Raw highlight information before text extraction.
///
/// Used internally during the highlighting process before converting
/// to full `HighlightSpan` instances.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawHighlight {
    /// Start byte offset in source.
    pub start: usize,
    /// End byte offset in source.
    pub end: usize,
    /// The capture group name.
    pub capture: String,
}

impl RawHighlight {
    /// Creates a new raw highlight.
    #[inline]
    pub fn new(start: usize, end: usize, capture: impl Into<String>) -> Self {
        Self {
            start,
            end,
            capture: capture.into(),
        }
    }

    /// Returns the byte range.
    #[inline]
    pub fn range(&self) -> ByteRange {
        self.start..self.end
    }

    /// Returns the length in bytes.
    #[inline]
    pub fn len(&self) -> usize {
        self.end.saturating_sub(self.start)
    }

    /// Returns true if the range is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.start >= self.end
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_highlight_span_creation() {
        let span = HighlightSpan::new("fn", Style::new(), 0..2);
        assert_eq!(span.text, "fn");
        assert_eq!(span.range, 0..2);
        assert!(span.capture.is_none());
    }

    #[test]
    fn test_highlight_span_with_capture() {
        let span = HighlightSpan::with_capture("fn", Style::new(), 0..2, "keyword.function");
        assert_eq!(span.capture, Some("keyword.function".to_string()));
    }

    #[test]
    fn test_highlighted_text_to_styled() {
        let mut highlighted = HighlightedText::new();
        highlighted.push(HighlightSpan::plain("hello", 0..5));
        highlighted.push(HighlightSpan::plain(" world", 5..11));

        let styled = highlighted.to_styled_text();
        assert_eq!(styled.len(), 2);
        assert_eq!(styled.plain_text(), "hello world");
    }

    #[test]
    fn test_raw_highlight() {
        let raw = RawHighlight::new(10, 20, "string");
        assert_eq!(raw.range(), 10..20);
        assert_eq!(raw.len(), 10);
        assert!(!raw.is_empty());
    }
}
