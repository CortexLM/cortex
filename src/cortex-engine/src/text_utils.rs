//! Text utilities.
//!
//! Provides utilities for text processing including
//! text manipulation, formatting, and analysis.

use std::borrow::Cow;
use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Text statistics.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TextStats {
    /// Character count.
    pub chars: usize,
    /// Word count.
    pub words: usize,
    /// Line count.
    pub lines: usize,
    /// Sentence count.
    pub sentences: usize,
    /// Paragraph count.
    pub paragraphs: usize,
    /// Average word length.
    pub avg_word_length: f32,
    /// Average words per sentence.
    pub avg_words_per_sentence: f32,
}

impl TextStats {
    /// Compute stats for text.
    pub fn compute(text: &str) -> Self {
        let chars = text.chars().count();
        let words: Vec<&str> = text.split_whitespace().collect();
        let word_count = words.len();
        let lines = text.lines().count().max(1);

        // Count sentences (simple heuristic)
        let sentences = text
            .chars()
            .filter(|c| *c == '.' || *c == '!' || *c == '?')
            .count()
            .max(1);

        // Count paragraphs
        let paragraphs = text
            .split("\n\n")
            .filter(|p| !p.trim().is_empty())
            .count()
            .max(1);

        // Average word length
        let total_word_chars: usize = words.iter().map(|w| w.len()).sum();
        let avg_word_length = if word_count > 0 {
            total_word_chars as f32 / word_count as f32
        } else {
            0.0
        };

        // Average words per sentence
        let avg_words_per_sentence = if sentences > 0 {
            word_count as f32 / sentences as f32
        } else {
            0.0
        };

        Self {
            chars,
            words: word_count,
            lines,
            sentences,
            paragraphs,
            avg_word_length,
            avg_words_per_sentence,
        }
    }
}

/// Word frequency.
pub fn word_frequency(text: &str) -> HashMap<String, usize> {
    let mut freq = HashMap::new();

    for word in text.split_whitespace() {
        let word = word
            .to_lowercase()
            .chars()
            .filter(|c| c.is_alphanumeric())
            .collect::<String>();

        if !word.is_empty() {
            *freq.entry(word).or_insert(0) += 1;
        }
    }

    freq
}

/// Get most common words.
pub fn most_common_words(text: &str, count: usize) -> Vec<(String, usize)> {
    let freq = word_frequency(text);
    let mut words: Vec<_> = freq.into_iter().collect();
    words.sort_by(|a, b| b.1.cmp(&a.1));
    words.truncate(count);
    words
}

/// Text similarity (Levenshtein distance).
pub fn levenshtein_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();

    let n = a.len();
    let m = b.len();

    if n == 0 {
        return m;
    }
    if m == 0 {
        return n;
    }

    let mut matrix = vec![vec![0usize; m + 1]; n + 1];

    for i in 0..=n {
        matrix[i][0] = i;
    }
    for j in 0..=m {
        matrix[0][j] = j;
    }

    for i in 1..=n {
        for j in 1..=m {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            matrix[i][j] = (matrix[i - 1][j] + 1)
                .min(matrix[i][j - 1] + 1)
                .min(matrix[i - 1][j - 1] + cost);
        }
    }

    matrix[n][m]
}

/// Text similarity ratio (0.0 to 1.0).
pub fn similarity_ratio(a: &str, b: &str) -> f32 {
    let distance = levenshtein_distance(a, b);
    let max_len = a.len().max(b.len());

    if max_len == 0 {
        return 1.0;
    }

    1.0 - (distance as f32 / max_len as f32)
}

/// Truncate text with ellipsis.
pub fn truncate(text: &str, max_len: usize) -> Cow<'_, str> {
    if text.len() <= max_len {
        return Cow::Borrowed(text);
    }

    let mut end = max_len.saturating_sub(3);
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }

    Cow::Owned(format!("{}...", &text[..end]))
}

/// Truncate text at word boundary.
pub fn truncate_words(text: &str, max_len: usize) -> Cow<'_, str> {
    if text.len() <= max_len {
        return Cow::Borrowed(text);
    }

    let target = max_len.saturating_sub(3);
    let mut end = target.min(text.len());

    // Find word boundary
    while end > 0 && !text[..end].ends_with(char::is_whitespace) {
        end -= 1;
    }

    if end == 0 {
        return truncate(text, max_len);
    }

    Cow::Owned(format!("{}...", text[..end].trim()))
}

/// Wrap text at specified width.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();

    for paragraph in text.split('\n') {
        let words: Vec<&str> = paragraph.split_whitespace().collect();
        let mut current_line = String::new();

        for word in words {
            if current_line.is_empty() {
                current_line = word.to_string();
            } else if current_line.len() + 1 + word.len() <= width {
                current_line.push(' ');
                current_line.push_str(word);
            } else {
                lines.push(current_line);
                current_line = word.to_string();
            }
        }

        if !current_line.is_empty() {
            lines.push(current_line);
        } else if paragraph.is_empty() {
            lines.push(String::new());
        }
    }

    lines
}

/// Indent text.
pub fn indent(text: &str, prefix: &str) -> String {
    text.lines()
        .map(|line| format!("{prefix}{line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Dedent text (remove common indentation).
pub fn dedent(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();

    // Find minimum indentation (ignoring empty lines)
    let min_indent = lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);

    lines
        .iter()
        .map(|line| {
            if line.len() >= min_indent {
                &line[min_indent..]
            } else {
                *line
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Normalize whitespace.
pub fn normalize_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Strip HTML tags (simple).
pub fn strip_html(text: &str) -> String {
    let mut result = String::new();
    let mut in_tag = false;

    for c in text.chars() {
        if c == '<' {
            in_tag = true;
        } else if c == '>' {
            in_tag = false;
        } else if !in_tag {
            result.push(c);
        }
    }

    result
}

/// Extract URLs from text.
pub fn extract_urls(text: &str) -> Vec<String> {
    let mut urls = Vec::new();
    let mut current_url = String::new();
    let mut in_url = false;

    for c in text.chars() {
        if !in_url {
            if current_url.ends_with("http://") || current_url.ends_with("https://") {
                in_url = true;
            }
            current_url.push(c);
        } else if c.is_whitespace() || c == '"' || c == '\'' || c == '>' || c == ')' {
            urls.push(current_url.clone());
            current_url.clear();
            in_url = false;
        } else {
            current_url.push(c);
        }
    }

    if in_url && !current_url.is_empty() {
        urls.push(current_url);
    }

    urls
}

/// Capitalize first letter.
pub fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Title case.
pub fn title_case(text: &str) -> String {
    text.split_whitespace()
        .map(capitalize)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Slugify text.
pub fn slugify(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

/// Convert to snake_case.
pub fn to_snake_case(text: &str) -> String {
    let mut result = String::new();

    for (i, c) in text.chars().enumerate() {
        if c.is_uppercase() {
            if i > 0 {
                result.push('_');
            }
            result.push(c.to_ascii_lowercase());
        } else if c.is_alphanumeric() {
            result.push(c);
        } else {
            result.push('_');
        }
    }

    result
        .split('_')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("_")
}

/// Convert to camelCase.
pub fn to_camel_case(text: &str) -> String {
    let mut result = String::new();
    let mut capitalize_next = false;

    for c in text.chars() {
        if c == '_' || c == '-' || c == ' ' {
            capitalize_next = true;
        } else if capitalize_next {
            result.push(c.to_ascii_uppercase());
            capitalize_next = false;
        } else {
            result.push(c.to_ascii_lowercase());
        }
    }

    result
}

/// Convert to PascalCase.
pub fn to_pascal_case(text: &str) -> String {
    capitalize(&to_camel_case(text))
}

/// Escape special characters.
pub fn escape(text: &str, chars: &[char]) -> String {
    let mut result = String::new();

    for c in text.chars() {
        if chars.contains(&c) {
            result.push('\\');
        }
        result.push(c);
    }

    result
}

/// Unescape special characters.
pub fn unescape(text: &str) -> String {
    let mut result = String::new();
    let mut chars = text.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(next) = chars.next() {
                result.push(next);
            } else {
                result.push(c);
            }
        } else {
            result.push(c);
        }
    }

    result
}

/// Highlight text (wrap matches in markers).
pub fn highlight(text: &str, pattern: &str, start: &str, end: &str) -> String {
    if pattern.is_empty() {
        return text.to_string();
    }

    text.replace(pattern, &format!("{start}{pattern}{end}"))
}

/// Count occurrences of pattern.
pub fn count_occurrences(text: &str, pattern: &str) -> usize {
    if pattern.is_empty() {
        return 0;
    }

    text.matches(pattern).count()
}

/// Find all indices of pattern.
pub fn find_indices(text: &str, pattern: &str) -> Vec<usize> {
    if pattern.is_empty() {
        return vec![];
    }

    text.match_indices(pattern).map(|(i, _)| i).collect()
}

/// Replace nth occurrence.
pub fn replace_nth(text: &str, pattern: &str, replacement: &str, n: usize) -> String {
    let indices = find_indices(text, pattern);

    if n >= indices.len() {
        return text.to_string();
    }

    let idx = indices[n];
    format!(
        "{}{}{}",
        &text[..idx],
        replacement,
        &text[idx + pattern.len()..]
    )
}

/// Reverse string.
pub fn reverse(text: &str) -> String {
    text.chars().rev().collect()
}

/// Check if text is palindrome.
pub fn is_palindrome(text: &str) -> bool {
    let cleaned: String = text
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect();

    cleaned == cleaned.chars().rev().collect::<String>()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_text_stats() {
        let text = "Hello world. This is a test.";
        let stats = TextStats::compute(text);

        assert_eq!(stats.words, 6);
        assert_eq!(stats.sentences, 2);
    }

    #[test]
    fn test_word_frequency() {
        let text = "hello world hello";
        let freq = word_frequency(text);

        assert_eq!(freq.get("hello"), Some(&2));
        assert_eq!(freq.get("world"), Some(&1));
    }

    #[test]
    fn test_levenshtein() {
        assert_eq!(levenshtein_distance("kitten", "sitting"), 3);
        assert_eq!(levenshtein_distance("", "abc"), 3);
        assert_eq!(levenshtein_distance("abc", "abc"), 0);
    }

    #[test]
    fn test_similarity() {
        assert!(similarity_ratio("hello", "hello") > 0.99);
        assert!(similarity_ratio("hello", "helo") > 0.7);
    }

    #[test]
    fn test_truncate() {
        assert_eq!(truncate("hello world", 8), "hello...");
        assert_eq!(truncate("short", 10), "short");
    }

    #[test]
    fn test_wrap() {
        let text = "hello world this is a test";
        let lines = wrap(text, 10);

        assert!(lines.len() > 1);
        assert!(lines.iter().all(|l| l.len() <= 10));
    }

    #[test]
    fn test_indent_dedent() {
        let text = "hello\nworld";
        let indented = indent(text, "  ");

        assert!(indented.starts_with("  "));
        assert_eq!(dedent(&indented), text);
    }

    #[test]
    fn test_case_conversions() {
        assert_eq!(to_snake_case("helloWorld"), "hello_world");
        assert_eq!(to_camel_case("hello_world"), "helloWorld");
        assert_eq!(to_pascal_case("hello_world"), "HelloWorld");
    }

    #[test]
    fn test_slugify() {
        assert_eq!(slugify("Hello World!"), "hello-world");
        assert_eq!(slugify("  Multiple   Spaces  "), "multiple-spaces");
    }

    #[test]
    fn test_strip_html() {
        assert_eq!(strip_html("<p>Hello</p>"), "Hello");
        assert_eq!(strip_html("No <b>tags</b> here"), "No tags here");
    }

    #[test]
    fn test_highlight() {
        assert_eq!(highlight("hello world", "world", "[", "]"), "hello [world]");
    }

    #[test]
    fn test_count_occurrences() {
        assert_eq!(count_occurrences("ababa", "aba"), 1);
        assert_eq!(count_occurrences("hello", "l"), 2);
    }

    #[test]
    fn test_palindrome() {
        assert!(is_palindrome("A man a plan a canal Panama"));
        assert!(!is_palindrome("hello"));
    }

    #[test]
    fn test_reverse() {
        assert_eq!(reverse("hello"), "olleh");
    }
}
