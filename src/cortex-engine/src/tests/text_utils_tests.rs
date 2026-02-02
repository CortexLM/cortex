//! Tests for text_utils module.

use crate::text_utils::*;

#[test]
fn test_text_stats_compute() {
    let text = "Hello world. This is a test.";
    let stats = TextStats::compute(text);

    assert_eq!(stats.words, 6);
    assert_eq!(stats.sentences, 2);
    assert!(stats.chars > 0);
}

#[test]
fn test_text_stats_empty() {
    let stats = TextStats::compute("");
    assert_eq!(stats.words, 0);
    assert_eq!(stats.chars, 0);
}

#[test]
fn test_text_stats_multiline() {
    let text = "Line 1.\nLine 2.\nLine 3.";
    let stats = TextStats::compute(text);

    assert_eq!(stats.lines, 3);
    assert_eq!(stats.sentences, 3);
}

#[test]
fn test_word_frequency() {
    let text = "hello world hello";
    let freq = word_frequency(text);

    assert_eq!(freq.get("hello"), Some(&2));
    assert_eq!(freq.get("world"), Some(&1));
}

#[test]
fn test_word_frequency_empty() {
    let freq = word_frequency("");
    assert!(freq.is_empty());
}

#[test]
fn test_most_common_words() {
    let text = "the quick brown fox jumps over the lazy dog the";
    let common = most_common_words(text, 2);

    assert_eq!(common.len(), 2);
    assert_eq!(common[0].0, "the");
    assert_eq!(common[0].1, 3);
}

#[test]
fn test_levenshtein_distance() {
    assert_eq!(levenshtein_distance("kitten", "sitting"), 3);
    assert_eq!(levenshtein_distance("", "abc"), 3);
    assert_eq!(levenshtein_distance("abc", "abc"), 0);
}

#[test]
fn test_levenshtein_distance_empty() {
    assert_eq!(levenshtein_distance("", ""), 0);
    assert_eq!(levenshtein_distance("abc", ""), 3);
}

#[test]
fn test_similarity_ratio() {
    assert!(similarity_ratio("hello", "hello") > 0.99);
    assert!(similarity_ratio("hello", "helo") > 0.7);
    assert!(similarity_ratio("abc", "xyz") < 0.5);
}

#[test]
fn test_similarity_ratio_empty() {
    assert!((similarity_ratio("", "") - 1.0).abs() < 0.01);
}

#[test]
fn test_truncate() {
    assert_eq!(truncate("hello world", 8).as_ref(), "hello...");
    assert_eq!(truncate("short", 10).as_ref(), "short");
}

#[test]
fn test_truncate_exact() {
    let result = truncate("12345", 5);
    assert_eq!(result.as_ref(), "12345");
}

#[test]
fn test_truncate_words() {
    let result = truncate_words("hello beautiful world", 12);
    assert!(result.ends_with("..."));
}

#[test]
fn test_wrap() {
    let text = "hello world this is a test";
    let lines = wrap(text, 10);

    assert!(lines.len() > 1);
}

#[test]
fn test_wrap_single_line() {
    let lines = wrap("short", 20);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0], "short");
}

#[test]
fn test_indent() {
    let text = "hello\nworld";
    let indented = indent(text, "  ");

    assert!(indented.starts_with("  "));
    assert!(indented.contains("\n  "));
}

#[test]
fn test_dedent() {
    let text = "  hello\n  world";
    let result = dedent(text);

    assert!(result.starts_with("hello"));
}

#[test]
fn test_normalize_whitespace() {
    let text = "hello   world\n\ttab";
    let result = normalize_whitespace(text);

    assert_eq!(result, "hello world tab");
}

#[test]
fn test_strip_html() {
    assert_eq!(strip_html("<p>Hello</p>"), "Hello");
    assert_eq!(strip_html("No <b>tags</b> here"), "No tags here");
}

#[test]
fn test_strip_html_nested() {
    assert_eq!(strip_html("<div><p>Nested</p></div>"), "Nested");
}

#[test]
fn test_capitalize() {
    assert_eq!(capitalize("hello"), "Hello");
    assert_eq!(capitalize("HELLO"), "HELLO");
    assert_eq!(capitalize(""), "");
}

#[test]
fn test_title_case() {
    assert_eq!(title_case("hello world"), "Hello World");
}

#[test]
fn test_slugify() {
    assert_eq!(slugify("Hello World!"), "hello-world");
    assert_eq!(slugify("  Multiple   Spaces  "), "multiple-spaces");
}

#[test]
fn test_to_snake_case() {
    assert_eq!(to_snake_case("helloWorld"), "hello_world");
    assert_eq!(to_snake_case("HelloWorld"), "hello_world");
}

#[test]
fn test_to_camel_case() {
    assert_eq!(to_camel_case("hello_world"), "helloWorld");
    assert_eq!(to_camel_case("hello-world"), "helloWorld");
}

#[test]
fn test_to_pascal_case() {
    assert_eq!(to_pascal_case("hello_world"), "HelloWorld");
}

#[test]
fn test_escape() {
    let result = escape("hello'world", &['\'']);
    assert_eq!(result, "hello\\'world");
}

#[test]
fn test_unescape() {
    let result = unescape("hello\\'world");
    assert_eq!(result, "hello'world");
}

#[test]
fn test_highlight() {
    assert_eq!(highlight("hello world", "world", "[", "]"), "hello [world]");
}

#[test]
fn test_highlight_empty_pattern() {
    assert_eq!(highlight("hello", "", "[", "]"), "hello");
}

#[test]
fn test_count_occurrences() {
    assert_eq!(count_occurrences("ababa", "aba"), 1);
    assert_eq!(count_occurrences("hello", "l"), 2);
}

#[test]
fn test_count_occurrences_empty() {
    assert_eq!(count_occurrences("hello", ""), 0);
}

#[test]
fn test_find_indices() {
    let indices = find_indices("hello", "l");
    assert_eq!(indices, vec![2, 3]);
}

#[test]
fn test_find_indices_empty() {
    assert!(find_indices("hello", "").is_empty());
}

#[test]
fn test_replace_nth() {
    let result = replace_nth("ababa", "a", "X", 1);
    assert_eq!(result, "abXba");
}

#[test]
fn test_replace_nth_out_of_bounds() {
    let result = replace_nth("hello", "l", "X", 10);
    assert_eq!(result, "hello");
}

#[test]
fn test_reverse() {
    assert_eq!(reverse("hello"), "olleh");
    assert_eq!(reverse(""), "");
}

#[test]
fn test_is_palindrome() {
    assert!(is_palindrome("A man a plan a canal Panama"));
    assert!(!is_palindrome("hello"));
    assert!(is_palindrome("racecar"));
}

#[test]
fn test_extract_urls() {
    let text = "Check out https://example.com for more info";
    let urls = extract_urls(text);
    assert!(urls.iter().any(|u| u.contains("example.com")));
}

#[test]
fn test_text_stats_default() {
    let stats = TextStats::default();
    assert_eq!(stats.words, 0);
    assert_eq!(stats.chars, 0);
}
