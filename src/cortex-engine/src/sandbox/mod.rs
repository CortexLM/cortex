//! Sandbox management for command execution.
//!
//! Provides cross-platform sandboxing with:
//! - Linux: Landlock + Seccomp + Mount namespaces
//! - macOS: Seatbelt (sandbox-exec)
//! - Windows: Job Objects + ACLs + Restricted Tokens

#[cfg(target_os = "macos")]
mod seatbelt;

#[cfg(target_os = "linux")]
mod landlock;

#[cfg(target_os = "windows")]
mod windows;

mod manager;
mod policy;
mod runner;

// Re-export main types
pub use manager::{
    CORTEX_SANDBOX_CWD_ENV_VAR, CORTEX_SANDBOX_ENV_VAR, CORTEX_SANDBOX_NETWORK_DISABLED_ENV_VAR,
    CORTEX_SANDBOX_POLICY_ENV_VAR, SandboxManager,
};
pub use policy::{ProtectedPaths, SandboxAction, SandboxPolicyType, WritableRoot};
pub use runner::{SandboxBackend, SandboxRunner, SandboxedCommand, get_platform_sandbox};

// Platform-specific exports
#[cfg(target_os = "windows")]
pub use windows::{apply_windows_sandbox, get_capabilities_info, spawn_sandboxed_process};
