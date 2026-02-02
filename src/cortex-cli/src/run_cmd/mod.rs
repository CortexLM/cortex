//! Run command for non-interactive Cortex CLI execution.
//!
//! Provides a comprehensive run command for batch execution:
//! - `cortex run [message..]` - Execute with a message prompt
//! - `--command` - Execute a predefined command
//! - `--continue/-c` - Continue the last session
//! - `--session/-s` - Specify session ID to continue
//! - `--share` - Auto-share the session
//! - `--model/-m` - Model override (provider/model format)
//! - `--agent` - Select specific agent
//! - `--format` - Output format (default/json)
//! - `--file/-f` - Attach file(s) to the message
//! - `--title` - Set session title
//! - `--attach` - Attach to a running server
//! - `--temperature/-t` - Model temperature (0.0-2.0)
//! - `--top-p` - Top-p sampling parameter
//! - `--seed` - Random seed for reproducibility
//! - `--notification/-n` - Desktop notification on completion
//! - `--stream` - Stream output as it arrives
//! - `--copy/-C` - Copy final response to clipboard

mod attachments;
mod cli;
mod execution;
mod mime;
mod output;
mod session;
mod system;

#[cfg(test)]
mod tests;

// Re-export public types
pub use cli::{OutputFormat, RunCli};
pub use system::ModelSpec;
