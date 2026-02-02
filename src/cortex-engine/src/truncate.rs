//! Text truncation and summarization utilities.
//!
//! Provides intelligent truncation of text content for context management,
//! supporting various strategies and format preservation.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Truncation configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TruncateConfig {
    /// Maximum length in characters.
    pub max_chars: usize,
    /// Maximum length in tokens (approximate).
    pub max_tokens: Option<usize>,
    /// Truncation strategy.
    pub strategy: TruncateStrategy,
    /// Suffix to add when truncated.
    pub suffix: String,
    /// Prefix to add when truncated.
    pub prefix: String,
    /// Preserve code blocks.
    pub preserve_code: bool,
    /// Preserve markdown structure.
    pub preserve_markdown: bool,
    /// Word boundary alignment.
    pub word_boundary: bool,
    /// Sentence boundary alignment.
    pub sentence_boundary: bool,
    /// Line boundary alignment.
    pub line_boundary: bool,
}

impl Default for TruncateConfig {
    fn default() -> Self {
        Self {
            max_chars: 10000,
            max_tokens: None,
            strategy: TruncateStrategy::End,
            suffix: "... [truncated]".to_string(),
            prefix: String::new(),
            preserve_code: true,
            preserve_markdown: true,
            word_boundary: true,
            sentence_boundary: false,
            line_boundary: false,
        }
    }
}

/// Truncation strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TruncateStrategy {
    /// Truncate from the end (keep beginning).
    End,
    /// Truncate from the beginning (keep end).
    Start,
    /// Keep both ends, remove middle.
    Middle,
    /// Smart truncation based on content analysis.
    Smart,
    /// Summarize instead of truncate.
    Summarize,
}

/// Truncate text according to configuration.
pub fn truncate(text: &str, config: &TruncateConfig) -> TruncateResult {
    let original_len = text.len();
    let original_tokens = estimate_tokens(text);

    // Check if truncation needed
    let needs_truncation = original_len > config.max_chars
        || config
            .max_tokens
            .map(|t| original_tokens > t)
            .unwrap_or(false);

    if !needs_truncation {
        return TruncateResult {
            text: text.to_string(),
            truncated: false,
            original_chars: original_len,
            final_chars: original_len,
            original_tokens,
            final_tokens: original_tokens,
            strategy_used: config.strategy,
        };
    }

    let truncated_text = match config.strategy {
        TruncateStrategy::End => truncate_end(text, config),
        TruncateStrategy::Start => truncate_start(text, config),
        TruncateStrategy::Middle => truncate_middle(text, config),
        TruncateStrategy::Smart => truncate_smart(text, config),
        TruncateStrategy::Summarize => truncate_summarize(text, config),
    };

    let final_len = truncated_text.len();
    let final_tokens = estimate_tokens(&truncated_text);

    TruncateResult {
        text: truncated_text,
        truncated: true,
        original_chars: original_len,
        final_chars: final_len,
        original_tokens,
        final_tokens,
        strategy_used: config.strategy,
    }
}

/// Simple truncation from end.
fn truncate_end(text: &str, config: &TruncateConfig) -> String {
    let target_len = config.max_chars.saturating_sub(config.suffix.len());

    if text.len() <= target_len {
        return text.to_string();
    }

    let mut end = target_len;

    // Align to boundary
    if config.sentence_boundary {
        end = find_sentence_boundary(text, end, false);
    } else if config.line_boundary {
        end = find_line_boundary(text, end, false);
    } else if config.word_boundary {
        end = find_word_boundary(text, end, false);
    }

    format!("{}{}", &text[..end], config.suffix)
}

/// Truncation from start (keep end).
fn truncate_start(text: &str, config: &TruncateConfig) -> String {
    let target_len = config.max_chars.saturating_sub(config.prefix.len());

    if text.len() <= target_len {
        return text.to_string();
    }

    let mut start = text.len() - target_len;

    // Align to boundary
    if config.sentence_boundary {
        start = find_sentence_boundary(text, start, true);
    } else if config.line_boundary {
        start = find_line_boundary(text, start, true);
    } else if config.word_boundary {
        start = find_word_boundary(text, start, true);
    }

    format!("{}{}", config.prefix, &text[start..])
}

/// Truncation from middle (keep both ends).
fn truncate_middle(text: &str, config: &TruncateConfig) -> String {
    let separator = "\n\n[...content omitted...]\n\n";
    let target_len = config.max_chars.saturating_sub(separator.len());

    if text.len() <= target_len {
        return text.to_string();
    }

    let keep_each = target_len / 2;
    let start_end = find_boundary(text, keep_each, true, config);
    let end_start = text.len() - find_boundary(text, keep_each, false, config);

    format!("{}{}{}", &text[..start_end], separator, &text[end_start..])
}

/// Smart truncation based on content analysis.
fn truncate_smart(text: &str, config: &TruncateConfig) -> String {
    // Analyze content structure
    let has_code = text.contains("```") || text.contains("    ");
    let has_lists = text.contains("\n- ") || text.contains("\n* ") || text.contains("\n1.");
    let has_headers = text.contains("\n#") || text.contains("\n==");

    // Choose strategy based on content
    if has_code && config.preserve_code {
        truncate_preserve_code(text, config)
    } else if has_lists || has_headers {
        truncate_preserve_structure(text, config)
    } else {
        truncate_end(text, config)
    }
}

/// Truncate while preserving code blocks.
fn truncate_preserve_code(text: &str, config: &TruncateConfig) -> String {
    let mut result = String::new();
    let mut remaining = config.max_chars;
    let mut in_code_block = false;
    let mut code_block_content = String::new();

    for line in text.lines() {
        if line.starts_with("```") {
            if in_code_block {
                // End of code block - add it if it fits
                code_block_content.push_str(line);
                code_block_content.push('\n');

                if code_block_content.len() <= remaining {
                    result.push_str(&code_block_content);
                    remaining -= code_block_content.len();
                }
                code_block_content.clear();
                in_code_block = false;
            } else {
                // Start of code block
                in_code_block = true;
                code_block_content.push_str(line);
                code_block_content.push('\n');
            }
        } else if in_code_block {
            code_block_content.push_str(line);
            code_block_content.push('\n');
        } else {
            let line_len = line.len() + 1;
            if line_len <= remaining {
                result.push_str(line);
                result.push('\n');
                remaining -= line_len;
            } else {
                break;
            }
        }
    }

    if result.len() < text.len() {
        result.push_str(&config.suffix);
    }

    result
}

/// Truncate while preserving document structure.
fn truncate_preserve_structure(text: &str, config: &TruncateConfig) -> String {
    let mut result = String::new();
    let mut remaining = config.max_chars.saturating_sub(config.suffix.len());
    let mut current_section = String::new();
    let mut section_header = String::new();

    for line in text.lines() {
        let is_header = line.starts_with('#') || line.starts_with("==") || line.starts_with("--");

        if is_header {
            // Flush previous section
            if !current_section.is_empty() && current_section.len() <= remaining {
                result.push_str(&section_header);
                result.push_str(&current_section);
                remaining -= section_header.len() + current_section.len();
            }
            section_header = format!("{line}\n");
            current_section.clear();
        } else {
            current_section.push_str(line);
            current_section.push('\n');
        }

        if remaining == 0 {
            break;
        }
    }

    // Add last section if it fits
    if !current_section.is_empty() && section_header.len() + current_section.len() <= remaining {
        result.push_str(&section_header);
        result.push_str(&current_section);
    }

    if result.len() < text.len() {
        result.push_str(&config.suffix);
    }

    result
}

/// Summarize instead of truncate (placeholder).
fn truncate_summarize(text: &str, config: &TruncateConfig) -> String {
    // In a real implementation, this would use an LLM to summarize
    // For now, fall back to smart truncation
    truncate_smart(text, config)
}

/// Find appropriate boundary position.
fn find_boundary(text: &str, pos: usize, forward: bool, config: &TruncateConfig) -> usize {
    if config.sentence_boundary {
        find_sentence_boundary(text, pos, forward)
    } else if config.line_boundary {
        find_line_boundary(text, pos, forward)
    } else if config.word_boundary {
        find_word_boundary(text, pos, forward)
    } else {
        pos.min(text.len())
    }
}

/// Find word boundary near position.
fn find_word_boundary(text: &str, pos: usize, forward: bool) -> usize {
    if pos >= text.len() {
        return text.len();
    }

    let bytes = text.as_bytes();

    if forward {
        // Search forward for space or punctuation
        for i in pos..text.len().min(pos + 50) {
            if bytes[i] == b' ' || bytes[i] == b'\n' {
                return i;
            }
        }
        // Search backward if nothing found
        for i in (pos.saturating_sub(50)..pos).rev() {
            if bytes[i] == b' ' || bytes[i] == b'\n' {
                return i + 1;
            }
        }
    } else {
        // Search backward
        for i in (pos.saturating_sub(50)..pos).rev() {
            if bytes[i] == b' ' || bytes[i] == b'\n' {
                return i + 1;
            }
        }
    }

    pos
}

/// Find sentence boundary near position.
fn find_sentence_boundary(text: &str, pos: usize, forward: bool) -> usize {
    let sentence_ends = [". ", "! ", "? ", ".\n", "!\n", "?\n"];

    if forward {
        for i in pos..text.len().min(pos + 200) {
            for end in &sentence_ends {
                if text[i..].starts_with(end) {
                    return i + end.len();
                }
            }
        }
    } else {
        for i in (pos.saturating_sub(200)..pos).rev() {
            for end in &sentence_ends {
                if text[i..].starts_with(end) {
                    return i + end.len();
                }
            }
        }
    }

    find_word_boundary(text, pos, forward)
}

/// Find line boundary near position.
fn find_line_boundary(text: &str, pos: usize, forward: bool) -> usize {
    if forward {
        text[pos..].find('\n').map(|i| pos + i + 1).unwrap_or(pos)
    } else {
        text[..pos].rfind('\n').map(|i| i + 1).unwrap_or(0)
    }
}

/// Estimate token count (rough approximation).
pub fn estimate_tokens(text: &str) -> usize {
    // Rough estimate: ~4 chars per token for English
    // This is a simplification - real tokenization varies by model
    let char_count = text.chars().count();
    let word_count = text.split_whitespace().count();

    // Average of character-based and word-based estimates
    (char_count / 4 + word_count) / 2
}

/// More accurate token estimation with caching.
pub struct TokenEstimator {
    /// Cache of token counts.
    cache: HashMap<u64, usize>,
    /// Chars per token ratio.
    chars_per_token: f64,
}

impl TokenEstimator {
    /// Create a new estimator.
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
            chars_per_token: 4.0,
        }
    }

    /// Create with custom ratio.
    pub fn with_ratio(chars_per_token: f64) -> Self {
        Self {
            cache: HashMap::new(),
            chars_per_token,
        }
    }

    /// Estimate tokens for text.
    pub fn estimate(&mut self, text: &str) -> usize {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        text.hash(&mut hasher);
        let hash = hasher.finish();

        if let Some(&count) = self.cache.get(&hash) {
            return count;
        }

        let estimate = self.calculate(text);

        // Cache if not too large
        if self.cache.len() < 10000 {
            self.cache.insert(hash, estimate);
        }

        estimate
    }

    /// Calculate token estimate.
    fn calculate(&self, text: &str) -> usize {
        let char_count = text.chars().count() as f64;
        (char_count / self.chars_per_token).ceil() as usize
    }

    /// Calibrate ratio based on actual token counts.
    pub fn calibrate(&mut self, samples: &[(String, usize)]) {
        if samples.is_empty() {
            return;
        }

        let total_chars: f64 = samples
            .iter()
            .map(|(text, _)| text.chars().count() as f64)
            .sum();
        let total_tokens: f64 = samples.iter().map(|(_, tokens)| *tokens as f64).sum();

        if total_tokens > 0.0 {
            self.chars_per_token = total_chars / total_tokens;
        }
    }

    /// Clear cache.
    pub fn clear_cache(&mut self) {
        self.cache.clear();
    }
}

impl Default for TokenEstimator {
    fn default() -> Self {
        Self::new()
    }
}

/// Truncation result.
#[derive(Debug, Clone)]
pub struct TruncateResult {
    /// Resulting text.
    pub text: String,
    /// Whether truncation occurred.
    pub truncated: bool,
    /// Original character count.
    pub original_chars: usize,
    /// Final character count.
    pub final_chars: usize,
    /// Original estimated tokens.
    pub original_tokens: usize,
    /// Final estimated tokens.
    pub final_tokens: usize,
    /// Strategy that was used.
    pub strategy_used: TruncateStrategy,
}

impl TruncateResult {
    /// Get reduction percentage.
    pub fn reduction_percent(&self) -> f64 {
        if self.original_chars == 0 {
            return 0.0;
        }
        (1.0 - (self.final_chars as f64 / self.original_chars as f64)) * 100.0
    }

    /// Check if truncation was successful.
    pub fn is_ok(&self) -> bool {
        !self.text.is_empty()
    }
}

/// Truncate file content intelligently.
pub fn truncate_file(content: &str, file_type: &str, max_chars: usize) -> String {
    let config = TruncateConfig {
        max_chars,
        preserve_code: matches!(file_type, "rs" | "py" | "js" | "ts" | "go" | "c" | "cpp"),
        preserve_markdown: matches!(file_type, "md" | "markdown"),
        strategy: TruncateStrategy::Smart,
        ..Default::default()
    };

    truncate(content, &config).text
}

/// Truncate multiple strings to fit total budget.
pub fn truncate_batch(items: &[&str], total_chars: usize) -> Vec<String> {
    if items.is_empty() {
        return Vec::new();
    }

    let total_len: usize = items.iter().map(|s| s.len()).sum();

    if total_len <= total_chars {
        return items.iter().map(std::string::ToString::to_string).collect();
    }

    // Proportional allocation
    let ratio = total_chars as f64 / total_len as f64;

    items
        .iter()
        .map(|item| {
            let target = (item.len() as f64 * ratio) as usize;
            let config = TruncateConfig {
                max_chars: target,
                ..Default::default()
            };
            truncate(item, &config).text
        })
        .collect()
}

/// Builder for truncation configuration.
#[derive(Debug, Default)]
pub struct TruncateBuilder {
    config: TruncateConfig,
}

impl TruncateBuilder {
    /// Create a new builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set maximum characters.
    pub fn max_chars(mut self, max: usize) -> Self {
        self.config.max_chars = max;
        self
    }

    /// Set maximum tokens.
    pub fn max_tokens(mut self, max: usize) -> Self {
        self.config.max_tokens = Some(max);
        self
    }

    /// Set strategy.
    pub fn strategy(mut self, strategy: TruncateStrategy) -> Self {
        self.config.strategy = strategy;
        self
    }

    /// Set suffix.
    pub fn suffix(mut self, suffix: impl Into<String>) -> Self {
        self.config.suffix = suffix.into();
        self
    }

    /// Set prefix.
    pub fn prefix(mut self, prefix: impl Into<String>) -> Self {
        self.config.prefix = prefix.into();
        self
    }

    /// Enable word boundary alignment.
    pub fn word_boundary(mut self, enabled: bool) -> Self {
        self.config.word_boundary = enabled;
        self
    }

    /// Enable sentence boundary alignment.
    pub fn sentence_boundary(mut self, enabled: bool) -> Self {
        self.config.sentence_boundary = enabled;
        self
    }

    /// Enable line boundary alignment.
    pub fn line_boundary(mut self, enabled: bool) -> Self {
        self.config.line_boundary = enabled;
        self
    }

    /// Preserve code blocks.
    pub fn preserve_code(mut self, enabled: bool) -> Self {
        self.config.preserve_code = enabled;
        self
    }

    /// Preserve markdown structure.
    pub fn preserve_markdown(mut self, enabled: bool) -> Self {
        self.config.preserve_markdown = enabled;
        self
    }

    /// Build configuration.
    pub fn build(self) -> TruncateConfig {
        self.config
    }

    /// Truncate text with built configuration.
    pub fn truncate(self, text: &str) -> TruncateResult {
        truncate(text, &self.config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_truncation_needed() {
        let config = TruncateConfig {
            max_chars: 100,
            ..Default::default()
        };
        let result = truncate("Hello, world!", &config);
        assert!(!result.truncated);
        assert_eq!(result.text, "Hello, world!");
    }

    #[test]
    fn test_truncate_end() {
        let config = TruncateConfig {
            max_chars: 20,
            suffix: "...".to_string(),
            ..Default::default()
        };
        let result = truncate("This is a long text that needs to be truncated", &config);
        assert!(result.truncated);
        assert!(result.text.ends_with("..."));
        assert!(result.text.len() <= 20);
    }

    #[test]
    fn test_truncate_start() {
        let config = TruncateConfig {
            max_chars: 20,
            strategy: TruncateStrategy::Start,
            prefix: "...".to_string(),
            ..Default::default()
        };
        let result = truncate("This is a long text that needs to be truncated", &config);
        assert!(result.truncated);
        assert!(result.text.len() <= config.max_chars);
        assert!(result.text.starts_with("..."));
    }

    #[test]
    fn test_truncate_middle() {
        let config = TruncateConfig {
            max_chars: 50,
            strategy: TruncateStrategy::Middle,
            ..Default::default()
        };
        let text = "A".repeat(100);
        let result = truncate(&text, &config);
        assert!(result.truncated);
        assert!(result.text.len() <= config.max_chars);
        assert!(result.text.contains("omitted"));
    }

    #[test]
    fn test_estimate_tokens() {
        assert!(estimate_tokens("Hello world") > 0);
        assert!(estimate_tokens("") == 0);
    }

    #[test]
    fn test_token_estimator() {
        let mut estimator = TokenEstimator::new();
        let count1 = estimator.estimate("Hello world");
        let count2 = estimator.estimate("Hello world"); // Should be cached
        assert_eq!(count1, count2);
    }

    #[test]
    fn test_truncate_builder() {
        let result = TruncateBuilder::new()
            .max_chars(10)
            .suffix("~")
            .word_boundary(true)
            .truncate("Hello beautiful world");

        assert!(result.truncated);
        assert!(result.text.ends_with("~"));
    }

    #[test]
    fn test_truncate_batch() {
        let items = vec!["Hello world", "Goodbye world", "Another text"];
        let result = truncate_batch(&items, 20);

        let total_len: usize = result.iter().map(|s| s.len()).sum();
        assert!(total_len <= 20 + 30); // Allow some overhead for suffixes
    }
}
