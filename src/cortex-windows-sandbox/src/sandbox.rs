//! Windows sandbox implementation.
//!
//! Combines Job Objects, Restricted Tokens, and Process Mitigations
//! to provide comprehensive process sandboxing on Windows.

use crate::job::{JobLimits, JobObject};
use crate::mitigation::{MitigationConfig, ProcessMitigations};
use crate::policy::{PolicyLevel, SandboxPolicy};
use crate::token::{RestrictedToken, TokenConfig};
use crate::{Result, WindowsSandboxError};
use std::path::PathBuf;
use tracing::{debug, info, warn};

/// Configuration for Windows sandbox.
#[derive(Debug, Clone)]
pub struct SandboxConfig {
    /// Paths that can be written to.
    pub writable_roots: Vec<PathBuf>,

    /// Allow network access.
    pub network_access: bool,

    /// Use restricted token.
    pub use_restricted_token: bool,

    /// Use job object for process isolation.
    pub use_job_object: bool,

    /// Apply process mitigations.
    pub apply_mitigations: bool,

    /// Policy level.
    pub policy_level: PolicyLevel,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            writable_roots: vec![],
            network_access: true,
            use_restricted_token: true,
            use_job_object: true,
            apply_mitigations: true,
            policy_level: PolicyLevel::Moderate,
        }
    }
}

impl SandboxConfig {
    /// Create a new sandbox configuration.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create configuration from a policy.
    pub fn from_policy(policy: &SandboxPolicy) -> Self {
        Self {
            writable_roots: policy.writable_paths.clone(),
            network_access: policy.allow_network,
            use_restricted_token: policy.level != PolicyLevel::Minimal,
            use_job_object: true,
            apply_mitigations: policy.level != PolicyLevel::Minimal,
            policy_level: policy.level,
        }
    }

    /// Add a writable root path.
    pub fn with_writable_root(mut self, path: PathBuf) -> Self {
        self.writable_roots.push(path);
        self
    }

    /// Enable or disable network access.
    pub fn with_network(mut self, allow: bool) -> Self {
        self.network_access = allow;
        self
    }

    /// Enable or disable restricted token.
    pub fn with_restricted_token(mut self, use_token: bool) -> Self {
        self.use_restricted_token = use_token;
        self
    }

    /// Enable or disable job object.
    pub fn with_job_object(mut self, use_job: bool) -> Self {
        self.use_job_object = use_job;
        self
    }

    /// Enable or disable process mitigations.
    pub fn with_mitigations(mut self, apply: bool) -> Self {
        self.apply_mitigations = apply;
        self
    }

    /// Set the policy level.
    pub fn with_policy_level(mut self, level: PolicyLevel) -> Self {
        self.policy_level = level;
        self
    }
}

/// Windows sandbox using Job Objects, Restricted Tokens, and Process Mitigations.
///
/// This provides comprehensive sandboxing for Windows processes including:
/// - Process isolation via Job Objects
/// - Privilege reduction via Restricted Tokens
/// - Exploit prevention via Process Mitigations
pub struct WindowsSandbox {
    config: SandboxConfig,
    job_object: Option<JobObject>,
    mitigations_applied: bool,
}

impl WindowsSandbox {
    /// Create a new Windows sandbox with the given configuration.
    pub fn new(config: SandboxConfig) -> Result<Self> {
        let mut sandbox = Self {
            config,
            job_object: None,
            mitigations_applied: false,
        };

        // Create job object if configured
        if sandbox.config.use_job_object {
            let job_limits = sandbox.get_job_limits();
            match JobObject::new(job_limits) {
                Ok(job) => {
                    sandbox.job_object = Some(job);
                    debug!("Created Job Object for sandbox");
                }
                Err(e) => {
                    warn!("Failed to create Job Object: {}", e);
                    // Continue without job object - not fatal
                }
            }
        }

        Ok(sandbox)
    }

    /// Create a sandbox from a policy.
    pub fn from_policy(policy: &SandboxPolicy) -> Result<Self> {
        let config = SandboxConfig::from_policy(policy);
        Self::new(config)
    }

    /// Create a sandbox with minimal restrictions.
    pub fn minimal() -> Result<Self> {
        Self::new(SandboxConfig {
            policy_level: PolicyLevel::Minimal,
            use_restricted_token: false,
            apply_mitigations: false,
            ..SandboxConfig::default()
        })
    }

    /// Create a sandbox with moderate restrictions (default).
    pub fn moderate() -> Result<Self> {
        Self::new(SandboxConfig::default())
    }

    /// Create a sandbox with maximum restrictions.
    pub fn maximum() -> Result<Self> {
        Self::new(SandboxConfig {
            policy_level: PolicyLevel::Maximum,
            network_access: false,
            ..SandboxConfig::default()
        })
    }

    /// Get job limits based on policy level.
    fn get_job_limits(&self) -> JobLimits {
        match self.config.policy_level {
            PolicyLevel::Minimal => JobLimits::minimal(),
            PolicyLevel::Moderate => JobLimits::default(),
            PolicyLevel::Maximum => JobLimits::restrictive(),
        }
    }

    /// Get mitigation config based on policy level.
    fn get_mitigation_config(&self) -> MitigationConfig {
        match self.config.policy_level {
            PolicyLevel::Minimal => MitigationConfig::minimal(),
            PolicyLevel::Moderate => MitigationConfig::balanced(),
            PolicyLevel::Maximum => MitigationConfig::maximum_security(),
        }
    }

    /// Get token config based on policy level.
    fn get_token_config(&self) -> TokenConfig {
        match self.config.policy_level {
            PolicyLevel::Minimal => TokenConfig::default(),
            PolicyLevel::Moderate => TokenConfig::moderate(),
            PolicyLevel::Maximum => TokenConfig::restrictive(),
        }
    }

    /// Apply sandbox restrictions to the current process.
    ///
    /// This applies:
    /// 1. Process mitigations (if enabled)
    /// 2. Assigns current process to job object (if enabled)
    /// 3. Note: Restricted tokens require process restart to take effect
    pub fn apply(&mut self) -> Result<()> {
        info!(
            "Applying Windows sandbox with policy level: {:?}",
            self.config.policy_level
        );

        // Apply process mitigations
        if self.config.apply_mitigations && !self.mitigations_applied {
            let mitigation_config = self.get_mitigation_config();
            let mut mitigations = ProcessMitigations::new(mitigation_config);

            if let Err(e) = mitigations.apply() {
                warn!("Some process mitigations failed: {}", e);
            }

            self.mitigations_applied = true;
            debug!("Applied process mitigations");
        }

        // Assign current process to job object
        if let Some(ref job) = self.job_object {
            match job.assign_current_process() {
                Ok(()) => {
                    debug!("Assigned current process to job object");
                }
                Err(e) => {
                    // Process may already be in a job - this is common
                    warn!(
                        "Could not assign process to job: {} (process may already be in a job)",
                        e
                    );
                }
            }
        }

        // Clear sensitive environment variables
        clear_sensitive_env_vars();

        info!("Windows sandbox applied successfully");
        Ok(())
    }

    /// Create a restricted token for spawning sandboxed child processes.
    ///
    /// The returned token can be used with CreateProcessAsUser to spawn
    /// a child process with reduced privileges.
    pub fn create_restricted_token(&self) -> Result<RestrictedToken> {
        if !self.config.use_restricted_token {
            return Err(WindowsSandboxError::CreateFailed(
                "Restricted tokens disabled in configuration".to_string(),
            ));
        }

        let token_config = self.get_token_config();
        RestrictedToken::from_current_process(token_config)
    }

    /// Get the job object handle for assigning child processes.
    pub fn job_object(&self) -> Option<&JobObject> {
        self.job_object.as_ref()
    }

    /// Check if Windows sandbox is available on this system.
    pub fn is_available() -> bool {
        Self::check_availability()
    }

    /// Perform detailed availability check.
    pub fn check_availability() -> bool {
        // Check if we're on Windows
        if !cfg!(windows) {
            return false;
        }

        // Check if Job Objects work
        let job_available = JobObject::is_available();
        if !job_available {
            debug!("Job Objects not available");
        }

        // Check if process mitigations work
        let mitigations_available = ProcessMitigations::is_available();
        if !mitigations_available {
            debug!("Process mitigations not available");
        }

        // At minimum, we need job objects or mitigations
        job_available || mitigations_available
    }

    /// Get information about sandbox capabilities.
    pub fn capabilities_info() -> SandboxCapabilities {
        SandboxCapabilities {
            job_objects: JobObject::is_available(),
            restricted_tokens: RestrictedToken::is_available(),
            process_mitigations: ProcessMitigations::is_available(),
        }
    }
}

/// Information about available sandbox capabilities.
#[derive(Debug, Clone)]
pub struct SandboxCapabilities {
    /// Job Objects are available.
    pub job_objects: bool,

    /// Restricted Tokens can be created.
    pub restricted_tokens: bool,

    /// Process Mitigations are available.
    pub process_mitigations: bool,
}

impl SandboxCapabilities {
    /// Check if any sandbox features are available.
    pub fn any_available(&self) -> bool {
        self.job_objects || self.restricted_tokens || self.process_mitigations
    }

    /// Check if all sandbox features are available.
    pub fn all_available(&self) -> bool {
        self.job_objects && self.restricted_tokens && self.process_mitigations
    }
}

/// Clear sensitive environment variables.
///
/// This removes environment variables that could contain secrets or
/// be used for privilege escalation.
fn clear_sensitive_env_vars() {
    const SENSITIVE_VARS: &[&str] = &[
        // Cloud credentials
        "AWS_ACCESS_KEY_ID",
        "AWS_SECRET_ACCESS_KEY",
        "AWS_SESSION_TOKEN",
        "AZURE_CLIENT_SECRET",
        "AZURE_TENANT_ID",
        "GOOGLE_APPLICATION_CREDENTIALS",
        "GOOGLE_CLOUD_PROJECT",
        // Version control tokens
        "GH_TOKEN",
        "GITHUB_TOKEN",
        "GITLAB_TOKEN",
        "BITBUCKET_TOKEN",
        // Package manager tokens
        "NPM_TOKEN",
        "PYPI_TOKEN",
        "CARGO_REGISTRY_TOKEN",
        // Database credentials
        "DATABASE_URL",
        "DB_PASSWORD",
        "MYSQL_PASSWORD",
        "POSTGRES_PASSWORD",
        // Generic secrets
        "API_KEY",
        "SECRET_KEY",
        "PRIVATE_KEY",
        "AUTH_TOKEN",
        "ACCESS_TOKEN",
        // Windows-specific injection vectors (note: usually handled by loader)
        // but clearing them doesn't hurt
    ];

    let mut cleared = 0;
    for var in SENSITIVE_VARS {
        if std::env::var_os(var).is_some() {
            // SAFETY: We're intentionally clearing potentially dangerous env vars
            unsafe {
                std::env::remove_var(var);
            }
            cleared += 1;
            debug!("Cleared sensitive environment variable: {}", var);
        }
    }

    if cleared > 0 {
        info!("Cleared {} sensitive environment variables", cleared);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sandbox_config_default() {
        let config = SandboxConfig::default();
        assert!(config.network_access);
        assert!(config.use_job_object);
        assert!(config.use_restricted_token);
    }

    #[test]
    fn test_sandbox_config_builder() {
        let config = SandboxConfig::new()
            .with_network(false)
            .with_writable_root(PathBuf::from("C:\\temp"))
            .with_policy_level(PolicyLevel::Maximum);

        assert!(!config.network_access);
        assert!(config.writable_roots.contains(&PathBuf::from("C:\\temp")));
        assert_eq!(config.policy_level, PolicyLevel::Maximum);
    }

    #[test]
    fn test_sandbox_config_from_policy() {
        let policy = SandboxPolicy::maximum();
        let config = SandboxConfig::from_policy(&policy);

        assert!(!config.network_access);
        assert_eq!(config.policy_level, PolicyLevel::Maximum);
    }
}
