//! File utilities for the Cortex CLI.
//!
//! Provides file validation, attachment handling, and encoding detection.

use anyhow::{Context, Result, bail};
use std::fs::Metadata;
use std::path::Path;

use super::paths::is_sensitive_path;

/// Maximum file size for attachments (10MB).
pub const MAX_ATTACHMENT_SIZE: u64 = 10 * 1024 * 1024;

/// Maximum total size for all attachments (50MB).
pub const MAX_TOTAL_ATTACHMENT_SIZE: u64 = 50 * 1024 * 1024;

/// File attachment information.
#[derive(Debug, Clone)]
pub struct FileAttachment {
    /// The resolved file path.
    pub path: std::path::PathBuf,
    /// The filename for display.
    pub filename: String,
    /// The detected MIME type.
    pub mime_type: String,
    /// File size in bytes.
    pub size: u64,
}

/// Validate that a file is safe to attach.
///
/// Checks:
/// - Is a regular file (not device, directory, socket, etc.)
/// - Size is within limits
/// - Not a sensitive system file (warns but doesn't block)
///
/// # Arguments
/// * `path` - The file path to validate
/// * `metadata` - Pre-fetched file metadata
///
/// # Returns
/// `Ok(())` if safe, error otherwise.
pub fn validate_file_attachment(path: &Path, metadata: &Metadata) -> Result<()> {
    // Check if it's a regular file
    if !metadata.is_file() {
        bail!(
            "Cannot attach '{}': not a regular file (is it a directory or special file?)",
            path.display()
        );
    }

    // Check file size
    if metadata.len() > MAX_ATTACHMENT_SIZE {
        bail!(
            "File '{}' exceeds maximum size of 10MB ({} bytes)",
            path.display(),
            metadata.len()
        );
    }

    // Security warning for sensitive system files
    if is_sensitive_path(path) {
        eprintln!(
            "\n⚠️  SECURITY WARNING: '{}' appears to be a sensitive system file.\n\
             Contents will be sent to an external AI provider.\n\
             This file may contain passwords, keys, or other private information.\n\
             Press Ctrl+C to cancel, or the request will proceed.\n",
            path.display()
        );
        // Give user a moment to read the warning and cancel
        std::thread::sleep(std::time::Duration::from_secs(2));
    }

    // Block device files on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        let file_type = metadata.file_type();
        if file_type.is_block_device() {
            bail!(
                "Cannot attach '{}': block device files are not allowed",
                path.display()
            );
        }
        if file_type.is_char_device() {
            bail!(
                "Cannot attach '{}': character device files are not allowed",
                path.display()
            );
        }
        if file_type.is_fifo() {
            bail!(
                "Cannot attach '{}': FIFO/pipe files are not allowed",
                path.display()
            );
        }
        if file_type.is_socket() {
            bail!(
                "Cannot attach '{}': socket files are not allowed",
                path.display()
            );
        }
    }

    Ok(())
}

/// Read a file with automatic encoding detection.
///
/// Handles UTF-8, UTF-16 LE, UTF-16 BE, and normalizes line endings.
///
/// # Arguments
/// * `path` - The file path to read
///
/// # Returns
/// The file contents as a string.
pub fn read_file_with_encoding(path: &Path) -> Result<String> {
    let bytes =
        std::fs::read(path).with_context(|| format!("Failed to read {}", path.display()))?;

    // Check for UTF-16 BOM and convert if needed
    let content = if bytes.starts_with(&[0xFF, 0xFE]) {
        // UTF-16 LE BOM
        let u16_chars: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .collect();
        String::from_utf16(&u16_chars)
            .with_context(|| format!("Invalid UTF-16 LE content in {}", path.display()))?
    } else if bytes.starts_with(&[0xFE, 0xFF]) {
        // UTF-16 BE BOM
        let u16_chars: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|chunk| u16::from_be_bytes([chunk[0], chunk[1]]))
            .collect();
        String::from_utf16(&u16_chars)
            .with_context(|| format!("Invalid UTF-16 BE content in {}", path.display()))?
    } else if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        // UTF-8 BOM - skip it
        String::from_utf8(bytes[3..].to_vec())
            .with_context(|| format!("Invalid UTF-8 content in {}", path.display()))?
    } else {
        // Assume UTF-8
        String::from_utf8(bytes)
            .with_context(|| format!("Invalid UTF-8 content in {}", path.display()))?
    };

    // Normalize line endings to handle mixed CRLF/LF
    Ok(normalize_line_endings(content))
}

/// Normalize line endings by converting CRLF to LF.
///
/// Handles files with mixed line endings (common when editing on different OSes).
fn normalize_line_endings(content: String) -> String {
    content.replace("\r\n", "\n").replace('\r', "\n")
}

/// Process file attachments from paths.
///
/// Resolves paths, validates files, and returns attachment information.
///
/// # Arguments
/// * `file_paths` - The file paths to process
/// * `base_dir` - Base directory for resolving relative paths
///
/// # Returns
/// A list of validated file attachments.
pub fn process_file_attachments(
    file_paths: &[std::path::PathBuf],
    base_dir: &Path,
) -> Result<Vec<FileAttachment>> {
    use super::mime::mime_type_from_path;

    let mut attachments = Vec::new();
    let mut total_size = 0u64;

    for file_path in file_paths {
        let resolved_path = if file_path.is_absolute() {
            file_path.clone()
        } else {
            base_dir.join(file_path)
        };

        // Check file exists
        if !resolved_path.exists() {
            bail!("File not found: {}", file_path.display());
        }

        let metadata = std::fs::metadata(&resolved_path).with_context(|| {
            format!("Failed to read file metadata: {}", resolved_path.display())
        })?;

        // Validate file
        validate_file_attachment(&resolved_path, &metadata)?;

        // Check total size
        total_size += metadata.len();
        if total_size > MAX_TOTAL_ATTACHMENT_SIZE {
            bail!(
                "Total attachment size exceeds limit of {} bytes",
                MAX_TOTAL_ATTACHMENT_SIZE
            );
        }

        let filename = resolved_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown".to_string());

        let mime_type = if metadata.is_dir() {
            "application/x-directory".to_string()
        } else {
            mime_type_from_path(&resolved_path).to_string()
        };

        attachments.push(FileAttachment {
            path: resolved_path,
            filename,
            mime_type,
            size: metadata.len(),
        });
    }

    Ok(attachments)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_normalize_line_endings() {
        assert_eq!(normalize_line_endings("a\r\nb\r\n".to_string()), "a\nb\n");
        assert_eq!(normalize_line_endings("a\rb\r".to_string()), "a\nb\n");
        assert_eq!(normalize_line_endings("a\nb\n".to_string()), "a\nb\n");
    }

    #[test]
    fn test_max_attachment_size_constant() {
        assert_eq!(MAX_ATTACHMENT_SIZE, 10 * 1024 * 1024); // 10MB
    }

    #[test]
    fn test_max_total_attachment_size_constant() {
        assert_eq!(MAX_TOTAL_ATTACHMENT_SIZE, 50 * 1024 * 1024); // 50MB
    }

    #[test]
    fn test_file_attachment_struct() {
        let attachment = FileAttachment {
            path: PathBuf::from("/test/file.txt"),
            filename: "file.txt".to_string(),
            mime_type: "text/plain".to_string(),
            size: 1024,
        };
        assert_eq!(attachment.filename, "file.txt");
        assert_eq!(attachment.mime_type, "text/plain");
        assert_eq!(attachment.size, 1024);
    }

    #[test]
    fn test_normalize_line_endings_empty() {
        assert_eq!(normalize_line_endings("".to_string()), "");
    }

    #[test]
    fn test_normalize_line_endings_no_change() {
        assert_eq!(
            normalize_line_endings("hello\nworld\n".to_string()),
            "hello\nworld\n"
        );
    }

    #[test]
    fn test_normalize_line_endings_mixed() {
        // Mixed CRLF and LF
        assert_eq!(
            normalize_line_endings("a\r\nb\nc\r\n".to_string()),
            "a\nb\nc\n"
        );
    }
}
