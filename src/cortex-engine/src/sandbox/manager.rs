//! Sandbox Manager - Orchestrates sandbox application across platforms.
//!
//! Provides a unified interface for sandboxing commands on Linux, macOS, and Windows.

use std::path::PathBuf;

use tracing::{debug, info, warn};

use super::policy::{ProtectedPaths, SandboxAction, SandboxPolicyType, WritableRoot};
use super::runner::{SandboxBackend, SandboxedCommand};
use crate::error::Result;

/// Environment variable indicating sandbox type.
pub const CORTEX_SANDBOX_ENV_VAR: &str = "CORTEX_SANDBOX";

/// Environment variable indicating network is disabled.
pub const CORTEX_SANDBOX_NETWORK_DISABLED_ENV_VAR: &str = "CORTEX_SANDBOX_NETWORK_DISABLED";

/// Environment variable containing the serialized sandbox policy.
pub const CORTEX_SANDBOX_POLICY_ENV_VAR: &str = "CORTEX_SANDBOX_POLICY";

/// Environment variable containing the sandbox policy cwd.
pub const CORTEX_SANDBOX_CWD_ENV_VAR: &str = "CORTEX_SANDBOX_CWD";

/// Sandbox manager that orchestrates sandbox application.
pub struct SandboxManager {
    /// The sandbox policy.
    policy: SandboxPolicyType,
    /// The current working directory for the sandbox.
    cwd: PathBuf,
    /// The platform-specific backend.
    backend: Option<Box<dyn SandboxBackend>>,
    /// Computed writable roots.
    writable_roots: Vec<WritableRoot>,
    /// Protected paths.
    protected_paths: ProtectedPaths,
}

impl SandboxManager {
    /// Create a new sandbox manager.
    pub fn new(policy: SandboxPolicyType, cwd: PathBuf) -> Self {
        let writable_roots = policy.get_writable_roots_with_cwd(&cwd);
        let protected_paths = ProtectedPaths::for_workspace(&cwd);
        let backend = super::runner::get_platform_sandbox();

        Self {
            policy,
            cwd,
            backend,
            writable_roots,
            protected_paths,
        }
    }

    /// Check if sandboxing is available on this platform.
    pub fn is_available(&self) -> bool {
        self.backend
            .as_ref()
            .map(|b| b.is_available())
            .unwrap_or(false)
    }

    /// Get the backend name.
    pub fn backend_name(&self) -> Option<&str> {
        self.backend.as_ref().map(|b| b.name())
    }

    /// Get the sandbox policy.
    pub fn policy(&self) -> &SandboxPolicyType {
        &self.policy
    }

    /// Get the writable roots.
    pub fn writable_roots(&self) -> &[WritableRoot] {
        &self.writable_roots
    }

    /// Get the protected paths.
    pub fn protected_paths(&self) -> &ProtectedPaths {
        &self.protected_paths
    }

    /// Check if an action is allowed by the policy.
    pub fn is_action_allowed(&self, action: &SandboxAction) -> bool {
        match action {
            SandboxAction::ReadFile(path) => {
                // Check denied paths
                for denied in &self.protected_paths.denied {
                    if path.starts_with(denied) {
                        return false;
                    }
                }
                self.policy.has_full_disk_read_access()
            }
            SandboxAction::WriteFile(path) => {
                if self.policy.has_full_disk_write_access() {
                    return true;
                }
                // Check if path is in a writable root and not in read-only subpath
                for root in &self.writable_roots {
                    if root.is_path_writable(path) {
                        return true;
                    }
                }
                false
            }
            SandboxAction::Execute(_) => {
                // Always allow execution (the sandbox restricts what the executed process can do)
                true
            }
            SandboxAction::NetworkConnect(_) => self.policy.has_full_network_access(),
        }
    }

    /// Prepare a command for sandboxed execution.
    ///
    /// Returns a `SandboxedCommand` that wraps the original command with
    /// platform-specific sandboxing.
    pub fn prepare_command(&self, command: &[String]) -> Result<SandboxedCommand> {
        if command.is_empty() {
            return Ok(SandboxedCommand::passthrough(&[]));
        }

        // If full access or no backend available, just pass through
        if self.policy.has_full_disk_write_access() {
            debug!("Sandbox: DangerFullAccess policy, passing through command");
            return Ok(SandboxedCommand::passthrough(command));
        }

        if let Some(backend) = &self.backend {
            if backend.is_available() {
                info!("Sandbox: Using {} backend", backend.name());
                return backend.prepare_command(
                    command,
                    &self.policy,
                    &self.cwd,
                    &self.writable_roots,
                );
            }
        }

        // No sandbox available - pass through with warning
        warn!("Sandbox: No sandbox backend available, executing without isolation");
        Ok(SandboxedCommand::passthrough(command))
    }

    /// Get environment variables to set for sandboxed execution.
    pub fn get_sandbox_env(&self) -> Vec<(String, String)> {
        let mut env = Vec::new();

        if let Some(backend) = &self.backend {
            env.push((
                CORTEX_SANDBOX_ENV_VAR.to_string(),
                backend.name().to_string(),
            ));
        }

        if !self.policy.has_full_network_access() {
            env.push((
                CORTEX_SANDBOX_NETWORK_DISABLED_ENV_VAR.to_string(),
                "1".to_string(),
            ));
        }

        // Serialize policy for the wrapper
        if let Ok(policy_json) = serde_json::to_string(&self.policy) {
            env.push((CORTEX_SANDBOX_POLICY_ENV_VAR.to_string(), policy_json));
        }

        env.push((
            CORTEX_SANDBOX_CWD_ENV_VAR.to_string(),
            self.cwd.display().to_string(),
        ));

        env
    }

    /// Create a read-only sandbox manager.
    pub fn read_only(cwd: PathBuf) -> Self {
        Self::new(SandboxPolicyType::ReadOnly, cwd)
    }

    /// Create a workspace-write sandbox manager.
    pub fn workspace_write(cwd: PathBuf) -> Self {
        Self::new(SandboxPolicyType::workspace_write(), cwd)
    }

    /// Create a workspace-write sandbox manager without network access.
    pub fn workspace_write_no_network(cwd: PathBuf) -> Self {
        Self::new(SandboxPolicyType::workspace_write_no_network(), cwd)
    }
}

impl Default for SandboxManager {
    fn default() -> Self {
        Self::new(
            SandboxPolicyType::default(),
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sandbox_manager_creation() {
        let manager = SandboxManager::workspace_write(PathBuf::from("/workspace"));

        assert!(!manager.writable_roots().is_empty());
        assert!(!manager.policy().has_full_disk_write_access());
        assert!(manager.policy().has_full_network_access());
    }

    #[test]
    fn test_action_allowed() {
        let manager = SandboxManager::workspace_write(PathBuf::from("/workspace"));

        // Write to workspace should be allowed
        assert!(
            manager.is_action_allowed(&SandboxAction::WriteFile(PathBuf::from(
                "/workspace/src/main.rs"
            )))
        );

        // Write to .git should be denied
        assert!(
            !manager.is_action_allowed(&SandboxAction::WriteFile(PathBuf::from(
                "/workspace/.git/config"
            )))
        );

        // Network should be allowed
        assert!(manager.is_action_allowed(&SandboxAction::NetworkConnect(
            "example.com:443".to_string()
        )));
    }

    #[test]
    fn test_no_network_policy() {
        let manager = SandboxManager::workspace_write_no_network(PathBuf::from("/workspace"));

        assert!(!manager.is_action_allowed(&SandboxAction::NetworkConnect(
            "example.com:443".to_string()
        )));
    }
}
