//! Sandbox utilities.
//!
//! Provides utilities for sandboxed execution
//! with resource limits and isolation.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::error::{CortexError, Result};

/// Sandbox configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxConfig {
    /// Enable sandbox.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Allowed commands.
    #[serde(default)]
    pub allowed_commands: HashSet<String>,
    /// Blocked commands.
    #[serde(default)]
    pub blocked_commands: HashSet<String>,
    /// Allowed paths (for read).
    #[serde(default)]
    pub allowed_read_paths: HashSet<PathBuf>,
    /// Allowed paths (for write).
    #[serde(default)]
    pub allowed_write_paths: HashSet<PathBuf>,
    /// Max memory (bytes).
    pub max_memory: Option<u64>,
    /// Max CPU time (seconds).
    pub max_cpu_time: Option<u64>,
    /// Max file size (bytes).
    pub max_file_size: Option<u64>,
    /// Max processes.
    pub max_processes: Option<u32>,
    /// Allow network.
    #[serde(default = "default_true")]
    pub allow_network: bool,
    /// Working directory.
    pub working_dir: Option<PathBuf>,
    /// Environment whitelist.
    #[serde(default)]
    pub env_whitelist: HashSet<String>,
}

fn default_true() -> bool {
    true
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            allowed_commands: HashSet::new(),
            blocked_commands: default_blocked_commands(),
            allowed_read_paths: HashSet::new(),
            allowed_write_paths: HashSet::new(),
            max_memory: Some(512 * 1024 * 1024), // 512 MB
            max_cpu_time: Some(60),
            max_file_size: Some(100 * 1024 * 1024), // 100 MB
            max_processes: Some(10),
            allow_network: true,
            working_dir: None,
            env_whitelist: default_env_whitelist(),
        }
    }
}

/// Default blocked commands.
fn default_blocked_commands() -> HashSet<String> {
    [
        "rm -rf /",
        "rm -rf ~",
        "rm -rf /*",
        ":(){:|:&};:",
        "mkfs",
        "dd if=/dev/zero",
        "chmod -R 777 /",
        "chown -R",
        "> /dev/sda",
        "shutdown",
        "reboot",
        "halt",
        "poweroff",
        "init 0",
        "init 6",
    ]
    .iter()
    .map(std::string::ToString::to_string)
    .collect()
}

/// Default environment whitelist.
fn default_env_whitelist() -> HashSet<String> {
    [
        "PATH",
        "HOME",
        "USER",
        "SHELL",
        "TERM",
        "LANG",
        "LC_ALL",
        "TZ",
        "PWD",
        "TMPDIR",
        "XDG_RUNTIME_DIR",
    ]
    .iter()
    .map(std::string::ToString::to_string)
    .collect()
}

/// Sandbox policy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxPolicy {
    /// Policy name.
    pub name: String,
    /// Description.
    pub description: String,
    /// Configuration.
    pub config: SandboxConfig,
}

impl SandboxPolicy {
    /// Create a restrictive policy.
    pub fn restrictive() -> Self {
        Self {
            name: "restrictive".to_string(),
            description: "Highly restrictive sandbox".to_string(),
            config: SandboxConfig {
                enabled: true,
                allowed_commands: HashSet::new(),
                blocked_commands: default_blocked_commands(),
                allowed_read_paths: HashSet::new(),
                allowed_write_paths: HashSet::new(),
                max_memory: Some(128 * 1024 * 1024),
                max_cpu_time: Some(30),
                max_file_size: Some(10 * 1024 * 1024),
                max_processes: Some(5),
                allow_network: false,
                working_dir: None,
                env_whitelist: HashSet::new(),
            },
        }
    }

    /// Create a permissive policy.
    pub fn permissive() -> Self {
        Self {
            name: "permissive".to_string(),
            description: "Permissive sandbox (still with basic protections)".to_string(),
            config: SandboxConfig {
                enabled: true,
                allowed_commands: HashSet::new(),
                blocked_commands: default_blocked_commands(),
                allowed_read_paths: HashSet::new(),
                allowed_write_paths: HashSet::new(),
                max_memory: Some(2 * 1024 * 1024 * 1024),
                max_cpu_time: Some(300),
                max_file_size: Some(1024 * 1024 * 1024),
                max_processes: Some(50),
                allow_network: true,
                working_dir: None,
                env_whitelist: default_env_whitelist(),
            },
        }
    }

    /// Create a disabled policy.
    pub fn disabled() -> Self {
        Self {
            name: "disabled".to_string(),
            description: "Sandbox disabled".to_string(),
            config: SandboxConfig {
                enabled: false,
                ..Default::default()
            },
        }
    }
}

/// Sandbox validator.
pub struct SandboxValidator {
    config: SandboxConfig,
}

impl SandboxValidator {
    /// Create a new validator.
    pub fn new(config: SandboxConfig) -> Self {
        Self { config }
    }

    /// Validate a command.
    pub fn validate_command(&self, command: &str) -> Result<()> {
        if !self.config.enabled {
            return Ok(());
        }

        // Check blocked commands
        for blocked in &self.config.blocked_commands {
            if command.contains(blocked) {
                return Err(CortexError::SandboxDenied {
                    command: command.to_string(),
                });
            }
        }

        // Check allowed commands if list is not empty
        if !self.config.allowed_commands.is_empty() {
            let cmd_name = command.split_whitespace().next().unwrap_or("");
            if !self.config.allowed_commands.contains(cmd_name) {
                return Err(CortexError::SandboxDenied {
                    command: command.to_string(),
                });
            }
        }

        Ok(())
    }

    /// Validate a read path.
    pub fn validate_read_path(&self, path: &Path) -> Result<()> {
        if !self.config.enabled {
            return Ok(());
        }

        if self.config.allowed_read_paths.is_empty() {
            return Ok(());
        }

        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());

        for allowed in &self.config.allowed_read_paths {
            if canonical.starts_with(allowed) {
                return Ok(());
            }
        }

        Err(CortexError::PermissionDenied {
            path: path.to_path_buf(),
        })
    }

    /// Validate a write path.
    pub fn validate_write_path(&self, path: &Path) -> Result<()> {
        if !self.config.enabled {
            return Ok(());
        }

        if self.config.allowed_write_paths.is_empty() {
            return Ok(());
        }

        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());

        for allowed in &self.config.allowed_write_paths {
            if canonical.starts_with(allowed) {
                return Ok(());
            }
        }

        Err(CortexError::PermissionDenied {
            path: path.to_path_buf(),
        })
    }

    /// Validate network access.
    pub fn validate_network(&self) -> Result<()> {
        if !self.config.enabled {
            return Ok(());
        }

        if self.config.allow_network {
            Ok(())
        } else {
            Err(CortexError::Sandbox("Network access denied".to_string()))
        }
    }

    /// Get filtered environment.
    pub fn filter_env(&self) -> HashMap<String, String> {
        if !self.config.enabled {
            return std::env::vars().collect();
        }

        std::env::vars()
            .filter(|(key, _)| self.config.env_whitelist.contains(key))
            .collect()
    }
}

/// Sandbox execution context.
#[allow(dead_code)]
pub struct SandboxContext {
    /// Validator.
    validator: SandboxValidator,
    /// Execution count.
    exec_count: RwLock<u32>,
    /// Total CPU time used.
    cpu_time_used: RwLock<Duration>,
    /// Total memory used.
    memory_used: RwLock<u64>,
}

impl SandboxContext {
    /// Create a new context.
    pub fn new(config: SandboxConfig) -> Self {
        Self {
            validator: SandboxValidator::new(config),
            exec_count: RwLock::new(0),
            cpu_time_used: RwLock::new(Duration::ZERO),
            memory_used: RwLock::new(0),
        }
    }

    /// Check if can execute.
    pub async fn can_execute(&self) -> Result<()> {
        let config = &self.validator.config;

        if let Some(max_procs) = config.max_processes {
            let count = *self.exec_count.read().await;
            if count >= max_procs {
                return Err(CortexError::Sandbox(format!(
                    "Max process limit reached: {max_procs}"
                )));
            }
        }

        Ok(())
    }

    /// Record execution start.
    pub async fn start_execution(&self) {
        *self.exec_count.write().await += 1;
    }

    /// Record execution end.
    pub async fn end_execution(&self, duration: Duration) {
        let mut count = self.exec_count.write().await;
        if *count > 0 {
            *count -= 1;
        }

        *self.cpu_time_used.write().await += duration;
    }

    /// Check CPU time limit.
    pub async fn check_cpu_time(&self) -> Result<()> {
        let config = &self.validator.config;

        if let Some(max_time) = config.max_cpu_time {
            let used = self.cpu_time_used.read().await.as_secs();
            if used >= max_time {
                return Err(CortexError::Sandbox(format!(
                    "CPU time limit exceeded: {max_time}s"
                )));
            }
        }

        Ok(())
    }

    /// Validate command.
    pub fn validate_command(&self, command: &str) -> Result<()> {
        self.validator.validate_command(command)
    }

    /// Validate path for reading.
    pub fn validate_read(&self, path: &Path) -> Result<()> {
        self.validator.validate_read_path(path)
    }

    /// Validate path for writing.
    pub fn validate_write(&self, path: &Path) -> Result<()> {
        self.validator.validate_write_path(path)
    }

    /// Get filtered environment.
    pub fn get_env(&self) -> HashMap<String, String> {
        self.validator.filter_env()
    }
}

/// Resource limits.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceLimits {
    /// Max memory (bytes).
    pub memory: Option<u64>,
    /// Max CPU time (seconds).
    pub cpu_time: Option<u64>,
    /// Max file size (bytes).
    pub file_size: Option<u64>,
    /// Max open files.
    pub open_files: Option<u64>,
    /// Max processes.
    pub processes: Option<u32>,
    /// Max stack size (bytes).
    pub stack_size: Option<u64>,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            memory: Some(512 * 1024 * 1024),
            cpu_time: Some(60),
            file_size: Some(100 * 1024 * 1024),
            open_files: Some(256),
            processes: Some(10),
            stack_size: Some(8 * 1024 * 1024),
        }
    }
}

impl ResourceLimits {
    /// Create unlimited.
    pub fn unlimited() -> Self {
        Self {
            memory: None,
            cpu_time: None,
            file_size: None,
            open_files: None,
            processes: None,
            stack_size: None,
        }
    }

    /// Create minimal.
    pub fn minimal() -> Self {
        Self {
            memory: Some(64 * 1024 * 1024),
            cpu_time: Some(10),
            file_size: Some(1024 * 1024),
            open_files: Some(16),
            processes: Some(2),
            stack_size: Some(1024 * 1024),
        }
    }
}

/// Command risk assessment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommandRisk {
    /// Safe command.
    Safe,
    /// Low risk.
    Low,
    /// Medium risk.
    Medium,
    /// High risk.
    High,
    /// Critical risk.
    Critical,
}

impl CommandRisk {
    /// Assess command risk.
    pub fn assess(command: &str) -> Self {
        let cmd_lower = command.to_lowercase();

        // Critical risk patterns (must match exactly or with trailing whitespace)
        let critical_patterns = [
            "rm -rf /",
            "rm -rf ~",
            ":(){:|:&};:",
            "mkfs",
            "dd if=/dev",
            "format c:",
            "> /dev/sda",
        ];

        for pattern in &critical_patterns {
            // Only match critical if the command is exactly the pattern or starts with the pattern followed by whitespace or end
            if cmd_lower.trim_start().starts_with(pattern) {
                let after = &cmd_lower[pattern.len()..];
                if after.is_empty() || after.starts_with(' ') {
                    return Self::Critical;
                }
            }
        }

        // High risk patterns
        let high_risk_patterns = [
            "rm -rf",
            "chmod 777",
            "chown",
            "sudo",
            "su ",
            "curl | bash",
            "wget | bash",
            "eval",
            "exec",
        ];

        for pattern in &high_risk_patterns {
            if cmd_lower.contains(pattern) {
                return Self::High;
            }
        }

        // Medium risk patterns
        let medium_risk_patterns = [
            "rm ",
            "mv ",
            "chmod",
            "pip install",
            "npm install",
            "apt install",
            "brew install",
        ];

        for pattern in &medium_risk_patterns {
            if cmd_lower.contains(pattern) {
                return Self::Medium;
            }
        }

        // Low risk patterns
        let low_risk_patterns = ["cp ", "mkdir", "touch", "echo", "cat", "grep", "find"];

        for pattern in &low_risk_patterns {
            if cmd_lower.contains(pattern) {
                return Self::Low;
            }
        }

        // Default to safe for read-only commands
        let safe_patterns = ["ls", "pwd", "whoami", "date", "head", "tail", "less"];

        for pattern in &safe_patterns {
            if cmd_lower.starts_with(pattern) {
                return Self::Safe;
            }
        }

        Self::Low
    }

    /// Get description.
    pub fn description(&self) -> &'static str {
        match self {
            Self::Safe => "Safe read-only command",
            Self::Low => "Low risk command with minor side effects",
            Self::Medium => "Medium risk command with file modifications",
            Self::High => "High risk command with potential for damage",
            Self::Critical => "Critical risk command - potentially destructive",
        }
    }

    /// Should require confirmation.
    pub fn requires_confirmation(&self) -> bool {
        matches!(self, Self::High | Self::Critical)
    }

    /// Should be blocked by default.
    pub fn should_block(&self) -> bool {
        matches!(self, Self::Critical)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sandbox_config_default() {
        let config = SandboxConfig::default();
        assert!(config.enabled);
        assert!(!config.blocked_commands.is_empty());
    }

    #[test]
    fn test_sandbox_policy() {
        let restrictive = SandboxPolicy::restrictive();
        assert!(restrictive.config.enabled);
        assert!(!restrictive.config.allow_network);

        let permissive = SandboxPolicy::permissive();
        assert!(permissive.config.allow_network);
    }

    #[test]
    fn test_validate_command() {
        let config = SandboxConfig::default();
        let validator = SandboxValidator::new(config);

        // Safe command
        assert!(validator.validate_command("ls -la").is_ok());

        // Blocked command
        assert!(validator.validate_command("rm -rf /").is_err());
    }

    #[test]
    fn test_command_risk() {
        assert_eq!(CommandRisk::assess("ls"), CommandRisk::Safe);
        assert_eq!(CommandRisk::assess("cat file.txt"), CommandRisk::Low);
        assert_eq!(CommandRisk::assess("rm file.txt"), CommandRisk::Medium);
        assert_eq!(CommandRisk::assess("sudo rm -rf /tmp"), CommandRisk::High);
        assert_eq!(CommandRisk::assess("rm -rf /"), CommandRisk::Critical);
    }

    #[test]
    fn test_risk_confirmation() {
        assert!(!CommandRisk::Safe.requires_confirmation());
        assert!(!CommandRisk::Low.requires_confirmation());
        assert!(!CommandRisk::Medium.requires_confirmation());
        assert!(CommandRisk::High.requires_confirmation());
        assert!(CommandRisk::Critical.requires_confirmation());
    }

    #[test]
    fn test_resource_limits() {
        let default = ResourceLimits::default();
        assert!(default.memory.is_some());

        let unlimited = ResourceLimits::unlimited();
        assert!(unlimited.memory.is_none());

        let minimal = ResourceLimits::minimal();
        assert!(minimal.memory.unwrap() < default.memory.unwrap());
    }

    #[tokio::test]
    async fn test_sandbox_context() {
        let config = SandboxConfig::default();
        let context = SandboxContext::new(config);

        assert!(context.validate_command("ls").is_ok());
        assert!(context.validate_command("rm -rf /").is_err());
    }
}
