//! Duration calculation utilities with clock adjustment handling.
//!
//! Provides safe duration calculations that handle system clock adjustments
//! (e.g., NTP sync, manual time changes, VM snapshot restore).
//!
//! # Issue Addressed
//! - #2799: Duration calculations become negative when system clock adjusted backward

use std::time::{Duration, Instant, SystemTime};

/// Calculate elapsed duration safely, handling potential clock adjustments.
///
/// When using `SystemTime`, clock adjustments can cause the end time to be
/// before the start time. This function handles such cases by returning
/// `Duration::ZERO` instead of panicking or returning garbage values.
///
/// # Arguments
/// * `start` - The start time
/// * `end` - The end time
///
/// # Returns
/// The duration between start and end, or `Duration::ZERO` if end is before start.
///
/// # Examples
/// ```
/// use cortex_common::duration_utils::safe_duration_since;
/// use std::time::SystemTime;
///
/// let start = SystemTime::now();
/// // ... some operation ...
/// let end = SystemTime::now();
/// let elapsed = safe_duration_since(start, end);
/// // elapsed will never be negative
/// ```
pub fn safe_duration_since(start: SystemTime, end: SystemTime) -> Duration {
    end.duration_since(start).unwrap_or(Duration::ZERO)
}

/// A monotonic elapsed time tracker that is immune to system clock changes.
///
/// Uses `Instant` internally which is guaranteed to be monotonically increasing.
/// This should be preferred over `SystemTime` for measuring elapsed durations.
///
/// # Examples
/// ```
/// use cortex_common::duration_utils::MonotonicTimer;
///
/// let timer = MonotonicTimer::start();
/// // ... perform operation ...
/// let elapsed = timer.elapsed();
/// // elapsed is always non-negative
/// ```
#[derive(Debug, Clone, Copy)]
pub struct MonotonicTimer {
    start: Instant,
}

impl MonotonicTimer {
    /// Start a new timer.
    pub fn start() -> Self {
        Self {
            start: Instant::now(),
        }
    }

    /// Get elapsed duration since the timer was started.
    ///
    /// This is guaranteed to be non-negative and monotonically increasing.
    pub fn elapsed(&self) -> Duration {
        self.start.elapsed()
    }

    /// Get elapsed time in milliseconds.
    pub fn elapsed_ms(&self) -> u64 {
        self.elapsed().as_millis() as u64
    }

    /// Get elapsed time in seconds as a float.
    pub fn elapsed_secs_f64(&self) -> f64 {
        self.elapsed().as_secs_f64()
    }

    /// Reset the timer to now.
    pub fn reset(&mut self) {
        self.start = Instant::now();
    }

    /// Get the start instant (for advanced use cases).
    pub fn start_instant(&self) -> Instant {
        self.start
    }
}

impl Default for MonotonicTimer {
    fn default() -> Self {
        Self::start()
    }
}

/// Calculate a rate (items per second) safely.
///
/// Handles edge cases:
/// - Zero duration: returns 0.0 (not infinity)
/// - Negative duration: returns 0.0
/// - Very small durations: caps at a reasonable maximum
///
/// # Arguments
/// * `count` - Number of items processed
/// * `duration` - Time taken to process them
///
/// # Returns
/// Rate in items per second, or 0.0 if duration is zero/negative.
pub fn safe_rate(count: u64, duration: Duration) -> f64 {
    let secs = duration.as_secs_f64();
    if secs <= 0.0 {
        return 0.0;
    }

    let rate = count as f64 / secs;

    // Cap at a reasonable maximum to avoid displaying absurd values
    // for very short durations (e.g., 1M tokens/sec)
    rate.min(1_000_000.0)
}

/// Format a duration for display, handling edge cases.
///
/// # Arguments
/// * `duration` - The duration to format
///
/// # Returns
/// A human-readable string representation.
pub fn format_duration(duration: Duration) -> String {
    let secs = duration.as_secs();
    let millis = duration.subsec_millis();

    if secs == 0 {
        if millis == 0 {
            let micros = duration.subsec_micros();
            if micros == 0 {
                format!("{}ns", duration.subsec_nanos())
            } else {
                format!("{}μs", micros)
            }
        } else {
            format!("{}ms", millis)
        }
    } else if secs < 60 {
        format!("{}.{:03}s", secs, millis)
    } else if secs < 3600 {
        let mins = secs / 60;
        let rem_secs = secs % 60;
        format!("{}m {}s", mins, rem_secs)
    } else {
        let hours = secs / 3600;
        let mins = (secs % 3600) / 60;
        format!("{}h {}m", hours, mins)
    }
}

/// Format a rate for display, with appropriate units.
///
/// # Arguments
/// * `rate` - The rate to format (items per second)
/// * `unit` - The unit name (e.g., "tok", "req")
///
/// # Returns
/// A human-readable string representation.
pub fn format_rate(rate: f64, unit: &str) -> String {
    if rate <= 0.0 {
        return format!("0 {}/s", unit);
    }

    if rate >= 1000.0 {
        format!("{:.1}k {}/s", rate / 1000.0, unit)
    } else if rate >= 1.0 {
        format!("{:.1} {}/s", rate, unit)
    } else {
        format!("{:.3} {}/s", rate, unit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn test_safe_duration_since_normal() {
        let start = SystemTime::now();
        thread::sleep(Duration::from_millis(10));
        let end = SystemTime::now();

        let elapsed = safe_duration_since(start, end);
        assert!(elapsed.as_millis() >= 10);
    }

    #[test]
    fn test_safe_duration_since_reversed() {
        // Simulate clock going backward
        let end = SystemTime::UNIX_EPOCH + Duration::from_secs(1000);
        let start = SystemTime::UNIX_EPOCH + Duration::from_secs(2000);

        // Should return zero instead of panicking
        let elapsed = safe_duration_since(start, end);
        assert_eq!(elapsed, Duration::ZERO);
    }

    #[test]
    fn test_monotonic_timer() {
        let timer = MonotonicTimer::start();
        thread::sleep(Duration::from_millis(10));

        let elapsed = timer.elapsed();
        assert!(elapsed.as_millis() >= 10);
    }

    #[test]
    fn test_monotonic_timer_reset() {
        let mut timer = MonotonicTimer::start();
        thread::sleep(Duration::from_millis(10));

        timer.reset();
        let elapsed = timer.elapsed();
        assert!(elapsed.as_millis() < 5);
    }

    #[test]
    fn test_safe_rate_normal() {
        let rate = safe_rate(100, Duration::from_secs(10));
        assert!((rate - 10.0).abs() < 0.001);
    }

    #[test]
    fn test_safe_rate_zero_duration() {
        let rate = safe_rate(100, Duration::ZERO);
        assert_eq!(rate, 0.0);
    }

    #[test]
    fn test_format_duration() {
        assert!(format_duration(Duration::from_nanos(500)).contains("ns"));
        assert!(format_duration(Duration::from_micros(500)).contains("μs"));
        assert!(format_duration(Duration::from_millis(500)).contains("ms"));
        assert!(format_duration(Duration::from_secs(5)).contains("s"));
        assert!(format_duration(Duration::from_secs(120)).contains("m"));
        assert!(format_duration(Duration::from_secs(7200)).contains("h"));
    }

    #[test]
    fn test_format_rate() {
        assert!(format_rate(0.5, "tok").contains("0.500"));
        assert!(format_rate(50.0, "tok").contains("50.0"));
        assert!(format_rate(5000.0, "tok").contains("5.0k"));
    }
}
