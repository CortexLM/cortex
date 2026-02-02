#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::missing_errors_doc,
    clippy::uninlined_format_args,
    clippy::doc_markdown,
    clippy::field_reassign_with_default
)]
//! Cortex Execpolicy - Production-grade policy engine for command execution.
//!
//! This module determines if a command can be executed:
//! - `Allow` - Execution authorized without confirmation
//! - `Deny` - Execution prohibited
//! - `Ask` - Requires user confirmation
//!
//! # Features
//! - Comprehensive dangerous command detection
//! - Proper shell argument parsing (not string matching)
//! - Context-aware detection (container vs host)
//! - Configurable policy rules
//! - Fork bomb detection
//! - Privilege escalation detection
//!
//! # Default Policy
//!
//! ```text
//! ┌────────────────────────────────────────────┐
//! │              Command                        │
//! └────────────────────┬───────────────────────┘
//!                      │
//!                      ▼
//! ┌────────────────────────────────────────────┐
//! │     Is this a dangerous command?           │
//! │  (rm -rf /, sudo, dd, etc.)                │
//! └────────────────────┬───────────────────────┘
//!                      │
//!        ┌─────────────┴─────────────┐
//!        ▼                           ▼
//!      [Yes]                        [No]
//!        │                           │
//!        ▼                           ▼
//!   ┌─────────┐              ┌─────────────────┐
//!   │  DENY   │              │ Needs network   │
//!   └─────────┘              │ or write access?│
//!                            └────────┬────────┘
//!                                     │
//!                       ┌─────────────┴─────────────┐
//!                       ▼                           ▼
//!                     [Yes]                        [No]
//!                       │                           │
//!                       ▼                           ▼
//!                  ┌─────────┐                ┌─────────┐
//!                  │   ASK   │                │  ALLOW  │
//!                  └─────────┘                └─────────┘
//! ```

#[cfg(test)]
mod tests;

mod command;
mod config;
mod context;
mod danger;
mod decision;
mod detection;
mod error;
mod policy;

// Re-export all public types
pub use command::ParsedCommand;
pub use config::PolicyConfig;
pub use context::ExecutionContext;
pub use danger::{DangerCategory, DangerDetection};
pub use decision::Decision;
pub use error::PolicyError;
pub use policy::ExecPolicy;

// ============================================================================
// Convenience Functions
// ============================================================================

/// Quick evaluation with default policy.
pub fn evaluate(command: &[String]) -> Decision {
    ExecPolicy::new().evaluate(command)
}

/// Quick evaluation with container context.
pub fn evaluate_in_container(command: &[String]) -> Decision {
    ExecPolicy::with_context(ExecutionContext::container()).evaluate(command)
}

/// Quick evaluation with sandbox context.
pub fn evaluate_in_sandbox(command: &[String]) -> Decision {
    ExecPolicy::with_context(ExecutionContext::sandboxed()).evaluate(command)
}

/// Parse a shell command string and evaluate it.
pub fn evaluate_shell_command(cmd: &str) -> Decision {
    match ParsedCommand::from_shell_string(cmd) {
        Ok(parsed) => ExecPolicy::new().evaluate_parsed(&parsed),
        Err(_) => Decision::Deny,
    }
}

/// Parse a shell command string and evaluate with details.
pub fn evaluate_shell_command_with_details(cmd: &str) -> (Decision, DangerDetection) {
    match ParsedCommand::from_shell_string(cmd) {
        Ok(parsed) => {
            let policy = ExecPolicy::new();
            let danger = policy.detect_danger(&parsed);
            let decision = if danger.is_dangerous {
                Decision::Deny
            } else if policy.needs_confirmation(&parsed) {
                Decision::Ask
            } else {
                Decision::Allow
            };
            (decision, danger)
        }
        Err(e) => (
            Decision::Deny,
            DangerDetection::dangerous(
                DangerCategory::DestructiveFileOp,
                format!("failed to parse command: {e}"),
                10,
                false,
            ),
        ),
    }
}
