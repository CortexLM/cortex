//! File attachment handling for the run command.

use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::fs::Metadata;
use std::path::{Path, PathBuf};

use crate::utils::is_sensitive_path;

/// Maximum file size for attachments (10MB)
pub const MAX_FILE_SIZE: u64 = 10 * 1024 * 1024;

/// File attachment information.
#[derive(Debug, Clone, Serialize)]
pub struct FileAttachment {
    pub path: PathBuf,
    pub filename: String,
    pub mime_type: String,
    pub size: u64,
}

/// Process file attachments and validate they exist.
pub async fn process_file_attachments(
    files: &[PathBuf],
    cwd: Option<&PathBuf>,
) -> Result<Vec<FileAttachment>> {
    let mut attachments = Vec::new();

    for file_path in files {
        let resolved_path = if file_path.is_absolute() {
            file_path.clone()
        } else {
            let working_dir = cwd
                .cloned()
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
            working_dir.join(file_path)
        };

        // Check file exists
        if !resolved_path.exists() {
            bail!("File not found: {}", file_path.display());
        }

        let metadata = std::fs::metadata(&resolved_path).with_context(|| {
            format!("Failed to read file metadata: {}", resolved_path.display())
        })?;

        // Validate file is safe to attach
        validate_file_attachment(&resolved_path, &metadata)?;

        let filename = resolved_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown".to_string());

        // Determine MIME type
        let mime_type = if metadata.is_dir() {
            "application/x-directory".to_string()
        } else {
            super::mime::mime_type_from_extension(&resolved_path)
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

/// Validate that a file is safe to attach (not a device file, not too large).
/// Also warns about sensitive system files that may contain private information.
fn validate_file_attachment(path: &Path, metadata: &Metadata) -> Result<()> {
    // Check if it's a regular file
    if !metadata.is_file() {
        bail!(
            "Cannot attach '{}': not a regular file (is it a directory or special file?)",
            path.display()
        );
    }

    // Check file size
    if metadata.len() > MAX_FILE_SIZE {
        bail!(
            "File '{}' exceeds maximum size of 10MB ({} bytes)",
            path.display(),
            metadata.len()
        );
    }

    // Security warning for sensitive system files (#2038)
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
