//! Token counter display for streaming responses.

/// Token counter display for streaming responses.
///
/// Tracks input and output tokens with optional maximum,
/// and formats them for display.
///
/// # Example
/// ```
/// use cortex_engine::animation::TokenCounter;
///
/// let mut counter = TokenCounter::new().with_max(4096);
/// counter.add_input(512);
/// counter.add_output(1024);
/// println!("{}", counter.render()); // "1.5k / 4k tokens"
/// ```
#[derive(Debug, Clone, Default)]
pub struct TokenCounter {
    input_tokens: u64,
    output_tokens: u64,
    max_tokens: Option<u64>,
}

impl TokenCounter {
    /// Creates a new token counter.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds input tokens to the counter.
    ///
    /// # Arguments
    /// * `count` - Number of input tokens to add
    pub fn add_input(&mut self, count: u64) {
        self.input_tokens += count;
    }

    /// Adds output tokens to the counter.
    ///
    /// # Arguments
    /// * `count` - Number of output tokens to add
    pub fn add_output(&mut self, count: u64) {
        self.output_tokens += count;
    }

    /// Sets a maximum token limit for display.
    ///
    /// # Arguments
    /// * `max` - Maximum token count
    pub fn with_max(mut self, max: u64) -> Self {
        self.max_tokens = Some(max);
        self
    }

    /// Sets the maximum token limit.
    pub fn set_max(&mut self, max: u64) {
        self.max_tokens = Some(max);
    }

    /// Returns the total token count (input + output).
    pub fn total(&self) -> u64 {
        self.input_tokens + self.output_tokens
    }

    /// Returns the input token count.
    pub fn input(&self) -> u64 {
        self.input_tokens
    }

    /// Returns the output token count.
    pub fn output(&self) -> u64 {
        self.output_tokens
    }

    /// Renders the token count as a formatted string.
    ///
    /// Format: "1.2k / 4k tokens" or "1.2k tokens" (without max)
    pub fn render(&self) -> String {
        let total = Self::format_count(self.total());
        if let Some(max) = self.max_tokens {
            let max_str = Self::format_count(max);
            format!("{} / {} tokens", total, max_str)
        } else {
            format!("{} tokens", total)
        }
    }

    /// Renders detailed token breakdown.
    ///
    /// Format: "in: 512 | out: 1.2k"
    pub fn render_detailed(&self) -> String {
        format!(
            "in: {} | out: {}",
            Self::format_count(self.input_tokens),
            Self::format_count(self.output_tokens)
        )
    }

    /// Formats a number with k/M suffixes for readability.
    ///
    /// - 0-999: "123"
    /// - 1000-999999: "1.2k"
    /// - 1000000+: "1.2M"
    fn format_count(n: u64) -> String {
        if n >= 1_000_000 {
            format!("{:.1}M", n as f64 / 1_000_000.0)
        } else if n >= 1_000 {
            format!("{:.1}k", n as f64 / 1_000.0)
        } else {
            format!("{}", n)
        }
    }

    /// Resets all counters to zero.
    pub fn reset(&mut self) {
        self.input_tokens = 0;
        self.output_tokens = 0;
    }
}
