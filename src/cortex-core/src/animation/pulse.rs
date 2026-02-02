//! Pulse animation for breathing/pulsing effects.

use std::time::{Duration, Instant};

use ratatui::style::Color;

use crate::style::{CYAN_PRIMARY, ELECTRIC_BLUE};

use super::easing::interpolate_color;

/// Animation state for pulsing effects (brain, spinners).
///
/// Creates a continuous oscillating animation that loops indefinitely.
/// Perfect for breathing effects, loading indicators, and the brain pulse.
///
/// # Example
/// ```ignore
/// use cortex_core::animation::Pulse;
///
/// let mut pulse = Pulse::new(1000); // 1 second cycle
/// for _ in 0..10 {
///     pulse.tick();
///     let intensity = pulse.intensity(); // 0.0 -> 1.0 -> 0.0
///     // Use intensity for color interpolation, size changes, etc.
/// }
/// ```
#[derive(Debug, Clone)]
pub struct Pulse {
    start: Instant,
    duration: Duration,
    /// Current frame count (monotonically increasing)
    pub frame: u64,
}

impl Pulse {
    /// Creates a new pulse animation with the specified cycle duration.
    ///
    /// # Arguments
    /// * `cycle_duration_ms` - Duration of one complete pulse cycle in milliseconds
    pub fn new(cycle_duration_ms: u64) -> Self {
        Self {
            start: Instant::now(),
            duration: Duration::from_millis(cycle_duration_ms),
            frame: 0,
        }
    }

    /// Advances the animation by one frame.
    ///
    /// Should be called once per render frame (e.g., at 120 FPS).
    #[inline]
    pub fn tick(&mut self) {
        self.frame = self.frame.wrapping_add(1);
    }

    /// Returns the current position in the cycle as a value from 0.0 to 1.0.
    ///
    /// This is a linear progression through the cycle:
    /// - 0.0 at the start of a cycle
    /// - 0.5 at the midpoint
    /// - 1.0 at the end (wraps back to 0.0)
    pub fn progress(&self) -> f32 {
        let elapsed = self.start.elapsed();
        let cycle_nanos = self.duration.as_nanos() as f64;
        let elapsed_nanos = elapsed.as_nanos() as f64;

        // Calculate position within current cycle
        let position = elapsed_nanos % cycle_nanos;
        (position / cycle_nanos) as f32
    }

    /// Returns an intensity value that ping-pongs from 0.0 to 1.0 and back.
    ///
    /// Unlike `progress()`, this creates a smooth back-and-forth effect:
    /// - Starts at 0.0
    /// - Rises to 1.0 at cycle midpoint
    /// - Falls back to 0.0 at cycle end
    ///
    /// Uses a sine wave for smooth acceleration/deceleration.
    pub fn intensity(&self) -> f32 {
        let progress = self.progress();
        // Use sine wave for smooth ping-pong: sin goes 0 -> 1 -> 0 over [0, π]
        // We map our 0-1 progress to 0-π
        (progress * std::f32::consts::PI).sin()
    }

    /// Resets the pulse animation to the beginning.
    pub fn reset(&mut self) {
        self.start = Instant::now();
        self.frame = 0;
    }

    /// Returns the configured cycle duration.
    pub fn cycle_duration(&self) -> Duration {
        self.duration
    }

    /// Returns a color for the current intensity using Ocean/Cyan palette.
    ///
    /// Interpolates: CYAN_PRIMARY -> ELECTRIC_BLUE based on intensity.
    pub fn color(&self) -> Color {
        let intensity = self.intensity();
        interpolate_color(CYAN_PRIMARY, ELECTRIC_BLUE, intensity)
    }
}
