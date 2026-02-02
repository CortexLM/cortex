//! Error types for the policy engine.

use thiserror::Error;

/// Errors that can occur during policy evaluation.
#[derive(Debug, Error)]
pub enum PolicyError {
    /// Invalid command format
    #[error("invalid command format: {0}")]
    InvalidCommand(String),

    /// Configuration error
    #[error("configuration error: {0}")]
    ConfigurationError(String),

    /// Rule parsing error
    #[error("rule parsing error: {0}")]
    RuleParsingError(String),
}
