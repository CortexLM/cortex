//! Sandbox policy types and utilities.
//!
//! Defines the different sandbox policies and their associated permissions.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// A writable root directory with optional read-only subpaths.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WritableRoot {
    /// The writable root path.
    pub root: PathBuf,
    /// Subpaths that should remain read-only (e.g., .git, .cortex).
    pub read_only_subpaths: Vec<PathBuf>,
}

impl WritableRoot {
    /// Create a new writable root without read-only subpaths.
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            read_only_subpaths: Vec::new(),
        }
    }

    /// Create a writable root with standard protected subpaths (.git, .cortex).
    pub fn with_standard_protections(root: PathBuf) -> Self {
        let read_only_subpaths = vec![root.join(".git"), root.join(".cortex")];
        Self {
            root,
            read_only_subpaths,
        }
    }

    /// Add a read-only subpath.
    pub fn with_read_only(mut self, path: PathBuf) -> Self {
        self.read_only_subpaths.push(path);
        self
    }

    /// Check if a path is writable under this root.
    pub fn is_path_writable(&self, path: &Path) -> bool {
        if !path.starts_with(&self.root) {
            return false;
        }

        for ro_path in &self.read_only_subpaths {
            if path.starts_with(ro_path) {
                return false;
            }
        }

        true
    }
}

/// Sandbox policy defining access permissions.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(tag = "type")]
pub enum SandboxPolicyType {
    /// No sandbox - full access (dangerous, requires explicit opt-in).
    DangerFullAccess,

    /// Read-only access everywhere, no write access.
    #[default]
    ReadOnly,

    /// Write access to workspace only.
    WorkspaceWrite {
        /// Additional writable paths beyond the workspace.
        #[serde(default)]
        additional_writable: Vec<PathBuf>,
        /// Whether network access is allowed.
        #[serde(default = "default_true")]
        network_access: bool,
    },

    /// Custom policy with explicit paths.
    Custom {
        /// Writable roots with their read-only subpaths.
        writable_roots: Vec<WritableRoot>,
        /// Whether network access is allowed.
        #[serde(default)]
        network_access: bool,
        /// Whether to allow reading outside workspace.
        #[serde(default = "default_true")]
        allow_read_outside_workspace: bool,
    },
}

fn default_true() -> bool {
    true
}

impl SandboxPolicyType {
    /// Check if full disk write access is allowed.
    pub fn has_full_disk_write_access(&self) -> bool {
        matches!(self, Self::DangerFullAccess)
    }

    /// Check if full disk read access is allowed.
    pub fn has_full_disk_read_access(&self) -> bool {
        match self {
            Self::DangerFullAccess => true,
            Self::ReadOnly => true,
            Self::WorkspaceWrite { .. } => true,
            Self::Custom {
                allow_read_outside_workspace,
                ..
            } => *allow_read_outside_workspace,
        }
    }

    /// Check if network access is allowed.
    pub fn has_full_network_access(&self) -> bool {
        match self {
            Self::DangerFullAccess => true,
            Self::ReadOnly => false,
            Self::WorkspaceWrite { network_access, .. } => *network_access,
            Self::Custom { network_access, .. } => *network_access,
        }
    }

    /// Get writable roots for this policy, including the current working directory.
    pub fn get_writable_roots_with_cwd(&self, cwd: &Path) -> Vec<WritableRoot> {
        let mut roots = Vec::new();

        match self {
            Self::DangerFullAccess => {
                // Full access - single root at /
                roots.push(WritableRoot::new(PathBuf::from("/")));
            }
            Self::ReadOnly => {
                // No writable roots, but still allow /dev/null and temp
                roots.push(WritableRoot::new(get_temp_dir()));
            }
            Self::WorkspaceWrite {
                additional_writable,
                ..
            } => {
                // Workspace with standard protections
                roots.push(WritableRoot::with_standard_protections(cwd.to_path_buf()));

                // Temp directories
                roots.push(WritableRoot::new(get_temp_dir()));
                if let Some(tmpdir) = std::env::var_os("TMPDIR") {
                    let tmpdir_path = PathBuf::from(tmpdir);
                    if tmpdir_path != get_temp_dir() {
                        roots.push(WritableRoot::new(tmpdir_path));
                    }
                }

                // Additional writable paths
                for path in additional_writable {
                    roots.push(WritableRoot::new(path.clone()));
                }
            }
            Self::Custom { writable_roots, .. } => {
                roots.extend(writable_roots.clone());
            }
        }

        roots
    }

    /// Create a workspace write policy with default settings.
    pub fn workspace_write() -> Self {
        Self::WorkspaceWrite {
            additional_writable: Vec::new(),
            network_access: true,
        }
    }

    /// Create a workspace write policy without network access.
    pub fn workspace_write_no_network() -> Self {
        Self::WorkspaceWrite {
            additional_writable: Vec::new(),
            network_access: false,
        }
    }
}

/// Get the system temp directory.
fn get_temp_dir() -> PathBuf {
    #[cfg(windows)]
    {
        std::env::var_os("TEMP")
            .or_else(|| std::env::var_os("TMP"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("C:\\Windows\\Temp"))
    }

    #[cfg(not(windows))]
    {
        PathBuf::from("/tmp")
    }
}

/// Protected paths that should never be writable.
#[derive(Debug, Clone)]
pub struct ProtectedPaths {
    /// Paths that should be read-only.
    pub read_only: Vec<PathBuf>,
    /// Paths that should be completely inaccessible.
    pub denied: Vec<PathBuf>,
}

impl ProtectedPaths {
    /// Get standard protected paths for a workspace.
    pub fn for_workspace(workspace: &Path) -> Self {
        Self {
            read_only: vec![workspace.join(".git"), workspace.join(".cortex")],
            denied: vec![
                // Sensitive credential directories
                dirs::home_dir().map(|h| h.join(".ssh")).unwrap_or_default(),
                dirs::home_dir().map(|h| h.join(".aws")).unwrap_or_default(),
                dirs::home_dir()
                    .map(|h| h.join(".gnupg"))
                    .unwrap_or_default(),
                dirs::home_dir()
                    .map(|h| h.join(".kube"))
                    .unwrap_or_default(),
            ],
        }
    }
}

/// Actions that can be checked against sandbox policy.
#[derive(Debug, Clone)]
pub enum SandboxAction {
    /// Read a file.
    ReadFile(PathBuf),
    /// Write to a file.
    WriteFile(PathBuf),
    /// Execute a command.
    Execute(String),
    /// Network connection.
    NetworkConnect(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_writable_root_is_path_writable() {
        let root = WritableRoot::with_standard_protections(PathBuf::from("/workspace"));

        assert!(root.is_path_writable(Path::new("/workspace/src/main.rs")));
        assert!(!root.is_path_writable(Path::new("/workspace/.git/config")));
        assert!(!root.is_path_writable(Path::new("/workspace/.cortex/state")));
        assert!(!root.is_path_writable(Path::new("/other/file.txt")));
    }

    #[test]
    fn test_policy_has_full_access() {
        let policy = SandboxPolicyType::DangerFullAccess;
        assert!(policy.has_full_disk_write_access());
        assert!(policy.has_full_network_access());

        let policy = SandboxPolicyType::ReadOnly;
        assert!(!policy.has_full_disk_write_access());
        assert!(!policy.has_full_network_access());

        let policy = SandboxPolicyType::workspace_write();
        assert!(!policy.has_full_disk_write_access());
        assert!(policy.has_full_network_access());
    }

    #[test]
    fn test_get_writable_roots() {
        let policy = SandboxPolicyType::workspace_write();
        let cwd = Path::new("/home/user/project");
        let roots = policy.get_writable_roots_with_cwd(cwd);

        assert!(!roots.is_empty());
        assert_eq!(roots[0].root, cwd);
        assert!(roots[0].read_only_subpaths.contains(&cwd.join(".git")));
    }
}
