//! Archive extraction functionality for the LSP downloader.

use crate::{LspError, Result};
use std::path::Path;
use tokio::fs;

/// Validate that a path does not escape the destination directory.
/// Prevents ZIP slip / path traversal attacks.
pub fn validate_path_safe(
    dest_dir: &Path,
    entry_name: &str,
) -> std::result::Result<std::path::PathBuf, String> {
    // Reject paths with null bytes
    if entry_name.contains('\0') {
        return Err("Path contains null byte".to_string());
    }

    // Reject absolute paths
    let entry_path = std::path::Path::new(entry_name);
    if entry_path.is_absolute() {
        return Err(format!("Absolute path not allowed: {}", entry_name));
    }

    // Check for path traversal patterns
    for component in entry_path.components() {
        match component {
            std::path::Component::ParentDir => {
                return Err(format!(
                    "Path traversal (parent directory) not allowed: {}",
                    entry_name
                ));
            }
            std::path::Component::Prefix(_) => {
                return Err(format!("Path prefix not allowed: {}", entry_name));
            }
            std::path::Component::RootDir => {
                return Err(format!("Root directory not allowed: {}", entry_name));
            }
            _ => {}
        }
    }

    // Build the full path
    let full_path = dest_dir.join(entry_name);

    // Canonicalize destination to resolve any symlinks
    let canonical_dest = dest_dir
        .canonicalize()
        .unwrap_or_else(|_| dest_dir.to_path_buf());

    // For the full path, we can't canonicalize yet since it may not exist
    // Instead, we verify each component doesn't escape
    let mut current = canonical_dest.clone();
    for component in entry_path.components() {
        if let std::path::Component::Normal(name) = component {
            current = current.join(name);
        }
    }

    // Verify the resolved path is still under dest_dir
    // This handles edge cases with symlinks in the destination
    if !current.starts_with(&canonical_dest) {
        return Err(format!(
            "Path would escape destination directory: {}",
            entry_name
        ));
    }

    Ok(full_path)
}

/// Extract a ZIP archive.
pub async fn extract_zip(archive_path: &Path, dest_dir: &Path) -> Result<()> {
    let archive_path = archive_path.to_path_buf();
    let dest_dir = dest_dir.to_path_buf();

    tokio::task::spawn_blocking(move || {
        let file = std::fs::File::open(&archive_path)?;
        let mut archive = zip::ZipArchive::new(file)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;

        for i in 0..archive.len() {
            let mut file = archive
                .by_index(i)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;

            let entry_name = file.name().to_string();

            // Validate path to prevent directory traversal
            let outpath = validate_path_safe(&dest_dir, &entry_name)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;

            if entry_name.ends_with('/') {
                std::fs::create_dir_all(&outpath)?;
            } else {
                if let Some(parent) = outpath.parent() {
                    // Validate parent is also safe
                    if !parent.starts_with(&dest_dir) {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            format!("Parent directory would escape destination: {}", entry_name),
                        ));
                    }
                    std::fs::create_dir_all(parent)?;
                }
                let mut outfile = std::fs::File::create(&outpath)?;
                std::io::copy(&mut file, &mut outfile)?;
            }
        }
        Ok::<_, std::io::Error>(())
    })
    .await
    .map_err(|e| LspError::Communication(format!("Join error: {}", e)))?
    .map_err(LspError::Io)
}

/// Safely extract a tar archive with path validation.
pub fn safe_tar_unpack<R: std::io::Read>(
    archive: &mut tar::Archive<R>,
    dest_dir: &Path,
) -> std::io::Result<()> {
    for entry_result in archive.entries()? {
        let mut entry = entry_result?;
        let entry_path = entry.path()?;
        let entry_name = entry_path.to_string_lossy().to_string();

        // Validate path to prevent directory traversal
        let outpath = validate_path_safe(dest_dir, &entry_name)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;

        let entry_type = entry.header().entry_type();

        match entry_type {
            tar::EntryType::Directory => {
                std::fs::create_dir_all(&outpath)?;
            }
            tar::EntryType::Regular | tar::EntryType::Continuous => {
                if let Some(parent) = outpath.parent() {
                    if !parent.starts_with(dest_dir) {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            format!("Parent directory would escape destination: {}", entry_name),
                        ));
                    }
                    std::fs::create_dir_all(parent)?;
                }
                let mut outfile = std::fs::File::create(&outpath)?;
                std::io::copy(&mut entry, &mut outfile)?;

                // Preserve permissions on Unix
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if let Ok(mode) = entry.header().mode() {
                        let perms = std::fs::Permissions::from_mode(mode);
                        let _ = std::fs::set_permissions(&outpath, perms);
                    }
                }
            }
            tar::EntryType::Symlink | tar::EntryType::Link => {
                // Skip symlinks and hard links for security
                // They could be used for path traversal attacks
                tracing::warn!(
                    "Skipping symlink/hardlink in archive for security: {}",
                    entry_name
                );
            }
            _ => {
                // Skip other entry types (block devices, char devices, fifos, etc.)
            }
        }
    }
    Ok(())
}

/// Extract a tar.gz archive.
pub async fn extract_tar_gz(archive_path: &Path, dest_dir: &Path) -> Result<()> {
    let archive_path = archive_path.to_path_buf();
    let dest_dir = dest_dir.to_path_buf();

    tokio::task::spawn_blocking(move || {
        let file = std::fs::File::open(&archive_path)?;
        let gz = flate2::read::GzDecoder::new(file);
        let mut archive = tar::Archive::new(gz);
        safe_tar_unpack(&mut archive, &dest_dir)?;
        Ok::<_, std::io::Error>(())
    })
    .await
    .map_err(|e| LspError::Communication(format!("Join error: {}", e)))?
    .map_err(LspError::Io)
}

/// Extract a tar.xz archive.
pub async fn extract_tar_xz(archive_path: &Path, dest_dir: &Path) -> Result<()> {
    let archive_path = archive_path.to_path_buf();
    let dest_dir = dest_dir.to_path_buf();

    tokio::task::spawn_blocking(move || {
        let file = std::fs::File::open(&archive_path)?;
        let xz = xz2::read::XzDecoder::new(file);
        let mut archive = tar::Archive::new(xz);
        safe_tar_unpack(&mut archive, &dest_dir)?;
        Ok::<_, std::io::Error>(())
    })
    .await
    .map_err(|e| LspError::Communication(format!("Join error: {}", e)))?
    .map_err(LspError::Io)
}

/// Find a binary recursively in a directory.
pub async fn find_binary_recursive(
    dir: &Path,
    binary_name: &str,
) -> Result<Option<std::path::PathBuf>> {
    let mut entries = fs::read_dir(dir).await.map_err(LspError::Io)?;

    while let Some(entry) = entries.next_entry().await.map_err(LspError::Io)? {
        let path = entry.path();
        let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

        if file_name == binary_name || file_name == format!("{}.exe", binary_name) {
            return Ok(Some(path));
        }

        if path.is_dir() {
            if let Some(found) = Box::pin(find_binary_recursive(&path, binary_name)).await? {
                return Ok(Some(found));
            }
        }
    }

    Ok(None)
}
