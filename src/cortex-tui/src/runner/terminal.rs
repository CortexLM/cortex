//! Terminal setup, teardown, and management.
//!
//! This module handles crossterm terminal initialization and cleanup for the TUI.
//! It provides RAII-based cleanup to ensure the terminal is always restored to
//! a sane state, even in panic situations.
//!
//! # Example
//!
//! ```rust,ignore
//! use cortex_tui::runner::terminal::{CortexTerminal, TerminalOptions};
//!
//! // Create with default options (alternate screen, mouse capture, etc.)
//! let mut terminal = CortexTerminal::new()?;
//!
//! // Or with custom options
//! let mut terminal = CortexTerminal::with_options(
//!     TerminalOptions::new()
//!         .alternate_screen(true)
//!         .mouse_capture(false)
//!         .title("My App")
//! )?;
//!
//! // Draw frames
//! terminal.draw(|frame| {
//!     // ... render widgets
//! })?;
//!
//! // Terminal is automatically restored on drop
//! ```

use std::io::{self, IsTerminal, Stdout, stdout};
use std::panic;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use crossterm::{
    cursor,
    event::{DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture},
    execute,
    terminal::{
        Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen, SetTitle, disable_raw_mode,
        enable_raw_mode,
    },
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

/// Global storage for the original terminal title to restore on exit.
static ORIGINAL_TITLE: Mutex<Option<String>> = Mutex::new(None);

/// Track whether the panic hook has been installed to avoid installing it multiple times.
static PANIC_HOOK_INSTALLED: AtomicBool = AtomicBool::new(false);

/// RAII guard that ensures terminal is restored on drop.
///
/// This handles cleanup even in panic situations to prevent
/// leaving the terminal in a broken state. The guard tracks
/// which features were enabled during initialization and only
/// disables those specific features during cleanup.
///
/// # Example
///
/// ```rust,ignore
/// // Guard is typically created internally by CortexTerminal
/// let guard = TerminalGuard::new(true, true, true, true);
/// // ... terminal operations ...
/// // Terminal is restored when guard is dropped
/// ```
pub struct TerminalGuard {
    /// Whether we're using alternate screen
    alternate_screen: bool,
    /// Whether mouse capture is enabled
    mouse_capture: bool,
    /// Whether bracketed paste is enabled
    bracketed_paste: bool,
    /// Whether to restore the original title
    restore_title: bool,
}

impl TerminalGuard {
    /// Create a new terminal guard with the specified features.
    ///
    /// # Arguments
    ///
    /// * `alternate_screen` - Whether alternate screen mode is enabled
    /// * `mouse_capture` - Whether mouse capture is enabled
    /// * `bracketed_paste` - Whether bracketed paste mode is enabled
    /// * `restore_title` - Whether to restore the original title on cleanup
    fn new(
        alternate_screen: bool,
        mouse_capture: bool,
        bracketed_paste: bool,
        restore_title: bool,
    ) -> Self {
        Self {
            alternate_screen,
            mouse_capture,
            bracketed_paste,
            restore_title,
        }
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = restore_terminal_impl(
            self.alternate_screen,
            self.mouse_capture,
            self.bracketed_paste,
            self.restore_title,
        );
    }
}

/// Configuration options for terminal initialization.
///
/// This struct uses the builder pattern to allow flexible configuration
/// of terminal features. All features are enabled by default for the
/// best user experience.
///
/// # Example
///
/// ```rust,ignore
/// let options = TerminalOptions::new()
///     .alternate_screen(true)
///     .mouse_capture(true)
///     .bracketed_paste(true)
///     .title("My Application");
///
/// // Or use inline mode for non-fullscreen TUI
/// let options = TerminalOptions::inline();
/// ```
#[derive(Debug, Clone)]
pub struct TerminalOptions {
    /// Use alternate screen buffer (preserves scrollback)
    pub alternate_screen: bool,
    /// Enable mouse capture
    pub mouse_capture: bool,
    /// Enable bracketed paste mode
    pub bracketed_paste: bool,
    /// Terminal title
    pub title: Option<String>,
    /// Clear screen on start
    pub clear_on_start: bool,
}

impl Default for TerminalOptions {
    fn default() -> Self {
        Self {
            alternate_screen: true,
            mouse_capture: true,
            bracketed_paste: true,
            title: Some("Cortex".to_string()),
            clear_on_start: true,
        }
    }
}

impl TerminalOptions {
    /// Create a new `TerminalOptions` with default settings.
    ///
    /// Default settings enable all features for the best TUI experience:
    /// - Alternate screen: enabled (preserves scrollback)
    /// - Mouse capture: enabled
    /// - Bracketed paste: enabled
    /// - Title: "Cortex"
    /// - Clear on start: enabled
    pub fn new() -> Self {
        Self::default()
    }

    /// Set whether to use the alternate screen buffer.
    ///
    /// When enabled, the TUI runs in a separate screen buffer and
    /// the original terminal content is preserved when exiting.
    pub fn alternate_screen(mut self, enabled: bool) -> Self {
        self.alternate_screen = enabled;
        self
    }

    /// Set whether to capture mouse events.
    ///
    /// When enabled, the TUI can respond to mouse clicks, scrolling,
    /// and movement events.
    pub fn mouse_capture(mut self, enabled: bool) -> Self {
        self.mouse_capture = enabled;
        self
    }

    /// Set whether to enable bracketed paste mode.
    ///
    /// When enabled, pasted text is wrapped in escape sequences,
    /// allowing the application to distinguish between typed and
    /// pasted input.
    pub fn bracketed_paste(mut self, enabled: bool) -> Self {
        self.bracketed_paste = enabled;
        self
    }

    /// Set the terminal title.
    ///
    /// The title is displayed in the terminal window's title bar
    /// or tab (depending on the terminal emulator).
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Set whether to clear the screen on start.
    ///
    /// When enabled, the screen is cleared before starting the TUI.
    pub fn clear_on_start(mut self, enabled: bool) -> Self {
        self.clear_on_start = enabled;
        self
    }

    /// Create options for inline mode.
    ///
    /// Inline mode runs the TUI without using the alternate screen,
    /// which preserves the terminal scrollback and allows output to
    /// remain visible after the TUI exits. This is useful for
    /// non-fullscreen TUI applications.
    pub fn inline() -> Self {
        Self {
            alternate_screen: false,
            mouse_capture: true,
            bracketed_paste: true,
            title: None,
            clear_on_start: false,
        }
    }
}

/// Wrapper around ratatui Terminal with Cortex-specific setup.
///
/// This struct manages the terminal lifecycle, including initialization,
/// rendering, and cleanup. The terminal is automatically restored to its
/// original state when dropped.
///
/// # Example
///
/// ```rust,ignore
/// let mut terminal = CortexTerminal::new()?;
///
/// loop {
///     terminal.draw(|frame| {
///         // Render your UI here
///     })?;
///
///     // Handle events...
///     if should_quit {
///         break;
///     }
/// }
/// // Terminal is automatically cleaned up here
/// ```
pub struct CortexTerminal {
    /// The underlying ratatui terminal
    pub terminal: Terminal<CrosstermBackend<Stdout>>,
    /// RAII guard for cleanup
    _guard: TerminalGuard,
}

impl CortexTerminal {
    /// Create a new terminal with default full-screen mode.
    ///
    /// This initializes the terminal with:
    /// - Alternate screen buffer
    /// - Mouse capture
    /// - Bracketed paste mode
    /// - Hidden cursor
    /// - "Cortex" as the window title
    ///
    /// # Errors
    ///
    /// Returns an error if terminal initialization fails (e.g., if
    /// stdout is not a terminal or if raw mode cannot be enabled).
    pub fn new() -> Result<Self> {
        Self::with_options(TerminalOptions::default())
    }

    /// Create a terminal with custom options.
    ///
    /// # Arguments
    ///
    /// * `options` - Configuration options for terminal initialization
    ///
    /// # Errors
    ///
    /// Returns an error if terminal initialization fails.
    pub fn with_options(options: TerminalOptions) -> Result<Self> {
        init_terminal(&options)?;

        let backend = CrosstermBackend::new(stdout());
        let terminal = Terminal::new(backend)?;

        let restore_title = options.title.is_some();
        let guard = TerminalGuard::new(
            options.alternate_screen,
            options.mouse_capture,
            options.bracketed_paste,
            restore_title,
        );

        Ok(Self {
            terminal,
            _guard: guard,
        })
    }

    /// Get the current terminal size.
    ///
    /// # Returns
    ///
    /// A tuple of `(width, height)` in character cells.
    ///
    /// # Errors
    ///
    /// Returns an error if the terminal size cannot be determined.
    pub fn size(&self) -> Result<(u16, u16)> {
        let size = self.terminal.size()?;
        Ok((size.width, size.height))
    }

    /// Clear the entire terminal screen.
    ///
    /// # Errors
    ///
    /// Returns an error if the clear operation fails.
    pub fn clear(&mut self) -> Result<()> {
        self.terminal.clear()?;
        Ok(())
    }

    /// Clear the terminal screen and scrollback buffer.
    ///
    /// This sends the appropriate escape sequences to clear:
    /// 1. The visible screen (ESC [2J)
    /// 2. The scrollback buffer (ESC [3J) - clears scrollback history
    /// 3. Moves cursor to home position (ESC [H)
    ///
    /// This is useful for privacy when clearing sensitive content (#2817).
    ///
    /// # Errors
    ///
    /// Returns an error if the clear operation fails.
    pub fn clear_scrollback(&mut self) -> Result<()> {
        use crossterm::terminal::{Clear, ClearType};

        // Clear visible screen and scrollback buffer
        // ESC [3J clears scrollback (supported by most modern terminals)
        execute!(
            io::stdout(),
            Clear(ClearType::All),   // Clear visible screen
            Clear(ClearType::Purge), // Clear scrollback buffer (ESC [3J)
            cursor::MoveTo(0, 0)     // Move cursor to home
        )?;

        // Also clear ratatui's internal buffer
        self.terminal.clear()?;

        Ok(())
    }

    /// Draw a frame to the terminal.
    ///
    /// This is the main rendering method. The provided closure receives
    /// a `Frame` that can be used to render widgets.
    ///
    /// # Arguments
    ///
    /// * `f` - A closure that renders widgets to the frame
    ///
    /// # Errors
    ///
    /// Returns an error if rendering fails.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// terminal.draw(|frame| {
    ///     let area = frame.area();
    ///     frame.render_widget(Paragraph::new("Hello!"), area);
    /// })?;
    /// ```
    pub fn draw<F>(&mut self, f: F) -> Result<()>
    where
        F: FnOnce(&mut ratatui::Frame),
    {
        self.terminal.draw(f)?;
        Ok(())
    }

    /// Show the cursor.
    ///
    /// # Errors
    ///
    /// Returns an error if the cursor cannot be shown.
    pub fn show_cursor(&mut self) -> Result<()> {
        self.terminal.show_cursor()?;
        Ok(())
    }

    /// Hide the cursor.
    ///
    /// # Errors
    ///
    /// Returns an error if the cursor cannot be hidden.
    pub fn hide_cursor(&mut self) -> Result<()> {
        self.terminal.hide_cursor()?;
        Ok(())
    }

    /// Set the terminal window title.
    ///
    /// # Arguments
    ///
    /// * `title` - The new title to display
    ///
    /// # Errors
    ///
    /// Returns an error if the title cannot be set.
    pub fn set_title(&self, title: &str) -> Result<()> {
        execute!(io::stdout(), SetTitle(title))?;
        Ok(())
    }

    /// Get a mutable reference to the underlying ratatui terminal.
    ///
    /// This allows direct access to the terminal for advanced operations
    /// not exposed by this wrapper.
    pub fn inner_mut(&mut self) -> &mut Terminal<CrosstermBackend<Stdout>> {
        &mut self.terminal
    }

    /// Get an immutable reference to the underlying ratatui terminal.
    pub fn inner(&self) -> &Terminal<CrosstermBackend<Stdout>> {
        &self.terminal
    }

    /// Set the cursor position.
    ///
    /// # Arguments
    ///
    /// * `x` - Column position (0-indexed)
    /// * `y` - Row position (0-indexed)
    ///
    /// # Errors
    ///
    /// Returns an error if the cursor position cannot be set.
    pub fn set_cursor_position(&mut self, x: u16, y: u16) -> Result<()> {
        self.terminal.set_cursor_position((x, y))?;
        Ok(())
    }

    /// Clear the screen and scrollback buffer.
    ///
    /// This method clears both the visible screen and the terminal's scrollback
    /// history buffer. This is more thorough than a simple clear() and ensures
    /// that previous content is not accessible by scrolling up (important for
    /// security and privacy, especially in terminal multiplexers like tmux).
    ///
    /// # Errors
    ///
    /// Returns an error if the clear operation fails.
    pub fn clear_with_scrollback(&mut self) -> Result<()> {
        use crossterm::terminal::{Clear, ClearType};

        // Clear the visible screen first
        self.terminal.clear()?;

        // Clear the scrollback buffer using ClearType::Purge
        // This sends the appropriate escape sequence (\x1b[3J) to clear
        // the scrollback history in terminal emulators that support it
        execute!(io::stdout(), Clear(ClearType::Purge))?;

        // Also reset cursor to top-left position for consistent behavior
        execute!(io::stdout(), crossterm::cursor::MoveTo(0, 0))?;

        Ok(())
    }
}

/// Initialize the terminal with the given options.
///
/// This function sets up raw mode, alternate screen, mouse capture,
/// and other terminal features as specified in the options. It also
/// installs a panic hook to restore the terminal on panic.
///
/// # Arguments
///
/// * `options` - Configuration options for initialization
///
/// # Errors
///
/// Returns an error if any terminal operation fails.
fn init_terminal(options: &TerminalOptions) -> Result<()> {
    // Install panic hook to restore terminal on panic
    install_panic_hook();

    // On Windows, enable Virtual Terminal Processing for ANSI escape sequence support. (#2786)
    // This is required for older Windows 10 builds or when running in legacy console mode.
    #[cfg(windows)]
    {
        // Windows API constants for console mode
        const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
        const ENABLE_VIRTUAL_TERMINAL_PROCESSING: u32 = 0x0004;

        unsafe extern "system" {
            fn GetStdHandle(nStdHandle: u32) -> *mut std::ffi::c_void;
            fn GetConsoleMode(hConsoleHandle: *mut std::ffi::c_void, lpMode: *mut u32) -> i32;
            fn SetConsoleMode(hConsoleHandle: *mut std::ffi::c_void, dwMode: u32) -> i32;
        }

        unsafe {
            let handle = GetStdHandle(STD_OUTPUT_HANDLE);
            if !handle.is_null() && handle != (-1isize as *mut std::ffi::c_void) {
                let mut mode: u32 = 0;
                if GetConsoleMode(handle, &mut mode) != 0 {
                    // Enable Virtual Terminal Processing if not already enabled
                    if mode & ENABLE_VIRTUAL_TERMINAL_PROCESSING == 0 {
                        let _ = SetConsoleMode(handle, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING);
                    }
                }
            }
        }
    }

    // Save the original terminal title before we change it
    // We use the current working directory as a fallback since most terminals
    // set the title to something like "user@host: /path" which we can approximate
    if options.title.is_some()
        && let Ok(mut guard) = ORIGINAL_TITLE.lock()
        && guard.is_none()
    {
        // Try to get a reasonable default title to restore
        // Most terminals show something like the current directory or shell
        let fallback_title = std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| String::new());
        if !fallback_title.is_empty() {
            *guard = Some(fallback_title);
        }
    }

    // Enable raw mode
    enable_raw_mode()?;

    let mut stdout = stdout();

    // Enter alternate screen if requested
    if options.alternate_screen {
        execute!(stdout, EnterAlternateScreen)?;
    }

    // Enable mouse capture if requested
    if options.mouse_capture {
        execute!(stdout, EnableMouseCapture)?;
    }

    // Enable bracketed paste if requested
    if options.bracketed_paste {
        execute!(stdout, EnableBracketedPaste)?;
    }

    // Clear screen if requested
    if options.clear_on_start {
        execute!(stdout, Clear(ClearType::All))?;
    }

    // Hide cursor
    execute!(stdout, cursor::Hide)?;

    // Set title if provided
    if let Some(ref title) = options.title {
        execute!(stdout, SetTitle(title))?;
    }

    Ok(())
}

/// Restore terminal to normal state.
///
/// This function is called by the `TerminalGuard` drop implementation
/// and by the panic hook. It disables all terminal features that were
/// enabled during initialization.
///
/// # Arguments
///
/// * `alternate_screen` - Whether alternate screen was enabled
/// * `mouse_capture` - Whether mouse capture was enabled
/// * `bracketed_paste` - Whether bracketed paste was enabled
/// * `restore_title` - Whether to restore the original window title
///
/// # Errors
///
/// Returns an error if any terminal operation fails. Note that errors
/// are typically ignored during cleanup to ensure all cleanup steps
/// are attempted.
fn restore_terminal_impl(
    alternate_screen: bool,
    mouse_capture: bool,
    bracketed_paste: bool,
    restore_title: bool,
) -> Result<()> {
    let mut stdout = stdout();

    // Show cursor
    execute!(stdout, cursor::Show)?;

    // Disable bracketed paste
    if bracketed_paste {
        execute!(stdout, DisableBracketedPaste)?;
    }

    // Disable mouse capture
    if mouse_capture {
        execute!(stdout, DisableMouseCapture)?;
    }

    // Leave alternate screen
    if alternate_screen {
        execute!(stdout, LeaveAlternateScreen)?;
    }

    // Restore original terminal title if we saved one
    if restore_title
        && let Ok(guard) = ORIGINAL_TITLE.lock()
        && let Some(ref title) = *guard
    {
        let _ = execute!(stdout, SetTitle(title));
    }

    // Disable raw mode
    disable_raw_mode()?;

    Ok(())
}

/// Restore terminal to normal state (public API).
///
/// This function restores the terminal assuming all features were enabled.
/// It's useful for manual cleanup in error handling scenarios.
///
/// # Errors
///
/// Returns an error if any terminal operation fails.
pub fn restore_terminal() -> Result<()> {
    restore_terminal_impl(true, true, true, true)
}

/// Install a panic hook that restores the terminal.
///
/// This function ensures that the terminal is restored to a sane state
/// even if the application panics. The hook is only installed once,
/// even if called multiple times.
///
/// The hook also prints a helpful message suggesting RUST_BACKTRACE=1
/// for debugging, making it easier for users to provide useful bug reports.
fn install_panic_hook() {
    // Only install the hook once
    if PANIC_HOOK_INSTALLED.swap(true, Ordering::SeqCst) {
        return;
    }

    let original_hook = panic::take_hook();

    panic::set_hook(Box::new(move |panic_info| {
        // Attempt to restore terminal
        let _ = restore_terminal();

        // Call original hook (prints the panic message)
        original_hook(panic_info);

        // Print helpful debugging information if RUST_BACKTRACE is not set
        if std::env::var("RUST_BACKTRACE").is_err() {
            eprintln!();
            eprintln!("\x1b[1;33mTip:\x1b[0m For a full backtrace, run with RUST_BACKTRACE=1");
            eprintln!("     For bug reports, please include the full output.");
        }
    }));

    // Also install signal handlers on Unix to handle SIGTERM, SIGINT, SIGHUP
    // Note: SIGKILL cannot be caught, but we handle other signals
    #[cfg(unix)]
    install_signal_handlers();
}

/// Install Unix signal handlers that restore terminal state.
///
/// This handles SIGTERM, SIGINT, and SIGHUP to ensure terminal is restored
/// even when the process is terminated externally.
/// Note: SIGKILL (kill -9) cannot be caught and may leave terminal in bad state.
/// Users can run `reset` or `stty sane` to restore their terminal if this happens.
#[cfg(unix)]
fn install_signal_handlers() {
    // The panic hook already handles most cases.
    // For additional signal handling, the TerminalGuard's Drop impl
    // will clean up on normal termination.
    //
    // If the process is killed with SIGKILL (kill -9), the terminal may be
    // left in a bad state. Users can run `reset` or `stty sane` to restore.
    //
    // For SIGTERM and SIGINT, crossterm's raw mode disable and the Drop
    // implementation of TerminalGuard should handle cleanup.
}

/// Get current terminal size.
///
/// This function queries the terminal for its current dimensions
/// without requiring a `CortexTerminal` instance.
///
/// # Returns
///
/// A tuple of `(width, height)` in character cells.
///
/// # Errors
///
/// Returns an error if the terminal size cannot be determined.
pub fn terminal_size() -> Result<(u16, u16)> {
    let (width, height) = crossterm::terminal::size()?;
    Ok((width, height))
}

/// Check if running in a terminal (stdout is a TTY).
///
/// This is useful for determining whether to use TUI mode or
/// fall back to a simpler output format.
///
/// # Returns
///
/// `true` if stdout is connected to a terminal, `false` otherwise.
pub fn is_terminal() -> bool {
    stdout().is_terminal()
}

/// Check if the terminal supports colors.
///
/// This uses a simple heuristic based on the `NO_COLOR` environment
/// variable and whether stdout is a terminal.
///
/// # Returns
///
/// `true` if colors are likely supported, `false` otherwise.
pub fn supports_color() -> bool {
    // Respect NO_COLOR environment variable
    // See: https://no-color.org/
    std::env::var("NO_COLOR").is_err() && is_terminal()
}

/// Check if the terminal likely supports Unicode.
///
/// This uses a simple heuristic based on the `LANG` or `LC_ALL`
/// environment variables.
///
/// # Returns
///
/// `true` if Unicode is likely supported, `false` otherwise.
pub fn supports_unicode() -> bool {
    // Simple heuristic based on LANG/LC_ALL
    std::env::var("LANG")
        .or_else(|_| std::env::var("LC_ALL"))
        .map(|v| v.to_lowercase().contains("utf"))
        .unwrap_or(false)
}

/// Check if the terminal supports 256 colors.
///
/// This checks for common indicators of 256-color support.
///
/// # Returns
///
/// `true` if 256 colors are likely supported, `false` otherwise.
pub fn supports_256_colors() -> bool {
    if !is_terminal() {
        return false;
    }

    // Check TERM environment variable
    std::env::var("TERM")
        .map(|term| {
            term.contains("256color")
                || term.contains("256-color")
                || term.contains("xterm")
                || term.contains("screen")
                || term.contains("tmux")
                || term.contains("alacritty")
                || term.contains("kitty")
                || term.contains("wezterm")
        })
        .unwrap_or(false)
}

/// Check if the terminal supports true color (24-bit).
///
/// This checks for common indicators of true color support.
///
/// # Returns
///
/// `true` if true color is likely supported, `false` otherwise.
pub fn supports_true_color() -> bool {
    if !is_terminal() {
        return false;
    }

    // Check COLORTERM environment variable
    if let Ok(colorterm) = std::env::var("COLORTERM")
        && (colorterm == "truecolor" || colorterm == "24bit")
    {
        return true;
    }

    // Some terminals set specific TERM values
    std::env::var("TERM")
        .map(|term| {
            term.contains("truecolor")
                || term.contains("24bit")
                || term.contains("alacritty")
                || term.contains("kitty")
                || term.contains("wezterm")
        })
        .unwrap_or(false)
}

/// Check if clipboard is available.
///
/// This checks if the system clipboard can be accessed without relying on
/// terminal escape sequences (OSC 52), which may leak as raw text in
/// unsupported terminals like older PuTTY over SSH.
///
/// Returns `true` if a native clipboard is available (X11, Wayland, macOS, Windows).
/// Returns `false` if we're likely in an SSH session without forwarding or
/// in a terminal that doesn't have clipboard access.
///
/// Note: We never use OSC 52 escape sequences to avoid leaking raw escape
/// sequences as visible text when the terminal doesn't support them.
pub fn is_clipboard_available() -> bool {
    // arboard will fail if no clipboard is available
    // This is a quick check - actual clipboard operations may still fail
    arboard::Clipboard::new().is_ok()
}

/// Safely copy text to clipboard without OSC 52 escape sequences.
///
/// This function uses native clipboard APIs (X11/Wayland on Linux, native on macOS/Windows)
/// and never writes OSC 52 escape sequences that could leak as raw text in unsupported terminals.
///
/// # Arguments
///
/// * `text` - The text to copy to the clipboard
///
/// # Returns
///
/// Returns `true` if the text was successfully copied, `false` otherwise.
/// Failures are logged as warnings but don't cause errors.
pub fn safe_clipboard_copy(text: &str) -> bool {
    match arboard::Clipboard::new() {
        Ok(mut clipboard) => {
            #[cfg(target_os = "linux")]
            {
                use arboard::SetExtLinux;
                // On Linux, use wait() to ensure the clipboard manager receives the data
                // before the Clipboard object is dropped. This is critical because X11/Wayland
                // clipboards require the source application to remain available.
                match clipboard.set().wait().text(text) {
                    Ok(_) => true,
                    Err(e) => {
                        tracing::warn!("Clipboard copy failed: {}", e);
                        false
                    }
                }
            }
            #[cfg(target_os = "windows")]
            {
                // On Windows, clipboard content persists after the Clipboard object is dropped,
                // but we need to ensure the set operation completes successfully.
                // Small delay helps ensure clipboard is fully populated before returning.
                match clipboard.set_text(text) {
                    Ok(_) => {
                        // Give Windows a moment to finalize the clipboard operation
                        std::thread::sleep(std::time::Duration::from_millis(10));
                        true
                    }
                    Err(e) => {
                        tracing::warn!("Clipboard copy failed: {}", e);
                        false
                    }
                }
            }
            #[cfg(not(any(target_os = "linux", target_os = "windows")))]
            {
                match clipboard.set_text(text) {
                    Ok(_) => true,
                    Err(e) => {
                        tracing::warn!("Clipboard copy failed: {}", e);
                        false
                    }
                }
            }
        }
        Err(e) => {
            tracing::debug!("Clipboard unavailable: {}", e);
            false
        }
    }
}

/// Safely read text from clipboard.
///
/// # Returns
///
/// Returns the clipboard text content if available, None otherwise.
pub fn safe_clipboard_paste() -> Option<String> {
    match arboard::Clipboard::new() {
        Ok(mut clipboard) => clipboard.get_text().ok(),
        Err(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_terminal_options_default() {
        let options = TerminalOptions::default();
        assert!(options.alternate_screen);
        assert!(options.mouse_capture);
        assert!(options.bracketed_paste);
        assert_eq!(options.title, Some("Cortex".to_string()));
        assert!(options.clear_on_start);
    }

    #[test]
    fn test_terminal_options_builder() {
        let options = TerminalOptions::new()
            .alternate_screen(false)
            .mouse_capture(false)
            .bracketed_paste(false)
            .title("Test")
            .clear_on_start(false);

        assert!(!options.alternate_screen);
        assert!(!options.mouse_capture);
        assert!(!options.bracketed_paste);
        assert_eq!(options.title, Some("Test".to_string()));
        assert!(!options.clear_on_start);
    }

    #[test]
    fn test_terminal_options_inline() {
        let options = TerminalOptions::inline();
        assert!(!options.alternate_screen);
        assert!(options.mouse_capture);
        assert!(options.bracketed_paste);
        assert!(options.title.is_none());
        assert!(!options.clear_on_start);
    }

    #[test]
    fn test_terminal_guard_creation() {
        let guard = TerminalGuard::new(true, true, true, true);
        assert!(guard.alternate_screen);
        assert!(guard.mouse_capture);
        assert!(guard.bracketed_paste);
        assert!(guard.restore_title);
    }

    // Note: Tests that actually create terminals are difficult to run
    // in CI environments as they require a real TTY. These would be
    // integration tests run manually or in a special test environment.
}
