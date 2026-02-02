//! Backup and restore functionality for undo capability.

use crate::error::{PatchError, PatchResult};
use crate::hunk::FileChange;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Manages backup files for undo capability.
#[derive(Debug)]
pub struct BackupManager {
    /// Base directory for backups.
    backup_dir: PathBuf,
}

impl BackupManager {
    /// Create a new backup manager with the given backup directory.
    pub fn new(backup_dir: PathBuf) -> Self {
        Self { backup_dir }
    }

    /// Create backups of all files that will be modified.
    pub fn create_backup(&mut self, changes: &[FileChange], cwd: &Path) -> PatchResult<BackupSet> {
        // Generate unique backup ID
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let backup_id = format!("patch-backup-{timestamp}");

        let backup_path = self.backup_dir.join(&backup_id);
        fs::create_dir_all(&backup_path).map_err(|e| PatchError::BackupError {
            path: backup_path.clone(),
            message: format!("Failed to create backup directory: {e}"),
        })?;

        let mut file_backups = HashMap::new();
        let mut new_files = Vec::new();
        let mut deleted_files = Vec::new();

        for change in changes {
            // Handle new files (need to track for deletion on restore)
            if change.is_new_file {
                if let Some(ref new_path) = change.new_path {
                    new_files.push(new_path.clone());
                }
                continue;
            }

            // Handle deleted files (need to backup content)
            if change.is_deleted {
                if let Some(ref old_path) = change.old_path {
                    let full_path = cwd.join(old_path);
                    if full_path.exists() {
                        let backup_file = backup_path.join(sanitize_path_for_backup(old_path));

                        // Create parent directories in backup
                        if let Some(parent) = backup_file.parent() {
                            fs::create_dir_all(parent).map_err(|e| PatchError::BackupError {
                                path: parent.to_path_buf(),
                                message: format!("Failed to create backup subdirectory: {e}"),
                            })?;
                        }

                        fs::copy(&full_path, &backup_file).map_err(|e| {
                            PatchError::BackupError {
                                path: full_path.clone(),
                                message: format!("Failed to backup file: {e}"),
                            }
                        })?;

                        deleted_files.push(old_path.clone());
                        file_backups.insert(old_path.clone(), backup_file);
                    }
                }
                continue;
            }

            // Handle modifications (backup existing content)
            if let Some(path) = change.effective_path() {
                let full_path = cwd.join(path);
                if full_path.exists() {
                    let backup_file = backup_path.join(sanitize_path_for_backup(path));

                    // Create parent directories in backup
                    if let Some(parent) = backup_file.parent() {
                        fs::create_dir_all(parent).map_err(|e| PatchError::BackupError {
                            path: parent.to_path_buf(),
                            message: format!("Failed to create backup subdirectory: {e}"),
                        })?;
                    }

                    fs::copy(&full_path, &backup_file).map_err(|e| PatchError::BackupError {
                        path: full_path.clone(),
                        message: format!("Failed to backup file: {e}"),
                    })?;

                    file_backups.insert(path.clone(), backup_file);
                }
            }
        }

        // Write metadata
        let metadata = BackupMetadata {
            backup_id: backup_id.clone(),
            timestamp,
            file_backups: file_backups.keys().cloned().collect(),
            new_files: new_files.clone(),
            deleted_files: deleted_files.clone(),
        };

        let metadata_path = backup_path.join("metadata.json");
        let metadata_json =
            serde_json::to_string_pretty(&metadata).map_err(|e| PatchError::BackupError {
                path: metadata_path.clone(),
                message: format!("Failed to serialize metadata: {e}"),
            })?;

        fs::write(&metadata_path, metadata_json).map_err(|e| PatchError::BackupError {
            path: metadata_path,
            message: format!("Failed to write metadata: {e}"),
        })?;

        Ok(BackupSet {
            backup_id,
            backup_path,
            file_backups,
            new_files,
            deleted_files,
        })
    }

    /// Restore files from a backup set.
    pub fn restore(&self, backup_set: &BackupSet, cwd: &Path) -> PatchResult<()> {
        // First, delete any new files that were created
        for new_file in &backup_set.new_files {
            let full_path = cwd.join(new_file);
            if full_path.exists() {
                fs::remove_file(&full_path).map_err(|e| PatchError::RestoreError {
                    path: full_path,
                    message: format!("Failed to remove new file: {e}"),
                })?;
            }
        }

        // Restore modified and deleted files
        for (original_path, backup_file) in &backup_set.file_backups {
            let full_path = cwd.join(original_path);

            // Create parent directories if needed
            if let Some(parent) = full_path.parent() {
                fs::create_dir_all(parent).map_err(|e| PatchError::RestoreError {
                    path: parent.to_path_buf(),
                    message: format!("Failed to create directory: {e}"),
                })?;
            }

            // Restore from backup
            fs::copy(backup_file, &full_path).map_err(|e| PatchError::RestoreError {
                path: full_path,
                message: format!("Failed to restore file: {e}"),
            })?;
        }

        Ok(())
    }

    /// Load a backup set from disk.
    pub fn load_backup(&self, backup_id: &str) -> PatchResult<BackupSet> {
        let backup_path = self.backup_dir.join(backup_id);
        let metadata_path = backup_path.join("metadata.json");

        let metadata_json =
            fs::read_to_string(&metadata_path).map_err(|e| PatchError::BackupError {
                path: metadata_path.clone(),
                message: format!("Failed to read metadata: {e}"),
            })?;

        let metadata: BackupMetadata =
            serde_json::from_str(&metadata_json).map_err(|e| PatchError::BackupError {
                path: metadata_path,
                message: format!("Failed to parse metadata: {e}"),
            })?;

        // Rebuild file_backups map
        let mut file_backups = HashMap::new();
        for path in &metadata.file_backups {
            let backup_file = backup_path.join(sanitize_path_for_backup(path));
            if backup_file.exists() {
                file_backups.insert(path.clone(), backup_file);
            }
        }

        Ok(BackupSet {
            backup_id: metadata.backup_id,
            backup_path,
            file_backups,
            new_files: metadata.new_files,
            deleted_files: metadata.deleted_files,
        })
    }

    /// List all available backups.
    pub fn list_backups(&self) -> PatchResult<Vec<BackupInfo>> {
        let mut backups = Vec::new();

        if !self.backup_dir.exists() {
            return Ok(backups);
        }

        let entries = fs::read_dir(&self.backup_dir).map_err(|e| PatchError::BackupError {
            path: self.backup_dir.clone(),
            message: format!("Failed to read backup directory: {e}"),
        })?;

        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let metadata_path = path.join("metadata.json");
                if metadata_path.exists()
                    && let Ok(content) = fs::read_to_string(&metadata_path)
                    && let Ok(metadata) = serde_json::from_str::<BackupMetadata>(&content)
                {
                    backups.push(BackupInfo {
                        backup_id: metadata.backup_id,
                        timestamp: metadata.timestamp,
                        files_count: metadata.file_backups.len(),
                    });
                }
            }
        }

        // Sort by timestamp (newest first)
        backups.sort_by_key(|b| std::cmp::Reverse(b.timestamp));

        Ok(backups)
    }

    /// Clean up old backups, keeping only the most recent N.
    pub fn cleanup(&self, keep_count: usize) -> PatchResult<usize> {
        let backups = self.list_backups()?;
        let mut removed = 0;

        for backup in backups.into_iter().skip(keep_count) {
            let backup_path = self.backup_dir.join(&backup.backup_id);
            if fs::remove_dir_all(&backup_path).is_ok() {
                removed += 1;
            }
        }

        Ok(removed)
    }
}

/// A set of backup files.
#[derive(Debug, Clone)]
pub struct BackupSet {
    /// Unique identifier for this backup.
    pub backup_id: String,
    /// Path to the backup directory.
    pub backup_path: PathBuf,
    /// Map from original paths to backup file paths.
    pub file_backups: HashMap<PathBuf, PathBuf>,
    /// Paths of new files that were created (to be deleted on restore).
    pub new_files: Vec<PathBuf>,
    /// Paths of files that were deleted (to be restored from backup).
    pub deleted_files: Vec<PathBuf>,
}

impl BackupSet {
    /// Create an empty backup set.
    pub fn empty() -> Self {
        Self {
            backup_id: String::new(),
            backup_path: PathBuf::new(),
            file_backups: HashMap::new(),
            new_files: Vec::new(),
            deleted_files: Vec::new(),
        }
    }

    /// Check if this backup set is empty.
    pub fn is_empty(&self) -> bool {
        self.file_backups.is_empty() && self.new_files.is_empty() && self.deleted_files.is_empty()
    }

    /// Get the total number of files tracked.
    pub fn file_count(&self) -> usize {
        self.file_backups.len() + self.new_files.len()
    }
}

/// Metadata stored with each backup.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct BackupMetadata {
    backup_id: String,
    timestamp: u128,
    file_backups: Vec<PathBuf>,
    new_files: Vec<PathBuf>,
    deleted_files: Vec<PathBuf>,
}

/// Summary information about a backup.
#[derive(Debug, Clone)]
pub struct BackupInfo {
    /// Unique identifier.
    pub backup_id: String,
    /// When the backup was created (milliseconds since epoch).
    pub timestamp: u128,
    /// Number of files in the backup.
    pub files_count: usize,
}

/// Sanitize a path for use as a backup filename.
fn sanitize_path_for_backup(path: &Path) -> PathBuf {
    let path_str = path.to_string_lossy();

    // Replace problematic characters
    let sanitized = path_str.replace(':', "_").replace('\\', "/");

    PathBuf::from(sanitized)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_test_file(dir: &Path, name: &str, content: &str) -> PathBuf {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn test_backup_and_restore() {
        let work_dir = TempDir::new().unwrap();
        let backup_dir = TempDir::new().unwrap();

        // Create a test file
        let file_path = create_test_file(work_dir.path(), "test.txt", "original content");

        // Create a mock file change
        let changes = vec![FileChange::new(
            Some(PathBuf::from("test.txt")),
            Some(PathBuf::from("test.txt")),
        )];

        // Create backup
        let mut manager = BackupManager::new(backup_dir.path().to_path_buf());
        let backup_set = manager.create_backup(&changes, work_dir.path()).unwrap();

        assert!(!backup_set.is_empty());
        assert!(
            backup_set
                .file_backups
                .contains_key(&PathBuf::from("test.txt"))
        );

        // Modify the file
        fs::write(&file_path, "modified content").unwrap();
        assert_eq!(fs::read_to_string(&file_path).unwrap(), "modified content");

        // Restore
        manager.restore(&backup_set, work_dir.path()).unwrap();
        assert_eq!(fs::read_to_string(&file_path).unwrap(), "original content");
    }

    #[test]
    fn test_backup_new_file() {
        let work_dir = TempDir::new().unwrap();
        let backup_dir = TempDir::new().unwrap();

        // Create a change that represents a new file
        let mut change = FileChange::new(
            Some(PathBuf::from("/dev/null")),
            Some(PathBuf::from("new_file.txt")),
        );
        change.is_new_file = true;

        let changes = vec![change];

        // Create backup
        let mut manager = BackupManager::new(backup_dir.path().to_path_buf());
        let backup_set = manager.create_backup(&changes, work_dir.path()).unwrap();

        // New file should be tracked for deletion on restore
        assert!(
            backup_set
                .new_files
                .contains(&PathBuf::from("new_file.txt"))
        );

        // Simulate the new file being created
        create_test_file(work_dir.path(), "new_file.txt", "new content");
        assert!(work_dir.path().join("new_file.txt").exists());

        // Restore should delete the new file
        manager.restore(&backup_set, work_dir.path()).unwrap();
        assert!(!work_dir.path().join("new_file.txt").exists());
    }

    #[test]
    fn test_backup_deleted_file() {
        let work_dir = TempDir::new().unwrap();
        let backup_dir = TempDir::new().unwrap();

        // Create a file to be deleted
        create_test_file(work_dir.path(), "to_delete.txt", "will be deleted");

        // Create a change that represents a file deletion
        let mut change = FileChange::new(
            Some(PathBuf::from("to_delete.txt")),
            Some(PathBuf::from("/dev/null")),
        );
        change.is_deleted = true;

        let changes = vec![change];

        // Create backup
        let mut manager = BackupManager::new(backup_dir.path().to_path_buf());
        let backup_set = manager.create_backup(&changes, work_dir.path()).unwrap();

        // File should be backed up
        assert!(
            backup_set
                .file_backups
                .contains_key(&PathBuf::from("to_delete.txt"))
        );

        // Simulate the file being deleted
        fs::remove_file(work_dir.path().join("to_delete.txt")).unwrap();
        assert!(!work_dir.path().join("to_delete.txt").exists());

        // Restore should recreate the file
        manager.restore(&backup_set, work_dir.path()).unwrap();
        assert!(work_dir.path().join("to_delete.txt").exists());
        assert_eq!(
            fs::read_to_string(work_dir.path().join("to_delete.txt")).unwrap(),
            "will be deleted"
        );
    }

    #[test]
    fn test_list_and_cleanup_backups() {
        let work_dir = TempDir::new().unwrap();
        let backup_dir = TempDir::new().unwrap();

        create_test_file(work_dir.path(), "test.txt", "content");

        let changes = vec![FileChange::new(
            Some(PathBuf::from("test.txt")),
            Some(PathBuf::from("test.txt")),
        )];

        let mut manager = BackupManager::new(backup_dir.path().to_path_buf());

        // Create multiple backups
        for _ in 0..3 {
            manager.create_backup(&changes, work_dir.path()).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        // List backups
        let backups = manager.list_backups().unwrap();
        assert_eq!(backups.len(), 3);

        // Cleanup, keeping only 1
        let removed = manager.cleanup(1).unwrap();
        assert_eq!(removed, 2);

        // Verify only 1 remains
        let backups = manager.list_backups().unwrap();
        assert_eq!(backups.len(), 1);
    }

    #[test]
    fn test_load_backup() {
        let work_dir = TempDir::new().unwrap();
        let backup_dir = TempDir::new().unwrap();

        create_test_file(work_dir.path(), "test.txt", "content");

        let changes = vec![FileChange::new(
            Some(PathBuf::from("test.txt")),
            Some(PathBuf::from("test.txt")),
        )];

        let mut manager = BackupManager::new(backup_dir.path().to_path_buf());
        let original_set = manager.create_backup(&changes, work_dir.path()).unwrap();

        // Load the backup
        let loaded_set = manager.load_backup(&original_set.backup_id).unwrap();

        assert_eq!(loaded_set.backup_id, original_set.backup_id);
        assert_eq!(
            loaded_set.file_backups.len(),
            original_set.file_backups.len()
        );
    }

    #[test]
    fn test_empty_backup_set() {
        let set = BackupSet::empty();
        assert!(set.is_empty());
        assert_eq!(set.file_count(), 0);
    }

    #[test]
    fn test_sanitize_path() {
        let path = Path::new("src/main.rs");
        let sanitized = sanitize_path_for_backup(path);
        assert_eq!(sanitized, PathBuf::from("src/main.rs"));

        // Windows-style path
        let path = Path::new("src\\main.rs");
        let sanitized = sanitize_path_for_backup(path);
        assert!(
            sanitized.to_string_lossy().contains("/")
                || sanitized.to_string_lossy().contains("main.rs")
        );
    }
}
