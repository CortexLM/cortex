//! Windows sandbox support for Cortex CLI.
//!
//! Provides sandboxing on Windows using:
//! - Job Objects for process isolation and resource limits
//! - Restricted tokens for privilege reduction
//! - AppContainer for application sandboxing (Windows 8+)
//! - Process mitigation policies for exploit prevention

#[cfg(windows)]
pub mod job;
#[cfg(windows)]
pub mod mitigation;
#[cfg(windows)]
pub mod policy;
#[cfg(windows)]
pub mod sandbox;
#[cfg(windows)]
pub mod token;

#[cfg(not(windows))]
pub mod stub;

#[cfg(windows)]
pub use job::JobObject;
#[cfg(windows)]
pub use mitigation::ProcessMitigations;
#[cfg(windows)]
pub use policy::{PolicyLevel, SandboxPolicy};
#[cfg(windows)]
pub use sandbox::{SandboxConfig, WindowsSandbox};
#[cfg(windows)]
pub use token::RestrictedToken;

#[cfg(not(windows))]
pub use stub::*;

use thiserror::Error;

/// Errors that can occur during Windows sandbox operations.
#[derive(Error, Debug)]
pub enum WindowsSandboxError {
    /// Windows sandbox is not available on this platform.
    #[error("Windows sandbox not available on this platform")]
    NotAvailable,

    /// Failed to create sandbox.
    #[error("Failed to create sandbox: {0}")]
    CreateFailed(String),

    /// Failed to create Job Object.
    #[error("Failed to create Job Object: {0}")]
    JobObjectFailed(String),

    /// Failed to create restricted token.
    #[error("Failed to create restricted token: {0}")]
    TokenFailed(String),

    /// Failed to apply process mitigations.
    #[error("Failed to apply process mitigations: {0}")]
    MitigationFailed(String),

    /// Failed to run command in sandbox.
    #[error("Failed to run command in sandbox: {0}")]
    ExecutionFailed(String),

    /// Permission denied.
    #[error("Permission denied: {0}")]
    PermissionDenied(String),

    /// IO error.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// Windows API error.
    #[error("Windows API error: {0}")]
    #[cfg(windows)]
    WindowsApi(#[from] windows::core::Error),
}

/// Result type for Windows sandbox operations.
pub type Result<T> = std::result::Result<T, WindowsSandboxError>;

/// Check if Windows sandbox is available on this system.
///
/// Returns true if running on Windows with the necessary APIs available.
pub fn is_available() -> bool {
    #[cfg(windows)]
    {
        // Check if we can create basic sandbox primitives
        sandbox::WindowsSandbox::check_availability()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Get a description of the Windows sandbox capabilities.
pub fn capabilities_description() -> &'static str {
    #[cfg(windows)]
    {
        "Windows: Job Objects, Restricted Tokens, Process Mitigations"
    }
    #[cfg(not(windows))]
    {
        "Windows sandbox not available (not running on Windows)"
    }
}
