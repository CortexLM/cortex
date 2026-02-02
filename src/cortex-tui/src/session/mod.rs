//! Session management for Cortex TUI.
//!
//! This module provides local session storage and management.
//! Sessions are stored in `~/.config/cortex/sessions/` with the following structure:
//!
//! ```text
//! ~/.config/cortex/sessions/
//! +-- {session_id}/
//! |   +-- meta.json        # Session metadata
//! |   +-- history.jsonl    # Message history (append-only)
//! +-- ...
//! ```
//!
//! ## Features
//!
//! - **Auto-save**: Messages are automatically saved after each exchange
//! - **Resume**: Sessions can be resumed from where you left off
//! - **Fork**: Create a new session from any point in an existing session
//! - **Export**: Export sessions to Markdown, JSON, or plain text
//!
//! ## Example
//!
//! ```rust,ignore
//! use cortex_tui::session::CortexSession;
//!
//! // Create a new session
//! let mut session = CortexSession::new("cortex", "anthropic/claude-opus-4-20250514");
//!
//! // Add messages
//! session.add_user_message("Hello!");
//! session.add_assistant_message("Hi there!", TokenUsage::default());
//!
//! // Save automatically
//! session.save()?;
//!
//! // Later, resume the session
//! let session = CortexSession::load(&session_id)?;
//! ```

pub mod export;
pub mod manager;
pub mod storage;
pub mod types;

pub use export::{ExportFormat, default_export_filename, export_session, export_to_file};
pub use manager::CortexSession;
pub use storage::SessionStorage;
pub use types::{SessionMeta, SessionSummary, StoredMessage, StoredToolCall};
