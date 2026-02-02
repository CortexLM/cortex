//! Sandbox runner.

use std::path::PathBuf;

use super::policy::{SandboxPolicyType, WritableRoot};
use crate::error::Result;

/// Trait for sandbox backends.
pub trait SandboxBackend: Send + Sync {
    /// Get the sandbox name.
    fn name(&self) -> &str;

    /// Check if the sandbox is available on this platform.
    fn is_available(&self) -> bool;

    /// Prepare a command for sandboxed execution.
    fn prepare_command(
        &self,
        command: &[String],
        policy: &SandboxPolicyType,
        cwd: &PathBuf,
        writable_roots: &[WritableRoot],
    ) -> Result<SandboxedCommand>;
}

/// A command prepared for sandboxed execution.
#[derive(Debug, Clone)]
pub struct SandboxedCommand {
    /// The program to execute.
    pub program: String,
    /// Arguments to the program.
    pub args: Vec<String>,
    /// Additional environment variables.
    pub env: Vec<(String, String)>,
}

impl SandboxedCommand {
    /// Create a new sandboxed command (passthrough, no sandbox).
    pub fn passthrough(command: &[String]) -> Self {
        if command.is_empty() {
            return Self {
                program: String::new(),
                args: vec![],
                env: vec![],
            };
        }

        Self {
            program: command[0].clone(),
            args: command[1..].to_vec(),
            env: vec![],
        }
    }
}

/// Sandbox runner (legacy, use SandboxManager instead).
pub struct SandboxRunner {
    backend: Option<Box<dyn SandboxBackend>>,
}

impl SandboxRunner {
    /// Create a new sandbox runner.
    pub fn new() -> Self {
        Self {
            backend: get_platform_sandbox(),
        }
    }

    /// Check if sandboxing is available.
    pub fn is_available(&self) -> bool {
        self.backend
            .as_ref()
            .map(|b| b.is_available())
            .unwrap_or(false)
    }

    /// Get the sandbox backend name.
    pub fn backend_name(&self) -> Option<&str> {
        self.backend.as_ref().map(|b| b.name())
    }

    /// Prepare a command for sandboxed execution.
    pub fn prepare(
        &self,
        command: &[String],
        policy: &SandboxPolicyType,
        cwd: &PathBuf,
    ) -> Result<SandboxedCommand> {
        // If no sandbox or full access, just pass through
        if policy.has_full_disk_write_access() {
            return Ok(SandboxedCommand::passthrough(command));
        }

        let writable_roots = policy.get_writable_roots_with_cwd(cwd);

        if let Some(backend) = &self.backend {
            backend.prepare_command(command, policy, cwd, &writable_roots)
        } else {
            // No sandbox available, pass through
            Ok(SandboxedCommand::passthrough(command))
        }
    }
}

impl Default for SandboxRunner {
    fn default() -> Self {
        Self::new()
    }
}

/// Get the platform-specific sandbox backend.
pub fn get_platform_sandbox() -> Option<Box<dyn SandboxBackend>> {
    #[cfg(target_os = "macos")]
    {
        let seatbelt = super::seatbelt::SeatbeltBackend::new();
        if seatbelt.is_available() {
            return Some(Box::new(seatbelt));
        }
    }

    #[cfg(target_os = "linux")]
    {
        let landlock = super::landlock::LandlockBackend::new();
        if landlock.is_available() {
            return Some(Box::new(landlock));
        }
    }

    #[cfg(target_os = "windows")]
    {
        let windows = super::windows::WindowsBackend::new();
        if windows.is_available() {
            return Some(Box::new(windows));
        }
    }

    None
}
