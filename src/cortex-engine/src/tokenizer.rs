//! Tokenization utilities.
//!
//! Provides token counting and text tokenization for various models.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Tokenizer type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum TokenizerType {
    /// OpenAI tokenizer (cl100k_base).
    #[default]
    Cl100kBase,
    /// GPT-2 tokenizer.
    Gpt2,
    /// Claude tokenizer.
    Claude,
    /// Llama tokenizer.
    Llama,
    /// Simple word-based approximation.
    Simple,
}

impl TokenizerType {
    /// Get tokenizer for model.
    pub fn for_model(model: &str) -> Self {
        let model_lower = model.to_lowercase();

        if model_lower.contains("gpt-4") || model_lower.contains("gpt-3.5") {
            Self::Cl100kBase
        } else if model_lower.contains("claude") {
            Self::Claude
        } else if model_lower.contains("llama") {
            Self::Llama
        } else if model_lower.contains("gpt2") {
            Self::Gpt2
        } else {
            Self::Simple
        }
    }

    /// Get average characters per token.
    pub fn chars_per_token(&self) -> f32 {
        match self {
            Self::Cl100kBase => 4.0,
            Self::Gpt2 => 4.0,
            Self::Claude => 3.5,
            Self::Llama => 3.8,
            Self::Simple => 4.0,
        }
    }
}

/// Token counter.
pub struct TokenCounter {
    /// Tokenizer type.
    tokenizer: TokenizerType,
    /// Cache.
    cache: HashMap<u64, u32>,
}

impl TokenCounter {
    /// Create a new counter.
    pub fn new(tokenizer: TokenizerType) -> Self {
        Self {
            tokenizer,
            cache: HashMap::new(),
        }
    }

    /// Create for a model.
    pub fn for_model(model: &str) -> Self {
        Self::new(TokenizerType::for_model(model))
    }

    /// Count tokens in text.
    pub fn count(&mut self, text: &str) -> u32 {
        let hash = hash_text(text);

        if let Some(&cached) = self.cache.get(&hash) {
            return cached;
        }

        let count = self.count_uncached(text);
        self.cache.insert(hash, count);
        count
    }

    /// Count tokens without caching.
    fn count_uncached(&self, text: &str) -> u32 {
        match self.tokenizer {
            TokenizerType::Simple => self.count_simple(text),
            _ => self.count_approximate(text),
        }
    }

    /// Simple word-based counting.
    fn count_simple(&self, text: &str) -> u32 {
        // Split on whitespace and punctuation
        let mut count = 0u32;
        let mut in_word = false;

        for c in text.chars() {
            if c.is_whitespace() || c.is_ascii_punctuation() {
                if in_word {
                    count += 1;
                    in_word = false;
                }
                if c.is_ascii_punctuation() {
                    count += 1;
                }
            } else {
                in_word = true;
            }
        }

        if in_word {
            count += 1;
        }

        count
    }

    /// Approximate token count based on characters.
    fn count_approximate(&self, text: &str) -> u32 {
        let chars = text.len() as f32;
        (chars / self.tokenizer.chars_per_token()).ceil() as u32
    }

    /// Count tokens in messages.
    pub fn count_messages(&mut self, messages: &[ChatMessage]) -> u32 {
        let mut total = 0u32;

        for msg in messages {
            // Role overhead (~4 tokens)
            total += 4;
            // Content
            total += self.count(&msg.content);
            // Name if present
            if let Some(ref name) = msg.name {
                total += self.count(name);
                total += 1;
            }
        }

        // Message separator overhead
        total += messages.len() as u32 * 3;

        total
    }

    /// Clear cache.
    pub fn clear_cache(&mut self) {
        self.cache.clear();
    }
}

impl Default for TokenCounter {
    fn default() -> Self {
        Self::new(TokenizerType::default())
    }
}

/// Chat message for token counting.
#[derive(Debug, Clone)]
pub struct ChatMessage {
    /// Role.
    pub role: String,
    /// Content.
    pub content: String,
    /// Name.
    pub name: Option<String>,
}

impl ChatMessage {
    /// Create a new message.
    pub fn new(role: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            content: content.into(),
            name: None,
        }
    }

    /// Set name.
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }
}

/// Token budget tracker.
pub struct TokenBudget {
    /// Maximum tokens.
    max_tokens: u32,
    /// Used tokens.
    used_tokens: u32,
    /// Reserved for response.
    reserved_for_response: u32,
}

impl TokenBudget {
    /// Create a new budget.
    pub fn new(max_tokens: u32) -> Self {
        Self {
            max_tokens,
            used_tokens: 0,
            reserved_for_response: 0,
        }
    }

    /// Reserve tokens for response.
    pub fn reserve_for_response(&mut self, tokens: u32) {
        self.reserved_for_response = tokens;
    }

    /// Add tokens.
    pub fn add(&mut self, tokens: u32) {
        self.used_tokens += tokens;
    }

    /// Remove tokens.
    pub fn remove(&mut self, tokens: u32) {
        self.used_tokens = self.used_tokens.saturating_sub(tokens);
    }

    /// Get remaining tokens.
    pub fn remaining(&self) -> u32 {
        self.max_tokens
            .saturating_sub(self.used_tokens)
            .saturating_sub(self.reserved_for_response)
    }

    /// Check if has room.
    pub fn has_room(&self, tokens: u32) -> bool {
        self.remaining() >= tokens
    }

    /// Get usage percentage.
    pub fn usage_percent(&self) -> f32 {
        if self.max_tokens == 0 {
            0.0
        } else {
            (self.used_tokens as f32 / self.max_tokens as f32) * 100.0
        }
    }

    /// Reset.
    pub fn reset(&mut self) {
        self.used_tokens = 0;
    }
}

/// Truncation strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TruncationStrategy {
    /// Truncate from end.
    #[default]
    End,
    /// Truncate from beginning.
    Beginning,
    /// Truncate from middle.
    Middle,
}

/// Text truncator.
pub struct Truncator {
    /// Token counter.
    counter: TokenCounter,
    /// Strategy.
    strategy: TruncationStrategy,
    /// Ellipsis.
    ellipsis: String,
}

impl Truncator {
    /// Create a new truncator.
    pub fn new(tokenizer: TokenizerType) -> Self {
        Self {
            counter: TokenCounter::new(tokenizer),
            strategy: TruncationStrategy::default(),
            ellipsis: "...".to_string(),
        }
    }

    /// Set strategy.
    pub fn strategy(mut self, strategy: TruncationStrategy) -> Self {
        self.strategy = strategy;
        self
    }

    /// Set ellipsis.
    pub fn ellipsis(mut self, ellipsis: impl Into<String>) -> Self {
        self.ellipsis = ellipsis.into();
        self
    }

    /// Truncate text to fit within token limit.
    pub fn truncate(&mut self, text: &str, max_tokens: u32) -> String {
        let current = self.counter.count(text);

        if current <= max_tokens {
            return text.to_string();
        }

        let ellipsis_tokens = self.counter.count(&self.ellipsis);
        let target_tokens = max_tokens.saturating_sub(ellipsis_tokens);

        if target_tokens == 0 {
            return self.ellipsis.clone();
        }

        match self.strategy {
            TruncationStrategy::End => self.truncate_end(text, target_tokens),
            TruncationStrategy::Beginning => self.truncate_beginning(text, target_tokens),
            TruncationStrategy::Middle => self.truncate_middle(text, target_tokens),
        }
    }

    /// Truncate from end.
    fn truncate_end(&mut self, text: &str, max_tokens: u32) -> String {
        let chars: Vec<char> = text.chars().collect();
        let end = chars.len();

        // Binary search for the right length
        let mut low = 0;
        let mut high = end;

        while low < high {
            let mid = (low + high).div_ceil(2);
            let substring: String = chars[..mid].iter().collect();
            if self.counter.count(&substring) <= max_tokens {
                low = mid;
            } else {
                high = mid - 1;
            }
        }

        let truncated: String = chars[..low].iter().collect();
        format!("{}{}", truncated.trim_end(), self.ellipsis)
    }

    /// Truncate from beginning.
    fn truncate_beginning(&mut self, text: &str, max_tokens: u32) -> String {
        let chars: Vec<char> = text.chars().collect();

        let mut low = 0;
        let mut high = chars.len();

        while low < high {
            let mid = (low + high) / 2;
            let substring: String = chars[mid..].iter().collect();
            if self.counter.count(&substring) <= max_tokens {
                high = mid;
            } else {
                low = mid + 1;
            }
        }

        let truncated: String = chars[low..].iter().collect();
        format!("{}{}", self.ellipsis, truncated.trim_start())
    }

    /// Truncate from middle.
    fn truncate_middle(&mut self, text: &str, max_tokens: u32) -> String {
        let chars: Vec<char> = text.chars().collect();
        let half_tokens = max_tokens / 2;

        // Get start portion
        let mut start_end = 0;
        for i in 1..=chars.len() {
            let substring: String = chars[..i].iter().collect();
            if self.counter.count(&substring) > half_tokens {
                break;
            }
            start_end = i;
        }

        // Get end portion
        let mut end_start = chars.len();
        for i in (0..chars.len()).rev() {
            let substring: String = chars[i..].iter().collect();
            if self.counter.count(&substring) > half_tokens {
                break;
            }
            end_start = i;
        }

        let start: String = chars[..start_end].iter().collect();
        let end: String = chars[end_start..].iter().collect();

        format!(
            "{} {} {}",
            start.trim_end(),
            self.ellipsis,
            end.trim_start()
        )
    }
}

impl Default for Truncator {
    fn default() -> Self {
        Self::new(TokenizerType::default())
    }
}

/// Hash text for caching.
fn hash_text(text: &str) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

/// Model context limits.
pub fn get_context_limit(model: &str) -> u32 {
    let model_lower = model.to_lowercase();

    if model_lower.contains("gpt-4o") {
        128000
    } else if model_lower.contains("gpt-4-turbo") || model_lower.contains("gpt-4-1106") {
        128000
    } else if model_lower.contains("gpt-4-32k") {
        32768
    } else if model_lower.contains("gpt-4") {
        8192
    } else if model_lower.contains("gpt-3.5-turbo-16k") {
        16384
    } else if model_lower.contains("gpt-3.5") {
        4096
    } else if model_lower.contains("claude-3-opus") {
        200000
    } else if model_lower.contains("claude-3-sonnet") {
        200000
    } else if model_lower.contains("claude-3-haiku") {
        200000
    } else if model_lower.contains("claude-2") {
        100000
    } else if model_lower.contains("claude") {
        100000
    } else if model_lower.contains("llama-3") {
        8192
    } else if model_lower.contains("llama-2") {
        4096
    } else if model_lower.contains("mistral") {
        32000
    } else {
        4096 // Default
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tokenizer_for_model() {
        assert_eq!(TokenizerType::for_model("gpt-4"), TokenizerType::Cl100kBase);
        assert_eq!(
            TokenizerType::for_model("claude-3-opus"),
            TokenizerType::Claude
        );
        assert_eq!(TokenizerType::for_model("llama-3"), TokenizerType::Llama);
    }

    #[test]
    fn test_token_counter_simple() {
        let mut counter = TokenCounter::new(TokenizerType::Simple);

        let count = counter.count("Hello, world!");
        assert!(count > 0);

        // Test caching
        let count2 = counter.count("Hello, world!");
        assert_eq!(count, count2);
    }

    #[test]
    fn test_token_counter_approximate() {
        let mut counter = TokenCounter::for_model("gpt-4");

        let count = counter.count("Hello world");
        // ~11 chars / 4 = ~3 tokens
        assert!(count >= 2 && count <= 5);
    }

    #[test]
    fn test_count_messages() {
        let mut counter = TokenCounter::default();

        let messages = vec![
            ChatMessage::new("user", "Hello"),
            ChatMessage::new("assistant", "Hi there!"),
        ];

        let count = counter.count_messages(&messages);
        assert!(count > 0);
    }

    #[test]
    fn test_token_budget() {
        let mut budget = TokenBudget::new(100);
        budget.reserve_for_response(20);

        assert_eq!(budget.remaining(), 80);

        budget.add(50);
        assert_eq!(budget.remaining(), 30);
        assert!(budget.has_room(30));
        assert!(!budget.has_room(31));
    }

    #[test]
    fn test_truncator_end() {
        let mut truncator = Truncator::new(TokenizerType::Simple).strategy(TruncationStrategy::End);

        let text = "This is a long text that needs to be truncated";
        let truncated = truncator.truncate(text, 5);

        assert!(truncated.ends_with("..."));
        assert!(truncated.len() < text.len());
    }

    #[test]
    fn test_truncator_beginning() {
        let mut truncator =
            Truncator::new(TokenizerType::Simple).strategy(TruncationStrategy::Beginning);

        let text = "This is a long text that needs to be truncated";
        let truncated = truncator.truncate(text, 5);

        assert!(truncated.starts_with("..."));
    }

    #[test]
    fn test_context_limits() {
        assert_eq!(get_context_limit("gpt-4o"), 128000);
        assert_eq!(get_context_limit("gpt-4"), 8192);
        assert_eq!(get_context_limit("claude-3-opus"), 200000);
        assert_eq!(get_context_limit("unknown"), 4096);
    }
}
