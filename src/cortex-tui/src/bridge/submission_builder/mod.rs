//! Builder for cortex_protocol Submission messages.
//!
//! This module provides a clean, ergonomic API for constructing the various
//! submission types that can be sent to cortex-core.
//!
//! # Example
//!
//! ```rust,ignore
//! use cortex_tui::bridge::SubmissionBuilder;
//!
//! // Simple text message
//! let submission = SubmissionBuilder::user_message("Hello, world!")
//!     .build()
//!     .expect("valid submission");
//!
//! // Interrupt the current task
//! let interrupt = SubmissionBuilder::interrupt()
//!     .build()
//!     .expect("valid submission");
//!
//! // Approve a tool execution
//! let approval = SubmissionBuilder::approve("call-123")
//!     .build()
//!     .expect("valid submission");
//! ```

mod approval;
mod control;
mod history;
mod mcp;
mod sender;
mod session;
mod user_input;

#[cfg(test)]
mod tests;

use cortex_protocol::{Op, Submission};
use uuid::Uuid;

// Re-export all public items for backwards compatibility
pub use sender::SubmissionSender;

// ============================================================================
// SubmissionBuilder
// ============================================================================

/// Builder for creating cortex_protocol Submission messages.
///
/// The builder provides a fluent API for constructing submissions with
/// automatically generated IDs and type-safe operation variants.
///
/// # Example
///
/// ```rust,ignore
/// let submission = SubmissionBuilder::user_message("Hello, world!")
///     .build();
/// ```
#[derive(Debug, Clone)]
pub struct SubmissionBuilder {
    id: String,
    op: Option<Op>,
}

impl SubmissionBuilder {
    /// Create a new builder with a generated UUID.
    ///
    /// The UUID is generated using `Uuid::new_v4()` for uniqueness.
    pub fn new() -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            op: None,
        }
    }

    /// Create a new builder with a specific ID.
    ///
    /// Use this when you need to correlate submissions with responses
    /// or maintain specific ID sequences.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let builder = SubmissionBuilder::with_id("custom-id-123");
    /// ```
    pub fn with_id(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            op: None,
        }
    }

    /// Get the submission ID.
    ///
    /// Useful for tracking submissions before building.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Build the Submission.
    ///
    /// Returns `None` if no operation was set on the builder.
    /// This can happen if only `new()` or `with_id()` was called
    /// without setting an operation.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let submission = SubmissionBuilder::user_message("Hello")
    ///     .build()
    ///     .expect("operation was set");
    /// ```
    pub fn build(self) -> Option<Submission> {
        self.op.map(|op| Submission { id: self.id, op })
    }

    /// Build the Submission, panicking if no operation was set.
    ///
    /// # Panics
    ///
    /// Panics if no operation was set on the builder.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let submission = SubmissionBuilder::interrupt().build_expect();
    /// ```
    pub fn build_expect(self) -> Submission {
        self.build().expect("SubmissionBuilder: no operation set")
    }
}

impl Default for SubmissionBuilder {
    fn default() -> Self {
        Self::new()
    }
}
