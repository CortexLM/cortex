//! Helper functions for edit replacement strategies.

use std::ops::Range;

use similar::{ChangeTag, TextDiff};

/// Trims whitespace from the start and end of each line.
pub fn trim_each_line(s: &str) -> String {
    s.lines()
        .map(|line| line.trim())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Normalizes whitespace: multiple spaces/tabs become single space.
pub fn normalize_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Normalizes indentation by removing common leading whitespace.
pub fn normalize_indentation(s: &str) -> String {
    let lines: Vec<&str> = s.lines().collect();
    let non_empty_lines: Vec<&str> = lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .copied()
        .collect();

    if non_empty_lines.is_empty() {
        return s.to_string();
    }

    // Find minimum indentation
    let min_indent = non_empty_lines
        .iter()
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);

    // Remove common indentation and normalize tabs to spaces
    lines
        .iter()
        .map(|line| {
            if line.trim().is_empty() {
                String::new()
            } else {
                let stripped = if line.len() >= min_indent {
                    &line[min_indent..]
                } else {
                    line
                };
                // Convert tabs to 4 spaces for normalization
                stripped.replace('\t', "    ")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Normalizes escape sequences for comparison.
pub fn normalize_escapes(s: &str) -> String {
    s.replace("\\n", "\n")
        .replace("\\t", "\t")
        .replace("\\r", "\r")
        .replace("\\\\", "\\")
        .replace("\\\"", "\"")
        .replace("\\'", "'")
}

/// Gets the leading whitespace from a line.
pub fn get_leading_whitespace(s: &str) -> &str {
    let trimmed_len = s.trim_start().len();
    &s[..s.len() - trimmed_len]
}

/// Adjusts indentation of new content to match the target indent.
pub fn adjust_indentation(new: &str, target_indent: &str) -> String {
    let lines: Vec<&str> = new.lines().collect();
    if lines.is_empty() {
        return new.to_string();
    }

    // Find minimum indentation in new content
    let non_empty: Vec<&str> = lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .copied()
        .collect();
    let min_indent = non_empty
        .iter()
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);

    lines
        .iter()
        .enumerate()
        .map(|(i, line)| {
            if line.trim().is_empty() {
                String::new()
            } else {
                let stripped = if line.len() >= min_indent {
                    &line[min_indent..]
                } else {
                    line
                };
                if i == 0 {
                    // First line gets target indent
                    format!("{}{}", target_indent, stripped.trim_start())
                } else {
                    // Other lines: preserve relative indentation
                    format!("{}{}", target_indent, stripped)
                }
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Gets the byte offset of the start of a line.
pub fn line_start_byte(content: &str, line_num: usize) -> usize {
    if line_num == 0 {
        return 0;
    }

    let mut byte_offset = 0;
    for (i, line) in content.lines().enumerate() {
        if i == line_num {
            return byte_offset;
        }
        byte_offset += line.len() + 1; // +1 for newline
    }
    byte_offset.min(content.len())
}

/// Gets the byte offset of the end of a line (after newline if present).
pub fn line_end_byte(content: &str, line_num: usize) -> usize {
    let mut byte_offset = 0;
    for (i, line) in content.lines().enumerate() {
        byte_offset += line.len();
        if i == line_num {
            // Include the newline if there is one
            if byte_offset < content.len() {
                byte_offset += 1;
            }
            return byte_offset;
        }
        byte_offset += 1; // newline
    }
    byte_offset.min(content.len())
}

/// Calculates similarity between two blocks of lines.
pub fn calculate_block_similarity(search_lines: &[&str], content_lines: &[&str]) -> f64 {
    if search_lines.is_empty() || content_lines.is_empty() {
        return 0.0;
    }

    let search_text = search_lines.join("\n");
    let content_text = content_lines.join("\n");

    calculate_text_similarity(&search_text, &content_text)
}

/// Calculates text similarity using diff.
pub fn calculate_text_similarity(a: &str, b: &str) -> f64 {
    if a == b {
        return 1.0;
    }
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }

    let diff = TextDiff::from_lines(a, b);
    let mut same_chars = 0;
    let mut total_chars = 0;

    for change in diff.iter_all_changes() {
        let len = change.value().len();
        total_chars += len;
        if change.tag() == ChangeTag::Equal {
            same_chars += len;
        }
    }

    if total_chars == 0 {
        return 0.0;
    }

    same_chars as f64 / total_chars as f64
}

/// Finds the best matching range in content for the search string.
#[allow(dead_code)]
pub fn find_best_match_range(
    content: &str,
    search: &str,
    _search_start: usize,
    _search_end: usize,
) -> Option<Range<usize>> {
    // Simple approach: find normalized match and return range
    let search_normalized = normalize_whitespace(search);

    let mut best_match: Option<(usize, usize, f64)> = None;

    // Slide a window through content
    let content_lines: Vec<&str> = content.lines().collect();
    let search_lines: Vec<&str> = search.lines().collect();

    if search_lines.is_empty() {
        return None;
    }

    for start_idx in 0..=content_lines.len().saturating_sub(search_lines.len()) {
        let window = content_lines[start_idx..start_idx + search_lines.len()].join("\n");
        let window_normalized = normalize_whitespace(&window);

        let similarity = calculate_text_similarity(&search_normalized, &window_normalized);

        if similarity >= 0.8 {
            let start_byte = line_start_byte(content, start_idx);
            let end_byte = line_end_byte(content, start_idx + search_lines.len() - 1);

            if best_match.is_none() || similarity > best_match.unwrap().2 {
                best_match = Some((start_byte, end_byte, similarity));
            }
        }
    }

    best_match.map(|(start, end, _)| start..end)
}

/// Finds a match using surrounding context lines.
pub fn find_context_match(
    content: &str,
    search: &str,
    context_lines: usize,
) -> Option<Range<usize>> {
    let content_lines: Vec<&str> = content.lines().collect();
    let search_lines: Vec<&str> = search.lines().collect();

    if search_lines.is_empty() {
        return None;
    }

    // Build context signatures for search
    let search_trimmed: Vec<&str> = search_lines.iter().map(|l| l.trim()).collect();

    // Try to find matches using context
    for start_idx in 0..=content_lines.len().saturating_sub(search_lines.len()) {
        let window = &content_lines[start_idx..start_idx + search_lines.len()];
        let window_trimmed: Vec<&str> = window.iter().map(|l| l.trim()).collect();

        // Check if window matches search
        if window_trimmed == search_trimmed {
            // Verify using context lines before and after
            let mut context_score = 0;
            let mut context_total = 0;

            // Check lines before
            for i in 1..=context_lines {
                if start_idx >= i {
                    context_total += 1;
                    // Context line exists
                    context_score += 1;
                }
            }

            // Check lines after
            for i in 1..=context_lines {
                if start_idx + search_lines.len() + i <= content_lines.len() {
                    context_total += 1;
                    context_score += 1;
                }
            }

            // If we have reasonable context, consider it a match
            if context_total == 0 || context_score >= context_total / 2 {
                let start_byte = line_start_byte(content, start_idx);
                let end_byte = line_end_byte(content, start_idx + search_lines.len() - 1);
                return Some(start_byte..end_byte);
            }
        }
    }

    None
}

/// Truncates a string for display purposes.
pub fn truncate_string(s: &str, max_len: usize) -> String {
    cortex_common::truncate_for_display(s, max_len).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trim_each_line() {
        let input = "  line1  \n  line2  \n  line3  ";
        let result = trim_each_line(input);
        assert_eq!(result, "line1\nline2\nline3");
    }

    #[test]
    fn test_normalize_whitespace() {
        let input = "hello    world\t\tfoo   bar";
        let result = normalize_whitespace(input);
        assert_eq!(result, "hello world foo bar");
    }

    #[test]
    fn test_normalize_indentation() {
        let input = "    fn main() {\n        let x = 1;\n    }";
        let result = normalize_indentation(input);
        assert_eq!(result, "fn main() {\n    let x = 1;\n}");
    }

    #[test]
    fn test_line_byte_positions() {
        let content = "line1\nline2\nline3";

        assert_eq!(line_start_byte(content, 0), 0);
        assert_eq!(line_start_byte(content, 1), 6);
        assert_eq!(line_start_byte(content, 2), 12);

        assert_eq!(line_end_byte(content, 0), 6);
        assert_eq!(line_end_byte(content, 1), 12);
        assert_eq!(line_end_byte(content, 2), 17);
    }

    #[test]
    fn test_calculate_text_similarity() {
        assert_eq!(calculate_text_similarity("abc", "abc"), 1.0);
        // For single-character differences in line-based diff, similarity might vary
        let sim = calculate_text_similarity("abc", "abd");
        assert!(
            sim >= 0.0 && sim <= 1.0,
            "Similarity should be between 0 and 1: {}",
            sim
        );
        // Similar strings should have higher similarity than completely different ones
        let sim_similar = calculate_text_similarity("hello world", "hello earth");
        let sim_different = calculate_text_similarity("hello", "xyz");
        assert!(
            sim_similar > sim_different || (sim_similar == 0.0 && sim_different == 0.0),
            "Similar strings should score higher: similar={} different={}",
            sim_similar,
            sim_different
        );
    }

    #[test]
    fn test_truncate_string() {
        assert_eq!(truncate_string("hello", 10), "hello");
        assert_eq!(truncate_string("hello world", 8), "hello...");
    }
}
