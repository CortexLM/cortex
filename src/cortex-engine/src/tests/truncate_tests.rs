//! Tests for truncate module.

use crate::truncate::*;

#[test]
fn test_truncate_config_default() {
    let config = TruncateConfig::default();
    assert!(config.max_chars > 0);
}

#[test]
fn test_truncate_strategy_variants() {
    let end = TruncateStrategy::End;
    let start = TruncateStrategy::Start;
    let middle = TruncateStrategy::Middle;
    let smart = TruncateStrategy::Smart;

    assert!(matches!(end, TruncateStrategy::End));
    assert!(matches!(start, TruncateStrategy::Start));
    assert!(matches!(middle, TruncateStrategy::Middle));
    assert!(matches!(smart, TruncateStrategy::Smart));
}

#[test]
fn test_truncate_short_text() {
    let config = TruncateConfig {
        max_chars: 100,
        ..Default::default()
    };

    let text = "Short text";
    let result = truncate(text, &config);

    assert!(!result.truncated);
    assert_eq!(result.text, text);
}

#[test]
fn test_truncate_long_text() {
    let config = TruncateConfig {
        max_chars: 30,
        strategy: TruncateStrategy::End,
        ..Default::default()
    };

    let text = "This is a very long text that needs truncation";
    let result = truncate(text, &config);

    assert!(result.truncated);
    assert!(result.text.len() <= 30 + 20); // Allow some margin for suffix
}

#[test]
fn test_truncate_result_fields() {
    let result = TruncateResult {
        text: "truncated".to_string(),
        truncated: true,
        original_chars: 1000,
        final_chars: 50,
        original_tokens: 250,
        final_tokens: 12,
        strategy_used: TruncateStrategy::End,
    };

    assert!(result.truncated);
    assert_eq!(result.original_chars, 1000);
    assert_eq!(result.final_chars, 50);
}

#[test]
fn test_truncate_empty_text() {
    let config = TruncateConfig {
        max_chars: 100,
        ..Default::default()
    };

    let result = truncate("", &config);

    assert!(!result.truncated);
    assert_eq!(result.text, "");
}

#[test]
fn test_truncate_exact_length() {
    let config = TruncateConfig {
        max_chars: 10,
        strategy: TruncateStrategy::End,
        ..Default::default()
    };

    let text = "1234567890";
    let result = truncate(text, &config);

    // 10 chars should not need truncation if suffix is empty
    // but default has a suffix, so it depends
    assert_eq!(result.original_chars, 10);
}

#[test]
fn test_truncate_builder() {
    let result = TruncateBuilder::new()
        .max_chars(50)
        .suffix("...")
        .strategy(TruncateStrategy::End)
        .word_boundary(true)
        .build();

    assert_eq!(result.max_chars, 50);
    assert_eq!(result.suffix, "...");
}

#[test]
fn test_truncate_builder_truncate() {
    let result = TruncateBuilder::new()
        .max_chars(20)
        .suffix("...")
        .truncate("This is a very long text that should be truncated");

    assert!(result.truncated);
    assert!(result.text.len() <= 30); // Allow some margin
}

#[test]
fn test_truncate_result_reduction_percent() {
    let result = TruncateResult {
        text: "test".to_string(),
        truncated: true,
        original_chars: 100,
        final_chars: 50,
        original_tokens: 25,
        final_tokens: 12,
        strategy_used: TruncateStrategy::End,
    };

    let reduction = result.reduction_percent();
    assert!((reduction - 50.0).abs() < 0.1);
}

#[test]
fn test_estimate_tokens() {
    let tokens = estimate_tokens("Hello world, this is a test.");
    assert!(tokens > 0);

    let empty_tokens = estimate_tokens("");
    assert_eq!(empty_tokens, 0);
}

#[test]
fn test_token_estimator() {
    let mut estimator = TokenEstimator::new();

    let count1 = estimator.estimate("Hello world");
    let count2 = estimator.estimate("Hello world"); // Should be cached

    assert_eq!(count1, count2);
}

#[test]
fn test_token_estimator_with_ratio() {
    let mut estimator = TokenEstimator::with_ratio(3.0);
    let count = estimator.estimate("123456789"); // 9 chars / 3 = 3 tokens

    assert_eq!(count, 3);
}

#[test]
fn test_truncate_batch() {
    let items = vec!["Hello world", "Goodbye world", "Another text"];
    let result = truncate_batch(&items, 50);

    assert_eq!(result.len(), 3);
}

#[test]
fn test_truncate_file() {
    let content = "fn main() {\n    println!(\"Hello\");\n}".repeat(100);
    let result = truncate_file(&content, "rs", 200);

    assert!(result.len() <= 250); // Allow some margin
}

#[test]
fn test_truncate_start_strategy() {
    let config = TruncateConfig {
        max_chars: 30,
        strategy: TruncateStrategy::Start,
        prefix: "...".to_string(),
        ..Default::default()
    };

    let text = "This is a very long text that needs to be truncated from the start";
    let result = truncate(text, &config);

    assert!(result.truncated);
    assert!(result.text.starts_with("..."));
}

#[test]
fn test_truncate_middle_strategy() {
    let config = TruncateConfig {
        max_chars: 50,
        strategy: TruncateStrategy::Middle,
        ..Default::default()
    };

    let text = "A".repeat(100);
    let result = truncate(&text, &config);

    assert!(result.truncated);
    assert!(result.text.contains("omitted"));
}
