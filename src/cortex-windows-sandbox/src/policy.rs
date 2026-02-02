//! Windows sandbox policy configuration.
//!
//! Provides a high-level policy interface for configuring Windows sandbox behavior.

use std::path::PathBuf;

/// Sandbox policy level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PolicyLevel {
    /// Minimal restrictions - only job object tracking.
    Minimal,

    /// Moderate restrictions - privilege reduction and resource limits.
    #[default]
    Moderate,

    /// Maximum restrictions - full isolation.
    Maximum,
}

/// Sandbox policy for Windows.
///
/// Configures the behavior of the Windows sandbox including:
/// - Network access control
/// - File system access control
/// - Resource limits
/// - Security restrictions
#[derive(Debug, Clone)]
pub struct SandboxPolicy {
    /// Overall policy level.
    pub level: PolicyLevel,

    /// Allow network access.
    pub allow_network: bool,

    /// Allow registry access (read/write).
    pub allow_registry: bool,

    /// Paths that can be read.
    pub readable_paths: Vec<PathBuf>,

    /// Paths that can be written.
    pub writable_paths: Vec<PathBuf>,

    /// Paths that are blocked entirely.
    pub blocked_paths: Vec<PathBuf>,

    /// Maximum number of processes.
    pub max_processes: u32,

    /// Maximum memory per process in bytes.
    pub max_memory_per_process: usize,

    /// Maximum total memory in bytes.
    pub max_total_memory: usize,

    /// Allow spawning child processes.
    pub allow_child_processes: bool,

    /// Allow clipboard access.
    pub allow_clipboard: bool,

    /// Inherit environment variables from parent.
    pub inherit_environment: bool,

    /// Environment variables to explicitly allow (if not inheriting all).
    pub allowed_env_vars: Vec<String>,

    /// Environment variables to explicitly block.
    pub blocked_env_vars: Vec<String>,
}

impl Default for SandboxPolicy {
    fn default() -> Self {
        Self {
            level: PolicyLevel::default(),
            allow_network: true,
            allow_registry: false,
            readable_paths: vec![],
            writable_paths: vec![],
            blocked_paths: vec![],
            max_processes: 100,
            max_memory_per_process: 2 * 1024 * 1024 * 1024, // 2 GB
            max_total_memory: 4 * 1024 * 1024 * 1024,       // 4 GB
            allow_child_processes: true,
            allow_clipboard: false,
            inherit_environment: true,
            allowed_env_vars: vec![],
            blocked_env_vars: vec![
                "AWS_ACCESS_KEY_ID".to_string(),
                "AWS_SECRET_ACCESS_KEY".to_string(),
                "AZURE_CLIENT_SECRET".to_string(),
                "GH_TOKEN".to_string(),
                "GITHUB_TOKEN".to_string(),
                "NPM_TOKEN".to_string(),
            ],
        }
    }
}

impl SandboxPolicy {
    /// Create a new policy with the default settings.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a minimal policy (primarily for process tracking).
    pub fn minimal() -> Self {
        Self {
            level: PolicyLevel::Minimal,
            allow_network: true,
            allow_registry: true,
            max_processes: 0,          // Unlimited
            max_memory_per_process: 0, // Unlimited
            max_total_memory: 0,       // Unlimited
            allow_child_processes: true,
            allow_clipboard: true,
            ..Self::default()
        }
    }

    /// Create a moderate policy (balanced security).
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
            allow_network: false,
            allow_registry: false,
            max_processes: 50,
            max_memory_per_process: 1 * 1024 * 1024 * 1024, // 1 GB
            max_total_memory: 2 * 1024 * 1024 * 1024,       // 2 GB
            allow_child_processes: true,                    // Needed for most build tools
            allow_clipboard: false,
            inherit_environment: false,
            allowed_env_vars: vec![
                "PATH".to_string(),
                "HOME".to_string(),
                "USERPROFILE".to_string(),
                "TEMP".to_string(),
                "TMP".to_string(),
                "COMSPEC".to_string(),
                "SYSTEMROOT".to_string(),
            ],
            ..Self::default()
        }
    }

    /// Allow network access.
    pub fn with_network(mut self) -> Self {
        self.allow_network = true;
        self
    }

    /// Disable network access.
    pub fn without_network(mut self) -> Self {
        self.allow_network = false;
        self
    }

    /// Add a writable path.
    pub fn with_writable_path(mut self, path: PathBuf) -> Self {
        self.writable_paths.push(path);
        self
    }

    /// Add multiple writable paths.
    pub fn with_writable_paths(mut self, paths: impl IntoIterator<Item = PathBuf>) -> Self {
        self.writable_paths.extend(paths);
        self
    }

    /// Add a readable path.
    pub fn with_readable_path(mut self, path: PathBuf) -> Self {
        self.readable_paths.push(path);
        self
    }

    /// Add multiple readable paths.
    pub fn with_readable_paths(mut self, paths: impl IntoIterator<Item = PathBuf>) -> Self {
        self.readable_paths.extend(paths);
        self
    }

    /// Block a specific path.
    pub fn with_blocked_path(mut self, path: PathBuf) -> Self {
        self.blocked_paths.push(path);
        self
    }

    /// Set maximum number of processes.
    pub fn with_max_processes(mut self, max: u32) -> Self {
        self.max_processes = max;
        self
    }

    /// Set maximum memory per process.
    pub fn with_max_memory_per_process(mut self, bytes: usize) -> Self {
        self.max_memory_per_process = bytes;
        self
    }

    /// Set maximum total memory.
    pub fn with_max_total_memory(mut self, bytes: usize) -> Self {
        self.max_total_memory = bytes;
        self
    }

    /// Allow clipboard access.
    pub fn with_clipboard(mut self) -> Self {
        self.allow_clipboard = true;
        self
    }

    /// Block an environment variable.
    pub fn with_blocked_env_var(mut self, var: impl Into<String>) -> Self {
        self.blocked_env_vars.push(var.into());
        self
    }

    /// Check if a path should be blocked.
    pub fn is_path_blocked(&self, path: &std::path::Path) -> bool {
        for blocked in &self.blocked_paths {
            if path.starts_with(blocked) {
                return true;
            }
        }
        false
    }

    /// Check if an environment variable should be blocked.
    pub fn is_env_var_blocked(&self, name: &str) -> bool {
        self.blocked_env_vars
            .iter()
            .any(|v| v.eq_ignore_ascii_case(name))
    }

    /// Get the list of environment variables to pass to the sandboxed process.
    pub fn get_filtered_environment(&self) -> Vec<(String, String)> {
        if self.inherit_environment {
            std::env::vars()
                .filter(|(key, _)| !self.is_env_var_blocked(key))
                .collect()
        } else {
            std::env::vars()
                .filter(|(key, _)| {
                    self.allowed_env_vars
                        .iter()
                        .any(|v| v.eq_ignore_ascii_case(key))
                })
                .collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_policy_default() {
        let policy = SandboxPolicy::default();
        assert!(policy.allow_network);
        assert!(!policy.allow_registry);
        assert_eq!(policy.level, PolicyLevel::Moderate);
    }

    #[test]
    fn test_policy_minimal() {
        let policy = SandboxPolicy::minimal();
        assert!(policy.allow_network);
        assert!(policy.allow_registry);
        assert_eq!(policy.level, PolicyLevel::Minimal);
    }

    #[test]
    fn test_policy_maximum() {
        let policy = SandboxPolicy::maximum();
        assert!(!policy.allow_network);
        assert!(!policy.allow_registry);
        assert_eq!(policy.level, PolicyLevel::Maximum);
    }

    #[test]
    fn test_env_var_blocking() {
        let policy = SandboxPolicy::default();
        assert!(policy.is_env_var_blocked("AWS_ACCESS_KEY_ID"));
        assert!(policy.is_env_var_blocked("aws_access_key_id")); // Case insensitive
        assert!(!policy.is_env_var_blocked("PATH"));
    }

    #[test]
    fn test_builder_pattern() {
        let policy = SandboxPolicy::new()
            .without_network()
            .with_max_processes(50)
            .with_writable_path(PathBuf::from("C:\\temp"));

        assert!(!policy.allow_network);
        assert_eq!(policy.max_processes, 50);
        assert!(policy.writable_paths.contains(&PathBuf::from("C:\\temp")));
    }
}
