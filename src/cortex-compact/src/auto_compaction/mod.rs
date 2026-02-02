//! Auto-compaction intelligent system for CLI backend.
//!
//! This module provides automated data compaction and state optimization:
//! - Scheduled background compaction of conversation history
//! - Intelligent log pruning with retention policies
//! - Database vacuuming for session storage optimization
//! - Race condition protection with file-system-safe operations
//!
//! # File System Safety
//!
//! All compaction operations follow these safety principles:
//! 1. Atomic writes using temp file + rename pattern
//! 2. fsync/sync_all() for durability before rename
//! 3. Lock files to prevent concurrent access
//! 4. Graceful handling of incomplete operations

mod atomic_ops;
mod config;
mod lock;
mod log_pruner;
mod scheduler;
mod utils;
mod vacuumer;

// Re-export all public items for backwards compatibility
pub use atomic_ops::{atomic_write, atomic_write_with_backup};
pub use config::{
    AutoCompactionConfig, BACKUP_SUFFIX, COMPACTION_LOCK_FILE, DEFAULT_COMPACTION_INTERVAL_SECS,
    DEFAULT_LOG_RETENTION_DAYS, DEFAULT_SESSION_RETENTION_DAYS, MAX_LOG_FILE_SIZE, TEMP_SUFFIX,
};
pub use lock::CompactionLock;
pub use log_pruner::{LogFileInfo, LogPruner, LogPruningResult};
pub use scheduler::{AutoCompactionScheduler, CompactionHandle, CompactionStats};
pub use utils::available_disk_space;
pub use vacuumer::{DatabaseVacuumer, VacuumResult};
