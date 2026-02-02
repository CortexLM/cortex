//! Text rendering and Unicode handling for Cortex TUI.
//!
//! This crate provides comprehensive text processing utilities for terminal UI applications,
//! with proper Unicode support including grapheme clustering, display width calculation,
//! text wrapping, and styled text composition.
//!
//! # Features
//!
//! - **Grapheme handling**: Proper Unicode grapheme cluster segmentation and width calculation
//! - **Text measurement**: Width and height calculation with wrap modes
//! - **Text wrapping**: Word and character-based wrapping with Unicode support
//! - **Styled text**: Composable styled text with colors and attributes
//! - **Line utilities**: Line splitting, joining, and manipulation
//!
//! # Example
//!
//! ```
//! use cortex_tui_text::{
//!     grapheme::grapheme_display_width,
//!     measurement::{measure_width, truncate_to_width},
//!     wrap::{wrap_text, WrapMode},
//!     styled::{Span, StyledText, Color},
//!     line::line_count,
//! };
//!
//! // Measure text width with proper Unicode handling
//! assert_eq!(measure_width("Hello 世界"), 10);
//!
//! // Truncate with ellipsis
//! assert_eq!(truncate_to_width("Hello World", 8), "Hello W…");
//!
//! // Wrap text at word boundaries
//! let lines = wrap_text("Hello World", 6, WrapMode::Word);
//! assert_eq!(lines, vec!["Hello", "World"]);
//!
//! // Create styled text
//! let span = Span::new("Hello").bold().fg(Color::RED);
//! let styled = StyledText::from_span(span);
//!
//! // Count lines
//! assert_eq!(line_count("Hello\nWorld"), 2);
//! ```

pub mod grapheme;
pub mod line;
pub mod measurement;
pub mod styled;
pub mod wrap;

// Re-export commonly used types at the crate root
pub use grapheme::{
    grapheme_at, grapheme_byte_offset, grapheme_count, grapheme_display_width,
    grapheme_display_width_with_tab, grapheme_slice, graphemes, graphemes_with_widths,
    is_wide_char, is_zero_width_char, GraphemeInfo, GraphemeIterator,
};

pub use measurement::{
    find_wrap_position, find_wrap_position_with_tab, fits_in_width, max_line_width,
    measure_dimensions, measure_height, measure_width, measure_width_with_tab, truncate_to_width,
    truncate_with_options, TruncationOptions, TruncationStyle, WrapResult,
};

pub use wrap::{wrap_iter, wrap_text, wrap_text_with_options, WrapMode, WrapOptions};

pub use styled::{
    bg, blue, bold, cyan, dim, fg, green, italic, magenta, red, underline, yellow, Color, Span,
    Style, StyledText, StyledTextBuilder, TextAttributes,
};

pub use line::{
    dedent, get_line, get_line_range, indent, join_lines, join_lines_lf, line_count, lines,
    normalize_line_endings, offset_to_position, position_to_offset, split_lines, split_lines_owned,
    trim_trailing_whitespace, LineEnding, LineInfo, LineIterator,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_integration() {
        // Test that all modules work together
        let text = "Hello 世界!\nThis is a test.";

        // Grapheme functions
        assert_eq!(grapheme_count("Hello"), 5);
        assert_eq!(grapheme_display_width("世"), 2);

        // Measurement
        assert_eq!(measure_width("Hello"), 5);
        assert_eq!(measure_width("世界"), 4);

        // Wrapping
        let wrapped = wrap_text(text, 10, WrapMode::Word);
        assert!(!wrapped.is_empty());

        // Styled text
        let styled = StyledTextBuilder::new()
            .bold()
            .fg(Color::RED)
            .text("Hello")
            .reset()
            .text(" World")
            .build();
        assert_eq!(styled.plain_text(), "Hello World");

        // Line utilities
        assert_eq!(line_count(text), 2);
        assert_eq!(get_line(text, 0), Some("Hello 世界!"));
    }

    #[test]
    fn test_cjk_text() {
        let text = "日本語テスト";

        // Each CJK character has width 2
        assert_eq!(measure_width(text), 12);
        assert_eq!(grapheme_count(text), 6);

        // Wrapping CJK text
        let wrapped = wrap_text(text, 6, WrapMode::Char);
        assert_eq!(wrapped, vec!["日本語", "テスト"]);
    }

    #[test]
    fn test_mixed_text() {
        let text = "Hello世界";

        // "Hello" = 5, "世界" = 4
        assert_eq!(measure_width(text), 9);
        assert_eq!(grapheme_count(text), 7);
    }

    #[test]
    fn test_empty_text() {
        assert_eq!(measure_width(""), 0);
        assert_eq!(grapheme_count(""), 0);
        assert_eq!(line_count(""), 1);
        assert_eq!(wrap_text("", 10, WrapMode::Word), vec![""]);
    }
}
