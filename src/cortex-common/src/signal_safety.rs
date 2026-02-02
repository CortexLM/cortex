//! Signal handler safety utilities.
//!
//! Provides re-entrancy-safe signal handling to prevent issues when
//! multiple rapid signals are received.
//!
//! # Issue Addressed
//! - #2803: Signal handler not re-entrancy safe on rapid multiple Ctrl+C

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// Flag to track if signal handling is already in progress.
static SIGNAL_HANDLING_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

/// Counter for signal interrupts received.
static SIGNAL_COUNT: AtomicU32 = AtomicU32::new(0);

/// Maximum number of rapid signals before forcing exit.
const MAX_RAPID_SIGNALS: u32 = 3;

/// Attempt to acquire the signal handler lock.
///
/// Returns `true` if this is the first signal and handling should proceed.
/// Returns `false` if signal handling is already in progress (re-entrant call).
///
/// # Safety
/// This function uses atomic operations and is safe to call from signal handlers.
///
/// # Examples
/// ```
/// use cortex_common::signal_safety::{try_acquire_signal_lock, release_signal_lock};
///
/// // In a signal handler:
/// if try_acquire_signal_lock() {
///     // Perform cleanup
///     release_signal_lock();
/// }
/// // Otherwise, another signal handler is already running
/// ```
pub fn try_acquire_signal_lock() -> bool {
    // Try to atomically set the flag from false to true
    SIGNAL_HANDLING_IN_PROGRESS
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
}

/// Release the signal handler lock.
///
/// Should be called after signal handling cleanup is complete.
pub fn release_signal_lock() {
    SIGNAL_HANDLING_IN_PROGRESS.store(false, Ordering::SeqCst);
}

/// Check if signal handling is currently in progress.
pub fn is_signal_handling() -> bool {
    SIGNAL_HANDLING_IN_PROGRESS.load(Ordering::SeqCst)
}

/// Increment and return the signal count.
///
/// Used to track rapid successive signals and force exit after threshold.
pub fn increment_signal_count() -> u32 {
    SIGNAL_COUNT.fetch_add(1, Ordering::SeqCst) + 1
}

/// Reset the signal count to zero.
///
/// Should be called when normal operation resumes after handling a signal.
pub fn reset_signal_count() {
    SIGNAL_COUNT.store(0, Ordering::SeqCst);
}

/// Get the current signal count.
pub fn get_signal_count() -> u32 {
    SIGNAL_COUNT.load(Ordering::SeqCst)
}

/// Check if too many rapid signals have been received.
///
/// Returns `true` if the signal count exceeds the threshold, indicating
/// the user is pressing Ctrl+C repeatedly and wants to force exit.
pub fn should_force_exit() -> bool {
    SIGNAL_COUNT.load(Ordering::SeqCst) >= MAX_RAPID_SIGNALS
}

/// Execute a cleanup function safely with re-entrancy protection.
///
/// If another signal handler is already running, this function will:
/// 1. Increment the signal count
/// 2. If count exceeds threshold, force immediate exit
/// 3. Otherwise, return without executing the cleanup
///
/// # Arguments
/// * `cleanup` - The cleanup function to execute
///
/// # Examples
/// ```
/// use cortex_common::signal_safety::safe_signal_handler;
///
/// fn handle_interrupt() {
///     safe_signal_handler(|| {
///         // Restore terminal state
///         eprint!("\x1b[?25h"); // Show cursor
///         eprintln!("\nInterrupted.");
///     });
/// }
/// ```
pub fn safe_signal_handler<F>(cleanup: F)
where
    F: FnOnce(),
{
    // Increment signal count first
    let count = increment_signal_count();

    // Check for rapid signals requesting force exit
    if count >= MAX_RAPID_SIGNALS {
        // Force exit immediately without cleanup
        // This handles the case where cleanup itself might be hanging
        std::process::exit(130);
    }

    // Try to acquire the signal lock
    if try_acquire_signal_lock() {
        // We have the lock, execute cleanup
        cleanup();

        // Release the lock
        release_signal_lock();
    }
    // If we couldn't acquire the lock, another handler is running.
    // The signal count is already incremented, so if the user keeps
    // pressing Ctrl+C, we'll eventually force exit.
}

/// Create a signal-safe cleanup guard.
///
/// This guard ensures that cleanup code is only run once, even if
/// multiple signals are received. It also ensures the lock is released
/// when dropped.
pub struct SignalCleanupGuard {
    /// Whether this guard owns the lock
    owns_lock: bool,
}

impl SignalCleanupGuard {
    /// Try to create a new cleanup guard.
    ///
    /// Returns `Some(guard)` if the lock was acquired, `None` if signal
    /// handling is already in progress.
    pub fn try_new() -> Option<Self> {
        if try_acquire_signal_lock() {
            Some(Self { owns_lock: true })
        } else {
            None
        }
    }

    /// Check if this guard owns the lock.
    pub fn owns_lock(&self) -> bool {
        self.owns_lock
    }
}

impl Drop for SignalCleanupGuard {
    fn drop(&mut self) {
        if self.owns_lock {
            release_signal_lock();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    fn reset_state() {
        release_signal_lock();
        reset_signal_count();
    }

    #[test]
    #[serial]
    fn test_try_acquire_signal_lock() {
        reset_state();

        // First acquire should succeed
        assert!(try_acquire_signal_lock());

        // Second acquire should fail (re-entrant)
        assert!(!try_acquire_signal_lock());

        // After release, should succeed again
        release_signal_lock();
        assert!(try_acquire_signal_lock());

        reset_state();
    }

    #[test]
    #[serial]
    fn test_signal_count() {
        reset_state();

        assert_eq!(get_signal_count(), 0);

        assert_eq!(increment_signal_count(), 1);
        assert_eq!(increment_signal_count(), 2);
        assert_eq!(get_signal_count(), 2);

        reset_signal_count();
        assert_eq!(get_signal_count(), 0);

        reset_state();
    }

    #[test]
    #[serial]
    fn test_should_force_exit() {
        reset_state();

        assert!(!should_force_exit());

        increment_signal_count();
        assert!(!should_force_exit());

        increment_signal_count();
        assert!(!should_force_exit());

        increment_signal_count();
        assert!(should_force_exit());

        reset_state();
    }

    #[test]
    #[serial]
    fn test_cleanup_guard() {
        reset_state();

        // First guard should succeed
        {
            let guard = SignalCleanupGuard::try_new();
            assert!(guard.is_some());
            let guard = guard.unwrap();
            assert!(guard.owns_lock());

            // While guard is alive, another should fail
            let guard2 = SignalCleanupGuard::try_new();
            assert!(guard2.is_none());

            // Keep guard alive until end of scope
            drop(guard);
        }

        // After guard is dropped, new guard should succeed
        let guard = SignalCleanupGuard::try_new();
        assert!(guard.is_some());

        reset_state();
    }

    #[test]
    #[serial]
    fn test_safe_signal_handler() {
        reset_state();

        let mut cleanup_count = 0;

        // First call should execute cleanup
        safe_signal_handler(|| {
            cleanup_count += 1;
        });
        assert_eq!(cleanup_count, 1);

        reset_state();
    }
}
