//! Comprehensive tests for fuzzy_match module.

use crate::fuzzy_match::*;

#[test]
fn test_fuzzy_score_exact_match() {
    assert_eq!(fuzzy_score("test", "test"), 100);
    assert_eq!(fuzzy_score("hello", "hello"), 100);
    assert_eq!(fuzzy_score("a", "a"), 100);
}

#[test]
fn test_fuzzy_score_case_insensitive() {
    assert_eq!(fuzzy_score("TEST", "test"), 100);
    assert_eq!(fuzzy_score("test", "TEST"), 100);
    assert_eq!(fuzzy_score("TeSt", "tEsT"), 100);
}

#[test]
fn test_fuzzy_score_empty_pattern() {
    assert_eq!(fuzzy_score("", "test"), 100);
    assert_eq!(fuzzy_score("", "anything"), 100);
    assert_eq!(fuzzy_score("", ""), 100);
}

#[test]
fn test_fuzzy_score_empty_text() {
    assert_eq!(fuzzy_score("test", ""), 0);
    assert_eq!(fuzzy_score("a", ""), 0);
}

#[test]
fn test_fuzzy_score_starts_with() {
    let score = fuzzy_score("te", "test");
    assert!(score >= 90, "starts_with should score >= 90, got {}", score);

    let score = fuzzy_score("hel", "hello world");
    assert!(score >= 90);
}

#[test]
fn test_fuzzy_score_contains() {
    let score = fuzzy_score("es", "test");
    assert!(score >= 70, "contains should score >= 70, got {}", score);

    let score = fuzzy_score("wor", "hello world");
    assert!(score >= 70);
}

#[test]
fn test_fuzzy_score_no_match() {
    assert_eq!(fuzzy_score("xyz", "test"), 0);
    assert_eq!(fuzzy_score("abc", "def"), 0);
    assert_eq!(fuzzy_score("zzz", "aaa"), 0);
}

#[test]
fn test_fuzzy_score_partial_match_in_order() {
    // Characters found in order
    let score = fuzzy_score("tst", "test");
    assert!(score > 0, "partial in-order match should score > 0");
    assert!(score < 70, "should be less than contains score");
}

#[test]
fn test_fuzzy_score_partial_match_out_of_order() {
    // Characters out of order should score 0
    let _score = fuzzy_score("set", "test");
    // 's' before 'e' before 't' - not in order in "test"
    // Actually "test" has 't' 'e' 's' 't', so 'set' matches!
    // Let's use a better example
    let score = fuzzy_score("dcba", "abcd");
    assert_eq!(score, 0, "out of order should not match");
}

#[test]
fn test_fuzzy_score_scoring_comparison() {
    // Exact > starts_with > contains > fuzzy
    let exact = fuzzy_score("test", "test");
    let starts = fuzzy_score("te", "test");
    let contains = fuzzy_score("es", "test");
    let _fuzzy = fuzzy_score("tt", "test");

    assert!(
        exact > starts,
        "exact ({}) should be > starts ({})",
        exact,
        starts
    );
    assert!(
        starts > contains,
        "starts ({}) should be > contains ({})",
        starts,
        contains
    );
    // Fuzzy might be 0 or low
}

#[test]
fn test_fuzzy_filter_basic() {
    let items = vec!["apple", "application", "banana", "apply"];
    let results = fuzzy_filter(&items, "app", |s| s);

    assert!(!results.is_empty());

    // All results should be for items containing "app"
    for (idx, score) in &results {
        assert!(items[*idx].contains("app") || score > &0);
    }
}

#[test]
fn test_fuzzy_filter_empty_pattern() {
    let items = vec!["a", "b", "c"];
    let results = fuzzy_filter(&items, "", |s| s);

    // Empty pattern matches everything with score 100
    assert_eq!(results.len(), 3);
    for (_, score) in results {
        assert_eq!(score, 100);
    }
}

#[test]
fn test_fuzzy_filter_no_matches() {
    let items = vec!["apple", "banana", "cherry"];
    let results = fuzzy_filter(&items, "xyz", |s| s);

    assert!(results.is_empty());
}

#[test]
fn test_fuzzy_filter_sorted_by_score() {
    let items = vec!["application", "app", "apple", "ape"];
    let results = fuzzy_filter(&items, "app", |s| s);

    // Results should be sorted by score descending
    for i in 1..results.len() {
        assert!(
            results[i - 1].1 >= results[i].1,
            "Results should be sorted by score"
        );
    }
}

#[test]
fn test_fuzzy_filter_with_custom_getter() {
    #[derive(Debug)]
    struct Item {
        name: String,
        value: i32,
    }

    let items = vec![
        Item {
            name: "first".to_string(),
            value: 1,
        },
        Item {
            name: "second".to_string(),
            value: 2,
        },
        Item {
            name: "fire".to_string(),
            value: 3,
        },
    ];

    let results = fuzzy_filter(&items, "fir", |item| &item.name);

    assert!(!results.is_empty());
    // "first" and "fire" should match
    for (idx, _) in &results {
        assert!(items[*idx].name.starts_with("fir"));
    }
}

#[test]
fn test_fuzzy_score_unicode() {
    // Unicode should work
    let score = fuzzy_score("世界", "世界");
    assert_eq!(score, 100);

    let score = fuzzy_score("界", "世界");
    assert!(score >= 70); // contains
}

#[test]
fn test_fuzzy_score_special_characters() {
    assert_eq!(fuzzy_score("test.rs", "test.rs"), 100);
    assert_eq!(fuzzy_score("foo_bar", "foo_bar"), 100);
    assert_eq!(fuzzy_score("foo-bar", "foo-bar"), 100);
}

#[test]
fn test_fuzzy_score_long_strings() {
    let long_text = "a".repeat(1000);
    let score = fuzzy_score("aaa", &long_text);
    assert!(score > 0);
}

#[test]
fn test_fuzzy_filter_preserves_indices() {
    let items = vec!["z", "a", "m", "apple"];
    let results = fuzzy_filter(&items, "ap", |s| s);

    // Only "apple" should match
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].0, 3); // Index of "apple"
}

#[test]
fn test_fuzzy_score_single_char() {
    assert_eq!(fuzzy_score("a", "a"), 100);
    assert!(fuzzy_score("a", "abc") >= 90); // starts with
    assert!(fuzzy_score("b", "abc") >= 70); // contains
    assert_eq!(fuzzy_score("z", "abc"), 0); // not found
}

#[test]
fn test_fuzzy_score_whitespace() {
    let score = fuzzy_score("hello world", "hello world");
    assert_eq!(score, 100);

    let score = fuzzy_score("hello", "hello world");
    assert!(score >= 90); // starts with
}

#[test]
fn test_fuzzy_filter_empty_items() {
    let items: Vec<&str> = vec![];
    let results = fuzzy_filter(&items, "test", |s| s);
    assert!(results.is_empty());
}
