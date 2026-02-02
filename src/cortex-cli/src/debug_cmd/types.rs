//! Shared types for debug commands.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

// =============================================================================
// Config types
// =============================================================================

/// Config debug output.
#[derive(Debug, Serialize)]
pub struct ConfigDebugOutput {
    pub resolved: ResolvedConfig,
    pub locations: ConfigLocations,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub environment: Option<HashMap<String, String>>,
}

/// Resolved configuration values.
#[derive(Debug, Serialize)]
pub struct ResolvedConfig {
    pub model: String,
    pub provider: String,
    pub cwd: PathBuf,
    pub cortex_home: PathBuf,
}

/// Config file locations.
#[derive(Debug, Serialize)]
pub struct ConfigLocations {
    pub global_config: PathBuf,
    pub global_config_exists: bool,
    pub local_config: Option<PathBuf>,
    pub local_config_exists: bool,
}

/// Result of comparing two config files.
#[derive(Debug, Serialize)]
pub struct ConfigDiff {
    pub only_in_global: Vec<String>,
    pub only_in_local: Vec<String>,
    pub unified_diff: String,
}

// =============================================================================
// File types
// =============================================================================

/// File debug output.
#[derive(Debug, Serialize)]
pub struct FileDebugOutput {
    pub path: PathBuf,
    pub exists: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<FileMetadata>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    pub encoding: Option<String>,
    pub is_binary: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Warning when the file appears to be actively modified
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_modification_warning: Option<String>,
}

/// File metadata.
#[derive(Debug, Serialize)]
pub struct FileMetadata {
    pub size: u64,
    /// For virtual filesystems (procfs, sysfs), stat() returns 0 but reading
    /// the file may return actual content. This field stores the actual
    /// content size when available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual_size: Option<u64>,
    /// Whether the file is on a virtual filesystem (procfs, sysfs, etc.)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_virtual_fs: Option<bool>,
    pub is_file: bool,
    pub is_dir: bool,
    pub is_symlink: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symlink_target: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created: Option<String>,
    pub readonly: bool,
    /// Unix-style permission string (e.g., "rw-r--r--")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permissions: Option<String>,
    /// Numeric permission mode (e.g., 644)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<u32>,
}

// =============================================================================
// LSP types
// =============================================================================

/// LSP server info.
#[derive(Debug, Serialize)]
pub struct LspServerInfo {
    pub name: String,
    pub language: String,
    pub command: String,
    pub installed: bool,
    pub version: Option<String>,
    pub path: Option<PathBuf>,
}

/// LSP debug output.
#[derive(Debug, Serialize)]
pub struct LspDebugOutput {
    pub servers: Vec<LspServerInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection_test: Option<LspConnectionTest>,
}

/// LSP connection test result.
#[derive(Debug, Serialize)]
pub struct LspConnectionTest {
    pub server: String,
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

// =============================================================================
// Ripgrep types
// =============================================================================

/// Ripgrep debug output.
#[derive(Debug, Serialize)]
pub struct RipgrepDebugOutput {
    pub available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub searched_paths: Option<Vec<PathBuf>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub test_result: Option<RipgrepTestResult>,
}

/// Ripgrep test result.
#[derive(Debug, Serialize)]
pub struct RipgrepTestResult {
    pub pattern: String,
    pub directory: PathBuf,
    pub matches_found: usize,
    pub duration_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

// =============================================================================
// Skill types
// =============================================================================

/// Skill debug output.
#[derive(Debug, Serialize)]
pub struct SkillDebugOutput {
    pub name: String,
    pub path: Option<PathBuf>,
    pub found: bool,
    pub valid: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub definition: Option<SkillDefinition>,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

/// Skill definition (simplified).
#[derive(Debug, Serialize, Deserialize)]
pub struct SkillDefinition {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub commands: Vec<SkillCommand>,
}

/// Skill command definition.
#[derive(Debug, Serialize, Deserialize)]
pub struct SkillCommand {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub command: Option<String>,
}

// =============================================================================
// Snapshot types
// =============================================================================

/// Snapshot debug output.
#[derive(Debug, Serialize)]
pub struct SnapshotDebugOutput {
    pub snapshots_dir: PathBuf,
    pub snapshots_dir_exists: bool,
    pub snapshot_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_snapshots: Option<Vec<SnapshotInfo>>,
    pub total_size_bytes: u64,
}

/// Snapshot info.
#[derive(Debug, Serialize)]
pub struct SnapshotInfo {
    pub id: String,
    pub timestamp: String,
    pub size_bytes: u64,
}

// =============================================================================
// Paths types
// =============================================================================

/// Paths debug output.
#[derive(Debug, Serialize)]
pub struct PathsDebugOutput {
    pub cortex_home: PathInfo,
    pub config_dir: PathInfo,
    pub data_dir: PathInfo,
    pub cache_dir: PathInfo,
    pub sessions_dir: PathInfo,
    pub plugins_dir: PathInfo,
    pub skills_dir: PathInfo,
    pub agents_dir: PathInfo,
    pub mcp_dir: PathInfo,
    pub logs_dir: PathInfo,
    pub snapshots_dir: PathInfo,
    pub temp_dir: PathInfo,
}

/// Path info.
#[derive(Debug, Serialize)]
pub struct PathInfo {
    pub path: PathBuf,
    pub exists: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
}

impl PathInfo {
    pub fn new(path: PathBuf) -> Self {
        let exists = path.exists();
        let size_bytes = if exists && path.is_dir() {
            crate::debug_cmd::utils::dir_size(&path).ok()
        } else if exists {
            std::fs::metadata(&path).ok().map(|m| m.len())
        } else {
            None
        };
        Self {
            path,
            exists,
            size_bytes,
        }
    }
}

// =============================================================================
// Wait types
// =============================================================================

/// Wait result output.
#[derive(Debug, Serialize)]
pub struct WaitResult {
    pub condition: String,
    pub success: bool,
    pub waited_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

// =============================================================================
// System types
// =============================================================================

/// System debug output.
#[derive(Debug, Serialize)]
pub struct SystemDebugOutput {
    pub os: OsInfo,
    pub hardware: HardwareInfo,
    pub environment: EnvironmentInfo,
    pub cortex: CortexInfo,
}

/// Operating system information.
#[derive(Debug, Serialize)]
pub struct OsInfo {
    pub name: String,
    pub version: Option<String>,
    pub family: String,
}

/// Hardware information.
#[derive(Debug, Serialize)]
pub struct HardwareInfo {
    pub arch: String,
    pub cpu_cores: Option<usize>,
    /// Total memory in bytes (considers container limits on Linux)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_memory_bytes: Option<u64>,
    /// Human-readable memory string
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_memory: Option<String>,
    /// Whether running in a container with memory limits
    #[serde(skip_serializing_if = "Option::is_none")]
    pub container_memory_limit: Option<bool>,
}

/// Environment information.
#[derive(Debug, Serialize)]
pub struct EnvironmentInfo {
    pub shell: Option<String>,
    pub home_dir: Option<PathBuf>,
    pub current_dir: Option<PathBuf>,
    pub user: Option<String>,
    /// User ID (Unix only) - useful for container environments
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uid: Option<u32>,
    pub term: Option<String>,
}

/// Cortex-specific information.
#[derive(Debug, Serialize)]
pub struct CortexInfo {
    pub version: String,
    pub cortex_home: PathBuf,
    pub rust_version: Option<String>,
}
