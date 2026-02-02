//! Cortex Exec - Complete headless/non-interactive execution mode.
//!
//! This module implements a comprehensive exec mode similar to Droid's headless execution,
//! supporting:
//! - Autonomy levels (read-only, low, medium, high, skip-permissions-unsafe)
//! - Output formats (text, json, stream-json, stream-jsonrpc)
//! - Multi-turn conversations via stream-jsonrpc
//! - Session continuation
//! - Tool controls (enable/disable specific tools)
//! - Fail-fast behavior on permission violations
//! - Streaming input/output

mod autonomy;
mod cli;
mod helpers;
mod jsonrpc;
mod output;
mod runner;

// Re-export the main types
pub use autonomy::{AutonomyLevel, is_read_only_command};
pub use cli::ExecCli;
pub use helpers::{ensure_utf8_locale, validate_path_environment};
pub use output::{ExecInputFormat, ExecOutputFormat};
