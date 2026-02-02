//! PTY utilities for Cortex CLI.
//!
//! This crate provides pseudo-terminal (PTY) functionality for the Cortex CLI,
//! enabling interactive shell sessions and process management with terminal emulation.
//!
//! # Features
//!
//! - Cross-platform PTY support via `portable-pty`
//! - Async-ready with Tokio integration
//! - Standard terminal sizes and configurations
//!
//! # Platform Support
//!
//! - **Linux**: Full support via native PTY
//! - **macOS**: Full support via native PTY
//! - **Windows**: Support via ConPTY (Windows 10+) or WinPTY fallback
//!
//! # Example
//!
//! ```ignore
//! use cortex_utils_pty::{PtySize, native_pty_system, CommandBuilder};
//!
//! let pty_system = native_pty_system();
//! let pair = pty_system.openpty(PtySize::default()).expect("Failed to open PTY");
//! let mut cmd = CommandBuilder::new("bash");
//! cmd.cwd("/home/user");
//! let child = pair.slave.spawn_command(cmd).expect("Failed to spawn");
//! ```

// Re-export core types from portable-pty for convenient access
pub use portable_pty::{
    native_pty_system, Child, CommandBuilder, ExitStatus, MasterPty, PtyPair, PtySize, PtySystem,
    SlavePty,
};

/// Default terminal width in columns.
pub const DEFAULT_COLS: u16 = 120;

/// Default terminal height in rows.
pub const DEFAULT_ROWS: u16 = 30;

/// Default PTY size with standard dimensions.
///
/// Returns a `PtySize` configured with 120 columns and 30 rows,
/// which provides a comfortable default for most terminal applications.
#[must_use]
pub fn default_pty_size() -> PtySize {
    PtySize {
        rows: DEFAULT_ROWS,
        cols: DEFAULT_COLS,
        pixel_width: 0,
        pixel_height: 0,
    }
}

/// Creates a new PTY pair with default size.
///
/// This is a convenience function that creates a new PTY master/slave pair
/// using the native PTY system with the default terminal size.
///
/// # Errors
///
/// Returns an error if the PTY cannot be created, which may happen if:
/// - The system doesn't support PTY operations
/// - Resource limits have been reached
/// - On Windows, if ConPTY is not available and WinPTY fails
///
/// # Example
///
/// ```ignore
/// let pair = cortex_utils_pty::create_pty_pair()?;
/// // Use pair.master and pair.slave
/// ```
pub fn create_pty_pair() -> anyhow::Result<PtyPair> {
    let pty_system = native_pty_system();
    pty_system
        .openpty(default_pty_size())
        .map_err(|e| anyhow::anyhow!("Failed to open PTY: {}", e))
}

/// Creates a new PTY pair with custom size.
///
/// # Arguments
///
/// * `cols` - Terminal width in columns
/// * `rows` - Terminal height in rows
///
/// # Errors
///
/// Returns an error if the PTY cannot be created.
///
/// # Example
///
/// ```ignore
/// let pair = cortex_utils_pty::create_pty_pair_with_size(80, 24)?;
/// ```
pub fn create_pty_pair_with_size(cols: u16, rows: u16) -> anyhow::Result<PtyPair> {
    let pty_system = native_pty_system();
    pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| anyhow::anyhow!("Failed to open PTY with size {}x{}: {}", cols, rows, e))
}

/// Returns the default shell for the current platform.
///
/// - **Windows**: Returns PowerShell if available, otherwise `cmd.exe`
/// - **macOS**: Returns `SHELL` environment variable or `/bin/zsh`
/// - **Linux**: Returns `SHELL` environment variable or `/bin/bash`
#[must_use]
pub fn get_default_shell() -> String {
    #[cfg(target_os = "windows")]
    {
        // Check for PowerShell Core first
        let pwsh_paths = [
            "C:\\Program Files\\PowerShell\\7\\pwsh.exe",
            "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe",
        ];
        for path in pwsh_paths {
            if std::path::Path::new(path).exists() {
                return path.to_string();
            }
        }
        std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string())
    }

    #[cfg(target_os = "macos")]
    {
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string())
    }

    #[cfg(target_os = "linux")]
    {
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string())
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_pty_size() {
        let size = default_pty_size();
        assert_eq!(size.cols, DEFAULT_COLS);
        assert_eq!(size.rows, DEFAULT_ROWS);
        assert_eq!(size.pixel_width, 0);
        assert_eq!(size.pixel_height, 0);
    }

    #[test]
    fn test_get_default_shell_not_empty() {
        let shell = get_default_shell();
        assert!(!shell.is_empty(), "Default shell should not be empty");
    }
}
