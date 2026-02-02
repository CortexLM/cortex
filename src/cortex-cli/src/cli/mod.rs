//! CLI argument parsing and command dispatch.
//!
//! This module provides the core CLI infrastructure:
//! - Command-line argument definitions using clap
//! - Version and help text formatting
//! - Subcommand dispatch
//!
//! # Module Structure
//!
//! - `args` - Command-line argument structures
//! - `styles` - ANSI styling for help output
//! - `handlers` - Command execution handlers

pub mod args;
pub mod handlers;
pub mod styles;

// Re-export main types
pub use args::{Cli, ColorMode, Commands, InteractiveArgs, LogLevel};
pub use handlers::dispatch_command;
pub use styles::{AFTER_HELP, BEFORE_HELP, get_styles};
