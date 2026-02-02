//! Fade animation for smooth transitions.

use std::time::{Duration, Instant};

use super::easing::ease_in_out;

/// Direction of a fade animation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FadeDirection {
    /// Fade in: opacity goes from 0 to 1
    In,
    /// Fade out: opacity goes from 1 to 0
    Out,
}

/// Fade animation for smooth transitions.
///
/// Provides a one-shot fade effect that can be used for
/// element transitions, overlays, and screen changes.
///
/// # Example
/// ```
/// use cortex_engine::animation::Fade;
///
/// let fade = Fade::fade_in(300); // 300ms fade in
/// while !fade.is_complete() {
///     let opacity = fade.progress();
///     // Apply opacity to rendering
/// }
/// ```
#[derive(Debug, Clone)]
pub struct Fade {
    start: Instant,
    duration: Duration,
    direction: FadeDirection,
}

impl Fade {
    /// Creates a fade-in animation (0 -> 1).
    ///
    /// # Arguments
    /// * `duration_ms` - Duration of the fade in milliseconds
    pub fn fade_in(duration_ms: u64) -> Self {
        Self {
            start: Instant::now(),
            duration: Duration::from_millis(duration_ms),
            direction: FadeDirection::In,
        }
    }

    /// Creates a fade-out animation (1 -> 0).
    ///
    /// # Arguments
    /// * `duration_ms` - Duration of the fade in milliseconds
    pub fn fade_out(duration_ms: u64) -> Self {
        Self {
            start: Instant::now(),
            duration: Duration::from_millis(duration_ms),
            direction: FadeDirection::Out,
        }
    }

    /// Returns the current fade progress from 0.0 to 1.0.
    ///
    /// For `FadeDirection::In`: starts at 0.0, ends at 1.0
    /// For `FadeDirection::Out`: starts at 1.0, ends at 0.0
    ///
    /// Uses ease-in-out for smooth acceleration and deceleration.
    pub fn progress(&self) -> f32 {
        let elapsed = self.start.elapsed();
        let raw_progress = if self.duration.is_zero() {
            1.0
        } else {
            (elapsed.as_secs_f32() / self.duration.as_secs_f32()).min(1.0)
        };

        // Apply ease-in-out curve for smooth animation
        let eased = ease_in_out(raw_progress);

        match self.direction {
            FadeDirection::In => eased,
            FadeDirection::Out => 1.0 - eased,
        }
    }

    /// Returns `true` if the fade animation has completed.
    pub fn is_complete(&self) -> bool {
        self.start.elapsed() >= self.duration
    }

    /// Returns the fade direction.
    pub fn direction(&self) -> FadeDirection {
        self.direction
    }

    /// Resets the animation to the beginning.
    pub fn reset(&mut self) {
        self.start = Instant::now();
    }

    /// Returns the configured duration.
    pub fn duration(&self) -> Duration {
        self.duration
    }
}
