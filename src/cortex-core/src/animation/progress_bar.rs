//! Progress bar animation with percentage display.

/// Progress bar with percentage display.
///
/// Renders a visual progress indicator with customizable width and characters.
///
/// # Example
/// ```
/// use cortex_engine::animation::ProgressBar;
///
/// let mut progress = ProgressBar::new(100);
/// progress.set_progress(42);
/// println!("{}", progress.render()); // [████████░░░░░░░░░░░░] 42%
/// ```
#[derive(Debug, Clone)]
pub struct ProgressBar {
    current: u64,
    total: u64,
    width: u16,
    filled_char: char,
    empty_char: char,
}

impl ProgressBar {
    /// Creates a new progress bar with the specified total.
    ///
    /// # Arguments
    /// * `total` - The total value representing 100% progress
    pub fn new(total: u64) -> Self {
        Self {
            current: 0,
            total,
            width: 20,
            filled_char: '\u{2588}', // Full block █
            empty_char: '\u{2591}',  // Light shade ░
        }
    }

    /// Sets the current progress value.
    ///
    /// # Arguments
    /// * `current` - The current progress value (clamped to total)
    pub fn set_progress(&mut self, current: u64) {
        self.current = current.min(self.total);
    }

    /// Increments the progress by the specified amount.
    ///
    /// # Arguments
    /// * `amount` - Amount to increment by
    pub fn increment(&mut self, amount: u64) {
        self.current = (self.current + amount).min(self.total);
    }

    /// Returns the current percentage (0.0 to 100.0).
    pub fn percentage(&self) -> f64 {
        if self.total == 0 {
            return 0.0;
        }
        (self.current as f64 / self.total as f64) * 100.0
    }

    /// Renders the progress bar as a string.
    ///
    /// Format: `[████████░░░░░░░░░░░░] 42%`
    pub fn render(&self) -> String {
        let pct = self.percentage();
        let filled_count = ((pct / 100.0) * self.width as f64).round() as usize;
        let empty_count = self.width as usize - filled_count;

        let filled: String = std::iter::repeat(self.filled_char)
            .take(filled_count)
            .collect();
        let empty: String = std::iter::repeat(self.empty_char)
            .take(empty_count)
            .collect();

        format!("[{}{}] {:>3.0}%", filled, empty, pct)
    }

    /// Sets the width of the progress bar.
    ///
    /// # Arguments
    /// * `width` - Width in characters (default: 20)
    pub fn with_width(mut self, width: u16) -> Self {
        self.width = width;
        self
    }

    /// Sets custom fill and empty characters.
    ///
    /// # Arguments
    /// * `filled` - Character for filled portion
    /// * `empty` - Character for empty portion
    pub fn with_chars(mut self, filled: char, empty: char) -> Self {
        self.filled_char = filled;
        self.empty_char = empty;
        self
    }

    /// Returns the current progress value.
    pub fn current(&self) -> u64 {
        self.current
    }

    /// Returns the total value.
    pub fn total(&self) -> u64 {
        self.total
    }

    /// Returns true if progress is complete.
    pub fn is_complete(&self) -> bool {
        self.current >= self.total
    }
}
