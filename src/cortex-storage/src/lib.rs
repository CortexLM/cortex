//! Cortex Storage - Centralized, OS-aware storage for Cortex.
//!
//! This crate provides unified storage management for Cortex across all platforms:
//!
//! - **Windows**: `%APPDATA%\Cortex\`
//! - **macOS**: `~/Library/Application Support/Cortex/`
//! - **Linux**: `~/.local/share/Cortex/`
//!
//! # Features
//!
//! - Automatic OS detection for storage paths
//! - Session metadata persistence (JSON)
//! - Message history (JSONL for efficient appending)
//! - Both async and sync APIs
//!
//! # Usage
//!
//! ```rust,no_run
//! use cortex_storage::{SessionStorage, StoredSession, StoredMessage};
//!
//! #[tokio::main]
//! async fn main() -> cortex_storage::Result<()> {
//!     // Initialize storage
//!     let storage = SessionStorage::new()?;
//!     storage.init().await?;
//!
//!     // Create a session
//!     let session = StoredSession::new("gpt-4o", "/my/project");
//!     storage.save_session(&session).await?;
//!
//!     // Add messages
//!     let msg = StoredMessage::user("Hello!");
//!     storage.append_message(&session.id, &msg).await?;
//!
//!     // List sessions
//!     let sessions = storage.list_sessions().await?;
//!     println!("Found {} sessions", sessions.len());
//!
//!     Ok(())
//! }
//! ```

pub mod error;
pub mod paths;
pub mod sessions;

// Re-export main types at crate root
pub use error::{Result, StorageError};
pub use paths::{cortex_config_dir, cortex_data_dir, CortexPaths};
pub use sessions::{
    SessionQuery, SessionSort, SessionStorage, SessionSummary, ShareInfo, StoredMessage,
    StoredSession, StoredToolCall,
};
