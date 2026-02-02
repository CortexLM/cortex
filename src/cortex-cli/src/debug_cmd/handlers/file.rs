//! File command handler.

use anyhow::{Result, bail};

use crate::debug_cmd::commands::FileArgs;
use crate::debug_cmd::types::{FileDebugOutput, FileMetadata};
use crate::debug_cmd::utils::{
    detect_encoding_and_binary, detect_special_file_type, format_size, get_unix_permissions,
    guess_mime_type, is_virtual_filesystem, is_writable_by_current_user,
};

/// Run the file debug command.
pub async fn run_file(args: FileArgs) -> Result<()> {
    let path = if args.path.is_absolute() {
        args.path.clone()
    } else {
        std::env::current_dir()?.join(&args.path)
    };

    let exists = path.exists();

    if !exists {
        bail!("File does not exist: {}", path.display());
    }

    // Detect special file types using stat() BEFORE attempting any reads
    // This prevents blocking on FIFOs, sockets, and other special files
    let special_file_type = detect_special_file_type(&path);

    let (metadata, error) = match std::fs::metadata(&path) {
        Ok(meta) => {
            let modified = meta
                .modified()
                .ok()
                .map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339());
            let created = meta
                .created()
                .ok()
                .map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339());

            // Get symlink target if applicable
            let symlink_target = if meta.file_type().is_symlink() {
                std::fs::read_link(&path)
                    .ok()
                    .map(|p| p.to_string_lossy().to_string())
            } else {
                None
            };

            // Check if the current user can actually write to the file
            // This is more accurate than just checking permission bits
            // Skip this check for special files (FIFOs, etc.) to avoid blocking
            let readonly = if special_file_type.is_some() {
                false // Don't check writability for special files
            } else {
                !is_writable_by_current_user(&path)
            };

            // Check if this is a virtual filesystem (procfs, sysfs, etc.)
            // These report size=0 in stat() but may have actual content
            let is_virtual_fs = is_virtual_filesystem(&path);
            let stat_size = meta.len();

            // For virtual filesystem files that report 0 size, try to read actual content size
            let actual_size = if is_virtual_fs && stat_size == 0 && meta.is_file() {
                // Try to read the file to get actual content size
                // Limit read to 1MB to avoid hanging on infinite streams
                match std::fs::read(&path) {
                    Ok(content) if !content.is_empty() => Some(content.len() as u64),
                    _ => None,
                }
            } else {
                None
            };

            // Get file permissions
            let (permissions, mode) = get_unix_permissions(&meta);

            (
                Some(FileMetadata {
                    size: stat_size,
                    actual_size,
                    is_virtual_fs: if is_virtual_fs { Some(true) } else { None },
                    is_file: meta.is_file(),
                    is_dir: meta.is_dir(),
                    is_symlink: meta.file_type().is_symlink(),
                    file_type: special_file_type.clone(),
                    symlink_target,
                    modified,
                    created,
                    readonly,
                    permissions,
                    mode,
                }),
                None,
            )
        }
        Err(e) => (None, Some(e.to_string())),
    };

    // Detect MIME type from extension - skip for special files
    let mime_type = if path.is_file() && special_file_type.is_none() {
        path.extension()
            .and_then(|ext| ext.to_str())
            .map(guess_mime_type)
    } else {
        None
    };

    // Detect encoding and binary status - SKIP for special files to avoid blocking
    let (encoding, is_binary) = if path.is_file() && special_file_type.is_none() {
        detect_encoding_and_binary(&path)
    } else {
        (None, None)
    };

    // Check if the file appears to be actively modified by comparing
    // metadata from two reads with a small delay
    let active_modification_warning = if path.is_file() {
        // Get initial size
        let initial_size = std::fs::metadata(&path).ok().map(|m| m.len());
        // Brief delay to detect active writes
        std::thread::sleep(std::time::Duration::from_millis(50));
        // Get size again
        let final_size = std::fs::metadata(&path).ok().map(|m| m.len());

        match (initial_size, final_size) {
            (Some(s1), Some(s2)) if s1 != s2 => Some(format!(
                "File appears to be actively modified (size changed from {} to {} bytes during read). \
                     Content may be inconsistent.",
                s1, s2
            )),
            _ => None,
        }
    } else {
        None
    };

    let output = FileDebugOutput {
        path,
        exists,
        metadata,
        mime_type,
        encoding,
        is_binary,
        error,
        active_modification_warning,
    };

    if args.json {
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        println!("File Debug Info");
        println!("{}", "=".repeat(50));
        println!("  Path:   {}", output.path.display());
        println!("  Exists: {}", output.exists);

        if let Some(ref meta) = output.metadata {
            println!();
            println!("Metadata");
            println!("{}", "-".repeat(40));
            // Handle virtual filesystem files that report 0 size (#2829)
            if meta.is_virtual_fs.unwrap_or(false) {
                if let Some(actual) = meta.actual_size {
                    println!(
                        "  Size:     {} (stat reports 0, virtual filesystem)",
                        format_size(actual)
                    );
                } else {
                    println!("  Size:     unknown (virtual filesystem)");
                }
            } else {
                println!("  Size:     {}", format_size(meta.size));
            }

            // Display file type, including special types like FIFO, socket, etc.
            let type_str = if let Some(ref special_type) = meta.file_type {
                special_type.as_str()
            } else if meta.is_file {
                "file"
            } else if meta.is_dir {
                "directory"
            } else if meta.is_symlink {
                "symlink"
            } else {
                "unknown"
            };
            println!("  Type:     {}", type_str);

            if meta.is_virtual_fs.unwrap_or(false) {
                println!("  Virtual:  yes (procfs/sysfs/etc)");
            }
            // Display file permissions in Unix-style format
            if let Some(ref perms) = meta.permissions {
                let mode_str = meta.mode.map(|m| format!(" ({:o})", m)).unwrap_or_default();
                println!("  Perms:    {}{}", perms, mode_str);
            }
            println!("  Readonly: {}", meta.readonly);
            if let Some(ref modified) = meta.modified {
                println!("  Modified: {}", modified);
            }
            if let Some(ref created) = meta.created {
                println!("  Created:  {}", created);
            }
        }

        if let Some(ref mime) = output.mime_type {
            println!();
            println!("Content Detection");
            println!("{}", "-".repeat(40));
            println!("  MIME Type: {}", mime);
        }

        if let Some(ref enc) = output.encoding {
            println!("  Encoding:  {}", enc);
        }

        if let Some(binary) = output.is_binary {
            println!("  Binary:    {}", binary);
        }

        if let Some(ref err) = output.error {
            println!();
            println!("Error: {}", err);
        }

        if let Some(ref warning) = output.active_modification_warning {
            println!();
            println!("Warning: {}", warning);
        }
    }

    Ok(())
}
