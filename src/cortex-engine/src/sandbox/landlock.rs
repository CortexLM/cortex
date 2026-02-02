//! Linux Landlock sandbox backend.
//!
//! Uses the cortex-linux-sandbox wrapper binary to apply:
//! - Landlock filesystem restrictions
//! - Seccomp network filtering
//! - Mount namespace for read-only .git/.cortex protection

use std::path::PathBuf;

use super::manager::{
    CORTEX_SANDBOX_CWD_ENV_VAR, CORTEX_SANDBOX_ENV_VAR, CORTEX_SANDBOX_NETWORK_DISABLED_ENV_VAR,
    CORTEX_SANDBOX_POLICY_ENV_VAR,
};
use super::policy::{SandboxPolicyType, WritableRoot};
use super::runner::{SandboxBackend, SandboxedCommand};
use crate::error::Result;

/// Name of the Linux sandbox wrapper binary.
const LINUX_SANDBOX_BINARY: &str = "cortex-linux-sandbox";

/// Landlock sandbox backend.
pub struct LandlockBackend {
    available: bool,
    /// Path to the sandbox wrapper binary (if found).
    wrapper_path: Option<PathBuf>,
}

impl LandlockBackend {
    /// Create a new Landlock backend.
    pub fn new() -> Self {
        let available = Self::check_availability();
        let wrapper_path = Self::find_wrapper_binary();

        Self {
            available,
            wrapper_path,
        }
    }

    fn check_availability() -> bool {
        // Check if Landlock is available by checking kernel version
        // Landlock requires Linux 5.13+
        #[cfg(target_os = "linux")]
        {
            use std::fs;
            if let Ok(version) = fs::read_to_string("/proc/version") {
                // Parse kernel version
                if let Some(ver_str) = version.split_whitespace().nth(2) {
                    let parts: Vec<&str> = ver_str.split('.').collect();
                    if parts.len() >= 2 {
                        if let (Ok(major), Ok(minor)) =
                            (parts[0].parse::<u32>(), parts[1].parse::<u32>())
                        {
                            return major > 5 || (major == 5 && minor >= 13);
                        }
                    }
                }
            }
            false
        }

        #[cfg(not(target_os = "linux"))]
        {
            false
        }
    }

    /// Find the sandbox wrapper binary.
    fn find_wrapper_binary() -> Option<PathBuf> {
        // Check next to the current executable
        if let Ok(exe_path) = std::env::current_exe() {
            if let Some(exe_dir) = exe_path.parent() {
                let wrapper = exe_dir.join(LINUX_SANDBOX_BINARY);
                if wrapper.exists() {
                    return Some(wrapper);
                }
            }
        }

        // Check in PATH
        if let Ok(path) = which::which(LINUX_SANDBOX_BINARY) {
            return Some(path);
        }

        None
    }

    /// Check if the wrapper binary is available.
    #[allow(dead_code)]
    pub fn has_wrapper(&self) -> bool {
        self.wrapper_path.is_some()
    }
}

impl Default for LandlockBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl SandboxBackend for LandlockBackend {
    fn name(&self) -> &str {
        "landlock"
    }

    fn is_available(&self) -> bool {
        self.available
    }

    fn prepare_command(
        &self,
        command: &[String],
        policy: &SandboxPolicyType,
        cwd: &PathBuf,
        writable_roots: &[WritableRoot],
    ) -> Result<SandboxedCommand> {
        if command.is_empty() {
            return Ok(SandboxedCommand::passthrough(command));
        }

        // Build environment variables
        let mut env = vec![(CORTEX_SANDBOX_ENV_VAR.to_string(), "landlock".to_string())];

        if !policy.has_full_network_access() {
            env.push((
                CORTEX_SANDBOX_NETWORK_DISABLED_ENV_VAR.to_string(),
                "1".to_string(),
            ));
        }

        // Serialize policy to JSON for the wrapper
        let policy_json = serde_json::to_string(policy).unwrap_or_default();
        env.push((
            CORTEX_SANDBOX_POLICY_ENV_VAR.to_string(),
            policy_json.clone(),
        ));
        env.push((
            CORTEX_SANDBOX_CWD_ENV_VAR.to_string(),
            cwd.display().to_string(),
        ));

        // If we have the wrapper binary, use it
        if let Some(wrapper) = &self.wrapper_path {
            let mut args = vec![
                "--sandbox-policy-cwd".to_string(),
                cwd.display().to_string(),
                "--sandbox-policy".to_string(),
                policy_json,
            ];

            // Add writable roots
            for root in writable_roots {
                args.push("--writable-root".to_string());
                args.push(root.root.display().to_string());

                // Add read-only subpaths
                for ro in &root.read_only_subpaths {
                    args.push("--read-only-subpath".to_string());
                    args.push(ro.display().to_string());
                }
            }

            // Add the actual command
            args.push("--".to_string());
            args.extend(command.iter().cloned());

            return Ok(SandboxedCommand {
                program: wrapper.display().to_string(),
                args,
                env,
            });
        }

        // No wrapper available - pass through with environment variables
        // The environment variables can be used by the process if it understands them
        tracing::warn!("Linux sandbox wrapper binary not found, executing without full isolation");

        Ok(SandboxedCommand {
            program: command[0].clone(),
            args: command[1..].to_vec(),
            env,
        })
    }
}
