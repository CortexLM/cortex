//! Utility functions for form handling.

use unicode_segmentation::UnicodeSegmentation;

/// Converts a grapheme index to a byte offset in a string.
pub(crate) fn grapheme_to_byte_offset(s: &str, grapheme_idx: usize) -> usize {
    s.grapheme_indices(true)
        .nth(grapheme_idx)
        .map(|(byte_idx, _)| byte_idx)
        .unwrap_or(s.len())
}

/// Returns the number of graphemes in a string.
pub(crate) fn grapheme_count(s: &str) -> usize {
    s.graphemes(true).count()
}
