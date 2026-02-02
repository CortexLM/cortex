//! Background terminal management.
//!
//! This module provides functionality for creating and managing background terminals
//! that can run long-running processes and be monitored by agents.

mod manager;
mod process;

pub use manager::{TerminalInfo, TerminalManager, TerminalStatus};
pub use process::BackgroundTerminal;
