//! Elapsed time display for tracking operation duration.

use std::time::{Duration, Instant};

/// Elapsed time display for tracking operation duration.
///
/// Provides formatted time display that automatically chooses
/// the appropriate format based on duration.
///
/// # Example
/// ```
/// use cortex_engine::animation::ElapsedTimer;
/// use std::thread;
/// use std::time::Duration;
///
/// let timer = ElapsedTimer::new();
/// thread::sleep(Duration::from_secs(2));
/// println!("{}", timer.render()); // "2.0s"
/// ```
#[derive(Debug, Clone)]
pub struct ElapsedTimer {
    start: Instant,
}

impl ElapsedTimer {
    /// Creates a new elapsed timer, starting now.
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
        }
    }

    /// Resets the timer to now.
    pub fn reset(&mut self) {
        self.start = Instant::now();
    }

    /// Returns the elapsed duration since the timer was created/reset.
    pub fn elapsed(&self) -> Duration {
        self.start.elapsed()
    }

    /// Returns the elapsed time in seconds as a float.
    pub fn elapsed_secs(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    /// Renders the elapsed time as a formatted string.
    ///
    /// Format choices:
    /// - < 1 second: "0.3s"
    /// - < 60 seconds: "45s" or "45.2s"
    /// - < 1 hour: "1m 23s"
    /// - >= 1 hour: "1h 23m"
    pub fn render(&self) -> String {
        let elapsed = self.start.elapsed();
        let total_secs = elapsed.as_secs();
        let millis = elapsed.subsec_millis();

        if total_secs == 0 {
            // Sub-second: show with decimal
            format!("{:.1}s", elapsed.as_secs_f64())
        } else if total_secs < 60 {
            // Under a minute: show seconds with optional decimal
            if millis > 0 {
                format!("{}.{}s", total_secs, millis / 100)
            } else {
                format!("{}s", total_secs)
            }
        } else if total_secs < 3600 {
            // Under an hour: show minutes and seconds
            let mins = total_secs / 60;
            let secs = total_secs % 60;
            format!("{}m {}s", mins, secs)
        } else {
            // Over an hour: show hours and minutes
            let hours = total_secs / 3600;
            let mins = (total_secs % 3600) / 60;
            format!("{}h {}m", hours, mins)
        }
    }

    /// Renders as a compact format suitable for inline display.
    ///
    /// Format: "[1.2s]" or "[1m 23s]"
    pub fn render_bracketed(&self) -> String {
        format!("[{}]", self.render())
    }
}

impl Default for ElapsedTimer {
    fn default() -> Self {
        Self::new()
    }
}
