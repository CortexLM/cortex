//! Permission types for Cortex CLI.
//!
//! Defines the core types used throughout the permission system.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Response to a permission request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PermissionResponse {
    /// Allow the operation.
    Allow,
    /// Ask the user for permission.
    Ask,
    /// Deny the operation.
    Deny,
}

impl Default for PermissionResponse {
    fn default() -> Self {
        Self::Ask
    }
}

/// Scope of a permission grant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PermissionScope {
    /// Permission applies only to this specific request.
    Once,
    /// Permission applies for the duration of the session.
    Session,
    /// Permission is persisted and applies always.
    Always,
}

impl Default for PermissionScope {
    fn default() -> Self {
        Self::Once
    }
}

/// Risk level for operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum RiskLevel {
    /// Safe operation - minimal risk.
    Low,
    /// Standard operation - some risk.
    #[default]
    Medium,
    /// Potentially dangerous operation.
    High,
    /// Critical operation requiring careful review.
    Critical,
}

impl RiskLevel {
    /// Check if this risk level is dangerous (high or critical).
    pub fn is_dangerous(&self) -> bool {
        matches!(self, Self::High | Self::Critical)
    }
}

/// Context for a permission request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionContext {
    /// Optional file path being accessed.
    pub file_path: Option<PathBuf>,
    /// Optional command being executed.
    pub command: Option<String>,
    /// Risk level of the operation.
    pub risk_level: RiskLevel,
    /// Optional description of the operation.
    pub description: Option<String>,
    /// Working directory for the operation.
    pub working_dir: Option<PathBuf>,
    /// Skill name (if this is a skill-related permission).
    pub skill_name: Option<String>,
    /// Tool being used within a skill context.
    pub skill_tool: Option<String>,
    /// Additional context data.
    #[serde(default)]
    pub extra: std::collections::HashMap<String, String>,
}

impl PermissionContext {
    /// Create a new empty permission context.
    pub fn new() -> Self {
        Self {
            file_path: None,
            command: None,
            risk_level: RiskLevel::default(),
            description: None,
            working_dir: None,
            skill_name: None,
            skill_tool: None,
            extra: std::collections::HashMap::new(),
        }
    }

    /// Create a context for a file operation.
    pub fn for_file(path: impl Into<PathBuf>) -> Self {
        Self {
            file_path: Some(path.into()),
            command: None,
            risk_level: RiskLevel::Medium,
            description: None,
            working_dir: None,
            skill_name: None,
            skill_tool: None,
            extra: std::collections::HashMap::new(),
        }
    }

    /// Create a context for a command execution.
    pub fn for_command(command: impl Into<String>) -> Self {
        Self {
            file_path: None,
            command: Some(command.into()),
            risk_level: RiskLevel::Medium,
            description: None,
            working_dir: None,
            skill_name: None,
            skill_tool: None,
            extra: std::collections::HashMap::new(),
        }
    }

    /// Create a context for a skill execution.
    pub fn for_skill(skill_name: impl Into<String>) -> Self {
        Self {
            file_path: None,
            command: None,
            risk_level: RiskLevel::Medium,
            description: None,
            working_dir: None,
            skill_name: Some(skill_name.into()),
            skill_tool: None,
            extra: std::collections::HashMap::new(),
        }
    }

    /// Create a context for a tool call within a skill.
    pub fn for_skill_tool(skill_name: impl Into<String>, tool_name: impl Into<String>) -> Self {
        Self {
            file_path: None,
            command: None,
            risk_level: RiskLevel::Medium,
            description: None,
            working_dir: None,
            skill_name: Some(skill_name.into()),
            skill_tool: Some(tool_name.into()),
            extra: std::collections::HashMap::new(),
        }
    }

    /// Set the risk level.
    pub fn with_risk(mut self, risk: RiskLevel) -> Self {
        self.risk_level = risk;
        self
    }

    /// Set the description.
    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    /// Set the working directory.
    pub fn with_working_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.working_dir = Some(dir.into());
        self
    }

    /// Add extra context data.
    pub fn with_extra(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.extra.insert(key.into(), value.into());
        self
    }

    /// Set the skill name.
    pub fn with_skill(mut self, skill_name: impl Into<String>) -> Self {
        self.skill_name = Some(skill_name.into());
        self
    }

    /// Set the skill tool.
    pub fn with_skill_tool(mut self, tool_name: impl Into<String>) -> Self {
        self.skill_tool = Some(tool_name.into());
        self
    }

    /// Check if this context is for a skill.
    pub fn is_skill_context(&self) -> bool {
        self.skill_name.is_some()
    }

    /// Check if this context is for a tool within a skill.
    pub fn is_skill_tool_context(&self) -> bool {
        self.skill_name.is_some() && self.skill_tool.is_some()
    }
}

impl Default for PermissionContext {
    fn default() -> Self {
        Self::new()
    }
}

/// A stored permission entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Permission {
    /// Tool name this permission applies to.
    pub tool: String,
    /// Pattern to match (command, path, etc.).
    pub pattern: String,
    /// The permission response (allow/deny).
    pub response: PermissionResponse,
    /// Scope of this permission.
    pub scope: PermissionScope,
    /// When this permission was created.
    #[serde(default = "default_timestamp")]
    pub created_at: u64,
    /// Optional description of why this permission was granted.
    pub reason: Option<String>,
}

impl Permission {
    /// Create a new permission entry.
    pub fn new(
        tool: impl Into<String>,
        pattern: impl Into<String>,
        response: PermissionResponse,
        scope: PermissionScope,
    ) -> Self {
        Self {
            tool: tool.into(),
            pattern: pattern.into(),
            response,
            scope,
            created_at: current_timestamp(),
            reason: None,
        }
    }

    /// Set a reason for the permission.
    pub fn with_reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = Some(reason.into());
        self
    }

    /// Check if this permission is persistent (scope is Always).
    pub fn is_persistent(&self) -> bool {
        self.scope == PermissionScope::Always
    }

    /// Check if this permission is session-scoped.
    pub fn is_session(&self) -> bool {
        self.scope == PermissionScope::Session
    }

    /// Check if this permission allows the operation.
    pub fn allows(&self) -> bool {
        self.response == PermissionResponse::Allow
    }

    /// Check if this permission denies the operation.
    pub fn denies(&self) -> bool {
        self.response == PermissionResponse::Deny
    }
}

/// Result of a permission check.
#[derive(Debug, Clone)]
pub struct PermissionCheckResult {
    /// Whether permission was granted.
    pub granted: bool,
    /// The matching permission entry (if any).
    pub matching_permission: Option<Permission>,
    /// Whether this was auto-approved.
    pub auto_approved: bool,
    /// Reason for the decision.
    pub reason: Option<String>,
}

impl PermissionCheckResult {
    /// Create a granted result.
    pub fn granted(permission: Option<Permission>, auto: bool) -> Self {
        Self {
            granted: true,
            matching_permission: permission,
            auto_approved: auto,
            reason: None,
        }
    }

    /// Create a denied result.
    pub fn denied(permission: Option<Permission>, reason: impl Into<String>) -> Self {
        Self {
            granted: false,
            matching_permission: permission,
            auto_approved: false,
            reason: Some(reason.into()),
        }
    }

    /// Create a "needs asking" result.
    pub fn needs_asking() -> Self {
        Self {
            granted: false,
            matching_permission: None,
            auto_approved: false,
            reason: Some("Requires user permission".to_string()),
        }
    }
}

/// Tool categories for permission grouping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolCategory {
    /// File read operations.
    FileRead,
    /// File write operations.
    FileWrite,
    /// File delete operations.
    FileDelete,
    /// Shell/bash command execution.
    Shell,
    /// Network operations.
    Network,
    /// Git operations.
    Git,
    /// System operations.
    System,
    /// Skill execution.
    Skill,
    /// Unknown/other.
    Other,
}

impl ToolCategory {
    /// Get the default risk level for this category.
    pub fn default_risk(&self) -> RiskLevel {
        match self {
            Self::FileRead => RiskLevel::Low,
            Self::FileWrite => RiskLevel::Medium,
            Self::FileDelete => RiskLevel::High,
            Self::Shell => RiskLevel::Medium,
            Self::Network => RiskLevel::Medium,
            Self::Git => RiskLevel::Medium,
            Self::System => RiskLevel::High,
            Self::Skill => RiskLevel::Medium,
            Self::Other => RiskLevel::Medium,
        }
    }
}

/// Get current timestamp in seconds since epoch.
fn current_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Default timestamp for deserialization.
fn default_timestamp() -> u64 {
    current_timestamp()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_permission_response() {
        assert_eq!(PermissionResponse::default(), PermissionResponse::Ask);
    }

    #[test]
    fn test_permission_scope() {
        assert_eq!(PermissionScope::default(), PermissionScope::Once);
    }

    #[test]
    fn test_risk_level_dangerous() {
        assert!(!RiskLevel::Low.is_dangerous());
        assert!(!RiskLevel::Medium.is_dangerous());
        assert!(RiskLevel::High.is_dangerous());
        assert!(RiskLevel::Critical.is_dangerous());
    }

    #[test]
    fn test_permission_context() {
        let ctx = PermissionContext::for_command("git push")
            .with_risk(RiskLevel::High)
            .with_description("Pushing to remote");

        assert_eq!(ctx.command, Some("git push".to_string()));
        assert_eq!(ctx.risk_level, RiskLevel::High);
        assert_eq!(ctx.description, Some("Pushing to remote".to_string()));
    }

    #[test]
    fn test_permission() {
        let perm = Permission::new(
            "bash",
            "git diff*",
            PermissionResponse::Allow,
            PermissionScope::Always,
        );
        assert!(perm.allows());
        assert!(!perm.denies());
        assert!(perm.is_persistent());
        assert!(!perm.is_session());
    }
}
