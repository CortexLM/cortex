//! Utility functions for help browser.

// ============================================================
// TEXT WRAPPING UTILITY
// ============================================================

/// Wraps text to fit within a given width.
///
/// # Arguments
/// * `text` - The text to wrap
/// * `width` - Maximum line width
pub fn wrap_text(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![];
    }

    let mut lines = Vec::new();
    let mut current = String::new();

    for word in text.split_whitespace() {
        if current.len() + word.len() + 1 > width {
            if !current.is_empty() {
                lines.push(current);
                current = String::new();
            }
            // Handle words longer than width
            if word.len() > width {
                let mut remaining = word;
                while remaining.len() > width {
                    lines.push(remaining[..width].to_string());
                    remaining = &remaining[width..];
                }
                if !remaining.is_empty() {
                    current = remaining.to_string();
                }
                continue;
            }
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }
    if !current.is_empty() {
        lines.push(current);
    }

    lines
}
