//! File utilities.
//!
//! Provides utilities for file operations including
//! reading, writing, watching, and transformations.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use tokio::fs;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::error::Result;

/// File metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileMetadata {
    /// File path.
    pub path: PathBuf,
    /// File size in bytes.
    pub size: u64,
    /// Is directory.
    pub is_dir: bool,
    /// Is symlink.
    pub is_symlink: bool,
    /// Modified time (unix timestamp).
    pub modified: u64,
    /// Created time (unix timestamp).
    pub created: Option<u64>,
    /// File permissions (unix).
    #[cfg(unix)]
    pub permissions: u32,
}

impl FileMetadata {
    /// Get metadata for a path.
    pub async fn from_path(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let metadata = fs::metadata(path).await?;

        let modified = metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let created = metadata
            .created()
            .ok()
            .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map(|d| d.as_secs());

        #[cfg(unix)]
        let permissions = {
            use std::os::unix::fs::PermissionsExt;
            metadata.permissions().mode()
        };

        Ok(Self {
            path: path.to_path_buf(),
            size: metadata.len(),
            is_dir: metadata.is_dir(),
            is_symlink: metadata.file_type().is_symlink(),
            modified,
            created,
            #[cfg(unix)]
            permissions,
        })
    }

    /// Check if file is readable.
    pub fn is_readable(&self) -> bool {
        #[cfg(unix)]
        {
            self.permissions & 0o444 != 0
        }
        #[cfg(not(unix))]
        {
            true
        }
    }

    /// Check if file is writable.
    pub fn is_writable(&self) -> bool {
        #[cfg(unix)]
        {
            self.permissions & 0o222 != 0
        }
        #[cfg(not(unix))]
        {
            true
        }
    }

    /// Check if file is executable.
    pub fn is_executable(&self) -> bool {
        #[cfg(unix)]
        {
            self.permissions & 0o111 != 0
        }
        #[cfg(not(unix))]
        {
            false
        }
    }

    /// Get file extension.
    pub fn extension(&self) -> Option<&str> {
        self.path.extension().and_then(|e| e.to_str())
    }

    /// Get file name.
    pub fn name(&self) -> Option<&str> {
        self.path.file_name().and_then(|n| n.to_str())
    }

    /// Format size for display.
    pub fn format_size(&self) -> String {
        format_bytes(self.size)
    }
}

/// Format bytes for human reading.
pub fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    const TB: u64 = GB * 1024;

    if bytes >= TB {
        format!("{:.2} TB", bytes as f64 / TB as f64)
    } else if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}

/// Read a file to string with normalized line endings.
///
/// This function reads the file content and normalizes Windows CRLF line
/// endings to Unix LF, ensuring consistent handling across platforms.
pub async fn read_string(path: impl AsRef<Path>) -> Result<String> {
    let content = fs::read_to_string(path.as_ref()).await?;
    // Normalize CRLF to LF for consistent cross-platform handling
    Ok(content.replace("\r\n", "\n").replace('\r', "\n"))
}

/// Read a file to string without line ending normalization.
///
/// Use this when you need to preserve the original line endings.
pub async fn read_string_raw(path: impl AsRef<Path>) -> Result<String> {
    fs::read_to_string(path.as_ref()).await.map_err(Into::into)
}

/// Read a file to bytes.
pub async fn read_bytes(path: impl AsRef<Path>) -> Result<Vec<u8>> {
    fs::read(path.as_ref()).await.map_err(Into::into)
}

/// Read file lines.
pub async fn read_lines(path: impl AsRef<Path>) -> Result<Vec<String>> {
    let file = fs::File::open(path.as_ref()).await?;
    let reader = BufReader::new(file);
    let mut lines = reader.lines();
    let mut result = Vec::new();

    while let Some(line) = lines.next_line().await? {
        result.push(line);
    }

    Ok(result)
}

/// Read file with line range.
pub async fn read_range(
    path: impl AsRef<Path>,
    start: usize,
    end: Option<usize>,
) -> Result<Vec<String>> {
    let file = fs::File::open(path.as_ref()).await?;
    let reader = BufReader::new(file);
    let mut lines = reader.lines();
    let mut result = Vec::new();
    let mut line_num = 0;

    while let Some(line) = lines.next_line().await? {
        if line_num >= start {
            if let Some(e) = end
                && line_num >= e
            {
                break;
            }
            result.push(line);
        }
        line_num += 1;
    }

    Ok(result)
}

/// Write string to file.
pub async fn write_string(path: impl AsRef<Path>, content: impl AsRef<str>) -> Result<()> {
    fs::write(path.as_ref(), content.as_ref())
        .await
        .map_err(Into::into)
}

/// Write bytes to file.
pub async fn write_bytes(path: impl AsRef<Path>, content: impl AsRef<[u8]>) -> Result<()> {
    fs::write(path.as_ref(), content.as_ref())
        .await
        .map_err(Into::into)
}

/// Append to file.
pub async fn append_string(path: impl AsRef<Path>, content: impl AsRef<str>) -> Result<()> {
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path.as_ref())
        .await?;

    file.write_all(content.as_ref().as_bytes()).await?;
    Ok(())
}

/// Copy file.
pub async fn copy_file(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> Result<u64> {
    fs::copy(src.as_ref(), dst.as_ref())
        .await
        .map_err(Into::into)
}

/// Move/rename file.
pub async fn move_file(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> Result<()> {
    fs::rename(src.as_ref(), dst.as_ref())
        .await
        .map_err(Into::into)
}

/// Remove file.
pub async fn remove_file(path: impl AsRef<Path>) -> Result<()> {
    fs::remove_file(path.as_ref()).await.map_err(Into::into)
}

/// Create directory.
pub async fn create_dir(path: impl AsRef<Path>) -> Result<()> {
    fs::create_dir(path.as_ref()).await.map_err(Into::into)
}

/// Create directory recursively.
pub async fn create_dir_all(path: impl AsRef<Path>) -> Result<()> {
    fs::create_dir_all(path.as_ref()).await.map_err(Into::into)
}

/// Remove directory.
pub async fn remove_dir(path: impl AsRef<Path>) -> Result<()> {
    fs::remove_dir(path.as_ref()).await.map_err(Into::into)
}

/// Remove directory recursively.
pub async fn remove_dir_all(path: impl AsRef<Path>) -> Result<()> {
    fs::remove_dir_all(path.as_ref()).await.map_err(Into::into)
}

/// List directory contents.
pub async fn list_dir(path: impl AsRef<Path>) -> Result<Vec<PathBuf>> {
    let mut result = Vec::new();
    let mut entries = fs::read_dir(path.as_ref()).await?;

    while let Some(entry) = entries.next_entry().await? {
        result.push(entry.path());
    }

    Ok(result)
}

/// List directory with metadata.
pub async fn list_dir_detailed(path: impl AsRef<Path>) -> Result<Vec<FileMetadata>> {
    let mut result = Vec::new();
    let mut entries = fs::read_dir(path.as_ref()).await?;

    while let Some(entry) = entries.next_entry().await? {
        if let Ok(meta) = FileMetadata::from_path(entry.path()).await {
            result.push(meta);
        }
    }

    Ok(result)
}

/// Walk directory recursively.
pub async fn walk_dir(path: impl AsRef<Path>) -> Result<Vec<PathBuf>> {
    let mut result = Vec::new();
    walk_dir_recursive(path.as_ref(), &mut result).await?;
    Ok(result)
}

async fn walk_dir_recursive(path: &Path, result: &mut Vec<PathBuf>) -> Result<()> {
    let mut entries = fs::read_dir(path).await?;

    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();

        if path.is_dir() {
            Box::pin(walk_dir_recursive(&path, result)).await?;
        } else {
            result.push(path);
        }
    }

    Ok(())
}

/// Find files matching pattern.
pub async fn find_files(dir: impl AsRef<Path>, pattern: &str) -> Result<Vec<PathBuf>> {
    let all_files = walk_dir(dir).await?;
    let pattern = pattern.to_lowercase();

    Ok(all_files
        .into_iter()
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.to_lowercase().contains(&pattern))
                .unwrap_or(false)
        })
        .collect())
}

/// Check if path exists.
pub async fn exists(path: impl AsRef<Path>) -> bool {
    fs::metadata(path.as_ref()).await.is_ok()
}

/// Check if path is file.
pub async fn is_file(path: impl AsRef<Path>) -> bool {
    fs::metadata(path.as_ref())
        .await
        .map(|m| m.is_file())
        .unwrap_or(false)
}

/// Check if path is directory.
pub async fn is_dir(path: impl AsRef<Path>) -> bool {
    fs::metadata(path.as_ref())
        .await
        .map(|m| m.is_dir())
        .unwrap_or(false)
}

/// Get file size.
pub async fn file_size(path: impl AsRef<Path>) -> Result<u64> {
    let metadata = fs::metadata(path.as_ref()).await?;
    Ok(metadata.len())
}

/// Create temp file.
pub async fn create_temp_file(prefix: &str, suffix: &str) -> Result<PathBuf> {
    let dir = std::env::temp_dir();
    let timestamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);

    let filename = format!("{prefix}{timestamp}{suffix}");
    let path = dir.join(filename);

    fs::File::create(&path).await?;
    Ok(path)
}

/// Create temp directory.
pub async fn create_temp_dir(prefix: &str) -> Result<PathBuf> {
    let dir = std::env::temp_dir();
    let timestamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);

    let dirname = format!("{prefix}{timestamp}");
    let path = dir.join(dirname);

    fs::create_dir(&path).await?;
    Ok(path)
}

/// File diff.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileDiff {
    /// Added lines.
    pub added: Vec<(usize, String)>,
    /// Removed lines.
    pub removed: Vec<(usize, String)>,
    /// Changed lines.
    pub changed: Vec<(usize, String, String)>,
}

impl FileDiff {
    /// Compare two files.
    pub async fn compare(old_path: impl AsRef<Path>, new_path: impl AsRef<Path>) -> Result<Self> {
        let old_lines = read_lines(old_path).await?;
        let new_lines = read_lines(new_path).await?;

        Self::compare_lines(&old_lines, &new_lines)
    }

    /// Compare two line vectors.
    pub fn compare_lines(old: &[String], new: &[String]) -> Result<Self> {
        let mut added = Vec::new();
        let mut removed = Vec::new();
        let mut changed = Vec::new();

        let max_len = old.len().max(new.len());

        for i in 0..max_len {
            match (old.get(i), new.get(i)) {
                (Some(o), Some(n)) if o != n => {
                    changed.push((i, o.clone(), n.clone()));
                }
                (Some(o), None) => {
                    removed.push((i, o.clone()));
                }
                (None, Some(n)) => {
                    added.push((i, n.clone()));
                }
                _ => {}
            }
        }

        Ok(Self {
            added,
            removed,
            changed,
        })
    }

    /// Check if there are any differences.
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.changed.is_empty()
    }

    /// Get total number of changes.
    pub fn total_changes(&self) -> usize {
        self.added.len() + self.removed.len() + self.changed.len()
    }
}

/// File hash.
pub async fn hash_file(path: impl AsRef<Path>) -> Result<String> {
    use sha2::{Digest, Sha256};

    let content = read_bytes(path).await?;
    let mut hasher = Sha256::new();
    hasher.update(&content);
    let result = hasher.finalize();

    // Convert to hex string
    let hex_string: String = result.iter().map(|b| format!("{b:02x}")).collect();

    Ok(hex_string)
}

/// Compare file hashes.
pub async fn files_equal(path1: impl AsRef<Path>, path2: impl AsRef<Path>) -> Result<bool> {
    let hash1 = hash_file(path1).await?;
    let hash2 = hash_file(path2).await?;
    Ok(hash1 == hash2)
}

/// File encoding detection (simplified).
pub fn detect_encoding(content: &[u8]) -> &'static str {
    // Check for BOM
    if content.len() >= 3 && content[0..3] == [0xEF, 0xBB, 0xBF] {
        return "UTF-8";
    }
    if content.len() >= 2 {
        if content[0..2] == [0xFF, 0xFE] {
            return "UTF-16LE";
        }
        if content[0..2] == [0xFE, 0xFF] {
            return "UTF-16BE";
        }
    }

    // Check if valid UTF-8
    if std::str::from_utf8(content).is_ok() {
        return "UTF-8";
    }

    "BINARY"
}

/// Check if file is text.
pub async fn is_text_file(path: impl AsRef<Path>) -> Result<bool> {
    let content = read_bytes(&path).await?;
    let sample_size = 8192.min(content.len());

    // Check for null bytes
    let has_null = content[..sample_size].contains(&0);
    if has_null {
        return Ok(false);
    }

    // Check encoding
    let encoding = detect_encoding(&content[..sample_size]);
    Ok(encoding != "BINARY")
}

/// Line ending type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineEnding {
    Unix,    // LF
    Windows, // CRLF
    Mac,     // CR (old Mac)
    Mixed,
}

impl LineEnding {
    /// Detect from content.
    pub fn detect(content: &str) -> Self {
        let has_crlf = content.contains("\r\n");
        let has_lf = content.contains('\n') && !has_crlf;
        let has_cr = content.contains('\r') && !has_crlf;

        if has_crlf && !has_lf && !has_cr {
            Self::Windows
        } else if has_lf && !has_crlf && !has_cr {
            Self::Unix
        } else if has_cr && !has_crlf && !has_lf {
            Self::Mac
        } else if has_crlf || has_lf || has_cr {
            Self::Mixed
        } else {
            Self::Unix // Default
        }
    }

    /// Convert content to this line ending.
    pub fn convert(&self, content: &str) -> String {
        // First normalize to LF
        let normalized = content.replace("\r\n", "\n").replace('\r', "\n");

        match self {
            Self::Unix => normalized,
            Self::Windows => normalized.replace('\n', "\r\n"),
            Self::Mac => normalized.replace('\n', "\r"),
            Self::Mixed => normalized,
        }
    }

    /// Get as string.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Unix => "\n",
            Self::Windows => "\r\n",
            Self::Mac => "\r",
            Self::Mixed => "\n",
        }
    }
}

/// Information about a write location's accessibility.
#[derive(Debug, Clone)]
pub struct WriteLocationStatus {
    /// The path that was checked.
    pub path: PathBuf,
    /// Description of what this location is used for.
    pub description: &'static str,
    /// Whether the location is writable.
    pub is_writable: bool,
    /// Error message if not writable.
    pub error: Option<String>,
}

/// Check if a directory is writable by attempting to create a temp file.
fn check_dir_writable(path: &Path) -> std::result::Result<(), String> {
    if !path.exists() {
        // Try to create the directory
        if let Err(e) = std::fs::create_dir_all(path) {
            return Err(format!("Cannot create directory: {e}"));
        }
    }

    // Try to create a temp file in the directory
    let test_file = path.join(format!(".cortex_write_test_{}", std::process::id()));
    match std::fs::File::create(&test_file) {
        Ok(_) => {
            // Clean up
            let _ = std::fs::remove_file(&test_file);
            Ok(())
        }
        Err(e) => Err(format!("Cannot write to directory: {e}")),
    }
}

/// Validate all known write locations used by Cortex.
///
/// Returns a list of write location statuses. This is useful for:
/// - Docker read-only container debugging
/// - Diagnosing permission issues
/// - Validating environment variable configuration
///
/// Known write locations:
/// - `CORTEX_HOME` / `CORTEX_CONFIG_DIR` / `~/.cortex`: Config, sessions, logs
/// - `TMPDIR` / `/tmp`: Temporary files for MCP, transcription, patches
///
/// For Docker read-only containers, ensure these volumes are mounted:
/// - `/path/to/config:/root/.cortex` (or set `CORTEX_HOME`)
/// - `/tmp` or set `TMPDIR` to a writable location
pub fn validate_write_locations(cortex_home: &Path) -> Vec<WriteLocationStatus> {
    let mut results = Vec::new();

    // Check cortex home directory
    results.push(WriteLocationStatus {
        path: cortex_home.to_path_buf(),
        description: "Cortex home (config, sessions, logs)",
        is_writable: check_dir_writable(cortex_home).is_ok(),
        error: check_dir_writable(cortex_home).err(),
    });

    // Check sessions directory
    let sessions_dir = cortex_home.join("sessions");
    results.push(WriteLocationStatus {
        path: sessions_dir.clone(),
        description: "Session data",
        is_writable: check_dir_writable(&sessions_dir).is_ok(),
        error: check_dir_writable(&sessions_dir).err(),
    });

    // Check log directory
    let log_dir = cortex_home.join("log");
    results.push(WriteLocationStatus {
        path: log_dir.clone(),
        description: "Log files",
        is_writable: check_dir_writable(&log_dir).is_ok(),
        error: check_dir_writable(&log_dir).err(),
    });

    // Check temp directory (critical for MCP, transcription, etc.)
    let temp_dir = std::env::temp_dir();
    results.push(WriteLocationStatus {
        path: temp_dir.clone(),
        description: "Temporary files (TMPDIR)",
        is_writable: check_dir_writable(&temp_dir).is_ok(),
        error: check_dir_writable(&temp_dir).err(),
    });

    results
}

/// Check if running in a read-only environment and return warnings.
///
/// This function checks all known write locations and returns a formatted
/// warning message if any are not writable.
pub fn check_write_permissions(cortex_home: &Path) -> Option<String> {
    let statuses = validate_write_locations(cortex_home);
    let failed: Vec<_> = statuses.iter().filter(|s| !s.is_writable).collect();

    if failed.is_empty() {
        return None;
    }

    let mut msg = String::from("Warning: Some write locations are not accessible.\n");
    msg.push_str(
        "This may cause failures in read-only environments (e.g., Docker --read-only).\n\n",
    );

    for status in &failed {
        msg.push_str(&format!(
            "  ✗ {} ({})\n    Path: {}\n    Error: {}\n\n",
            status.description,
            if status.path == std::env::temp_dir() {
                "set TMPDIR"
            } else {
                "set CORTEX_HOME or mount volume"
            },
            status.path.display(),
            status.error.as_deref().unwrap_or("Unknown error")
        ));
    }

    msg.push_str("To fix for Docker, mount writable volumes:\n");
    msg.push_str("  docker run --read-only \\\n");
    msg.push_str("    -v /path/to/config:/root/.cortex \\\n");
    msg.push_str("    -v /tmp:/tmp \\\n");
    msg.push_str("    your-image\n\n");
    msg.push_str("Or set environment variables:\n");
    msg.push_str("  CORTEX_HOME=/writable/path TMPDIR=/writable/tmp cortex\n");

    Some(msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_bytes() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1024), "1.00 KB");
        assert_eq!(format_bytes(1536), "1.50 KB");
        assert_eq!(format_bytes(1048576), "1.00 MB");
    }

    #[tokio::test]
    async fn test_temp_file() {
        let path = create_temp_file("test_", ".txt").await.unwrap();
        assert!(exists(&path).await);
        remove_file(&path).await.unwrap();
        assert!(!exists(&path).await);
    }

    #[tokio::test]
    async fn test_read_write() {
        let path = create_temp_file("rw_test_", ".txt").await.unwrap();

        write_string(&path, "hello world").await.unwrap();
        let content = read_string(&path).await.unwrap();

        assert_eq!(content, "hello world");

        remove_file(&path).await.unwrap();
    }

    #[tokio::test]
    async fn test_append() {
        let path = create_temp_file("append_test_", ".txt").await.unwrap();

        write_string(&path, "line1\n").await.unwrap();
        append_string(&path, "line2\n").await.unwrap();

        let content = read_string(&path).await.unwrap();
        assert!(content.contains("line1"));
        assert!(content.contains("line2"));

        remove_file(&path).await.unwrap();
    }

    #[test]
    fn test_line_ending_detect() {
        assert_eq!(LineEnding::detect("hello\nworld"), LineEnding::Unix);
        assert_eq!(LineEnding::detect("hello\r\nworld"), LineEnding::Windows);
        assert_eq!(LineEnding::detect("hello\rworld"), LineEnding::Mac);
    }

    #[test]
    fn test_line_ending_convert() {
        let unix = "line1\nline2\n";
        let windows = LineEnding::Windows.convert(unix);
        assert_eq!(windows, "line1\r\nline2\r\n");
    }

    #[test]
    fn test_detect_encoding() {
        assert_eq!(detect_encoding(b"hello world"), "UTF-8");
        assert_eq!(detect_encoding(&[0xEF, 0xBB, 0xBF, 0x41]), "UTF-8");
        assert_eq!(detect_encoding(&[0xFF, 0xFE, 0x41, 0x00]), "UTF-16LE");
    }

    #[test]
    fn test_file_diff() {
        let old = vec!["line1".to_string(), "line2".to_string()];
        let new = vec![
            "line1".to_string(),
            "line3".to_string(),
            "line4".to_string(),
        ];

        let diff = FileDiff::compare_lines(&old, &new).unwrap();

        assert_eq!(diff.changed.len(), 1);
        assert_eq!(diff.added.len(), 1);
        assert!(diff.removed.is_empty());
    }
}
