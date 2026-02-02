//! Restore from ghost commits.

use crate::{GhostCommit, GhostError, Result};
use std::path::Path;
use tokio::process::Command;
use tracing::{info, warn};

/// Options for restoring a ghost commit.
#[derive(Debug, Clone, Default)]
pub struct RestoreOptions {
    /// Whether to create a backup before restore.
    pub create_backup: bool,
    /// Whether to keep untracked files.
    pub keep_untracked: bool,
    /// Whether to hard reset (lose all changes).
    pub hard_reset: bool,
}

/// Result of restore operation.
#[derive(Debug, Clone)]
pub struct RestoreResult {
    /// Whether restore was successful.
    pub success: bool,
    /// Files that were restored.
    pub restored_files: Vec<String>,
    /// Files that were backed up.
    pub backed_up_files: Vec<String>,
    /// Error message if failed.
    pub error: Option<String>,
}

/// Restore to a ghost commit.
pub async fn restore_ghost_commit(
    repo_path: &Path,
    ghost: &GhostCommit,
    options: RestoreOptions,
) -> Result<RestoreResult> {
    let mut result = RestoreResult {
        success: false,
        restored_files: Vec::new(),
        backed_up_files: Vec::new(),
        error: None,
    };

    // Verify the commit exists
    let verify = Command::new("git")
        .args(["cat-file", "-t", &ghost.sha])
        .current_dir(repo_path)
        .output()
        .await?;

    if !verify.status.success() {
        return Err(GhostError::NotFound(ghost.sha.clone()));
    }

    // Create backup if requested
    if options.create_backup {
        let backup_branch = format!("backup-before-undo-{}", chrono::Utc::now().timestamp());
        let backup = Command::new("git")
            .args(["branch", &backup_branch])
            .current_dir(repo_path)
            .output()
            .await?;

        if backup.status.success() {
            info!("Created backup branch: {}", backup_branch);
        }
    }

    // Get list of files that will be affected
    let diff = Command::new("git")
        .args(["diff", "--name-only", &ghost.sha, "HEAD"])
        .current_dir(repo_path)
        .output()
        .await?;

    if diff.status.success() {
        result.restored_files = String::from_utf8_lossy(&diff.stdout)
            .lines()
            .map(String::from)
            .collect();
    }

    // Perform the restore
    if options.hard_reset {
        // Hard reset to the ghost commit
        let reset = Command::new("git")
            .args(["reset", "--hard", &ghost.sha])
            .current_dir(repo_path)
            .output()
            .await?;

        if !reset.status.success() {
            let stderr = String::from_utf8_lossy(&reset.stderr);
            result.error = Some(stderr.to_string());
            return Err(GhostError::RestoreFailed(stderr.to_string()));
        }
    } else {
        // Soft restore - checkout files from ghost commit
        let checkout = Command::new("git")
            .args(["checkout", &ghost.sha, "--", "."])
            .current_dir(repo_path)
            .output()
            .await?;

        if !checkout.status.success() {
            let stderr = String::from_utf8_lossy(&checkout.stderr);
            result.error = Some(stderr.to_string());
            return Err(GhostError::RestoreFailed(stderr.to_string()));
        }
    }

    // Clean untracked files if requested
    if !options.keep_untracked {
        let clean = Command::new("git")
            .args(["clean", "-fd"])
            .current_dir(repo_path)
            .output()
            .await?;

        if !clean.status.success() {
            warn!("Failed to clean untracked files");
        }
    }

    result.success = true;
    info!(
        "Restored to ghost commit {} ({} files)",
        ghost.sha,
        result.restored_files.len()
    );

    Ok(result)
}

/// Undo the most recent changes by restoring to previous ghost commit.
pub async fn undo_to_previous(
    repo_path: &Path,
    current_sha: &str,
    ghosts: &[GhostCommit],
) -> Result<Option<RestoreResult>> {
    // Find current position and previous ghost
    let current_idx = ghosts.iter().position(|g| g.sha == current_sha);

    if let Some(idx) = current_idx {
        if idx > 0 {
            let previous = &ghosts[idx - 1];
            let result = restore_ghost_commit(
                repo_path,
                previous,
                RestoreOptions {
                    create_backup: true,
                    keep_untracked: false,
                    hard_reset: false,
                },
            )
            .await?;
            return Ok(Some(result));
        }
    }

    // If no current position found, try to find the latest ghost before HEAD
    if let Some(latest) = ghosts.last() {
        let result = restore_ghost_commit(
            repo_path,
            latest,
            RestoreOptions {
                create_backup: true,
                keep_untracked: false,
                hard_reset: false,
            },
        )
        .await?;
        return Ok(Some(result));
    }

    Ok(None)
}

#[cfg(test)]
mod tests {
    // Tests would go here
}
