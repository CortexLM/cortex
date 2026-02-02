//! Sandbox execution modes.
//!
//! This module defines different security levels for sandbox execution:
//! - `ReadOnly`: No file writes allowed
//! - `WorkspaceWrite`: Writes only to workspace and temp directories
//! - `DangerFullAccess`: Full system access (dangerous!)
//!
//! # Usage
//!
//! ```rust
//! use cortex_sandbox::modes::{SandboxMode, SandboxModeConfig};
//! use std::path::Path;
//!
//! let mode = SandboxMode::WorkspaceWrite;
//! let workspace = Path::new("/home/user/project");
//! let config = SandboxModeConfig::new(mode, workspace.to_path_buf());
//!
//! // Get writable paths for this mode
//! let writable = config.writable_paths();
//! ```

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Sandbox execution mode defining the level of file system access.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SandboxMode {
    /// No file writes allowed.
    /// Only read access to the filesystem is permitted.
    ReadOnly,

    /// Writes allowed to workspace and temp directories only.
    /// This is the default safe mode for most operations.
    #[default]
    WorkspaceWrite,

    /// DANGEROUS: Full filesystem access.
    /// Only use when absolutely necessary and with user confirmation.
    DangerFullAccess,
}

impl SandboxMode {
    /// Get a description of this mode.
    pub fn description(&self) -> &'static str {
        match self {
            SandboxMode::ReadOnly => "Read-only mode - no file modifications allowed",
            SandboxMode::WorkspaceWrite => {
                "Workspace write mode - writes limited to workspace and temp directories"
            }
            SandboxMode::DangerFullAccess => {
                "[DANGER] Full access mode - unrestricted filesystem access"
            }
        }
    }

    /// Check if this mode allows any writes.
    pub fn allows_writes(&self) -> bool {
        !matches!(self, SandboxMode::ReadOnly)
    }

    /// Check if this mode allows network access.
    ///
    /// Network access is typically tied to sandbox mode for security:
    /// - ReadOnly: No network (prevent data exfiltration)
    /// - WorkspaceWrite: Limited network (through proxy)
    /// - DangerFullAccess: Full network
    pub fn allows_network(&self) -> bool {
        !matches!(self, SandboxMode::ReadOnly)
    }

    /// Check if this mode is dangerous and requires user confirmation.
    pub fn is_dangerous(&self) -> bool {
        matches!(self, SandboxMode::DangerFullAccess)
    }

    /// Get the security level (0 = most restrictive, 2 = least restrictive).
    pub fn security_level(&self) -> u8 {
        match self {
            SandboxMode::ReadOnly => 0,
            SandboxMode::WorkspaceWrite => 1,
            SandboxMode::DangerFullAccess => 2,
        }
    }
}

impl std::fmt::Display for SandboxMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SandboxMode::ReadOnly => write!(f, "read-only"),
            SandboxMode::WorkspaceWrite => write!(f, "workspace-write"),
            SandboxMode::DangerFullAccess => write!(f, "danger-full-access"),
        }
    }
}

impl std::str::FromStr for SandboxMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().replace('_', "-").as_str() {
            "read-only" | "readonly" | "ro" => Ok(SandboxMode::ReadOnly),
            "workspace-write" | "workspacewrite" | "workspace" | "ws" | "default" => {
                Ok(SandboxMode::WorkspaceWrite)
            }
            "danger-full-access" | "dangerfullaccess" | "full" | "danger" | "unsafe" => {
                Ok(SandboxMode::DangerFullAccess)
            }
            _ => Err(format!(
                "Unknown sandbox mode: '{}'. Valid modes: read-only, workspace-write, danger-full-access",
                s
            )),
        }
    }
}

/// Configuration for sandbox mode including workspace path.
#[derive(Debug, Clone)]
pub struct SandboxModeConfig {
    /// The sandbox execution mode.
    mode: SandboxMode,

    /// The workspace root directory.
    workspace: PathBuf,

    /// Additional writable paths (beyond workspace and temp).
    additional_writable: Vec<PathBuf>,

    /// Additional readable paths.
    additional_readable: Vec<PathBuf>,
}

impl SandboxModeConfig {
    /// Create a new sandbox mode configuration.
    pub fn new(mode: SandboxMode, workspace: PathBuf) -> Self {
        Self {
            mode,
            workspace,
            additional_writable: Vec::new(),
            additional_readable: Vec::new(),
        }
    }

    /// Create a read-only configuration.
    pub fn read_only(workspace: PathBuf) -> Self {
        Self::new(SandboxMode::ReadOnly, workspace)
    }

    /// Create a workspace-write configuration (default).
    pub fn workspace_write(workspace: PathBuf) -> Self {
        Self::new(SandboxMode::WorkspaceWrite, workspace)
    }

    /// Create a danger-full-access configuration.
    ///
    /// # Safety
    /// This grants unrestricted filesystem access. Use with extreme caution.
    pub fn danger_full_access(workspace: PathBuf) -> Self {
        Self::new(SandboxMode::DangerFullAccess, workspace)
    }

    /// Add an additional writable path.
    pub fn add_writable(&mut self, path: PathBuf) -> &mut Self {
        self.additional_writable.push(path);
        self
    }

    /// Add an additional readable path.
    pub fn add_readable(&mut self, path: PathBuf) -> &mut Self {
        self.additional_readable.push(path);
        self
    }

    /// Get the sandbox mode.
    pub fn mode(&self) -> SandboxMode {
        self.mode
    }

    /// Get the workspace path.
    pub fn workspace(&self) -> &PathBuf {
        &self.workspace
    }

    /// Get all writable paths for this configuration.
    ///
    /// Returns paths based on the sandbox mode:
    /// - ReadOnly: Empty (no writes allowed)
    /// - WorkspaceWrite: Workspace, temp dirs, cache dir, plus additional
    /// - DangerFullAccess: Root filesystem (/)
    pub fn writable_paths(&self) -> Vec<PathBuf> {
        match self.mode {
            SandboxMode::ReadOnly => Vec::new(),
            SandboxMode::WorkspaceWrite => {
                let mut paths = vec![
                    self.workspace.clone(),
                    PathBuf::from("/tmp"),
                    PathBuf::from("/var/tmp"),
                ];

                // Add platform-specific temp/cache directories
                #[cfg(target_os = "macos")]
                {
                    if let Some(home) = dirs::home_dir() {
                        paths.push(home.join("Library/Caches"));
                    }
                    // macOS also uses /private/tmp
                    paths.push(PathBuf::from("/private/tmp"));
                }

                #[cfg(target_os = "linux")]
                {
                    if let Some(cache) = dirs::cache_dir() {
                        paths.push(cache);
                    }
                    // Add XDG runtime dir if set
                    if let Ok(runtime) = std::env::var("XDG_RUNTIME_DIR") {
                        paths.push(PathBuf::from(runtime));
                    }
                }

                #[cfg(target_os = "windows")]
                {
                    if let Some(cache) = dirs::cache_dir() {
                        paths.push(cache);
                    }
                    // Add TEMP/TMP directories
                    if let Ok(temp) = std::env::var("TEMP") {
                        paths.push(PathBuf::from(temp));
                    }
                    if let Ok(tmp) = std::env::var("TMP") {
                        paths.push(PathBuf::from(tmp));
                    }
                }

                // Add user-specified additional paths
                paths.extend(self.additional_writable.clone());

                paths
            }
            SandboxMode::DangerFullAccess => {
                // Full access - return root
                vec![PathBuf::from("/")]
            }
        }
    }

    /// Get all readable paths for this configuration.
    ///
    /// By default, the entire filesystem is readable except for
    /// sensitive paths (handled at the sandbox implementation level).
    pub fn readable_paths(&self) -> Vec<PathBuf> {
        let mut paths = vec![
            PathBuf::from("/"), // Base read access
        ];

        // Add additional readable paths
        paths.extend(self.additional_readable.clone());

        paths
    }

    /// Check if network access is allowed in this mode.
    pub fn allows_network(&self) -> bool {
        self.mode.allows_network()
    }

    /// Validate that the configuration is safe.
    ///
    /// Returns an error if the configuration has security issues.
    pub fn validate(&self) -> Result<(), String> {
        // Check workspace exists
        if !self.workspace.exists() {
            return Err(format!(
                "Workspace directory does not exist: {}",
                self.workspace.display()
            ));
        }

        // Check workspace is a directory
        if !self.workspace.is_dir() {
            return Err(format!(
                "Workspace path is not a directory: {}",
                self.workspace.display()
            ));
        }

        // Warn about dangerous mode
        if self.mode.is_dangerous() {
            // We don't error here, but callers should check is_dangerous()
            // and require explicit user confirmation
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sandbox_mode_parse() {
        assert_eq!(
            "read-only".parse::<SandboxMode>().unwrap(),
            SandboxMode::ReadOnly
        );
        assert_eq!(
            "readonly".parse::<SandboxMode>().unwrap(),
            SandboxMode::ReadOnly
        );
        assert_eq!("ro".parse::<SandboxMode>().unwrap(), SandboxMode::ReadOnly);

        assert_eq!(
            "workspace-write".parse::<SandboxMode>().unwrap(),
            SandboxMode::WorkspaceWrite
        );
        assert_eq!(
            "workspace".parse::<SandboxMode>().unwrap(),
            SandboxMode::WorkspaceWrite
        );
        assert_eq!(
            "default".parse::<SandboxMode>().unwrap(),
            SandboxMode::WorkspaceWrite
        );

        assert_eq!(
            "danger-full-access".parse::<SandboxMode>().unwrap(),
            SandboxMode::DangerFullAccess
        );
        assert_eq!(
            "full".parse::<SandboxMode>().unwrap(),
            SandboxMode::DangerFullAccess
        );
    }

    #[test]
    fn test_sandbox_mode_display() {
        assert_eq!(SandboxMode::ReadOnly.to_string(), "read-only");
        assert_eq!(SandboxMode::WorkspaceWrite.to_string(), "workspace-write");
        assert_eq!(
            SandboxMode::DangerFullAccess.to_string(),
            "danger-full-access"
        );
    }

    #[test]
    fn test_sandbox_mode_allows() {
        assert!(!SandboxMode::ReadOnly.allows_writes());
        assert!(!SandboxMode::ReadOnly.allows_network());

        assert!(SandboxMode::WorkspaceWrite.allows_writes());
        assert!(SandboxMode::WorkspaceWrite.allows_network());

        assert!(SandboxMode::DangerFullAccess.allows_writes());
        assert!(SandboxMode::DangerFullAccess.allows_network());
    }

    #[test]
    fn test_sandbox_mode_is_dangerous() {
        assert!(!SandboxMode::ReadOnly.is_dangerous());
        assert!(!SandboxMode::WorkspaceWrite.is_dangerous());
        assert!(SandboxMode::DangerFullAccess.is_dangerous());
    }

    #[test]
    fn test_sandbox_mode_security_level() {
        assert!(
            SandboxMode::ReadOnly.security_level() < SandboxMode::WorkspaceWrite.security_level()
        );
        assert!(
            SandboxMode::WorkspaceWrite.security_level()
                < SandboxMode::DangerFullAccess.security_level()
        );
    }

    #[test]
    fn test_writable_paths_readonly() {
        let config = SandboxModeConfig::read_only(PathBuf::from("/tmp/workspace"));
        assert!(config.writable_paths().is_empty());
    }

    #[test]
    fn test_writable_paths_workspace_write() {
        let workspace = PathBuf::from("/tmp/workspace");
        let config = SandboxModeConfig::workspace_write(workspace.clone());
        let paths = config.writable_paths();

        assert!(paths.contains(&workspace));
        assert!(paths.contains(&PathBuf::from("/tmp")));
    }

    #[test]
    fn test_writable_paths_danger_full_access() {
        let config = SandboxModeConfig::danger_full_access(PathBuf::from("/tmp/workspace"));
        let paths = config.writable_paths();

        assert!(paths.contains(&PathBuf::from("/")));
    }

    #[test]
    fn test_additional_paths() {
        let mut config = SandboxModeConfig::workspace_write(PathBuf::from("/tmp/workspace"));
        config.add_writable(PathBuf::from("/custom/writable"));
        config.add_readable(PathBuf::from("/custom/readable"));

        let writable = config.writable_paths();
        assert!(writable.contains(&PathBuf::from("/custom/writable")));

        let readable = config.readable_paths();
        assert!(readable.contains(&PathBuf::from("/custom/readable")));
    }

    #[test]
    fn test_default_mode() {
        let mode = SandboxMode::default();
        assert_eq!(mode, SandboxMode::WorkspaceWrite);
    }
}
