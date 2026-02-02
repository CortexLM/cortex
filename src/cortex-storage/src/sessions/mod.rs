//! Session storage - metadata and message history persistence.
//!
//! Provides unified storage for:
//! - Session metadata (JSON files)
//! - Message history (JSONL files for append-only efficiency)
//! - Session favorites, tags, and sharing functionality
//!
//! # Module Structure
//!
//! - [`types`] - Core data structures (StoredSession, StoredMessage, etc.)
//! - [`query`] - Query system for filtering and sorting sessions
//! - [`storage`] - Storage operations (CRUD, history, sharing)

mod query;
mod storage;
#[cfg(test)]
mod tests;
mod types;

// Re-export all public types for backwards compatibility
pub use query::{SessionQuery, SessionSort};
pub use storage::SessionStorage;
pub use types::{SessionSummary, ShareInfo, StoredMessage, StoredSession, StoredToolCall};
