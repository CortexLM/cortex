//! Typewriter animation for streaming text reveal.

/// Typewriter effect for streaming text character by character.
///
/// Reveals text gradually at a configurable rate, perfect for
/// streaming AI responses or dramatic text reveals.
///
/// **Dynamic mode**: When enabled, the typewriter automatically speeds up
/// when there's a backlog of text to reveal, ensuring the display keeps
/// pace with incoming stream data. This creates a smooth experience where
/// text appears as fast as it arrives, with a nice animation when slow.
///
/// # Example
/// ```
/// use cortex_engine::animation::Typewriter;
///
/// // Static typewriter (fixed speed)
/// let mut tw = Typewriter::new("Hello, World!".to_string(), 30.0);
///
/// // Dynamic typewriter (adapts to stream speed)
/// let mut tw_dynamic = Typewriter::dynamic("".to_string(), 60.0);
///
/// while !tw.is_complete() {
///     tw.tick();
///     println!("{}", tw.visible_text());
/// }
/// ```
#[derive(Debug, Clone)]
pub struct Typewriter {
    text: String,
    visible_chars: usize,
    /// Base characters per frame (minimum speed)
    base_chars_per_frame: f32,
    /// Current effective chars per frame (may be boosted in dynamic mode)
    chars_per_frame: f32,
    accumulator: f32,
    complete: bool,
    /// Enable dynamic speed adjustment based on backlog
    dynamic: bool,
    /// Maximum chars per frame when catching up
    max_chars_per_frame: f32,
    /// Backlog threshold (chars) to start accelerating
    catchup_threshold: usize,
}

impl Typewriter {
    /// Creates a new typewriter effect with fixed speed.
    ///
    /// # Arguments
    /// * `text` - The full text to reveal
    /// * `chars_per_second` - How many characters to reveal per second
    ///
    /// # Note
    /// The actual reveal rate depends on how frequently `tick()` is called.
    /// At 120 FPS, `chars_per_second = 120.0` would reveal one char per frame.
    pub fn new(text: String, chars_per_second: f32) -> Self {
        // Assuming 120 FPS target
        const TARGET_FPS: f32 = 120.0;
        let chars_per_frame = chars_per_second / TARGET_FPS;

        Self {
            text,
            visible_chars: 0,
            base_chars_per_frame: chars_per_frame,
            chars_per_frame,
            accumulator: 0.0,
            complete: false,
            dynamic: false,
            max_chars_per_frame: 100.0, // Can reveal up to 100 chars/frame when catching up
            catchup_threshold: 10,      // Start accelerating when 10+ chars behind
        }
    }

    /// Creates a dynamic typewriter that adapts speed to stream rate.
    ///
    /// When text arrives faster than the base reveal rate, the typewriter
    /// automatically speeds up to prevent falling behind. This creates
    /// a smooth streaming experience without visible lag.
    ///
    /// # Arguments
    /// * `text` - Initial text (can be empty for streaming)
    /// * `base_chars_per_second` - Minimum reveal speed when not catching up
    pub fn dynamic(text: String, base_chars_per_second: f32) -> Self {
        let mut tw = Self::new(text, base_chars_per_second);
        tw.dynamic = true;
        tw
    }

    /// Enables or disables dynamic speed adjustment.
    pub fn set_dynamic(&mut self, dynamic: bool) {
        self.dynamic = dynamic;
    }

    /// Advances the typewriter by one frame.
    ///
    /// Uses fractional accumulation for sub-character precision,
    /// ensuring smooth reveal at any speed. In dynamic mode,
    /// automatically adjusts speed based on backlog.
    pub fn tick(&mut self) {
        if self.complete {
            return;
        }

        let total_chars = self.text.chars().count();
        let backlog = total_chars.saturating_sub(self.visible_chars);

        // Dynamic speed adjustment based on backlog
        if self.dynamic && backlog > self.catchup_threshold {
            // Exponential speedup based on backlog size
            // At threshold: base speed
            // At 2x threshold: ~2x speed
            // At large backlog: max speed
            let excess = (backlog - self.catchup_threshold) as f32;
            let boost_factor = (excess / 30.0).min(1.0); // Full boost at 30+ chars excess
            self.chars_per_frame = self.base_chars_per_frame
                + (self.max_chars_per_frame - self.base_chars_per_frame) * boost_factor;
        } else {
            self.chars_per_frame = self.base_chars_per_frame;
        }

        self.accumulator += self.chars_per_frame;

        // Reveal characters when accumulator exceeds 1.0
        while self.accumulator >= 1.0 && !self.complete {
            self.accumulator -= 1.0;
            self.visible_chars += 1;

            if self.visible_chars >= total_chars {
                self.visible_chars = total_chars;
                self.complete = true;
                self.accumulator = 0.0;
            }
        }
    }

    /// Returns the currently visible portion of the text.
    ///
    /// This properly handles multi-byte UTF-8 characters.
    pub fn visible_text(&self) -> &str {
        // Find the byte index for the visible character count
        let byte_index = self
            .text
            .char_indices()
            .nth(self.visible_chars)
            .map(|(i, _)| i)
            .unwrap_or(self.text.len());

        &self.text[..byte_index]
    }

    /// Returns `true` if all text has been revealed.
    #[inline]
    pub fn is_complete(&self) -> bool {
        self.complete
    }

    /// Returns the number of characters waiting to be revealed.
    #[inline]
    pub fn backlog(&self) -> usize {
        self.text.chars().count().saturating_sub(self.visible_chars)
    }

    /// Immediately reveals all remaining text.
    pub fn skip_to_end(&mut self) {
        self.visible_chars = self.text.chars().count();
        self.complete = true;
        self.accumulator = 0.0;
    }

    /// Replaces the text and resets the animation.
    pub fn set_text(&mut self, text: String) {
        self.text = text;
        self.visible_chars = 0;
        self.accumulator = 0.0;
        self.complete = false;
    }

    /// Appends text to the existing content.
    ///
    /// The animation continues from where it was, revealing
    /// the new text after the existing visible text.
    pub fn append(&mut self, text: &str) {
        self.text.push_str(text);
        self.complete = false;
    }

    /// Returns the total character count.
    pub fn total_chars(&self) -> usize {
        self.text.chars().count()
    }

    /// Returns the number of currently visible characters.
    pub fn visible_char_count(&self) -> usize {
        self.visible_chars
    }

    /// Returns the full text (including unrevealed portion).
    pub fn full_text(&self) -> &str {
        &self.text
    }

    /// Resets the animation to the beginning while keeping the text.
    pub fn reset(&mut self) {
        self.visible_chars = 0;
        self.accumulator = 0.0;
        self.complete = false;
    }
}
