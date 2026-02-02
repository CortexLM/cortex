//! Cortex Exec - Headless execution mode.
//!
//! This module allows running Cortex in non-interactive mode for:
//! - Scripting and automation
//! - CI/CD integration
//! - Batch prompt execution
//!
//! # Sandbox Modes
//!
//! - `sandbox: true` - Execute with restrictions (default)
//! - `sandbox: false` - Execute without restrictions (dangerous)
//!
//! # Example
//!
//! ```rust,ignore
//! use cortex_exec::{ExecRunner, ExecOptions};
//!
//! let options = ExecOptions {
//!     prompt: "Create a hello world program".to_string(),
//!     full_auto: true,
//!     ..Default::default()
//! };
//!
//! let mut runner = ExecRunner::new(config, options);
//! let result = runner.run().await?;
//! println!("Success: {}", result.success);
//! ```

mod output;
mod runner;

#[cfg(test)]
mod tests;

pub use output::{OutputFormat, OutputWriter};
pub use runner::{ExecOptions, ExecResult, ExecRunner, ToolCallRecord};
