//! Batch operations and MultiEdit tools for Cortex CLI.
//!
//! Provides tools for:
//! - Multi-file editing with a single operation
//! - Batch file operations (create, delete, move)
//! - Search and replace across files

pub mod batch_ops;
pub mod multi_edit;
pub mod search_replace;

pub use batch_ops::{BatchOperation, BatchOps, BatchResult};
pub use multi_edit::{EditOperation, MultiEdit, MultiEditResult};
pub use search_replace::{ReplaceResult, SearchPattern, SearchReplace};

use thiserror::Error;

#[derive(Error, Debug)]
pub enum BatchError {
    #[error("File not found: {0}")]
    FileNotFound(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Pattern error: {0}")]
    Pattern(String),
    #[error("Edit failed: {0}")]
    EditFailed(String),
    #[error("Operation cancelled")]
    Cancelled,
}

pub type Result<T> = std::result::Result<T, BatchError>;
