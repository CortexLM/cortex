//! Clipboard operations for the Cortex CLI.
//!
//! Provides cross-platform clipboard read/write functionality.

use anyhow::{Context, Result, bail};
use std::io::Write;
use std::process::{Command, Stdio};

/// Copy text to the system clipboard.
///
/// Uses platform-specific commands:
/// - macOS: `pbcopy`
/// - Linux: `xclip` or `xsel` (with X11 fallback)
/// - Windows: `clip.exe`
///
/// # Arguments
/// * `text` - The text to copy to clipboard
///
/// # Returns
/// `Ok(())` on success, or an error if clipboard access failed.
///
/// # Example
/// ```ignore
/// copy_to_clipboard("Hello, world!")?;
/// ```
pub fn copy_to_clipboard(text: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        let mut child = Command::new("pbcopy")
            .stdin(Stdio::piped())
            .spawn()
            .context("Failed to spawn pbcopy")?;

        if let Some(ref mut stdin) = child.stdin {
            stdin.write_all(text.as_bytes())?;
        }
        child.wait()?;
        Ok(())
    }

    #[cfg(target_os = "linux")]
    {
        // Try xclip first, then xsel as fallback
        let result = Command::new("xclip")
            .args(["-selection", "clipboard"])
            .stdin(Stdio::piped())
            .spawn();

        let mut child = match result {
            Ok(c) => c,
            Err(_) => Command::new("xsel")
                .args(["--clipboard", "--input"])
                .stdin(Stdio::piped())
                .spawn()
                .context("Failed to spawn clipboard command (tried xclip and xsel)")?,
        };

        if let Some(ref mut stdin) = child.stdin {
            stdin.write_all(text.as_bytes())?;
        }
        child.wait()?;
        Ok(())
    }

    #[cfg(target_os = "windows")]
    {
        let mut child = Command::new("clip")
            .stdin(Stdio::piped())
            .spawn()
            .context("Failed to spawn clip.exe")?;

        if let Some(ref mut stdin) = child.stdin {
            stdin.write_all(text.as_bytes())?;
        }
        child.wait()?;
        Ok(())
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        bail!("Clipboard not supported on this platform")
    }
}

/// Read text from the system clipboard.
///
/// Uses platform-specific commands:
/// - macOS: `pbpaste`
/// - Linux: `xclip` or `xsel` (with X11 fallback)
/// - Windows: `Get-Clipboard` via PowerShell
///
/// # Returns
/// The clipboard content as a string, or an error if clipboard access failed.
///
/// # Example
/// ```ignore
/// let content = read_clipboard()?;
/// println!("Clipboard: {}", content);
/// ```
pub fn read_clipboard() -> Result<String> {
    #[cfg(target_os = "macos")]
    {
        let output = Command::new("pbpaste").output()?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        } else {
            bail!("Failed to read clipboard")
        }
    }

    #[cfg(target_os = "linux")]
    {
        // Try xclip first, then xsel
        let output = Command::new("xclip")
            .args(["-selection", "clipboard", "-o"])
            .output()
            .or_else(|_| {
                Command::new("xsel")
                    .args(["--clipboard", "--output"])
                    .output()
            })?;

        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        } else {
            bail!("Failed to read clipboard (tried xclip and xsel)")
        }
    }

    #[cfg(target_os = "windows")]
    {
        let output = Command::new("powershell")
            .args(["-Command", "Get-Clipboard"])
            .output()?;

        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        } else {
            bail!("Failed to read clipboard")
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        bail!("Clipboard not supported on this platform")
    }
}

#[cfg(test)]
mod tests {
    // Clipboard tests are difficult to run in CI environments
    // as they require a display server or clipboard service
}
