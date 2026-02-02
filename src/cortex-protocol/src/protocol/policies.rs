//! Approval and sandbox policies for command execution.

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use strum_macros::Display;

// ============================================================
// Approval Policies
// ============================================================

/// Determines when user approval is requested for commands.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, Display, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum AskForApproval {
    /// Only auto-approve known safe read-only commands.
    #[serde(rename = "untrusted")]
    #[strum(serialize = "untrusted")]
    UnlessTrusted,

    /// Auto-approve in sandbox, escalate on failure.
    OnFailure,

    /// Model decides when to ask (default).
    #[default]
    OnRequest,

    /// Never ask for approval.
    Never,
}

/// User's decision for an approval request.
#[derive(
    Debug, Default, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, Display, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ReviewDecision {
    /// Approved for execution.
    Approved,
    /// Approved for this and future identical requests.
    ApprovedForSession,
    /// Denied, agent should try something else.
    #[default]
    Denied,
    /// Denied, agent should stop until next user input.
    Abort,
}

/// Action for MCP elicitation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ElicitationAction {
    Approve,
    Deny,
}

// ============================================================
// Sandbox Policies
// ============================================================

/// Determines execution restrictions for shell commands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Display, JsonSchema)]
#[serde(tag = "type", rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum SandboxPolicy {
    /// No restrictions. Use with caution.
    #[serde(rename = "danger-full-access")]
    DangerFullAccess,

    /// Read-only access to filesystem.
    #[serde(rename = "read-only")]
    ReadOnly,

    /// Read access + write to workspace.
    #[serde(rename = "workspace-write")]
    WorkspaceWrite {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        writable_roots: Vec<PathBuf>,
        #[serde(default)]
        network_access: bool,
        #[serde(default)]
        exclude_tmpdir_env_var: bool,
        #[serde(default)]
        exclude_slash_tmp: bool,
    },
}

impl Default for SandboxPolicy {
    fn default() -> Self {
        Self::new_workspace_write_policy()
    }
}

impl SandboxPolicy {
    pub fn new_read_only_policy() -> Self {
        Self::ReadOnly
    }

    pub fn new_workspace_write_policy() -> Self {
        Self::WorkspaceWrite {
            writable_roots: vec![],
            network_access: false,
            exclude_tmpdir_env_var: false,
            exclude_slash_tmp: false,
        }
    }

    pub fn has_full_disk_read_access(&self) -> bool {
        true
    }

    pub fn has_full_disk_write_access(&self) -> bool {
        matches!(self, Self::DangerFullAccess)
    }

    pub fn has_full_network_access(&self) -> bool {
        match self {
            Self::DangerFullAccess => true,
            Self::ReadOnly => false,
            Self::WorkspaceWrite { network_access, .. } => *network_access,
        }
    }

    /// Returns the list of writable roots including cwd, /tmp, and $TMPDIR by default.
    pub fn get_writable_roots_with_cwd(&self, cwd: &std::path::Path) -> Vec<WritableRoot> {
        match self {
            Self::DangerFullAccess => Vec::new(),
            Self::ReadOnly => Vec::new(),
            Self::WorkspaceWrite {
                writable_roots,
                exclude_tmpdir_env_var,
                exclude_slash_tmp,
                network_access: _,
            } => {
                let mut roots: Vec<PathBuf> = writable_roots.clone();
                let _ = exclude_slash_tmp; // Used in #[cfg(unix)] block below

                // Always include cwd
                roots.push(cwd.to_path_buf());

                // Include /tmp on Unix unless explicitly excluded
                #[cfg(unix)]
                if !exclude_slash_tmp {
                    let slash_tmp = PathBuf::from("/tmp");
                    if slash_tmp.is_dir() {
                        roots.push(slash_tmp);
                    }
                }

                // Include $TMPDIR unless explicitly excluded (important for macOS)
                if !exclude_tmpdir_env_var
                    && let Some(tmpdir) = std::env::var_os("TMPDIR")
                    && !tmpdir.is_empty()
                {
                    roots.push(PathBuf::from(tmpdir));
                }

                // Include common tool cache directories under $HOME
                // This allows npm, yarn, pip, cargo, etc. to work without permission errors
                if let Some(home) = std::env::var_os("HOME") {
                    let home_path = PathBuf::from(home);
                    // npm cache
                    let npm_cache = home_path.join(".npm");
                    if npm_cache.is_dir() {
                        roots.push(npm_cache);
                    }
                    // General cache directory (used by many tools)
                    let cache_dir = home_path.join(".cache");
                    if cache_dir.is_dir() {
                        roots.push(cache_dir);
                    }
                    // yarn cache
                    let yarn_cache = home_path.join(".yarn");
                    if yarn_cache.is_dir() {
                        roots.push(yarn_cache);
                    }
                    // pnpm cache
                    let pnpm_cache = home_path.join(".pnpm");
                    if pnpm_cache.is_dir() {
                        roots.push(pnpm_cache);
                    }
                    // pip cache
                    let pip_cache = home_path.join(".pip");
                    if pip_cache.is_dir() {
                        roots.push(pip_cache);
                    }
                    // local bin (for tools installed with --user)
                    let local_bin = home_path.join(".local");
                    if local_bin.is_dir() {
                        roots.push(local_bin);
                    }
                }

                // Convert to WritableRoot with .git protection
                roots
                    .into_iter()
                    .map(|root| {
                        let mut subpaths = Vec::new();
                        let top_level_git = root.join(".git");
                        if top_level_git.is_dir() {
                            subpaths.push(top_level_git);
                        }
                        WritableRoot {
                            root,
                            read_only_subpaths: subpaths,
                        }
                    })
                    .collect()
            }
        }
    }
}

// ============================================================
// Writable Root
// ============================================================

/// A writable root path with optional read-only subpaths (e.g., .git directories).
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema)]
pub struct WritableRoot {
    /// Absolute path to the writable root.
    pub root: PathBuf,
    /// Subpaths that should remain read-only (e.g., .git).
    pub read_only_subpaths: Vec<PathBuf>,
}

impl WritableRoot {
    /// Normalize a path by resolving `.` and `..` components without filesystem access.
    fn normalize_path(path: &std::path::Path) -> PathBuf {
        use std::path::Component;
        let mut normalized = PathBuf::new();

        for component in path.components() {
            match component {
                Component::ParentDir => {
                    if !normalized.pop() {
                        if !path.is_absolute() {
                            normalized.push("..");
                        }
                    }
                }
                Component::CurDir => {}
                _ => {
                    normalized.push(component);
                }
            }
        }

        normalized
    }

    /// Check if a path is writable under this root.
    ///
    /// This method:
    /// 1. Canonicalizes both paths to resolve symlinks and relative components
    /// 2. Validates the path is within the writable root
    /// 3. Checks against read-only subpaths
    pub fn is_path_writable(&self, path: &std::path::Path) -> bool {
        // Canonicalize the root path if it exists, otherwise normalize
        let canonical_root = if self.root.exists() {
            match self.root.canonicalize() {
                Ok(p) => p,
                Err(_) => Self::normalize_path(&self.root),
            }
        } else {
            Self::normalize_path(&self.root)
        };

        // Canonicalize the input path if it exists, otherwise normalize
        let canonical_path = if path.exists() {
            match path.canonicalize() {
                Ok(p) => p,
                Err(_) => Self::normalize_path(path),
            }
        } else {
            // For non-existent paths, try to canonicalize the parent
            if let Some(parent) = path.parent() {
                if parent.exists() {
                    match parent.canonicalize() {
                        Ok(canonical_parent) => {
                            if let Some(file_name) = path.file_name() {
                                canonical_parent.join(file_name)
                            } else {
                                Self::normalize_path(path)
                            }
                        }
                        Err(_) => Self::normalize_path(path),
                    }
                } else {
                    Self::normalize_path(path)
                }
            } else {
                Self::normalize_path(path)
            }
        };

        // Check if the canonical path starts with the canonical root
        if !canonical_path.starts_with(&canonical_root) {
            return false;
        }

        // Check if path is under any read-only subpath
        for subpath in &self.read_only_subpaths {
            let canonical_subpath = if subpath.exists() {
                match subpath.canonicalize() {
                    Ok(p) => p,
                    Err(_) => Self::normalize_path(subpath),
                }
            } else {
                Self::normalize_path(subpath)
            };

            if canonical_path.starts_with(&canonical_subpath) {
                return false;
            }
        }

        // Additional check: verify symlinks don't escape the root
        if path.exists() {
            if let Ok(metadata) = std::fs::symlink_metadata(path) {
                if metadata.file_type().is_symlink() {
                    if let Ok(target) = std::fs::read_link(path) {
                        let absolute_target = if target.is_absolute() {
                            target
                        } else {
                            path.parent().map(|p| p.join(&target)).unwrap_or(target)
                        };

                        let target_canonical = if absolute_target.exists() {
                            match absolute_target.canonicalize() {
                                Ok(p) => p,
                                Err(_) => Self::normalize_path(&absolute_target),
                            }
                        } else {
                            Self::normalize_path(&absolute_target)
                        };

                        // Symlink target must also be within root
                        if !target_canonical.starts_with(&canonical_root) {
                            return false;
                        }
                    }
                }
            }
        }

        true
    }
}
