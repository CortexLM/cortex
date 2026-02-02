//! Input event reader for terminal applications.
//!
//! This module provides the `InputReader` type which reads and parses terminal
//! input events using crossterm. It supports both blocking and non-blocking modes,
//! and handles keyboard, mouse, resize, paste, and focus events.

use crate::event::Event;
use crossterm::event::{
    self, DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste,
    EnableFocusChange, EnableMouseCapture,
};
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use std::io;
use std::time::Duration;

/// Configuration for the input reader.
#[derive(Debug, Clone)]
pub struct InputReaderConfig {
    /// Enable mouse event capture.
    pub enable_mouse: bool,
    /// Enable bracketed paste mode.
    pub enable_paste: bool,
    /// Enable focus events.
    pub enable_focus: bool,
    /// Default timeout for polling (None = infinite wait).
    pub poll_timeout: Option<Duration>,
}

impl Default for InputReaderConfig {
    fn default() -> Self {
        Self {
            enable_mouse: true,
            enable_paste: true,
            enable_focus: true,
            poll_timeout: None,
        }
    }
}

impl InputReaderConfig {
    /// Creates a minimal configuration with only keyboard input.
    #[must_use]
    pub fn keyboard_only() -> Self {
        Self {
            enable_mouse: false,
            enable_paste: false,
            enable_focus: false,
            poll_timeout: None,
        }
    }

    /// Creates a configuration with all input types enabled.
    #[must_use]
    pub fn all() -> Self {
        Self::default()
    }

    /// Sets the poll timeout.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.poll_timeout = Some(timeout);
        self
    }

    /// Enables or disables mouse capture.
    #[must_use]
    pub fn with_mouse(mut self, enable: bool) -> Self {
        self.enable_mouse = enable;
        self
    }

    /// Enables or disables bracketed paste mode.
    #[must_use]
    pub fn with_paste(mut self, enable: bool) -> Self {
        self.enable_paste = enable;
        self
    }

    /// Enables or disables focus events.
    #[must_use]
    pub fn with_focus(mut self, enable: bool) -> Self {
        self.enable_focus = enable;
        self
    }
}

/// Error types for input reading operations.
#[derive(Debug)]
pub enum InputError {
    /// An I/O error occurred.
    Io(io::Error),
    /// The terminal is not in raw mode.
    NotRawMode,
    /// The reader has not been initialized.
    NotInitialized,
    /// The reader is already initialized.
    AlreadyInitialized,
}

impl std::fmt::Display for InputError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InputError::Io(err) => write!(f, "I/O error: {err}"),
            InputError::NotRawMode => write!(f, "Terminal is not in raw mode"),
            InputError::NotInitialized => write!(f, "Input reader not initialized"),
            InputError::AlreadyInitialized => write!(f, "Input reader already initialized"),
        }
    }
}

impl std::error::Error for InputError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            InputError::Io(err) => Some(err),
            _ => None,
        }
    }
}

impl From<io::Error> for InputError {
    fn from(err: io::Error) -> Self {
        InputError::Io(err)
    }
}

/// Result type for input operations.
pub type InputResult<T> = Result<T, InputError>;

/// Reads and parses terminal input events.
///
/// The `InputReader` manages terminal raw mode and optional mouse/paste/focus
/// event capture. It provides both blocking and non-blocking methods to read events.
///
/// # Example
///
/// ```no_run
/// use cortex_tui_input::reader::{InputReader, InputReaderConfig};
/// use std::time::Duration;
///
/// let mut reader = InputReader::new(InputReaderConfig::default());
/// reader.init().expect("Failed to initialize");
///
/// loop {
///     if let Some(event) = reader.poll(Duration::from_millis(100)).expect("Poll failed") {
///         println!("Event: {:?}", event);
///     }
/// }
/// ```
pub struct InputReader {
    /// Configuration.
    config: InputReaderConfig,
    /// Whether the reader has been initialized.
    initialized: bool,
    /// Previous terminal state for restoration.
    was_raw_mode: bool,
}

impl InputReader {
    /// Creates a new input reader with the given configuration.
    #[must_use]
    pub fn new(config: InputReaderConfig) -> Self {
        Self {
            config,
            initialized: false,
            was_raw_mode: false,
        }
    }

    /// Creates a new input reader with default configuration.
    #[must_use]
    pub fn with_defaults() -> Self {
        Self::new(InputReaderConfig::default())
    }

    /// Initializes the input reader.
    ///
    /// This enables raw mode and configures mouse/paste/focus capture
    /// according to the configuration. Must be called before reading events.
    pub fn init(&mut self) -> InputResult<()> {
        if self.initialized {
            return Err(InputError::AlreadyInitialized);
        }

        // Enable raw mode
        self.was_raw_mode = crossterm::terminal::is_raw_mode_enabled().unwrap_or(false);
        if !self.was_raw_mode {
            enable_raw_mode()?;
        }

        let mut stdout = io::stdout();

        // Enable optional features
        if self.config.enable_mouse {
            execute!(stdout, EnableMouseCapture)?;
        }

        if self.config.enable_paste {
            execute!(stdout, EnableBracketedPaste)?;
        }

        if self.config.enable_focus {
            execute!(stdout, EnableFocusChange)?;
        }

        self.initialized = true;
        Ok(())
    }

    /// Cleans up the input reader and restores terminal state.
    ///
    /// This disables raw mode (if it wasn't enabled before) and turns off
    /// any optional features that were enabled.
    pub fn cleanup(&mut self) -> InputResult<()> {
        if !self.initialized {
            return Ok(());
        }

        let mut stdout = io::stdout();

        // Disable optional features
        if self.config.enable_focus {
            let _ = execute!(stdout, DisableFocusChange);
        }

        if self.config.enable_paste {
            let _ = execute!(stdout, DisableBracketedPaste);
        }

        if self.config.enable_mouse {
            let _ = execute!(stdout, DisableMouseCapture);
        }

        // Restore raw mode state
        if !self.was_raw_mode {
            disable_raw_mode()?;
        }

        self.initialized = false;
        Ok(())
    }

    /// Returns true if the reader has been initialized.
    #[must_use]
    pub fn is_initialized(&self) -> bool {
        self.initialized
    }

    /// Returns a reference to the configuration.
    #[must_use]
    pub fn config(&self) -> &InputReaderConfig {
        &self.config
    }

    /// Reads an event, blocking until one is available.
    ///
    /// # Errors
    ///
    /// Returns an error if the reader is not initialized or if an I/O error occurs.
    pub fn read(&self) -> InputResult<Event> {
        if !self.initialized {
            return Err(InputError::NotInitialized);
        }

        let crossterm_event = event::read()?;
        Ok(crossterm_event.into())
    }

    /// Polls for an event with a timeout.
    ///
    /// Returns `Ok(Some(event))` if an event is available within the timeout,
    /// `Ok(None)` if the timeout elapsed with no event, or an error.
    ///
    /// # Errors
    ///
    /// Returns an error if the reader is not initialized or if an I/O error occurs.
    pub fn poll(&self, timeout: Duration) -> InputResult<Option<Event>> {
        if !self.initialized {
            return Err(InputError::NotInitialized);
        }

        if event::poll(timeout)? {
            let crossterm_event = event::read()?;
            Ok(Some(crossterm_event.into()))
        } else {
            Ok(None)
        }
    }

    /// Polls for an event using the configured default timeout.
    ///
    /// If no timeout is configured, this will block indefinitely.
    pub fn poll_default(&self) -> InputResult<Option<Event>> {
        match self.config.poll_timeout {
            Some(timeout) => self.poll(timeout),
            None => self.read().map(Some),
        }
    }

    /// Checks if an event is available without blocking.
    ///
    /// Note: If stdin is in non-blocking mode and returns EAGAIN repeatedly,
    /// this function will properly return false instead of busy-looping.
    /// Callers should use `poll()` with a timeout for efficient waiting.
    ///
    /// # Errors
    ///
    /// Returns an error if the reader is not initialized or if an I/O error occurs.
    pub fn has_event(&self) -> InputResult<bool> {
        if !self.initialized {
            return Err(InputError::NotInitialized);
        }

        Ok(event::poll(Duration::ZERO)?)
    }

    /// Polls for an event with automatic backoff to prevent CPU spin.
    ///
    /// This method is designed to avoid busy-looping when stdin is in non-blocking
    /// mode and repeatedly returns EAGAIN/WouldBlock. It introduces a small delay
    /// between poll attempts to reduce CPU usage.
    ///
    /// # Arguments
    ///
    /// * `timeout` - Maximum time to wait for an event
    /// * `backoff` - Minimum delay between poll attempts (recommended: 1-10ms)
    ///
    /// # Errors
    ///
    /// Returns an error if the reader is not initialized or if an I/O error occurs.
    pub fn poll_with_backoff(
        &self,
        timeout: Duration,
        backoff: Duration,
    ) -> InputResult<Option<Event>> {
        if !self.initialized {
            return Err(InputError::NotInitialized);
        }

        let start = std::time::Instant::now();

        loop {
            // Check if event is available
            match event::poll(Duration::from_millis(1)) {
                Ok(true) => {
                    let crossterm_event = event::read()?;
                    return Ok(Some(crossterm_event.into()));
                }
                Ok(false) => {
                    // No event available - check if timeout exceeded
                    if start.elapsed() >= timeout {
                        return Ok(None);
                    }
                    // Small sleep to prevent CPU spin on EAGAIN
                    std::thread::sleep(backoff);
                }
                Err(e) => {
                    // Check for WouldBlock/EAGAIN (treat as "no event available")
                    if e.kind() == io::ErrorKind::WouldBlock {
                        if start.elapsed() >= timeout {
                            return Ok(None);
                        }
                        std::thread::sleep(backoff);
                    } else {
                        return Err(InputError::Io(e));
                    }
                }
            }
        }
    }

    /// Drains all currently available events.
    ///
    /// This is useful for clearing any buffered input.
    ///
    /// # Errors
    ///
    /// Returns an error if the reader is not initialized or if an I/O error occurs.
    pub fn drain(&self) -> InputResult<Vec<Event>> {
        if !self.initialized {
            return Err(InputError::NotInitialized);
        }

        let mut events = Vec::new();
        while event::poll(Duration::ZERO)? {
            let crossterm_event = event::read()?;
            events.push(crossterm_event.into());
        }
        Ok(events)
    }
}

impl Drop for InputReader {
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}

/// A RAII guard for managing raw mode.
///
/// When this guard is dropped, raw mode is disabled automatically.
pub struct RawModeGuard {
    /// Whether raw mode was already enabled when we entered.
    was_enabled: bool,
}

impl RawModeGuard {
    /// Enters raw mode and returns a guard.
    ///
    /// When the guard is dropped, the previous raw mode state is restored.
    ///
    /// # Errors
    ///
    /// Returns an error if entering raw mode fails.
    pub fn new() -> io::Result<Self> {
        let was_enabled = crossterm::terminal::is_raw_mode_enabled().unwrap_or(false);
        if !was_enabled {
            enable_raw_mode()?;
        }
        Ok(Self { was_enabled })
    }

    /// Returns true if raw mode was already enabled before this guard was created.
    #[must_use]
    pub fn was_raw_mode_enabled(&self) -> bool {
        self.was_enabled
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        if !self.was_enabled {
            let _ = disable_raw_mode();
        }
    }
}

/// A RAII guard for managing mouse capture.
///
/// When this guard is dropped, mouse capture is disabled automatically.
pub struct MouseCaptureGuard {
    /// Whether we successfully enabled mouse capture.
    enabled: bool,
}

impl MouseCaptureGuard {
    /// Enables mouse capture and returns a guard.
    ///
    /// When the guard is dropped, mouse capture is disabled.
    ///
    /// # Errors
    ///
    /// Returns an error if enabling mouse capture fails.
    pub fn new() -> io::Result<Self> {
        execute!(io::stdout(), EnableMouseCapture)?;
        Ok(Self { enabled: true })
    }
}

impl Drop for MouseCaptureGuard {
    fn drop(&mut self) {
        if self.enabled {
            let _ = execute!(io::stdout(), DisableMouseCapture);
        }
    }
}

/// Iterator over input events.
///
/// This iterator blocks on each call to `next()` until an event is available.
pub struct EventIterator<'a> {
    reader: &'a InputReader,
}

impl<'a> EventIterator<'a> {
    /// Creates a new event iterator over the given reader.
    #[must_use]
    pub fn new(reader: &'a InputReader) -> Self {
        Self { reader }
    }
}

impl Iterator for EventIterator<'_> {
    type Item = InputResult<Event>;

    fn next(&mut self) -> Option<Self::Item> {
        if !self.reader.is_initialized() {
            return Some(Err(InputError::NotInitialized));
        }
        Some(self.reader.read())
    }
}

/// Creates an iterator over events from the reader.
impl InputReader {
    /// Returns an iterator that yields events.
    ///
    /// The iterator blocks on each call to `next()` until an event is available.
    /// The iterator is infinite and will never return `None` unless an error occurs.
    #[must_use]
    pub fn iter(&self) -> EventIterator<'_> {
        EventIterator::new(self)
    }
}

impl<'a> IntoIterator for &'a InputReader {
    type Item = InputResult<Event>;
    type IntoIter = EventIterator<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

/// Helper function to run a closure with terminal input configured.
///
/// This function:
/// 1. Enables raw mode
/// 2. Enables mouse capture, bracketed paste, and focus events
/// 3. Runs the provided closure
/// 4. Restores the terminal to its previous state
///
/// # Example
///
/// ```no_run
/// use cortex_tui_input::reader::with_input;
///
/// with_input(|| {
///     // Read events here
///     Ok(())
/// }).expect("Input handling failed");
/// ```
///
/// # Errors
///
/// Returns an error if terminal setup or the closure fails.
pub fn with_input<F, T>(f: F) -> InputResult<T>
where
    F: FnOnce() -> InputResult<T>,
{
    let mut reader = InputReader::with_defaults();
    reader.init()?;
    let result = f();
    reader.cleanup()?;
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_builder() {
        let config = InputReaderConfig::default()
            .with_mouse(false)
            .with_paste(true)
            .with_focus(false)
            .with_timeout(Duration::from_millis(100));

        assert!(!config.enable_mouse);
        assert!(config.enable_paste);
        assert!(!config.enable_focus);
        assert_eq!(config.poll_timeout, Some(Duration::from_millis(100)));
    }

    #[test]
    fn test_keyboard_only_config() {
        let config = InputReaderConfig::keyboard_only();
        assert!(!config.enable_mouse);
        assert!(!config.enable_paste);
        assert!(!config.enable_focus);
    }

    #[test]
    fn test_reader_not_initialized() {
        let reader = InputReader::with_defaults();
        assert!(!reader.is_initialized());

        let result = reader.has_event();
        assert!(matches!(result, Err(InputError::NotInitialized)));
    }

    // Note: Tests that actually initialize the reader and read events
    // require a real terminal and cannot be run in automated test environments.
    // They would look something like:
    //
    // #[test]
    // #[ignore] // Run manually with: cargo test -- --ignored
    // fn test_reader_init_cleanup() {
    //     let mut reader = InputReader::with_defaults();
    //     reader.init().expect("init failed");
    //     assert!(reader.is_initialized());
    //     reader.cleanup().expect("cleanup failed");
    //     assert!(!reader.is_initialized());
    // }
}
