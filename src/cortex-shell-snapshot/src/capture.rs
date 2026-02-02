//! Shell state capture.

use super::{Result, ShellSnapshot, ShellType, SnapshotError, SnapshotMetadata, scripts};
use std::path::Path;
use std::time::Duration;
use tokio::process::Command;
use uuid::Uuid;

/// Options for capturing shell state.
#[derive(Debug, Clone)]
pub struct CaptureOptions {
    /// Shell type to capture.
    pub shell_type: ShellType,

    /// Timeout for capture operation.
    pub timeout: Duration,

    /// Session ID for the snapshot.
    pub session_id: Uuid,

    /// Additional environment variables to set.
    pub env_vars: Vec<(String, String)>,
}

impl CaptureOptions {
    /// Create new capture options.
    pub fn new(shell_type: ShellType, session_id: Uuid) -> Self {
        Self {
            shell_type,
            timeout: super::SNAPSHOT_TIMEOUT,
            session_id,
            env_vars: Vec::new(),
        }
    }

    /// Set the timeout.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Add an environment variable.
    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env_vars.push((key.into(), value.into()));
        self
    }
}

/// Capture shell state and save to a snapshot.
pub async fn capture_shell_state(
    options: CaptureOptions,
    output_dir: &Path,
) -> Result<ShellSnapshot> {
    let shell_type = options.shell_type;

    // Check if shell supports snapshotting
    if !shell_type.supports_snapshot() {
        return Err(SnapshotError::UnsupportedShell(shell_type.to_string()));
    }

    // Get the capture script
    let script = scripts::capture_script(shell_type);
    if script.is_empty() {
        return Err(SnapshotError::UnsupportedShell(shell_type.to_string()));
    }

    // Run the capture script
    let output = run_capture_script(shell_type, script, &options).await?;

    // Create snapshot metadata
    let mut metadata = SnapshotMetadata::new(options.session_id, shell_type);
    metadata.size_bytes = output.len() as u64;

    // Create snapshot path
    let snapshot_path = ShellSnapshot::path_for(output_dir, options.session_id, shell_type);

    // Create and save snapshot
    let snapshot = ShellSnapshot::new(snapshot_path, metadata);
    snapshot.save(&output).await?;

    Ok(snapshot)
}

/// Run the capture script and return output.
async fn run_capture_script(
    shell_type: ShellType,
    script: &str,
    options: &CaptureOptions,
) -> Result<String> {
    let shell_path = shell_type.default_path();

    // Build command
    let mut cmd = Command::new(shell_path);
    cmd.arg("-c").arg(script);

    // Add environment variables
    for (key, value) in &options.env_vars {
        cmd.env(key, value);
    }

    // Set HOME if not set (needed for rc file discovery)
    if std::env::var("HOME").is_err()
        && let Some(home) = dirs::home_dir()
    {
        cmd.env("HOME", home);
    }

    // Run with timeout
    let output = tokio::time::timeout(options.timeout, cmd.output())
        .await
        .map_err(|_| SnapshotError::Timeout)?
        .map_err(|e| SnapshotError::CaptureFailed(e.to_string()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(SnapshotError::CaptureFailed(format!(
            "Shell exited with status {}: {}",
            output.status,
            stderr.trim()
        )));
    }

    let stdout = String::from_utf8(output.stdout)
        .map_err(|e| SnapshotError::CaptureFailed(format!("Invalid UTF-8 output: {e}")))?;

    // Basic validation
    if stdout.trim().is_empty() {
        return Err(SnapshotError::CaptureFailed(
            "Capture produced empty output".to_string(),
        ));
    }

    Ok(stdout)
}

/// Capture shell state asynchronously in background.
pub fn start_capture(
    options: CaptureOptions,
    output_dir: std::path::PathBuf,
) -> tokio::sync::watch::Receiver<Option<std::sync::Arc<ShellSnapshot>>> {
    let (tx, rx) = tokio::sync::watch::channel(None);

    tokio::spawn(async move {
        match capture_shell_state(options, &output_dir).await {
            Ok(snapshot) => {
                let _ = tx.send(Some(std::sync::Arc::new(snapshot)));
            }
            Err(e) => {
                tracing::warn!("Shell snapshot capture failed: {}", e);
            }
        }
    });

    rx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_capture_options() {
        let options = CaptureOptions::new(ShellType::Bash, Uuid::new_v4())
            .timeout(Duration::from_secs(30))
            .env("FOO", "bar");

        assert_eq!(options.shell_type, ShellType::Bash);
        assert_eq!(options.timeout, Duration::from_secs(30));
        assert_eq!(options.env_vars.len(), 1);
    }
}
