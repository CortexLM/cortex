//! XML formatting utilities.

/// Format XML with proper indentation (Issue #1980).
/// Simple XML pretty-printer that adds indentation to make XML readable.
pub fn format_xml(xml: &str) -> String {
    let mut result = String::new();
    let mut indent_level: usize = 0;
    let mut in_tag = false;
    let mut current_tag = String::new();
    let mut is_closing_tag;
    let mut is_self_closing;
    let mut text_content = String::new();

    for c in xml.chars() {
        match c {
            '<' => {
                // Output any accumulated text content
                let trimmed = text_content.trim();
                if !trimmed.is_empty() {
                    result.push_str(trimmed);
                }
                text_content.clear();

                in_tag = true;
                current_tag.clear();
            }
            '>' => {
                if in_tag {
                    // Check if this is a closing tag
                    is_closing_tag = current_tag.starts_with('/');
                    // Check if self-closing
                    is_self_closing = current_tag.ends_with('/');

                    // For closing tags, decrease indent first
                    if is_closing_tag && indent_level > 0 {
                        indent_level -= 1;
                    }

                    // Add newline and indent before tag (except for first tag)
                    if !result.is_empty() {
                        result.push('\n');
                        result.push_str(&"  ".repeat(indent_level));
                    }

                    // Output the tag
                    result.push('<');
                    result.push_str(&current_tag);
                    result.push('>');

                    // For opening tags (not self-closing), increase indent
                    if !is_closing_tag
                        && !is_self_closing
                        && !current_tag.starts_with('?')
                        && !current_tag.starts_with('!')
                    {
                        indent_level += 1;
                    }

                    in_tag = false;
                    current_tag.clear();
                }
            }
            _ => {
                if in_tag {
                    current_tag.push(c);
                } else {
                    text_content.push(c);
                }
            }
        }
    }

    // Handle any remaining text
    let trimmed = text_content.trim();
    if !trimmed.is_empty() {
        result.push_str(trimmed);
    }

    result
}
