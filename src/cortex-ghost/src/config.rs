//! Ghost commit configuration.

use serde::{Deserialize, Serialize};

/// Configuration for ghost commits.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GhostConfig {
    /// Whether ghost commits are enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Maximum size for untracked files to include (bytes).
    #[serde(default = "default_max_file_size")]
    pub max_untracked_file_size: i64,
    /// Maximum number of files in untracked directories.
    #[serde(default = "default_max_dir_files")]
    pub max_untracked_dir_files: i64,
    /// Directories to always ignore.
    #[serde(default = "default_ignored_dirs")]
    pub ignored_dirs: Vec<String>,
    /// Whether to show warnings for large files.
    #[serde(default = "default_true")]
    pub show_warnings: bool,
}

fn default_true() -> bool {
    true
}

fn default_max_file_size() -> i64 {
    10 * 1024 * 1024 // 10 MiB
}

fn default_max_dir_files() -> i64 {
    200
}

fn default_ignored_dirs() -> Vec<String> {
    vec![
        "node_modules".to_string(),
        ".venv".to_string(),
        "venv".to_string(),
        "target".to_string(),
        "dist".to_string(),
        "build".to_string(),
        ".pytest_cache".to_string(),
        ".mypy_cache".to_string(),
        "__pycache__".to_string(),
        ".cache".to_string(),
        ".tox".to_string(),
    ]
}

impl Default for GhostConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_untracked_file_size: default_max_file_size(),
            max_untracked_dir_files: default_max_dir_files(),
            ignored_dirs: default_ignored_dirs(),
            show_warnings: true,
        }
    }
}
