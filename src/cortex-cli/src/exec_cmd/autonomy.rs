//! Autonomy levels for exec mode.
//!
//! Controls what operations the agent can perform without user approval.

use std::path::Path;

use clap::ValueEnum;
use cortex_protocol::{AskForApproval, SandboxPolicy};
use serde::{Deserialize, Serialize};

/// Autonomy level for exec mode.
///
/// Controls what operations the agent can perform without user approval.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AutonomyLevel {
    /// Read-only mode (default). No file modifications or command execution.
    /// Safe for reviewing planned changes without execution.
    #[default]
    ReadOnly,

    /// Low-risk operations. Enables basic file operations while blocking system changes.
    /// Good for: documentation updates, code formatting, adding comments.
    Low,

    /// Development operations. Adds package installation, builds, local git operations.
    /// Good for: local development, testing, dependency management.
    Medium,

    /// Production operations. Enables git push, deployments, sensitive operations.
    /// Good for: CI/CD pipelines, automated deployments.
    High,
}

impl std::fmt::Display for AutonomyLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AutonomyLevel::ReadOnly => write!(f, "read-only"),
            AutonomyLevel::Low => write!(f, "low"),
            AutonomyLevel::Medium => write!(f, "medium"),
            AutonomyLevel::High => write!(f, "high"),
        }
    }
}

impl AutonomyLevel {
    /// Convert to approval policy.
    pub fn to_approval_policy(&self) -> AskForApproval {
        match self {
            AutonomyLevel::ReadOnly => AskForApproval::UnlessTrusted,
            AutonomyLevel::Low => AskForApproval::OnRequest,
            AutonomyLevel::Medium => AskForApproval::OnFailure,
            AutonomyLevel::High => AskForApproval::Never,
        }
    }

    /// Convert to sandbox policy.
    pub fn to_sandbox_policy(&self, cwd: &Path) -> SandboxPolicy {
        match self {
            AutonomyLevel::ReadOnly => SandboxPolicy::ReadOnly,
            AutonomyLevel::Low | AutonomyLevel::Medium => SandboxPolicy::WorkspaceWrite {
                writable_roots: vec![cwd.to_path_buf()],
                network_access: *self == AutonomyLevel::Medium,
                exclude_tmpdir_env_var: false,
                exclude_slash_tmp: false,
            },
            AutonomyLevel::High => SandboxPolicy::WorkspaceWrite {
                writable_roots: vec![cwd.to_path_buf()],
                network_access: true,
                exclude_tmpdir_env_var: false,
                exclude_slash_tmp: false,
            },
        }
    }

    /// Check if a command risk level is allowed.
    ///
    /// # Arguments
    /// * `risk` - The risk level of the command ("low", "medium", "high")
    /// * `command` - The actual command string to check if it's read-only
    pub fn allows_risk(&self, risk: &str, command: &str) -> bool {
        match self {
            AutonomyLevel::ReadOnly => risk == "low" && is_read_only_command(command),
            AutonomyLevel::Low => risk == "low",
            AutonomyLevel::Medium => risk == "low" || risk == "medium",
            AutonomyLevel::High => true,
        }
    }
}

/// Check if a command is read-only (safe for read-only mode).
pub fn is_read_only_command(cmd: &str) -> bool {
    let read_only_commands = [
        "cat", "less", "head", "tail", "ls", "pwd", "echo", "whoami", "date", "uname", "ps", "top",
        "find", "grep", "rg", "fd", "tree", "wc", "file",
    ];
    let read_only_git_subcommands = ["git status", "git log", "git diff", "git branch"];

    let cmd_lower = cmd.to_lowercase();

    // Check git subcommands first (they contain spaces)
    if read_only_git_subcommands
        .iter()
        .any(|p| cmd_lower.starts_with(p))
    {
        return true;
    }

    // Extract the first word (the command itself) for exact matching
    let first_word = cmd_lower.split_whitespace().next().unwrap_or("");

    // Also check for absolute paths (e.g., /bin/cat, /usr/bin/ls)
    let command_name = first_word.rsplit('/').next().unwrap_or(first_word);

    read_only_commands
        .iter()
        .any(|p| command_name == *p || first_word == *p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_autonomy_level_display() {
        assert_eq!(AutonomyLevel::ReadOnly.to_string(), "read-only");
        assert_eq!(AutonomyLevel::Low.to_string(), "low");
        assert_eq!(AutonomyLevel::Medium.to_string(), "medium");
        assert_eq!(AutonomyLevel::High.to_string(), "high");
    }

    #[test]
    fn test_autonomy_to_approval_policy() {
        assert!(matches!(
            AutonomyLevel::ReadOnly.to_approval_policy(),
            AskForApproval::UnlessTrusted
        ));
        assert!(matches!(
            AutonomyLevel::High.to_approval_policy(),
            AskForApproval::Never
        ));
    }

    #[test]
    fn test_is_read_only_command() {
        // Basic read-only commands
        assert!(is_read_only_command("cat file.txt"));
        assert!(is_read_only_command("ls -la"));
        assert!(is_read_only_command("git status"));
        assert!(is_read_only_command("git log --oneline"));
        assert!(is_read_only_command("pwd"));
        assert!(is_read_only_command("echo hello"));
        assert!(is_read_only_command("/bin/cat file.txt"));
        assert!(is_read_only_command("/usr/bin/ls -la"));

        // Non-read-only commands
        assert!(!is_read_only_command("rm -rf /"));
        assert!(!is_read_only_command("git push"));

        // Ensure prefix matching doesn't cause false positives (Issue #3820)
        assert!(!is_read_only_command("catfile")); // Not "cat file"
        assert!(!is_read_only_command("lsmod")); // Not "ls mod"
        assert!(!is_read_only_command("datestamp")); // Not "date stamp"
        assert!(!is_read_only_command("categorical-analysis")); // Not "cat"
    }

    #[test]
    fn test_allows_risk() {
        // Test ReadOnly level
        assert!(AutonomyLevel::ReadOnly.allows_risk("low", "cat file.txt"));
        assert!(AutonomyLevel::ReadOnly.allows_risk("low", "ls -la"));
        assert!(!AutonomyLevel::ReadOnly.allows_risk("low", "rm file.txt"));
        assert!(!AutonomyLevel::ReadOnly.allows_risk("medium", "cat file.txt"));
        assert!(!AutonomyLevel::ReadOnly.allows_risk("high", "cat file.txt"));

        // Test Low level
        assert!(AutonomyLevel::Low.allows_risk("low", "any command"));
        assert!(!AutonomyLevel::Low.allows_risk("medium", "any command"));
        assert!(!AutonomyLevel::Low.allows_risk("high", "any command"));

        // Test Medium level
        assert!(AutonomyLevel::Medium.allows_risk("low", "any command"));
        assert!(AutonomyLevel::Medium.allows_risk("medium", "any command"));
        assert!(!AutonomyLevel::Medium.allows_risk("high", "any command"));

        // Test High level
        assert!(AutonomyLevel::High.allows_risk("low", "any command"));
        assert!(AutonomyLevel::High.allows_risk("medium", "any command"));
        assert!(AutonomyLevel::High.allows_risk("high", "any command"));
    }
}
