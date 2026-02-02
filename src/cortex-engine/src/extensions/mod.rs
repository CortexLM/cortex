//! Extensions module - integrates new feature crates into cortex-core.
//!
//! This module provides access to:
//! - LSP support for diagnostics and code intelligence
//! - Snapshots and revert functionality
//! - Extended agent system with permissions
//! - Enhanced hooks with formatters
//! - Session sharing
//! - Plugin system
//! - Batch operations
//!
//! ## Usage
//!
//! All extension crates are re-exported directly:
//!
//! ```ignore
//! use cortex_engine::extensions::lsp;
//! use cortex_engine::extensions::snapshot;
//! ```

// Re-exports from extension crates
pub use cortex_agents_ext as agents;
pub use cortex_batch as batch;
pub use cortex_hooks_ext as hooks;
pub use cortex_lsp as lsp;
pub use cortex_plugins_ext as plugins;
pub use cortex_share as share;
pub use cortex_snapshot as snapshot;
