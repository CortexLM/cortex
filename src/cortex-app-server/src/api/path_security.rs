//! Path traversal protection utilities.
//!
//! Provides functions to validate and sanitize paths for safe file operations,
//! preventing path traversal attacks and restricting access to allowed directories.

use cortex_common::normalize_path as normalize_path_util;

/// Normalizes a path by resolving `.` and `..` components without filesystem access.
pub fn normalize_path(path: &std::path::Path) -> std::path::PathBuf {
    normalize_path_util(path)
}

/// Validates that a path is safe for file operations.
/// Prevents path traversal attacks by canonicalizing and checking against allowed roots.
pub fn validate_path_safe(path: &std::path::Path) -> Result<std::path::PathBuf, String> {
    let normalized = normalize_path(path);

    // Get canonical path if it exists
    let canonical = if path.exists() {
        path.canonicalize()
            .map_err(|e| format!("Failed to canonicalize path: {}", e))?
    } else {
        // For non-existent paths, check if parent exists
        if let Some(parent) = normalized.parent() {
            if parent.exists() {
                let canonical_parent = parent
                    .canonicalize()
                    .map_err(|e| format!("Failed to canonicalize parent: {}", e))?;
                let file_name = normalized
                    .file_name()
                    .ok_or_else(|| "Invalid file name".to_string())?;
                canonical_parent.join(file_name)
            } else {
                normalized.clone()
            }
        } else {
            normalized.clone()
        }
    };

    // Validate against allowed roots
    let allowed_roots = get_allowed_roots();
    let is_within_allowed = allowed_roots.iter().any(|root| {
        if let Ok(canonical_root) = root.canonicalize() {
            canonical.starts_with(&canonical_root)
        } else {
            canonical.starts_with(root)
        }
    });

    if !is_within_allowed {
        return Err(format!(
            "Path '{}' is outside allowed directories",
            path.display()
        ));
    }

    // Check symlinks for existing paths
    if path.exists()
        && let Ok(metadata) = std::fs::symlink_metadata(path)
        && metadata.file_type().is_symlink()
        && let Ok(target) = std::fs::read_link(path)
    {
        let absolute_target = if target.is_absolute() {
            target
        } else {
            path.parent().map(|p| p.join(&target)).unwrap_or(target)
        };

        let target_canonical = if absolute_target.exists() {
            absolute_target
                .canonicalize()
                .map_err(|e| format!("Failed to canonicalize symlink target: {}", e))?
        } else {
            normalize_path(&absolute_target)
        };

        let target_is_safe = allowed_roots.iter().any(|root| {
            if let Ok(canonical_root) = root.canonicalize() {
                target_canonical.starts_with(&canonical_root)
            } else {
                target_canonical.starts_with(root)
            }
        });

        if !target_is_safe {
            return Err(format!(
                "Symlink '{}' points outside allowed directories",
                path.display()
            ));
        }
    }

    Ok(canonical)
}

/// Get allowed root directories for file operations.
pub fn get_allowed_roots() -> Vec<std::path::PathBuf> {
    let mut roots = Vec::new();

    // Home directory
    if let Some(home) = dirs::home_dir() {
        roots.push(home);
    }

    // Documents directory
    if let Some(docs) = dirs::document_dir() {
        roots.push(docs);
    }

    // Temp directory
    roots.push(std::env::temp_dir());

    // Current working directory
    if let Ok(cwd) = std::env::current_dir() {
        roots.push(cwd);
    }

    // On Windows, allow all drives
    #[cfg(windows)]
    {
        for letter in b'A'..=b'Z' {
            let drive = format!("{}:\\", letter as char);
            let path = std::path::PathBuf::from(&drive);
            if path.exists() {
                roots.push(path);
            }
        }
    }

    // On Unix, allow common dev paths
    #[cfg(unix)]
    {
        let unix_paths = ["/home", "/Users", "/tmp", "/var/tmp", "/workspace"];
        for p in unix_paths {
            let path = std::path::PathBuf::from(p);
            if path.exists() {
                roots.push(path);
            }
        }
    }

    roots
}

/// Validate a path for write operations (more restrictive).
pub fn validate_path_for_write(path: &std::path::Path) -> Result<std::path::PathBuf, String> {
    let normalized = normalize_path(path);

    // Forbid writing to system paths
    let forbidden_prefixes: &[&str] = if cfg!(windows) {
        &[
            "C:\\Windows",
            "C:\\Program Files",
            "C:\\Program Files (x86)",
        ]
    } else {
        &[
            "/bin",
            "/sbin",
            "/usr/bin",
            "/usr/sbin",
            "/etc",
            "/var/log",
            "/boot",
        ]
    };

    let path_str = normalized.to_string_lossy().to_lowercase();
    for prefix in forbidden_prefixes {
        if path_str.starts_with(&prefix.to_lowercase()) {
            return Err(format!(
                "Writing to system directory '{}' is not allowed",
                path.display()
            ));
        }
    }

    validate_path_safe(path)
}

/// Validate a path for delete operations (most restrictive).
pub fn validate_path_for_delete(path: &std::path::Path) -> Result<std::path::PathBuf, String> {
    let validated = validate_path_for_write(path)?;
    let normalized = normalize_path(path);

    // Prevent deletion of home directory
    if let Some(home) = dirs::home_dir()
        && (normalized == home || validated == home)
    {
        return Err("Cannot delete home directory".to_string());
    }

    // Prevent deletion of root directories
    if normalized.parent().is_none() || normalized.as_os_str().is_empty() {
        return Err("Cannot delete root directory".to_string());
    }

    Ok(validated)
}
