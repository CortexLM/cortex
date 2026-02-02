//! Main policy engine for command execution evaluation.

use crate::command::ParsedCommand;
use crate::config::PolicyConfig;
use crate::context::ExecutionContext;
use crate::danger::{DangerCategory, DangerDetection};
use crate::decision::Decision;
use crate::detection::DetectionHelper;

/// Main structure for evaluating execution policies.
pub struct ExecPolicy {
    /// Policy configuration.
    config: PolicyConfig,

    /// Execution context.
    context: ExecutionContext,
}

impl ExecPolicy {
    /// Creates a new policy instance with default configuration.
    pub fn new() -> Self {
        Self {
            config: PolicyConfig::default(),
            context: ExecutionContext::default(),
        }
    }

    /// Creates a new policy instance with custom configuration.
    pub fn with_config(config: PolicyConfig) -> Self {
        Self {
            config,
            context: ExecutionContext::default(),
        }
    }

    /// Creates a new policy instance with custom context.
    pub fn with_context(context: ExecutionContext) -> Self {
        Self {
            config: PolicyConfig::default(),
            context,
        }
    }

    /// Creates a new policy instance with both custom config and context.
    pub fn with_config_and_context(config: PolicyConfig, context: ExecutionContext) -> Self {
        Self { config, context }
    }

    /// Updates the execution context.
    pub fn set_context(&mut self, context: ExecutionContext) {
        self.context = context;
    }

    /// Updates the policy configuration.
    pub fn set_config(&mut self, config: PolicyConfig) {
        self.config = config;
    }

    /// Returns a reference to the current context.
    pub fn context(&self) -> &ExecutionContext {
        &self.context
    }

    /// Returns a reference to the current configuration.
    pub fn config(&self) -> &PolicyConfig {
        &self.config
    }

    /// Evaluates a command and returns the decision.
    pub fn evaluate(&self, command: &[String]) -> Decision {
        if command.is_empty() {
            return Decision::Deny;
        }

        // Parse the command properly
        let parsed = match ParsedCommand::from_args(command) {
            Ok(p) => p,
            Err(_) => return Decision::Deny,
        };

        self.evaluate_parsed(&parsed)
    }

    /// Evaluates a parsed command and returns the decision.
    pub fn evaluate_parsed(&self, parsed: &ParsedCommand) -> Decision {
        // Check if explicitly denied
        if self
            .context
            .denied_programs
            .contains(&parsed.program_basename)
        {
            return Decision::Deny;
        }

        // Check for dangerous commands
        let danger = self.detect_danger(parsed);
        if danger.is_dangerous {
            // Check if context mitigates the danger
            if danger.context_mitigatable
                && (self.context.is_container || self.context.is_sandboxed)
            {
                // Downgrade to Ask instead of Deny in safe contexts
                return Decision::Ask;
            }
            return Decision::Deny;
        }

        // Check if explicitly allowed
        if self
            .context
            .allowed_programs
            .contains(&parsed.program_basename)
        {
            return Decision::Allow;
        }

        // Commands requiring confirmation
        if self.needs_confirmation(parsed) {
            return Decision::Ask;
        }

        // Also evaluate subcommands if present
        for subcmd in &parsed.subcommands {
            let sub_decision = self.evaluate_parsed(subcmd);
            if sub_decision == Decision::Deny {
                return Decision::Deny;
            }
            if sub_decision == Decision::Ask {
                return Decision::Ask;
            }
        }

        Decision::Allow
    }

    /// Evaluates a command and returns detailed danger detection.
    pub fn evaluate_with_details(&self, command: &[String]) -> (Decision, DangerDetection) {
        if command.is_empty() {
            return (
                Decision::Deny,
                DangerDetection::dangerous(
                    DangerCategory::DestructiveFileOp,
                    "empty command",
                    10,
                    false,
                ),
            );
        }

        let parsed = match ParsedCommand::from_args(command) {
            Ok(p) => p,
            Err(e) => {
                return (
                    Decision::Deny,
                    DangerDetection::dangerous(
                        DangerCategory::DestructiveFileOp,
                        format!("invalid command: {e}"),
                        10,
                        false,
                    ),
                );
            }
        };

        let danger = self.detect_danger(&parsed);
        let decision = if danger.is_dangerous {
            if danger.context_mitigatable
                && (self.context.is_container || self.context.is_sandboxed)
            {
                Decision::Ask
            } else {
                Decision::Deny
            }
        } else if self.needs_confirmation(&parsed) {
            Decision::Ask
        } else {
            Decision::Allow
        };

        (decision, danger)
    }

    /// Comprehensive dangerous command detection.
    pub(crate) fn detect_danger(&self, parsed: &ParsedCommand) -> DangerDetection {
        let mut detection = DangerDetection::safe();
        let helper = DetectionHelper::new(&self.config);

        // Check all danger categories
        helper.check_destructive_file_ops(parsed, &mut detection);
        helper.check_disk_operations(parsed, &mut detection);
        helper.check_privilege_escalation(parsed, &mut detection);
        helper.check_fork_bomb(parsed, &mut detection);
        helper.check_remote_code_execution(parsed, &mut detection);
        helper.check_insecure_permissions(parsed, &mut detection);
        helper.check_system_service_mods(parsed, &mut detection);
        helper.check_network_exposure(parsed, &mut detection);
        helper.check_credential_access(parsed, &mut detection);
        helper.check_kernel_modifications(parsed, &mut detection);
        helper.check_container_escape(parsed, &mut detection);
        helper.check_history_manipulation(parsed, &mut detection);
        helper.check_crypto_mining(parsed, &mut detection);
        helper.check_custom_patterns(parsed, &mut detection);

        detection
    }

    /// Determines if a command needs user confirmation (but isn't outright denied).
    pub(crate) fn needs_confirmation(&self, parsed: &ParsedCommand) -> bool {
        let prog = parsed.program_basename.as_str();

        // Network access commands
        let network_commands = [
            "curl",
            "wget",
            "fetch",
            "httpie",
            "http",
            "ftp",
            "sftp",
            "scp",
            "rsync",
            "ssh",
            "telnet",
            "nc",
            "netcat",
            "ping",
            "traceroute",
            "nmap",
            "dig",
            "nslookup",
            "host",
        ];

        if network_commands.contains(&prog) && !self.context.network_allowed {
            return true;
        }

        // Package managers (install/remove operations)
        let pkg_managers = [
            "npm", "yarn", "pnpm", "pip", "pip3", "pipx", "cargo", "gem", "bundle", "composer",
            "apt", "apt-get", "yum", "dnf", "pacman", "brew", "port", "emerge", "zypper", "apk",
            "winget", "choco", "scoop",
        ];

        if pkg_managers.contains(&prog) {
            // Check for install/uninstall operations
            let modifying_ops = [
                "install",
                "uninstall",
                "remove",
                "add",
                "upgrade",
                "update",
                "rm",
                "delete",
            ];
            for op in modifying_ops {
                if parsed.has_arg(op) {
                    return true;
                }
            }
        }

        // Git operations that modify remote
        if prog == "git" {
            let remote_ops = ["push", "remote", "fetch", "pull", "clone"];
            for op in remote_ops {
                if parsed.has_arg(op) {
                    return true;
                }
            }
        }

        // Docker/container operations
        let container_commands = ["docker", "podman", "kubectl", "helm", "docker-compose"];
        if container_commands.contains(&prog) {
            return true;
        }

        // File writing commands that aren't in cwd
        let write_commands = ["touch", "mkdir", "tee", "dd", "install"];
        if write_commands.contains(&prog) {
            for arg in &parsed.args {
                if !arg.starts_with('-') {
                    let normalized = DetectionHelper::normalize_path(arg);
                    let helper = DetectionHelper::new(&self.config);
                    if helper.is_sensitive_path(&normalized) {
                        return true;
                    }
                }
            }
        }

        false
    }
}

impl Default for ExecPolicy {
    fn default() -> Self {
        Self::new()
    }
}
