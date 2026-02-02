//! Permission system for agents.
//!
//! This module provides permission configuration for controlling what agents
//! can do, including file operations, command execution, and web access.

use crate::spec::OperationMode;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Permission level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Permission {
    /// Always allow.
    Allow,
    /// Always ask for confirmation.
    #[default]
    Ask,
    /// Always deny.
    Deny,
}

impl Permission {
    pub fn is_allowed(&self) -> bool {
        matches!(self, Permission::Allow)
    }

    pub fn is_denied(&self) -> bool {
        matches!(self, Permission::Deny)
    }

    pub fn needs_confirmation(&self) -> bool {
        matches!(self, Permission::Ask)
    }
}

/// Permission configuration for an agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionConfig {
    /// Permission for file edits.
    pub edit: Permission,
    /// Permissions for bash commands (pattern -> permission).
    pub bash: BashPermissions,
    /// Permission for web fetch operations.
    pub webfetch: Permission,
    /// Permission for "doom loop" (repeated failures).
    pub doom_loop: Permission,
    /// Permission for accessing files outside project.
    pub external_directory: Permission,
}

impl Default for PermissionConfig {
    fn default() -> Self {
        Self {
            edit: Permission::Allow,
            bash: BashPermissions::default(),
            webfetch: Permission::Allow,
            doom_loop: Permission::Ask,
            external_directory: Permission::Ask,
        }
    }
}

impl PermissionConfig {
    /// Create a read-only permission config (for plan mode).
    pub fn read_only() -> Self {
        Self {
            edit: Permission::Deny,
            bash: BashPermissions::read_only(),
            webfetch: Permission::Allow,
            doom_loop: Permission::Ask,
            external_directory: Permission::Ask,
        }
    }

    /// Create a full-access permission config (for build mode).
    pub fn full_access() -> Self {
        Self {
            edit: Permission::Allow,
            bash: BashPermissions::full_access(),
            webfetch: Permission::Allow,
            doom_loop: Permission::Ask,
            external_directory: Permission::Ask,
        }
    }

    /// Check if a bash command is allowed.
    pub fn can_execute_bash(&self, command: &str) -> bool {
        self.bash.check(command).is_allowed()
    }

    /// Filter permissions based on the operation mode.
    ///
    /// In Plan and Spec modes, write operations are denied.
    /// This returns a new `PermissionConfig` with appropriate restrictions.
    ///
    /// # Example
    ///
    /// ```rust
    /// use cortex_agents::permission::PermissionConfig;
    /// use cortex_agents::spec::OperationMode;
    ///
    /// let config = PermissionConfig::full_access();
    /// let filtered = config.filter_for_mode(OperationMode::Plan);
    /// assert!(filtered.edit.is_denied());
    /// ```
    pub fn filter_for_mode(&self, mode: OperationMode) -> Self {
        match mode {
            OperationMode::Build => self.clone(),
            OperationMode::Plan | OperationMode::Spec => {
                Self {
                    edit: Permission::Deny,
                    bash: BashPermissions::read_only(),
                    // Keep read-only permissions
                    webfetch: self.webfetch,
                    doom_loop: self.doom_loop,
                    external_directory: self.external_directory,
                }
            }
        }
    }

    /// Check if write operations are allowed in the current config.
    pub fn can_write(&self) -> bool {
        self.edit.is_allowed()
    }

    /// Create a permission config appropriate for the given mode.
    ///
    /// This is a convenience method that returns the right config
    /// for each operation mode.
    pub fn for_mode(mode: OperationMode) -> Self {
        match mode {
            OperationMode::Build => Self::full_access(),
            OperationMode::Plan | OperationMode::Spec => Self::read_only(),
        }
    }
}

/// Bash command permissions with pattern matching.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BashPermissions {
    /// Pattern -> Permission mapping.
    /// Patterns are matched in order, first match wins.
    /// Use "*" as wildcard at end of pattern.
    patterns: HashMap<String, Permission>,
    /// Default permission for unmatched commands.
    default: Permission,
}

impl Default for BashPermissions {
    fn default() -> Self {
        let mut patterns = HashMap::new();
        patterns.insert("*".to_string(), Permission::Allow);
        Self {
            patterns,
            default: Permission::Allow,
        }
    }
}

impl BashPermissions {
    pub fn new() -> Self {
        Self {
            patterns: HashMap::new(),
            default: Permission::Ask,
        }
    }

    /// Read-only bash permissions (for plan mode).
    pub fn read_only() -> Self {
        let mut patterns = HashMap::new();

        // Allow read-only commands
        let read_only_commands = [
            "cat*",
            "head*",
            "tail*",
            "less*",
            "more*",
            "ls*",
            "pwd*",
            "tree*",
            "find *",
            "grep*",
            "rg*",
            "git status*",
            "git log*",
            "git diff*",
            "git show*",
            "git branch",
            "du*",
            "df*",
            "wc*",
            "file *",
            "stat*",
            "which*",
            "whereis*",
            "type *",
            "sort*",
            "uniq*",
            "cut*",
            "diff*",
        ];

        for cmd in read_only_commands {
            patterns.insert(cmd.to_string(), Permission::Allow);
        }

        // Deny dangerous patterns
        patterns.insert("rm *".to_string(), Permission::Deny);
        patterns.insert("mv *".to_string(), Permission::Deny);
        patterns.insert("cp *".to_string(), Permission::Deny);

        // Ask for everything else
        Self {
            patterns,
            default: Permission::Ask,
        }
    }

    /// Full access bash permissions (for build mode).
    pub fn full_access() -> Self {
        let mut patterns = HashMap::new();
        patterns.insert("*".to_string(), Permission::Allow);
        Self {
            patterns,
            default: Permission::Allow,
        }
    }

    /// Add a permission pattern.
    pub fn add_pattern(&mut self, pattern: impl Into<String>, permission: Permission) {
        self.patterns.insert(pattern.into(), permission);
    }

    /// Check permission for a command.
    pub fn check(&self, command: &str) -> Permission {
        let command = command.trim();

        // Check patterns in order
        for (pattern, permission) in &self.patterns {
            if Self::matches_pattern(pattern, command) {
                return *permission;
            }
        }

        self.default
    }

    fn matches_pattern(pattern: &str, command: &str) -> bool {
        if pattern == "*" {
            return true;
        }

        if let Some(prefix) = pattern.strip_suffix('*') {
            command.starts_with(prefix)
        } else {
            command == pattern || command.starts_with(&format!("{} ", pattern))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bash_permissions() {
        let perms = BashPermissions::read_only();

        assert!(perms.check("ls -la").is_allowed());
        assert!(perms.check("cat file.txt").is_allowed());
        assert!(perms.check("git status").is_allowed());
        assert!(perms.check("rm -rf /").is_denied());
    }

    #[test]
    fn test_permission_config() {
        let config = PermissionConfig::read_only();
        assert!(config.edit.is_denied());
        assert!(config.webfetch.is_allowed());
    }

    #[test]
    fn test_filter_for_mode_build() {
        let config = PermissionConfig::full_access();
        let filtered = config.filter_for_mode(OperationMode::Build);

        assert!(filtered.edit.is_allowed());
        assert!(filtered.can_write());
    }

    #[test]
    fn test_filter_for_mode_plan() {
        let config = PermissionConfig::full_access();
        let filtered = config.filter_for_mode(OperationMode::Plan);

        assert!(filtered.edit.is_denied());
        assert!(!filtered.can_write());
        // Read-only bash commands should still work
        assert!(filtered.bash.check("ls -la").is_allowed());
        assert!(filtered.bash.check("cat file.txt").is_allowed());
        // Write commands should be denied/ask
        assert!(filtered.bash.check("rm -rf /").is_denied());
    }

    #[test]
    fn test_filter_for_mode_spec() {
        let config = PermissionConfig::full_access();
        let filtered = config.filter_for_mode(OperationMode::Spec);

        assert!(filtered.edit.is_denied());
        assert!(!filtered.can_write());
    }

    #[test]
    fn test_for_mode() {
        let build_config = PermissionConfig::for_mode(OperationMode::Build);
        assert!(build_config.can_write());

        let plan_config = PermissionConfig::for_mode(OperationMode::Plan);
        assert!(!plan_config.can_write());

        let spec_config = PermissionConfig::for_mode(OperationMode::Spec);
        assert!(!spec_config.can_write());
    }
}
