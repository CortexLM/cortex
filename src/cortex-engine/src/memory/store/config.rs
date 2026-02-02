//! Memory store configuration.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Storage backend type.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageBackend {
    /// In-memory storage (session only).
    #[default]
    InMemory,
    /// JSON file storage.
    JsonFile(PathBuf),
    /// SQLite with vector extension.
    Sqlite(PathBuf),
}

/// Memory store configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryStoreConfig {
    /// Storage backend.
    pub backend: StorageBackend,
    /// Auto-save interval in seconds (for file backends).
    pub auto_save_interval: u64,
    /// Maximum memories per scope.
    pub max_per_scope: usize,
    /// Enable compression for storage.
    pub compression: bool,
    /// Relevance threshold for pruning.
    pub prune_threshold: f32,
}

impl Default for MemoryStoreConfig {
    fn default() -> Self {
        Self {
            backend: StorageBackend::InMemory,
            auto_save_interval: 30,
            max_per_scope: 5000,
            compression: false,
            prune_threshold: 0.1,
        }
    }
}
