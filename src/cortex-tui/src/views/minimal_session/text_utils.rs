//! Text utility functions for minimal session view.

/// Wraps text to fit within a maximum width.
///
/// This function performs word wrapping, breaking lines at word boundaries
/// when possible. Very long words that exceed the width are broken mid-word.
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
        if line.chars().count() <= max_width {
            result.push(line.to_string());
            continue;
        }

        // Word wrap the line
        let mut current_line = String::new();
        let mut current_width = 0;

        for word in line.split_whitespace() {
            let word_width = word.chars().count();

            if current_line.is_empty() {
                // First word on line
                if word_width <= max_width {
                    current_line = word.to_string();
                    current_width = word_width;
                } else {
                    // Word is too long - break it
                    let mut chars = word.chars().peekable();
                    while chars.peek().is_some() {
                        let chunk: String = chars.by_ref().take(max_width).collect();
                        if !chunk.is_empty() {
                            result.push(chunk);
                        }
                    }
                }
            } else if current_width + 1 + word_width <= max_width {
                // Word fits on current line
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
                    // Word is too long - break it
                    current_line = String::new();
                    current_width = 0;
                    let mut chars = word.chars().peekable();
                    while chars.peek().is_some() {
                        let chunk: String = chars.by_ref().take(max_width).collect();
                        let chunk_len = chunk.chars().count();
                        if chunk_len == max_width {
                            result.push(chunk);
                        } else {
                            // Last chunk - keep for next word
                            current_line = chunk;
                            current_width = chunk_len;
                        }
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
