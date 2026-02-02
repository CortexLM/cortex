#![allow(clippy::manual_inspect)]
//! Session resume functionality for Cortex CLI.
//!
//! Provides the ability to resume previous sessions with full context.

pub mod resume_picker;
pub mod session_meta;
pub mod session_store;

pub use resume_picker::ResumePicker;
pub use session_meta::{SessionMeta, SessionSummary};
pub use session_store::SessionStore;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum ResumeError {
    #[error("Session not found: {0}")]
    SessionNotFound(String),
    #[error("Session corrupted: {0}")]
    SessionCorrupted(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Parse error: {0}")]
    Parse(String),
}

pub type Result<T> = std::result::Result<T, ResumeError>;
