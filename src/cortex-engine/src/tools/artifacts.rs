//! Tool result artifacts management.
//!
//! When tool results exceed a size threshold, this module saves the full
//! content to a file and returns a truncated version with a reference
//! to the artifact file.
//!
//! This prevents "Payload Too Large" errors when tools like Glob or Grep
//! return thousands of results.

use std::path::{Path, PathBuf};

use tracing::{debug, warn};
use uuid::Uuid;

use super::ToolResult;
use crate::error::Result;

/// Default threshold in bytes before truncation (32KB).
pub const DEFAULT_TRUNCATE_THRESHOLD: usize = 32 * 1024;

/// Default number of lines to show in truncated output.
pub const DEFAULT_TRUNCATE_LINES: usize = 100;

/// Subdirectory name for tool artifacts within cortex data dir.
pub const ARTIFACTS_SUBDIR: &str = "tool_artifacts";

/// Configuration for artifact handling.
#[derive(Debug, Clone)]
pub struct ArtifactConfig {
    /// Directory to store artifacts.
    pub artifacts_dir: PathBuf,
    /// Threshold in bytes before truncating and saving artifact.
    pub truncate_threshold: usize,
    /// Number of lines to show in truncated output.
    pub truncate_lines: usize,
    /// Whether to enable artifact saving (can be disabled for testing).
    pub enabled: bool,
}

impl Default for ArtifactConfig {
    fn default() -> Self {
        // Default to temp dir if cortex home not available
        let artifacts_dir = std::env::var("CORTEX_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| std::env::temp_dir().join("cortex"))
            .join(ARTIFACTS_SUBDIR);

        Self {
            artifacts_dir,
            truncate_threshold: DEFAULT_TRUNCATE_THRESHOLD,
            truncate_lines: DEFAULT_TRUNCATE_LINES,
            enabled: true,
        }
    }
}

impl ArtifactConfig {
    /// Create config with custom artifacts directory.
    pub fn with_dir(dir: impl Into<PathBuf>) -> Self {
        Self {
            artifacts_dir: dir.into(),
            ..Default::default()
        }
    }

    /// Set the truncation threshold.
    pub fn with_threshold(mut self, threshold: usize) -> Self {
        self.truncate_threshold = threshold;
        self
    }

    /// Set the number of truncated lines.
    pub fn with_truncate_lines(mut self, lines: usize) -> Self {
        self.truncate_lines = lines;
        self
    }

    /// Disable artifact saving.
    pub fn disabled(mut self) -> Self {
        self.enabled = false;
        self
    }
}

/// Result of processing tool output through artifact handling.
#[derive(Debug)]
pub struct ArtifactResult {
    /// The (potentially truncated) output for the model.
    pub output: String,
    /// Path to the full artifact file, if content was truncated.
    pub artifact_path: Option<PathBuf>,
    /// Whether the output was truncated.
    pub was_truncated: bool,
    /// Original size in bytes.
    pub original_size: usize,
    /// Original line count.
    pub original_lines: usize,
}

/// Process tool output, potentially saving to artifact file if too large.
///
/// # Arguments
/// * `output` - The full tool output
/// * `session_id` - Session/conversation ID for organizing artifacts
/// * `tool_name` - Name of the tool that produced the output
/// * `config` - Artifact configuration
///
/// # Returns
/// An `ArtifactResult` containing the processed output and artifact info.
pub fn process_output(
    output: &str,
    session_id: &str,
    tool_name: &str,
    config: &ArtifactConfig,
) -> Result<ArtifactResult> {
    let original_size = output.len();
    let lines: Vec<&str> = output.lines().collect();
    let original_lines = lines.len();

    // Check if truncation is needed
    if !config.enabled || original_size <= config.truncate_threshold {
        return Ok(ArtifactResult {
            output: output.to_string(),
            artifact_path: None,
            was_truncated: false,
            original_size,
            original_lines,
        });
    }

    // Save full output to artifact file
    let artifact_path = save_artifact(output, session_id, tool_name, &config.artifacts_dir)?;

    // Create truncated output
    let truncated = create_truncated_output(&lines, config.truncate_lines, &artifact_path);

    Ok(ArtifactResult {
        output: truncated,
        artifact_path: Some(artifact_path),
        was_truncated: true,
        original_size,
        original_lines,
    })
}

/// Save full output to an artifact file.
fn save_artifact(
    content: &str,
    session_id: &str,
    tool_name: &str,
    artifacts_dir: &Path,
) -> Result<PathBuf> {
    // Create session-specific subdirectory
    let session_dir = artifacts_dir.join(session_id);
    std::fs::create_dir_all(&session_dir)?;

    // Generate unique filename
    let artifact_id = Uuid::new_v4();
    let filename = format!("{}_{}.txt", tool_name, artifact_id);
    let artifact_path = session_dir.join(&filename);

    // Write content
    std::fs::write(&artifact_path, content)?;
    debug!(
        path = %artifact_path.display(),
        size = content.len(),
        "Saved tool artifact"
    );

    Ok(artifact_path)
}

/// Create truncated output with reference to artifact file.
fn create_truncated_output(lines: &[&str], max_lines: usize, artifact_path: &Path) -> String {
    let total_lines = lines.len();
    let omitted = total_lines.saturating_sub(max_lines);

    // Take first portion of lines
    let shown_lines: Vec<&str> = lines.iter().take(max_lines).copied().collect();
    let shown_text = shown_lines.join("\n");

    format!(
        "{}\n\n[... {} more lines omitted ...]\n\n\
         Full output saved to: {}\n\
         [TIP] Use Read tool to view the full artifact if needed.",
        shown_text,
        omitted,
        artifact_path.display()
    )
}

/// Convenience function to process a ToolResult and handle artifacts.
pub fn process_tool_result(
    result: ToolResult,
    session_id: &str,
    tool_name: &str,
    config: &ArtifactConfig,
) -> Result<ToolResult> {
    // Only process successful results
    if !result.success {
        return Ok(result);
    }

    let artifact_result = process_output(&result.output, session_id, tool_name, config)?;

    if artifact_result.was_truncated {
        debug!(
            tool = tool_name,
            original_size = artifact_result.original_size,
            original_lines = artifact_result.original_lines,
            artifact = ?artifact_result.artifact_path,
            "Truncated tool output and saved artifact"
        );
    }

    Ok(ToolResult {
        output: artifact_result.output,
        success: result.success,
        error: result.error,
        metadata: result.metadata,
    })
}

/// Clean up old artifacts for a session.
pub fn cleanup_session_artifacts(session_id: &str, artifacts_dir: &Path) -> Result<()> {
    let session_dir = artifacts_dir.join(session_id);
    if session_dir.exists() {
        std::fs::remove_dir_all(&session_dir)?;
        debug!(session_id, "Cleaned up session artifacts");
    }
    Ok(())
}

/// Clean up artifacts older than the specified duration.
pub fn cleanup_old_artifacts(artifacts_dir: &Path, max_age: std::time::Duration) -> Result<usize> {
    let mut removed = 0;

    if !artifacts_dir.exists() {
        return Ok(0);
    }

    let now = std::time::SystemTime::now();

    for entry in std::fs::read_dir(artifacts_dir)? {
        let entry = entry?;
        let path = entry.path();

        if !path.is_dir() {
            continue;
        }

        // Check directory modification time
        if let Ok(metadata) = path.metadata() {
            if let Ok(modified) = metadata.modified() {
                if let Ok(age) = now.duration_since(modified) {
                    if age > max_age {
                        if let Err(e) = std::fs::remove_dir_all(&path) {
                            warn!(path = %path.display(), error = %e, "Failed to remove old artifact dir");
                        } else {
                            removed += 1;
                            debug!(path = %path.display(), "Removed old artifact directory");
                        }
                    }
                }
            }
        }
    }

    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_no_truncation_below_threshold() {
        let config = ArtifactConfig {
            artifacts_dir: tempdir().unwrap().keep(),
            truncate_threshold: 1000,
            truncate_lines: 10,
            enabled: true,
        };

        let output = "line1\nline2\nline3";
        let result = process_output(output, "test-session", "Glob", &config).unwrap();

        assert!(!result.was_truncated);
        assert!(result.artifact_path.is_none());
        assert_eq!(result.output, output);
    }

    #[test]
    fn test_truncation_above_threshold() {
        let temp_dir = tempdir().unwrap();
        let config = ArtifactConfig {
            artifacts_dir: temp_dir.path().to_path_buf(),
            truncate_threshold: 50,
            truncate_lines: 2,
            enabled: true,
        };

        let output = "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10";
        let result = process_output(output, "test-session", "Glob", &config).unwrap();

        assert!(result.was_truncated);
        assert!(result.artifact_path.is_some());
        assert!(result.output.contains("line1"));
        assert!(result.output.contains("line2"));
        assert!(result.output.contains("more lines omitted"));
        assert!(result.output.contains("Full output saved to"));

        // Verify artifact file was created and contains full output
        let artifact_content = std::fs::read_to_string(result.artifact_path.unwrap()).unwrap();
        assert_eq!(artifact_content, output);
    }

    #[test]
    fn test_disabled_config() {
        let config = ArtifactConfig {
            artifacts_dir: tempdir().unwrap().keep(),
            truncate_threshold: 10,
            truncate_lines: 1,
            enabled: false,
        };

        let output = "this is a very long output that would normally be truncated";
        let result = process_output(output, "test-session", "Glob", &config).unwrap();

        assert!(!result.was_truncated);
        assert_eq!(result.output, output);
    }

    #[test]
    fn test_cleanup_old_artifacts() {
        let temp_dir = tempdir().unwrap();
        let artifacts_dir = temp_dir.path();

        // Create a test session directory
        let session_dir = artifacts_dir.join("old-session");
        std::fs::create_dir_all(&session_dir).unwrap();
        std::fs::write(session_dir.join("test.txt"), "content").unwrap();

        // Set modification time to the past (by using a very short max_age)
        std::thread::sleep(std::time::Duration::from_millis(100));

        let removed =
            cleanup_old_artifacts(artifacts_dir, std::time::Duration::from_millis(50)).unwrap();
        assert_eq!(removed, 1);
        assert!(!session_dir.exists());
    }
}
