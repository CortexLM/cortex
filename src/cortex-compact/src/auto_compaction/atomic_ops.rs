//! Atomic file operations for safe data persistence.

use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::Path;
use tracing::debug;

use super::config::{BACKUP_SUFFIX, TEMP_SUFFIX};

/// Write data to a file atomically using temp file + rename pattern.
///
/// This ensures that the file is never left in a partial/corrupted state:
/// 1. Write to temporary file
/// 2. Sync to disk (fsync)
/// 3. Rename temp file to target file (atomic on POSIX)
pub fn atomic_write<P: AsRef<Path>>(path: P, data: &[u8]) -> io::Result<()> {
    let path = path.as_ref();
    let temp_path = path.with_extension(format!(
        "{}{}",
        path.extension().and_then(|e| e.to_str()).unwrap_or(""),
        TEMP_SUFFIX
    ));

    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    // Write to temp file
    let file = File::create(&temp_path)?;
    let mut writer = BufWriter::new(file);
    writer.write_all(data)?;
    writer.flush()?;

    // Sync to disk for durability
    writer.get_ref().sync_all()?;

    // Atomic rename
    fs::rename(&temp_path, path)?;

    debug!(path = %path.display(), "Atomic write completed");
    Ok(())
}

/// Write data to a file atomically with backup of the original.
///
/// Creates a backup of the original file before overwriting.
pub fn atomic_write_with_backup<P: AsRef<Path>>(path: P, data: &[u8]) -> io::Result<()> {
    let path = path.as_ref();

    // Create backup of original file if it exists
    if path.exists() {
        let backup_path = path.with_extension(format!(
            "{}{}",
            path.extension().and_then(|e| e.to_str()).unwrap_or(""),
            BACKUP_SUFFIX
        ));
        fs::copy(path, &backup_path)?;
    }

    atomic_write(path, data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_atomic_write() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.txt");

        let data = b"Hello, World!";
        atomic_write(&file_path, data).unwrap();

        let content = fs::read_to_string(&file_path).unwrap();
        assert_eq!(content, "Hello, World!");
    }
}
