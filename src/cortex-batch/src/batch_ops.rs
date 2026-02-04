//! Batch file operations with file locking and atomic writes.
//!
//! This module provides safe batch operations using:
//! - Process-level mutex locks to prevent concurrent access to the same file
//! - Atomic writes (write to temp file, then rename) for data integrity
//! - Proper error handling and cleanup on failures

use crate::{BatchError, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::fs;
use tokio::sync::Mutex as AsyncMutex;
use tracing::{debug, info, warn};

/// Global file lock manager for batch operations.
/// Prevents concurrent modifications to the same file within the process.
static FILE_LOCKS: once_cell::sync::Lazy<Mutex<HashMap<PathBuf, Arc<AsyncMutex<()>>>>> =
    once_cell::sync::Lazy::new(|| Mutex::new(HashMap::new()));

/// Acquire an async lock for a specific file path.
fn get_file_lock(path: &Path) -> Arc<AsyncMutex<()>> {
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let mut locks = FILE_LOCKS.lock().unwrap();
    locks
        .entry(canonical)
        .or_insert_with(|| Arc::new(AsyncMutex::new(())))
        .clone()
}

/// Perform an atomic write operation: write to temp file, then rename.
/// This ensures readers never see partial content.
async fn atomic_write(path: &Path, content: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Cannot determine parent directory",
        )
    })?;

    // Ensure parent directory exists
    if !parent.exists() {
        fs::create_dir_all(parent).await?;
    }

    // Create temp file name in same directory for same-filesystem rename
    let temp_path = parent.join(format!(
        ".{}.tmp.{}",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("file"),
        std::process::id()
    ));

    // Write content to temp file
    fs::write(&temp_path, content).await?;

    // Atomic rename - on Windows, we may need retries due to file locking
    let mut retries = 5;
    loop {
        // On Windows, we may need to remove the target first
        #[cfg(windows)]
        if path.exists() {
            let _ = fs::remove_file(path).await;
        }

        match fs::rename(&temp_path, path).await {
            Ok(()) => break,
            Err(_) if retries > 0 => {
                retries -= 1;
                tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
                continue;
            }
            Err(e) => {
                let _ = fs::remove_file(&temp_path).await;
                return Err(e);
            }
        }
    }

    Ok(())
}

/// Type of batch operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BatchOperation {
    /// Create a new file.
    Create {
        path: PathBuf,
        content: String,
        #[serde(default)]
        overwrite: bool,
    },
    /// Delete a file.
    Delete {
        path: PathBuf,
        #[serde(default)]
        recursive: bool,
    },
    /// Move/rename a file.
    Move {
        from: PathBuf,
        to: PathBuf,
        #[serde(default)]
        overwrite: bool,
    },
    /// Copy a file.
    Copy {
        from: PathBuf,
        to: PathBuf,
        #[serde(default)]
        overwrite: bool,
    },
    /// Create a directory.
    Mkdir {
        path: PathBuf,
        #[serde(default)]
        recursive: bool,
    },
    /// Append to a file.
    Append { path: PathBuf, content: String },
    /// Prepend to a file.
    Prepend { path: PathBuf, content: String },
}

impl BatchOperation {
    pub fn create(path: impl Into<PathBuf>, content: impl Into<String>) -> Self {
        Self::Create {
            path: path.into(),
            content: content.into(),
            overwrite: false,
        }
    }

    pub fn delete(path: impl Into<PathBuf>) -> Self {
        Self::Delete {
            path: path.into(),
            recursive: false,
        }
    }

    pub fn move_file(from: impl Into<PathBuf>, to: impl Into<PathBuf>) -> Self {
        Self::Move {
            from: from.into(),
            to: to.into(),
            overwrite: false,
        }
    }

    pub fn copy(from: impl Into<PathBuf>, to: impl Into<PathBuf>) -> Self {
        Self::Copy {
            from: from.into(),
            to: to.into(),
            overwrite: false,
        }
    }

    pub fn mkdir(path: impl Into<PathBuf>) -> Self {
        Self::Mkdir {
            path: path.into(),
            recursive: true,
        }
    }

    pub fn description(&self) -> String {
        match self {
            Self::Create { path, .. } => format!("Create {}", path.display()),
            Self::Delete { path, .. } => format!("Delete {}", path.display()),
            Self::Move { from, to, .. } => format!("Move {} -> {}", from.display(), to.display()),
            Self::Copy { from, to, .. } => format!("Copy {} -> {}", from.display(), to.display()),
            Self::Mkdir { path, .. } => format!("Create directory {}", path.display()),
            Self::Append { path, .. } => format!("Append to {}", path.display()),
            Self::Prepend { path, .. } => format!("Prepend to {}", path.display()),
        }
    }
}

/// Result of a single batch operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationResult {
    pub operation: String,
    pub success: bool,
    pub error: Option<String>,
}

/// Result of batch operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchResult {
    pub total: usize,
    pub successful: usize,
    pub failed: usize,
    pub results: Vec<OperationResult>,
}

impl BatchResult {
    pub fn new() -> Self {
        Self {
            total: 0,
            successful: 0,
            failed: 0,
            results: Vec::new(),
        }
    }

    pub fn add_success(&mut self, operation: String) {
        self.total += 1;
        self.successful += 1;
        self.results.push(OperationResult {
            operation,
            success: true,
            error: None,
        });
    }

    pub fn add_failure(&mut self, operation: String, error: String) {
        self.total += 1;
        self.failed += 1;
        self.results.push(OperationResult {
            operation,
            success: false,
            error: Some(error),
        });
    }

    pub fn is_success(&self) -> bool {
        self.failed == 0
    }
}

impl Default for BatchResult {
    fn default() -> Self {
        Self::new()
    }
}

/// Batch file operations executor.
pub struct BatchOps {
    /// Dry run mode.
    dry_run: bool,
    /// Stop on first error.
    stop_on_error: bool,
}

impl BatchOps {
    pub fn new() -> Self {
        Self {
            dry_run: false,
            stop_on_error: false,
        }
    }

    pub fn dry_run(mut self, enabled: bool) -> Self {
        self.dry_run = enabled;
        self
    }

    pub fn stop_on_error(mut self, enabled: bool) -> Self {
        self.stop_on_error = enabled;
        self
    }

    /// Execute batch operations.
    pub async fn execute(&self, operations: Vec<BatchOperation>) -> Result<BatchResult> {
        let mut result = BatchResult::new();

        for op in operations {
            let desc = op.description();
            match self.execute_single(op).await {
                Ok(()) => {
                    result.add_success(desc);
                }
                Err(e) => {
                    result.add_failure(desc, e.to_string());
                    if self.stop_on_error {
                        break;
                    }
                }
            }
        }

        info!(
            "Batch operations complete: {} total, {} successful, {} failed",
            result.total, result.successful, result.failed
        );

        Ok(result)
    }

    /// Execute a single operation with file locking.
    async fn execute_single(&self, op: BatchOperation) -> Result<()> {
        if self.dry_run {
            debug!("Dry run: {}", op.description());
            return Ok(());
        }

        match op {
            BatchOperation::Create {
                path,
                content,
                overwrite,
            } => {
                // Acquire lock for the target path
                let lock = get_file_lock(&path);
                let _guard = lock.lock().await;

                if path.exists() && !overwrite {
                    return Err(BatchError::EditFailed(format!(
                        "File already exists: {}",
                        path.display()
                    )));
                }
                // Use atomic write for safe file creation
                atomic_write(&path, content.as_bytes()).await?;
                debug!("Created: {}", path.display());
            }

            BatchOperation::Delete { path, recursive } => {
                // Acquire lock for the target path
                let lock = get_file_lock(&path);
                let _guard = lock.lock().await;

                if !path.exists() {
                    warn!("File not found for deletion: {}", path.display());
                    return Ok(());
                }
                if path.is_dir() {
                    if recursive {
                        fs::remove_dir_all(&path).await?;
                    } else {
                        fs::remove_dir(&path).await?;
                    }
                } else {
                    fs::remove_file(&path).await?;
                }
                debug!("Deleted: {}", path.display());
            }

            BatchOperation::Move {
                from,
                to,
                overwrite,
            } => {
                // Acquire locks for both source and destination
                let from_lock = get_file_lock(&from);
                let to_lock = get_file_lock(&to);

                // Always acquire locks in a consistent order to prevent deadlocks
                let (_guard1, _guard2) = if from < to {
                    (from_lock.lock().await, to_lock.lock().await)
                } else {
                    let g2 = to_lock.lock().await;
                    let g1 = from_lock.lock().await;
                    (g1, g2)
                };

                if !from.exists() {
                    return Err(BatchError::FileNotFound(from.display().to_string()));
                }
                if to.exists() && !overwrite {
                    return Err(BatchError::EditFailed(format!(
                        "Destination already exists: {}",
                        to.display()
                    )));
                }
                if let Some(parent) = to.parent() {
                    fs::create_dir_all(parent).await?;
                }
                fs::rename(&from, &to).await?;
                debug!("Moved: {} -> {}", from.display(), to.display());
            }

            BatchOperation::Copy {
                from,
                to,
                overwrite,
            } => {
                // Acquire locks for both source and destination
                let from_lock = get_file_lock(&from);
                let to_lock = get_file_lock(&to);

                // Always acquire locks in a consistent order to prevent deadlocks
                let (_guard1, _guard2) = if from < to {
                    (from_lock.lock().await, to_lock.lock().await)
                } else {
                    let g2 = to_lock.lock().await;
                    let g1 = from_lock.lock().await;
                    (g1, g2)
                };

                if !from.exists() {
                    return Err(BatchError::FileNotFound(from.display().to_string()));
                }
                if to.exists() && !overwrite {
                    return Err(BatchError::EditFailed(format!(
                        "Destination already exists: {}",
                        to.display()
                    )));
                }
                if let Some(parent) = to.parent() {
                    fs::create_dir_all(parent).await?;
                }
                fs::copy(&from, &to).await?;
                debug!("Copied: {} -> {}", from.display(), to.display());
            }

            BatchOperation::Mkdir { path, recursive } => {
                // Acquire lock for the target path
                let lock = get_file_lock(&path);
                let _guard = lock.lock().await;

                if recursive {
                    fs::create_dir_all(&path).await?;
                } else {
                    fs::create_dir(&path).await?;
                }
                debug!("Created directory: {}", path.display());
            }

            BatchOperation::Append { path, content } => {
                // Acquire lock for the target path to prevent concurrent append/read
                let lock = get_file_lock(&path);
                let _guard = lock.lock().await;

                let existing = if path.exists() {
                    fs::read_to_string(&path).await?
                } else {
                    String::new()
                };
                // Use atomic write for safe file modification
                atomic_write(&path, format!("{}{}", existing, content).as_bytes()).await?;
                debug!("Appended to: {}", path.display());
            }

            BatchOperation::Prepend { path, content } => {
                // Acquire lock for the target path to prevent concurrent prepend/read
                let lock = get_file_lock(&path);
                let _guard = lock.lock().await;

                let existing = if path.exists() {
                    fs::read_to_string(&path).await?
                } else {
                    String::new()
                };
                // Use atomic write for safe file modification
                atomic_write(&path, format!("{}{}", content, existing).as_bytes()).await?;
                debug!("Prepended to: {}", path.display());
            }
        }

        Ok(())
    }
}

impl Default for BatchOps {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_batch_create() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("test.txt");

        let ops = BatchOps::new();
        let result = ops
            .execute(vec![BatchOperation::create(&file, "Hello World")])
            .await
            .unwrap();

        assert!(
            result.is_success(),
            "Batch create failed: {:?}",
            result.results
        );
        assert_eq!(fs::read_to_string(&file).await.unwrap(), "Hello World");
    }

    #[tokio::test]
    async fn test_batch_move() {
        let dir = tempdir().unwrap();
        let from = dir.path().join("from.txt");
        let to = dir.path().join("to.txt");

        fs::write(&from, "content").await.unwrap();

        let ops = BatchOps::new();
        let result = ops
            .execute(vec![BatchOperation::move_file(&from, &to)])
            .await
            .unwrap();

        assert!(result.is_success());
        assert!(!from.exists());
        assert!(to.exists());
    }
}
