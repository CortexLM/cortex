#![allow(clippy::manual_while_let_some)]
//! Auto-compaction for Cortex CLI conversations.
//!
//! Automatically summarizes conversation history when context
//! limit is approaching to allow continued interaction.
//!
//! # Features
//!
//! - **Conversation Compaction**: Summarizes old conversation turns to reduce token usage
//! - **Auto-Compaction Scheduler**: Background task for periodic cleanup
//! - **Log Pruning**: Intelligent cleanup of log files with retention policies
//! - **Database Vacuuming**: Optimizes session storage by removing orphaned data
//! - **Race Condition Protection**: File-system-safe operations with locking
//!
//! # Example
//!
//! ```rust,no_run
//! use cortex_compact::{AutoCompactionScheduler, AutoCompactionConfig};
//! use std::path::PathBuf;
//!
//! let config = AutoCompactionConfig::default();
//! let scheduler = AutoCompactionScheduler::new(
//!     config,
//!     PathBuf::from("/data"),
//!     PathBuf::from("/data/logs"),
//!     PathBuf::from("/data/sessions"),
//!     PathBuf::from("/data/history"),
//! );
//!
//! // Run a single compaction cycle
//! let stats = scheduler.run_once().expect("compaction failed");
//! println!("Compaction completed in {}ms", stats.duration_ms);
//! ```

pub mod auto_compaction;
pub mod compactor;
pub mod config;
pub mod summarizer;

pub use auto_compaction::{
    atomic_write, atomic_write_with_backup, available_disk_space, AutoCompactionConfig,
    AutoCompactionScheduler, CompactionHandle, CompactionLock, CompactionStats, DatabaseVacuumer,
    LogFileInfo, LogPruner, LogPruningResult, VacuumResult,
};
pub use compactor::{CompactionResult, Compactor};
pub use config::CompactionConfig;
pub use summarizer::{ConversationItem, Summarizer, Summary};

use thiserror::Error;

#[derive(Error, Debug)]
pub enum CompactionError {
    #[error("Compaction failed: {0}")]
    Failed(String),
    #[error("Nothing to compact")]
    NothingToCompact,
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, CompactionError>;

/// Default prompt for compaction.
pub const COMPACTION_PROMPT: &str = r#"Summarize the conversation history above. Focus on:
1. Key decisions made
2. Important code changes
3. Outstanding tasks or issues
4. Context needed to continue the conversation

Be concise but preserve critical information."#;

/// Prefix for summaries.
pub const SUMMARY_PREFIX: &str = "[Previous conversation summary]\n";
