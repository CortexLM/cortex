//! Multi-file editing operations with file locking.
//!
//! This module provides safe multi-file editing using:
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

/// Global file lock manager for multi-edit operations.
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
            Err(e) if retries > 0 => {
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

/// A single edit operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditOperation {
    /// File path to edit.
    pub file_path: PathBuf,
    /// Old text to find.
    pub old_text: String,
    /// New text to replace with.
    pub new_text: String,
    /// Whether to replace all occurrences.
    #[serde(default)]
    pub replace_all: bool,
    /// Optional description.
    pub description: Option<String>,
}

impl EditOperation {
    pub fn new(
        file_path: impl Into<PathBuf>,
        old_text: impl Into<String>,
        new_text: impl Into<String>,
    ) -> Self {
        Self {
            file_path: file_path.into(),
            old_text: old_text.into(),
            new_text: new_text.into(),
            replace_all: false,
            description: None,
        }
    }

    pub fn replace_all(mut self) -> Self {
        self.replace_all = true;
        self
    }

    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }
}

/// Result of a single edit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditResult {
    pub file_path: PathBuf,
    pub success: bool,
    pub replacements: usize,
    pub error: Option<String>,
}

/// Result of multi-edit operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultiEditResult {
    pub total_files: usize,
    pub successful: usize,
    pub failed: usize,
    pub total_replacements: usize,
    pub results: Vec<EditResult>,
}

impl MultiEditResult {
    pub fn new() -> Self {
        Self {
            total_files: 0,
            successful: 0,
            failed: 0,
            total_replacements: 0,
            results: Vec::new(),
        }
    }

    pub fn add_success(&mut self, file_path: PathBuf, replacements: usize) {
        self.total_files += 1;
        self.successful += 1;
        self.total_replacements += replacements;
        self.results.push(EditResult {
            file_path,
            success: true,
            replacements,
            error: None,
        });
    }

    pub fn add_failure(&mut self, file_path: PathBuf, error: String) {
        self.total_files += 1;
        self.failed += 1;
        self.results.push(EditResult {
            file_path,
            success: false,
            replacements: 0,
            error: Some(error),
        });
    }

    pub fn is_success(&self) -> bool {
        self.failed == 0
    }
}

impl Default for MultiEditResult {
    fn default() -> Self {
        Self::new()
    }
}

/// Multi-file editor.
pub struct MultiEdit {
    /// Dry run mode (don't actually modify files).
    dry_run: bool,
    /// Create backups before editing.
    backup: bool,
    /// Backup suffix.
    backup_suffix: String,
}

impl MultiEdit {
    pub fn new() -> Self {
        Self {
            dry_run: false,
            backup: false,
            backup_suffix: ".bak".to_string(),
        }
    }

    pub fn dry_run(mut self, enabled: bool) -> Self {
        self.dry_run = enabled;
        self
    }

    pub fn with_backup(mut self, enabled: bool) -> Self {
        self.backup = enabled;
        self
    }

    pub fn backup_suffix(mut self, suffix: impl Into<String>) -> Self {
        self.backup_suffix = suffix.into();
        self
    }

    /// Execute multiple edit operations.
    pub async fn execute(&self, operations: Vec<EditOperation>) -> Result<MultiEditResult> {
        let mut result = MultiEditResult::new();

        // Group operations by file
        let mut by_file: HashMap<PathBuf, Vec<EditOperation>> = HashMap::new();
        for op in operations {
            by_file.entry(op.file_path.clone()).or_default().push(op);
        }

        for (file_path, ops) in by_file {
            match self.edit_file(&file_path, ops).await {
                Ok(replacements) => {
                    result.add_success(file_path, replacements);
                }
                Err(e) => {
                    result.add_failure(file_path, e.to_string());
                }
            }
        }

        info!(
            "MultiEdit complete: {} files, {} successful, {} failed, {} replacements",
            result.total_files, result.successful, result.failed, result.total_replacements
        );

        Ok(result)
    }

    /// Edit a single file with multiple operations.
    /// Uses file locking and atomic writes to prevent race conditions.
    async fn edit_file(&self, path: &Path, operations: Vec<EditOperation>) -> Result<usize> {
        // Acquire lock for this file to prevent concurrent edits
        let lock = get_file_lock(path);
        let _guard = lock.lock().await;

        if !path.exists() {
            return Err(BatchError::FileNotFound(path.display().to_string()));
        }

        // Read file while holding the lock
        let mut content = fs::read_to_string(path).await?;
        let original = content.clone();
        let mut total_replacements = 0;

        for op in operations {
            let (new_content, count) = if op.replace_all {
                let count = content.matches(&op.old_text).count();
                (content.replace(&op.old_text, &op.new_text), count)
            } else if content.contains(&op.old_text) {
                (content.replacen(&op.old_text, &op.new_text, 1), 1)
            } else {
                (content.clone(), 0)
            };

            if count == 0 {
                warn!(
                    "Pattern not found in {}: {:?}",
                    path.display(),
                    op.old_text.chars().take(50).collect::<String>()
                );
            }

            content = new_content;
            total_replacements += count;
        }

        if content != original && !self.dry_run {
            // Create backup if enabled
            if self.backup {
                let backup_path = path.with_extension(format!(
                    "{}{}",
                    path.extension()
                        .map(|e| e.to_string_lossy())
                        .unwrap_or_default(),
                    self.backup_suffix
                ));
                fs::copy(path, &backup_path).await?;
                debug!("Created backup: {}", backup_path.display());
            }

            // Use atomic write to ensure file is never partially written
            atomic_write(path, content.as_bytes()).await?;
            debug!(
                "Edited {}: {} replacements",
                path.display(),
                total_replacements
            );
        }

        Ok(total_replacements)
    }

    /// Execute a single edit operation.
    pub async fn edit_single(&self, operation: EditOperation) -> Result<EditResult> {
        let file_path = operation.file_path.clone();
        match self.edit_file(&file_path, vec![operation]).await {
            Ok(replacements) => Ok(EditResult {
                file_path,
                success: true,
                replacements,
                error: None,
            }),
            Err(e) => Ok(EditResult {
                file_path,
                success: false,
                replacements: 0,
                error: Some(e.to_string()),
            }),
        }
    }
}

impl Default for MultiEdit {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_multi_edit() {
        let dir = tempdir().unwrap();
        let file1 = dir.path().join("test1.txt");
        let file2 = dir.path().join("test2.txt");

        fs::write(&file1, "Hello World").await.unwrap();
        fs::write(&file2, "Hello World").await.unwrap();

        let editor = MultiEdit::new();
        let ops = vec![
            EditOperation::new(&file1, "World", "Rust"),
            EditOperation::new(&file2, "World", "Cortex"),
        ];

        let result = editor.execute(ops).await.unwrap();
        assert_eq!(result.successful, 2);
        assert_eq!(result.total_replacements, 2);

        assert_eq!(fs::read_to_string(&file1).await.unwrap(), "Hello Rust");
        assert_eq!(fs::read_to_string(&file2).await.unwrap(), "Hello Cortex");
    }

    #[tokio::test]
    async fn test_replace_all() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("test.txt");

        fs::write(&file, "foo bar foo baz foo").await.unwrap();

        let editor = MultiEdit::new();
        let ops = vec![EditOperation::new(&file, "foo", "qux").replace_all()];

        let result = editor.execute(ops).await.unwrap();
        assert_eq!(result.total_replacements, 3);

        assert_eq!(
            fs::read_to_string(&file).await.unwrap(),
            "qux bar qux baz qux"
        );
    }
}
