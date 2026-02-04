//! External editor support for composing long prompts.
//!
//! Allows users to open an external editor (via `$VISUAL` or `$EDITOR`)
//! to compose longer, multi-line prompts with their preferred editor.
//!
//! # Usage
//!
//! Triggered by `Ctrl+G` in the TUI, this module:
//! 1. Creates a temporary file with the current input content
//! 2. Opens the user's preferred editor
//! 3. Waits for the editor to close
//! 4. Reads the edited content back
//!
//! # Example
//!
//! ```rust,ignore
//! use cortex_tui::external_editor::open_external_editor;
//!
//! let initial = "Current prompt text";
//! match open_external_editor(initial).await {
//!     Ok(edited) => {
//!         // Use the edited text
//!         println!("Got: {}", edited);
//!     }
//!     Err(e) => {
//!         eprintln!("Editor failed: {}", e);
//!     }
//! }
//! ```

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitStatus;
use thiserror::Error;
use tokio::process::Command;

// ============================================================
// ERRORS
// ============================================================

/// Errors that can occur when opening an external editor.
#[derive(Debug, Error)]
pub enum EditorError {
    /// IO error (file creation, reading, etc.).
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// Editor process exited with non-zero status.
    #[error("Editor exited with status: {0}")]
    EditorFailed(ExitStatus),

    /// No editor found in environment.
    #[error("No editor found. Set $VISUAL or $EDITOR environment variable.")]
    NoEditor,

    /// Editor command not found.
    #[error("Editor command not found: {0}")]
    EditorNotFound(String),

    /// Failed to spawn editor process.
    #[error("Failed to spawn editor: {0}")]
    SpawnFailed(String),
}

// ============================================================
// EDITOR DETECTION
// ============================================================

/// Gets the preferred editor command from environment.
///
/// Checks in order:
/// 1. `$VISUAL` - for full-screen editors
/// 2. `$EDITOR` - fallback editor
/// 3. Default editors based on platform
pub fn get_editor() -> Result<String, EditorError> {
    // Check environment variables
    if let Ok(visual) = std::env::var("VISUAL")
        && !visual.is_empty()
    {
        return Ok(visual);
    }

    if let Ok(editor) = std::env::var("EDITOR")
        && !editor.is_empty()
    {
        return Ok(editor);
    }

    // Platform-specific defaults
    #[cfg(target_os = "windows")]
    {
        // Check for common editors
        if which::which("code").is_ok() {
            return Ok("code --wait".to_string());
        }
        if which::which("notepad++").is_ok() {
            return Ok("notepad++".to_string());
        }
        return Ok("notepad".to_string());
    }

    #[cfg(target_os = "macos")]
    {
        // Check for common editors
        if which::which("code").is_ok() {
            return Ok("code --wait".to_string());
        }
        if which::which("nvim").is_ok() {
            return Ok("nvim".to_string());
        }
        if which::which("vim").is_ok() {
            return Ok("vim".to_string());
        }
        return Ok("nano".to_string());
    }

    #[cfg(target_os = "linux")]
    {
        // Check for common editors
        if which::which("nvim").is_ok() {
            return Ok("nvim".to_string());
        }
        if which::which("vim").is_ok() {
            return Ok("vim".to_string());
        }
        if which::which("nano").is_ok() {
            return Ok("nano".to_string());
        }
        if which::which("code").is_ok() {
            return Ok("code --wait".to_string());
        }
    }

    // Fallback
    Ok("vi".to_string())
}

// ============================================================
// EXTERNAL EDITOR
// ============================================================

/// Opens an external editor with the given initial content.
///
/// Returns the edited content after the editor closes.
///
/// # Arguments
///
/// * `initial_content` - The initial text to populate the editor with.
///
/// # Returns
///
/// The edited content, trimmed of leading/trailing whitespace.
///
/// # Errors
///
/// Returns an error if:
/// - No editor is configured
/// - The temporary file cannot be created
/// - The editor fails to start or exits with an error
/// - The edited content cannot be read
pub async fn open_external_editor(initial_content: &str) -> Result<String, EditorError> {
    // Get the editor command
    let editor_cmd = get_editor()?;

    // Create a temporary file with a secure random name to prevent symlink attacks.
    // Using tempfile crate ensures proper security (O_EXCL, restricted permissions).
    let temp_file = tempfile::Builder::new()
        .prefix("cortex_prompt_")
        .suffix(".md")
        .rand_bytes(16)
        .tempfile()
        .map_err(EditorError::Io)?;

    // Write initial content using the secure file handle
    {
        let mut file = temp_file.reopen().map_err(EditorError::Io)?;
        file.write_all(initial_content.as_bytes())?;
        file.flush()?;
    }

    // Keep the temp file alive (don't let it be deleted yet)
    let temp_file = temp_file.into_temp_path();

    // Parse the editor command
    let parts: Vec<&str> = editor_cmd.split_whitespace().collect();
    let (editor, args) = match parts.split_first() {
        Some((cmd, args)) => (*cmd, args.to_vec()),
        None => return Err(EditorError::NoEditor),
    };

    // Build the command
    let mut cmd = Command::new(editor);
    for arg in args {
        cmd.arg(arg);
    }
    cmd.arg(&temp_file);

    // Spawn and wait for the editor
    // Note: We need to restore terminal state before spawning
    let status = cmd.status().await.map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            EditorError::EditorNotFound(editor.to_string())
        } else {
            EditorError::SpawnFailed(e.to_string())
        }
    })?;

    if !status.success() {
        // Clean up temp file
        let _ = std::fs::remove_file(&temp_file);
        return Err(EditorError::EditorFailed(status));
    }

    // Read the edited content
    let content = std::fs::read_to_string(&temp_file)?;

    // Clean up temp file
    let _ = std::fs::remove_file(&temp_file);

    Ok(content.trim().to_string())
}

/// Opens an external editor synchronously.
///
/// This is a blocking version for use when async is not available.
pub fn open_external_editor_sync(initial_content: &str) -> Result<String, EditorError> {
    // Get the editor command
    let editor_cmd = get_editor()?;

    // Create a temporary file with a secure random name to prevent symlink attacks.
    // Using tempfile crate ensures proper security (O_EXCL, restricted permissions).
    let temp_file = tempfile::Builder::new()
        .prefix("cortex_prompt_")
        .suffix(".md")
        .rand_bytes(16)
        .tempfile()
        .map_err(EditorError::Io)?;

    // Write initial content using the secure file handle
    {
        let mut file = temp_file.reopen().map_err(EditorError::Io)?;
        file.write_all(initial_content.as_bytes())?;
        file.flush()?;
    }

    // Keep the temp file alive (don't let it be deleted yet)
    let temp_file = temp_file.into_temp_path();

    // Parse the editor command
    let parts: Vec<&str> = editor_cmd.split_whitespace().collect();
    let (editor, args) = match parts.split_first() {
        Some((cmd, args)) => (*cmd, args.to_vec()),
        None => return Err(EditorError::NoEditor),
    };

    // Build and run the command
    let status = std::process::Command::new(editor)
        .args(&args)
        .arg(&temp_file)
        .status()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                EditorError::EditorNotFound(editor.to_string())
            } else {
                EditorError::SpawnFailed(e.to_string())
            }
        })?;

    if !status.success() {
        let _ = std::fs::remove_file(&temp_file);
        return Err(EditorError::EditorFailed(status));
    }

    // Read the edited content
    let content = std::fs::read_to_string(&temp_file)?;

    // Clean up
    let _ = std::fs::remove_file(&temp_file);

    Ok(content.trim().to_string())
}

/// Gets an example path pattern for temporary files.
///
/// Note: Actual temp files use random suffixes for security.
/// This function returns a pattern showing the general location.
pub fn get_temp_file_path() -> PathBuf {
    let temp_dir = std::env::temp_dir();
    temp_dir.join("cortex_prompt_XXXXXXXXXXXXXXXX.md")
}

// ============================================================
// TESTS
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_editor() {
        // Should not panic and return some editor
        let result = get_editor();
        assert!(result.is_ok());
        let editor = result.unwrap();
        assert!(!editor.is_empty());
    }

    #[test]
    fn test_get_editor_from_env() {
        // SAFETY: This test modifies environment variables.
        // It's only safe to run in a single-threaded test context.
        // Set a custom editor
        unsafe {
            std::env::set_var("VISUAL", "test_editor");
        }
        let result = get_editor();
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "test_editor");

        // Clean up
        unsafe {
            std::env::remove_var("VISUAL");
        }
    }

    #[test]
    fn test_temp_file_path() {
        let path = get_temp_file_path();
        assert!(path.to_string_lossy().contains("cortex_prompt"));
        assert!(path.to_string_lossy().ends_with(".md"));
    }

    // Note: We can't easily test open_external_editor without actually opening an editor
    // Those would be integration tests
}
