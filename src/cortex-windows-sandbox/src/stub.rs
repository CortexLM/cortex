//! Stub implementation for non-Windows platforms.
//!
//! These types are provided so that code can reference Windows sandbox types
//! on non-Windows platforms without conditional compilation at every use site.

use crate::{Result, WindowsSandboxError};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Sandbox configuration (stub).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SandboxConfig {
    /// Whether sandbox is enabled.
    pub enabled: bool,
    /// Allow network access.
    pub allow_network: bool,
    /// Paths that can be written.
    pub writable_paths: Vec<PathBuf>,
}

/// Sandbox policy level (stub).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PolicyLevel {
    /// Minimal restrictions.
    Minimal,
    /// Moderate restrictions.
    #[default]
    Moderate,
    /// Maximum restrictions.
    Maximum,
}

/// Sandbox policy (stub).
#[derive(Debug, Clone, Default)]
pub struct SandboxPolicy {
    /// Paths that can be read.
    pub read_paths: Vec<PathBuf>,
    /// Paths that can be written.
    pub write_paths: Vec<PathBuf>,
    /// Whether network is allowed.
    pub network_allowed: bool,
    /// Policy level.
    pub level: PolicyLevel,
}

impl SandboxPolicy {
    /// Create a new policy.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a minimal policy.
    pub fn minimal() -> Self {
        Self {
            level: PolicyLevel::Minimal,
            ..Self::default()
        }
    }

    /// Create a moderate policy.
    pub fn moderate() -> Self {
        Self {
            level: PolicyLevel::Moderate,
            ..Self::default()
        }
    }

    /// Create a maximum security policy.
    pub fn maximum() -> Self {
        Self {
            level: PolicyLevel::Maximum,
            network_allowed: false,
            ..Self::default()
        }
    }
}

/// Windows sandbox (stub).
///
/// On non-Windows platforms, this type exists but always returns an error
/// when attempting to create or use it.
pub struct WindowsSandbox {
    _private: (),
}

impl WindowsSandbox {
    /// Attempt to create a new sandbox (always fails on non-Windows).
    pub fn new(_config: SandboxConfig) -> Result<Self> {
        Err(WindowsSandboxError::NotAvailable)
    }

    /// Attempt to create from policy (always fails on non-Windows).
    pub fn from_policy(_policy: &SandboxPolicy) -> Result<Self> {
        Err(WindowsSandboxError::NotAvailable)
    }

    /// Create minimal sandbox (always fails on non-Windows).
    pub fn minimal() -> Result<Self> {
        Err(WindowsSandboxError::NotAvailable)
    }

    /// Create moderate sandbox (always fails on non-Windows).
    pub fn moderate() -> Result<Self> {
        Err(WindowsSandboxError::NotAvailable)
    }

    /// Create maximum sandbox (always fails on non-Windows).
    pub fn maximum() -> Result<Self> {
        Err(WindowsSandboxError::NotAvailable)
    }

    /// Apply sandbox (always fails on non-Windows).
    pub fn apply(&mut self) -> Result<()> {
        Err(WindowsSandboxError::NotAvailable)
    }

    /// Run a command in the sandbox (always fails on non-Windows).
    pub async fn run_command(&self, _command: &[String]) -> Result<std::process::Output> {
        Err(WindowsSandboxError::NotAvailable)
    }

    /// Check if Windows sandbox is supported (always false on non-Windows).
    pub fn is_supported() -> bool {
        false
    }

    /// Check if Windows sandbox is available (always false on non-Windows).
    pub fn is_available() -> bool {
        false
    }

    /// Check availability (always false on non-Windows).
    pub fn check_availability() -> bool {
        false
    }
}

/// Job object (stub).
pub struct JobObject {
    _private: (),
}

impl JobObject {
    /// Check availability (always false on non-Windows).
    pub fn is_available() -> bool {
        false
    }
}

/// Restricted token (stub).
pub struct RestrictedToken {
    _private: (),
}

impl RestrictedToken {
    /// Check availability (always false on non-Windows).
    pub fn is_available() -> bool {
        false
    }
}

/// Process mitigations (stub).
pub struct ProcessMitigations {
    _private: (),
}

impl ProcessMitigations {
    /// Check availability (always false on non-Windows).
    pub fn is_available() -> bool {
        false
    }

    /// Apply mitigations (does nothing on non-Windows).
    pub fn apply(&mut self) -> Result<()> {
        Ok(())
    }
}
