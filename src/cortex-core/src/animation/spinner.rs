//! Spinner animation for loading states.

use std::time::{Duration, Instant};

use super::types::{SpinnerFrames, SpinnerType};

/// Frame-based spinner for loading states.
///
/// Cycles through a set of Unicode characters to create
/// animated loading indicators. Supports multiple spinner types
/// for different contexts (thinking, tool execution, streaming, etc.).
///
/// # Example
/// ```
/// use cortex_engine::animation::{Spinner, SpinnerType};
///
/// // Create a thinking spinner (slow, contemplative)
/// let mut spinner = Spinner::thinking();
/// for _ in 0..10 {
///     spinner.tick();
///     // Use spinner.frame() for display
/// }
/// ```
#[derive(Debug, Clone)]
pub struct Spinner {
    frames: &'static [&'static str],
    current_frame: usize,
    interval_ms: u64,
    last_tick: Instant,
    spinner_type: SpinnerType,
}

impl Spinner {
    /// Creates a new spinner with the specified type.
    ///
    /// # Arguments
    /// * `spinner_type` - The type of spinner to create
    pub fn new(spinner_type: SpinnerType) -> Self {
        Self {
            frames: SpinnerFrames::for_type(spinner_type),
            current_frame: 0,
            interval_ms: SpinnerFrames::interval_for_type(spinner_type),
            last_tick: Instant::now(),
            spinner_type,
        }
    }

    /// Creates a thinking spinner - slow, contemplative animation for AI processing.
    pub fn thinking() -> Self {
        Self::new(SpinnerType::Thinking)
    }

    /// Creates a tool spinner - fast, active animation for running commands.
    pub fn tool() -> Self {
        Self::new(SpinnerType::Tool)
    }

    /// Creates a streaming spinner - wave animation for streaming responses.
    pub fn streaming() -> Self {
        Self::new(SpinnerType::Streaming)
    }

    /// Creates an approval spinner - slow pulsing for awaiting approval.
    pub fn approval() -> Self {
        Self::new(SpinnerType::Approval)
    }

    /// Creates a loading spinner - generic loading animation.
    pub fn loading() -> Self {
        Self::new(SpinnerType::Loading)
    }

    /// Creates a spinner with braille dot pattern (legacy).
    ///
    /// Frames: ⠋ ⠙ ⠹ ⠸ ⠼ ⠴ ⠦ ⠧ ⠇ ⠏
    pub fn dots() -> Self {
        const FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
        Self {
            frames: FRAMES,
            current_frame: 0,
            interval_ms: 80,
            last_tick: Instant::now(),
            spinner_type: SpinnerType::Loading,
        }
    }

    /// Creates a spinner with simple line pattern (legacy).
    ///
    /// Frames: - \ | /
    pub fn line() -> Self {
        const FRAMES: &[&str] = &["-", "\\", "|", "/"];
        Self {
            frames: FRAMES,
            current_frame: 0,
            interval_ms: 100,
            last_tick: Instant::now(),
            spinner_type: SpinnerType::Progress,
        }
    }

    /// Creates a spinner with bouncing dot pattern (legacy).
    ///
    /// Frames: ⠁ ⠂ ⠄ ⠂
    pub fn bounce() -> Self {
        const FRAMES: &[&str] = &["⠁", "⠂", "⠄", "⠂"];
        Self {
            frames: FRAMES,
            current_frame: 0,
            interval_ms: 120,
            last_tick: Instant::now(),
            spinner_type: SpinnerType::Loading,
        }
    }

    /// Creates a spinner with custom frames and timing.
    ///
    /// # Arguments
    /// * `frames` - Static slice of frame strings
    /// * `frame_duration_ms` - Duration each frame is shown in milliseconds
    pub fn custom(frames: &'static [&'static str], frame_duration_ms: u64) -> Self {
        Self {
            frames,
            current_frame: 0,
            interval_ms: frame_duration_ms,
            last_tick: Instant::now(),
            spinner_type: SpinnerType::Loading,
        }
    }

    /// Advances the spinner animation based on elapsed time.
    ///
    /// Only advances to the next frame if enough time has passed.
    pub fn tick(&mut self) {
        let elapsed = self.last_tick.elapsed();
        let frame_duration = Duration::from_millis(self.interval_ms);
        if elapsed >= frame_duration {
            // Calculate how many frames to advance
            let frames_to_advance = (elapsed.as_millis() / frame_duration.as_millis()) as usize;
            self.current_frame = (self.current_frame + frames_to_advance) % self.frames.len();
            self.last_tick = Instant::now();
        }
    }

    /// Returns the current spinner frame.
    #[inline]
    pub fn frame(&self) -> &str {
        self.frames[self.current_frame]
    }

    /// Returns the current spinner frame (legacy alias for frame()).
    #[inline]
    pub fn current(&self) -> &str {
        self.frame()
    }

    /// Returns the spinner type.
    #[inline]
    pub fn spinner_type(&self) -> SpinnerType {
        self.spinner_type
    }

    /// Resets the spinner to the first frame.
    pub fn reset(&mut self) {
        self.current_frame = 0;
        self.last_tick = Instant::now();
    }

    /// Returns the total number of frames.
    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }

    /// Returns the current frame index.
    pub fn current_index(&self) -> usize {
        self.current_frame
    }

    /// Returns the frame duration in milliseconds.
    pub fn interval_ms(&self) -> u64 {
        self.interval_ms
    }
}
