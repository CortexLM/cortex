//! Session sharing functionality for Cortex CLI.
//!
//! Allows sharing sessions via public URLs for collaboration.

pub mod share;
pub mod sync;

pub use share::{ShareManager, ShareMode, SharedSession};
pub use sync::ShareSync;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum ShareError {
    #[error("Share not found: {0}")]
    NotFound(String),
    #[error("Share API error: {0}")]
    ApiError(String),
    #[error("Network error: {0}")]
    Network(String),
    #[error("Session not shared")]
    NotShared,
}

pub type Result<T> = std::result::Result<T, ShareError>;

/// Default API URL for sharing.
pub const DEFAULT_SHARE_API: &str = "https://api.cortex.foundation";
