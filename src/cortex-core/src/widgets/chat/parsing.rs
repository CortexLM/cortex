//! Markdown-lite text parsing.
//!
//! Provides basic markdown formatting support for chat messages.

use super::types::StyledSegment;
use crate::style::CortexStyle;
use ratatui::prelude::*;

/// Parses text with basic markdown formatting.
///
/// Supports:
/// - Bold: **text** or __text__
/// - Code: `code`
pub fn parse_markdown_lite(text: &str, base_style: Style) -> Vec<StyledSegment> {
    let mut segments = Vec::new();
    let mut current_text = String::new();
    let mut chars = text.chars().peekable();

    while let Some(ch) = chars.next() {
        match ch {
            '*' if chars.peek() == Some(&'*') => {
                // Consume the second '*'
                chars.next();

                // Push current text if any
                if !current_text.is_empty() {
                    segments.push(StyledSegment {
                        text: std::mem::take(&mut current_text),
                        style: base_style,
                    });
                }

                // Collect bold text until **
                let mut bold_text = String::new();
                let mut found_end = false;
                while let Some(c) = chars.next() {
                    if c == '*' && chars.peek() == Some(&'*') {
                        chars.next(); // Consume second '*'
                        found_end = true;
                        break;
                    }
                    bold_text.push(c);
                }

                if found_end && !bold_text.is_empty() {
                    segments.push(StyledSegment {
                        text: bold_text,
                        style: base_style.add_modifier(Modifier::BOLD),
                    });
                } else {
                    // No closing **, treat as literal
                    current_text.push_str("**");
                    current_text.push_str(&bold_text);
                }
            }
            '_' if chars.peek() == Some(&'_') => {
                // Consume the second '_'
                chars.next();

                // Push current text if any
                if !current_text.is_empty() {
                    segments.push(StyledSegment {
                        text: std::mem::take(&mut current_text),
                        style: base_style,
                    });
                }

                // Collect bold text until __
                let mut bold_text = String::new();
                let mut found_end = false;
                while let Some(c) = chars.next() {
                    if c == '_' && chars.peek() == Some(&'_') {
                        chars.next(); // Consume second '_'
                        found_end = true;
                        break;
                    }
                    bold_text.push(c);
                }

                if found_end && !bold_text.is_empty() {
                    segments.push(StyledSegment {
                        text: bold_text,
                        style: base_style.add_modifier(Modifier::BOLD),
                    });
                } else {
                    // No closing __, treat as literal
                    current_text.push_str("__");
                    current_text.push_str(&bold_text);
                }
            }
            '`' => {
                // Push current text if any
                if !current_text.is_empty() {
                    segments.push(StyledSegment {
                        text: std::mem::take(&mut current_text),
                        style: base_style,
                    });
                }

                // Collect code text until `
                let mut code_text = String::new();
                let mut found_end = false;
                while let Some(c) = chars.next() {
                    if c == '`' {
                        found_end = true;
                        break;
                    }
                    code_text.push(c);
                }

                if found_end {
                    segments.push(StyledSegment {
                        text: code_text,
                        style: CortexStyle::code(),
                    });
                } else {
                    // No closing `, treat as literal
                    current_text.push('`');
                    current_text.push_str(&code_text);
                }
            }
            _ => {
                current_text.push(ch);
            }
        }
    }

    // Push remaining text
    if !current_text.is_empty() {
        segments.push(StyledSegment {
            text: current_text,
            style: base_style,
        });
    }

    // Return at least one empty segment if nothing was parsed
    if segments.is_empty() {
        segments.push(StyledSegment {
            text: String::new(),
            style: base_style,
        });
    }

    segments
}

// ============================================================
// TESTS
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_markdown_lite_plain() {
        let segments = parse_markdown_lite("Hello world", Style::default());
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].text, "Hello world");
    }

    #[test]
    fn test_parse_markdown_lite_bold() {
        let segments = parse_markdown_lite("Hello **bold** world", Style::default());
        assert_eq!(segments.len(), 3);
        assert_eq!(segments[0].text, "Hello ");
        assert_eq!(segments[1].text, "bold");
        assert!(segments[1].style.add_modifier == Modifier::BOLD.into());
        assert_eq!(segments[2].text, " world");
    }

    #[test]
    fn test_parse_markdown_lite_code() {
        let segments = parse_markdown_lite("Use `code` here", Style::default());
        assert_eq!(segments.len(), 3);
        assert_eq!(segments[0].text, "Use ");
        assert_eq!(segments[1].text, "code");
        assert_eq!(segments[2].text, " here");
    }

    #[test]
    fn test_parse_markdown_lite_unclosed() {
        let segments = parse_markdown_lite("Hello **unclosed", Style::default());
        // Should treat ** as literal - may create multiple segments with same style
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].text, "Hello ");
        assert_eq!(segments[1].text, "**unclosed");
    }
}
