//! Command execution with sandboxing.

mod output;
mod runner;

pub use output::OutputCapture;
pub use runner::{
    ExecOptions, ExecOutput, OutputChunk, execute_command, execute_command_streaming,
};

use std::time::Duration;

/// Default command timeout.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);

/// Maximum output size to capture.
pub const MAX_OUTPUT_SIZE: usize = 1024 * 1024; // 1MB
