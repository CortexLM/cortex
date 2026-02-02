#![allow(clippy::single_element_loop)]
//! Cortex Sandbox - Cross-platform command sandboxing.
//!
//! This module provides sandbox implementations for different platforms:
//! - **Landlock** (Linux 5.13+) - File system access restriction
//! - **Seatbelt** (macOS) - macOS sandbox with custom policy
//! - **Windows** - Restricted tokens and Job Objects
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────┐
//! │                   SandboxBackend                     │
//! │                     (trait)                          │
//! ├─────────────────┬──────────────────┬────────────────┤
//! │  LandlockSandbox│  SeatbeltSandbox │ WindowsSandbox │
//! │    (Linux)      │     (macOS)      │   (Windows)    │
//! └─────────────────┴──────────────────┴────────────────┘
//! ```
//!
//! # Usage
//!
//! ```rust,ignore
//! use cortex_sandbox::SandboxBackend;
//!
//! #[cfg(target_os = "linux")]
//! {
//!     let sandbox = landlock::LandlockSandbox::new();
//!     if sandbox.is_available() {
//!         // Apply sandbox rules
//!     }
//! }
//! ```

// Core modules available on all platforms
pub mod boundary;
pub mod modes;

#[cfg(target_os = "macos")]
pub mod seatbelt;

#[cfg(target_os = "linux")]
pub mod landlock;

#[cfg(target_os = "windows")]
pub mod windows;

#[cfg(test)]
mod tests;

// Re-exports for convenient access
pub use boundary::{
    BoundaryCheckResult, BoundaryContext, check_path_boundary, contains_traversal_pattern,
    validate_path_in_boundary,
};
pub use modes::{SandboxMode, SandboxModeConfig};

/// Trait for multi-platform sandbox backends.
///
/// Each implementation must provide:
/// - A name identifying the sandbox type
/// - A method to check availability
pub trait SandboxBackend: Send + Sync {
    /// Returns the backend name (e.g., "landlock", "seatbelt", "windows").
    fn name(&self) -> &str;

    /// Checks if the sandbox is available on the current system.
    ///
    /// May depend on:
    /// - Kernel version (Landlock requires Linux 5.13+)
    /// - User permissions
    /// - System configuration
    fn is_available(&self) -> bool;
}

/// Possible errors when applying sandbox.
#[derive(Debug, thiserror::Error)]
pub enum SandboxError {
    #[error("Sandbox not available on this system")]
    NotAvailable,

    #[error("Failed to apply sandbox rules: {0}")]
    ApplyFailed(String),

    #[error("Invalid writable root path: {0}")]
    InvalidPath(String),

    #[error("Permission denied: {0}")]
    PermissionDenied(String),
}

/// Result of a sandbox operation.
pub type SandboxResult<T> = Result<T, SandboxError>;
