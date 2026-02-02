//! Skill permission checker for Cortex CLI.
//!
//! Provides dedicated permission checking for skills, including:
//! - Skill loading permission
//! - Per-skill tool restrictions
//! - Permission inheritance from parent session
//!
//! # Usage
//!
//! ```rust,ignore
//! use cortex_engine::permission::skill_permissions::SkillPermissionChecker;
//!
//! let checker = SkillPermissionChecker::new(permission_manager);
//!
//! // Check if a skill can be loaded
//! if checker.can_load_skill("my-skill").await? {
//!     // Load and execute the skill
//!     if checker.can_skill_use_tool("my-skill", "bash").await? {
//!         // Allow tool usage
//!     }
//! }
//! ```

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use super::manager::PermissionManager;
use super::types::{PermissionContext, PermissionResponse, PermissionScope, RiskLevel};
use crate::error::Result;

/// Skill permission decision record for logging/auditing.
#[derive(Debug, Clone)]
pub struct SkillPermissionDecision {
    /// Skill name.
    pub skill_name: String,
    /// Tool name (if applicable).
    pub tool_name: Option<String>,
    /// The decision made.
    pub decision: PermissionResponse,
    /// Reason for the decision.
    pub reason: String,
    /// Timestamp of the decision.
    pub timestamp: u64,
}

impl SkillPermissionDecision {
    /// Create a new decision record.
    pub fn new(
        skill_name: impl Into<String>,
        tool_name: Option<String>,
        decision: PermissionResponse,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            skill_name: skill_name.into(),
            tool_name,
            decision,
            reason: reason.into(),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        }
    }
}

/// Skill-specific tool restrictions.
#[derive(Debug, Clone, Default)]
pub struct SkillToolRestrictions {
    /// Tools that are explicitly allowed for this skill.
    /// If Some, only these tools are permitted.
    pub allowed_tools: Option<Vec<String>>,
    /// Tools that are explicitly denied for this skill.
    pub denied_tools: Vec<String>,
}

impl SkillToolRestrictions {
    /// Create new restrictions with allowed tools.
    pub fn with_allowed(tools: Vec<String>) -> Self {
        Self {
            allowed_tools: Some(tools),
            denied_tools: Vec::new(),
        }
    }

    /// Create new restrictions with denied tools.
    pub fn with_denied(tools: Vec<String>) -> Self {
        Self {
            allowed_tools: None,
            denied_tools: tools,
        }
    }

    /// Check if a tool is allowed by these restrictions.
    pub fn allows_tool(&self, tool_name: &str) -> bool {
        // Check denied list first
        if self.denied_tools.iter().any(|t| t == tool_name) {
            return false;
        }

        // If there's an allowed list, tool must be in it
        if let Some(ref allowed) = self.allowed_tools {
            return allowed.iter().any(|t| t == tool_name);
        }

        // No restrictions
        true
    }
}

/// Dedicated permission checker for skills.
///
/// This provides a higher-level interface for skill permission checking
/// that integrates with the main permission manager.
pub struct SkillPermissionChecker {
    /// Reference to the main permission manager.
    permission_manager: PermissionManager,
    /// Per-skill tool restrictions.
    skill_restrictions: Arc<RwLock<HashMap<String, SkillToolRestrictions>>>,
    /// Decision log for auditing.
    decisions: Arc<RwLock<Vec<SkillPermissionDecision>>>,
    /// Whether to inherit permissions from parent session.
    inherit_parent_permissions: bool,
    /// Parent session's permission manager (if inheriting).
    parent_manager: Option<PermissionManager>,
}

impl SkillPermissionChecker {
    /// Create a new skill permission checker.
    pub fn new(permission_manager: PermissionManager) -> Self {
        Self {
            permission_manager,
            skill_restrictions: Arc::new(RwLock::new(HashMap::new())),
            decisions: Arc::new(RwLock::new(Vec::new())),
            inherit_parent_permissions: false,
            parent_manager: None,
        }
    }

    /// Create with parent session inheritance.
    pub fn with_parent(permission_manager: PermissionManager, parent: PermissionManager) -> Self {
        Self {
            permission_manager,
            skill_restrictions: Arc::new(RwLock::new(HashMap::new())),
            decisions: Arc::new(RwLock::new(Vec::new())),
            inherit_parent_permissions: true,
            parent_manager: Some(parent),
        }
    }

    /// Set tool restrictions for a skill.
    pub async fn set_skill_restrictions(
        &self,
        skill_name: &str,
        restrictions: SkillToolRestrictions,
    ) {
        self.skill_restrictions
            .write()
            .await
            .insert(skill_name.to_string(), restrictions);
    }

    /// Get tool restrictions for a skill.
    pub async fn get_skill_restrictions(&self, skill_name: &str) -> Option<SkillToolRestrictions> {
        self.skill_restrictions
            .read()
            .await
            .get(skill_name)
            .cloned()
    }

    /// Check if a skill can be loaded.
    ///
    /// This checks the permission to load and execute a skill.
    pub async fn can_load_skill(&self, skill_name: &str) -> Result<bool> {
        let response = self
            .permission_manager
            .check_skill_permission(skill_name)
            .await;

        let decision = SkillPermissionDecision::new(
            skill_name,
            None,
            response,
            match response {
                PermissionResponse::Allow => "Skill loading allowed",
                PermissionResponse::Deny => "Skill loading denied",
                PermissionResponse::Ask => "Skill loading requires user confirmation",
            },
        );

        self.log_decision(decision).await;

        match response {
            PermissionResponse::Allow => {
                info!(skill = skill_name, "Skill loading permitted");
                Ok(true)
            }
            PermissionResponse::Deny => {
                warn!(skill = skill_name, "Skill loading denied");
                Ok(false)
            }
            PermissionResponse::Ask => {
                debug!(skill = skill_name, "Skill loading requires confirmation");
                // For now, return false until user confirms
                Ok(false)
            }
        }
    }

    /// Check if a skill can use a specific tool.
    ///
    /// This checks both:
    /// 1. Skill-level restrictions (from skill metadata)
    /// 2. Permission manager rules
    pub async fn can_skill_use_tool(&self, skill_name: &str, tool_name: &str) -> Result<bool> {
        // First check skill-specific restrictions
        if let Some(restrictions) = self.get_skill_restrictions(skill_name).await {
            if !restrictions.allows_tool(tool_name) {
                let decision = SkillPermissionDecision::new(
                    skill_name,
                    Some(tool_name.to_string()),
                    PermissionResponse::Deny,
                    format!("Tool '{}' not in skill's allowed tools list", tool_name),
                );
                self.log_decision(decision).await;

                warn!(
                    skill = skill_name,
                    tool = tool_name,
                    "Tool blocked by skill restrictions"
                );
                return Ok(false);
            }
        }

        // Check permission manager
        let response = self
            .permission_manager
            .check_skill_tool_permission(skill_name, tool_name)
            .await;

        // If parent inheritance is enabled and current manager doesn't allow,
        // check parent
        let final_response = if response == PermissionResponse::Ask
            && self.inherit_parent_permissions
            && self.parent_manager.is_some()
        {
            if let Some(ref parent) = self.parent_manager {
                let parent_response = parent
                    .check_skill_tool_permission(skill_name, tool_name)
                    .await;
                if parent_response == PermissionResponse::Allow {
                    parent_response
                } else {
                    response
                }
            } else {
                response
            }
        } else {
            response
        };

        let decision = SkillPermissionDecision::new(
            skill_name,
            Some(tool_name.to_string()),
            final_response,
            match final_response {
                PermissionResponse::Allow => {
                    format!("Skill '{}' allowed to use tool '{}'", skill_name, tool_name)
                }
                PermissionResponse::Deny => format!(
                    "Skill '{}' denied access to tool '{}'",
                    skill_name, tool_name
                ),
                PermissionResponse::Ask => format!(
                    "Skill '{}' tool '{}' usage requires confirmation",
                    skill_name, tool_name
                ),
            },
        );
        self.log_decision(decision).await;

        match final_response {
            PermissionResponse::Allow => {
                debug!(
                    skill = skill_name,
                    tool = tool_name,
                    "Tool usage permitted for skill"
                );
                Ok(true)
            }
            PermissionResponse::Deny => {
                warn!(
                    skill = skill_name,
                    tool = tool_name,
                    "Tool usage denied for skill"
                );
                Ok(false)
            }
            PermissionResponse::Ask => {
                debug!(
                    skill = skill_name,
                    tool = tool_name,
                    "Tool usage requires confirmation"
                );
                Ok(false)
            }
        }
    }

    /// Grant permission for a skill.
    pub async fn grant_skill(&self, skill_name: &str, scope: PermissionScope) -> Result<()> {
        self.permission_manager
            .grant_skill_permission(skill_name, scope)
            .await?;

        info!(
            skill = skill_name,
            scope = ?scope,
            "Skill permission granted"
        );

        Ok(())
    }

    /// Deny permission for a skill.
    pub async fn deny_skill(&self, skill_name: &str) -> Result<()> {
        self.permission_manager
            .deny_skill_permission(skill_name)
            .await?;

        info!(skill = skill_name, "Skill permission denied");

        Ok(())
    }

    /// Grant permission for a tool within a skill context.
    pub async fn grant_skill_tool(
        &self,
        skill_name: &str,
        tool_name: &str,
        scope: PermissionScope,
    ) -> Result<()> {
        let pattern = format!("{}:{}", skill_name, tool_name);
        self.permission_manager
            .grant_permission("skill_tool", &pattern, scope)
            .await?;

        info!(
            skill = skill_name,
            tool = tool_name,
            scope = ?scope,
            "Skill tool permission granted"
        );

        Ok(())
    }

    /// Log a permission decision.
    async fn log_decision(&self, decision: SkillPermissionDecision) {
        debug!(
            skill = decision.skill_name,
            tool = ?decision.tool_name,
            decision = ?decision.decision,
            reason = decision.reason,
            "Skill permission decision"
        );

        self.decisions.write().await.push(decision);
    }

    /// Get all logged decisions.
    pub async fn get_decisions(&self) -> Vec<SkillPermissionDecision> {
        self.decisions.read().await.clone()
    }

    /// Get decisions for a specific skill.
    pub async fn get_decisions_for_skill(&self, skill_name: &str) -> Vec<SkillPermissionDecision> {
        self.decisions
            .read()
            .await
            .iter()
            .filter(|d| d.skill_name == skill_name)
            .cloned()
            .collect()
    }

    /// Clear the decision log.
    pub async fn clear_decisions(&self) {
        self.decisions.write().await.clear();
    }

    /// Create a permission context for a skill action.
    pub fn create_skill_context(
        &self,
        skill_name: &str,
        description: impl Into<String>,
    ) -> PermissionContext {
        PermissionContext::for_skill(skill_name)
            .with_risk(RiskLevel::Medium)
            .with_description(description)
    }

    /// Create a permission context for a skill tool action.
    pub fn create_skill_tool_context(
        &self,
        skill_name: &str,
        tool_name: &str,
        description: impl Into<String>,
    ) -> PermissionContext {
        PermissionContext::for_skill_tool(skill_name, tool_name)
            .with_risk(RiskLevel::Medium)
            .with_description(description)
    }
}

impl Clone for SkillPermissionChecker {
    fn clone(&self) -> Self {
        Self {
            permission_manager: self.permission_manager.clone(),
            skill_restrictions: Arc::clone(&self.skill_restrictions),
            decisions: Arc::clone(&self.decisions),
            inherit_parent_permissions: self.inherit_parent_permissions,
            parent_manager: self.parent_manager.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_skill_tool_restrictions() {
        let restrictions = SkillToolRestrictions::with_allowed(vec![
            "Read".to_string(),
            "Grep".to_string(),
            "Glob".to_string(),
        ]);

        assert!(restrictions.allows_tool("Read"));
        assert!(restrictions.allows_tool("Grep"));
        assert!(!restrictions.allows_tool("Execute"));
        assert!(!restrictions.allows_tool("bash"));
    }

    #[tokio::test]
    async fn test_skill_tool_restrictions_denied() {
        let restrictions =
            SkillToolRestrictions::with_denied(vec!["Execute".to_string(), "bash".to_string()]);

        assert!(restrictions.allows_tool("Read"));
        assert!(restrictions.allows_tool("Grep"));
        assert!(!restrictions.allows_tool("Execute"));
        assert!(!restrictions.allows_tool("bash"));
    }

    #[tokio::test]
    async fn test_skill_permission_checker_restrictions() {
        let manager = PermissionManager::new();
        let checker = SkillPermissionChecker::new(manager);

        // Set restrictions for a skill
        checker
            .set_skill_restrictions(
                "restricted-skill",
                SkillToolRestrictions::with_allowed(vec!["Read".to_string()]),
            )
            .await;

        // Check restrictions were set
        let restrictions = checker.get_skill_restrictions("restricted-skill").await;
        assert!(restrictions.is_some());
        assert!(restrictions.unwrap().allows_tool("Read"));
    }

    #[tokio::test]
    async fn test_skill_permission_decision_logging() {
        let manager = PermissionManager::new();
        let checker = SkillPermissionChecker::new(manager);

        // Trigger a permission check
        let _ = checker.can_load_skill("test-skill").await;

        // Check decision was logged
        let decisions = checker.get_decisions().await;
        assert!(!decisions.is_empty());
        assert_eq!(decisions[0].skill_name, "test-skill");
    }

    #[tokio::test]
    async fn test_skill_context_creation() {
        let manager = PermissionManager::new();
        let checker = SkillPermissionChecker::new(manager);

        let context = checker.create_skill_context("my-skill", "Testing skill execution");
        assert!(context.is_skill_context());
        assert!(!context.is_skill_tool_context());

        let tool_context =
            checker.create_skill_tool_context("my-skill", "Read", "Skill reading file");
        assert!(tool_context.is_skill_context());
        assert!(tool_context.is_skill_tool_context());
    }

    #[tokio::test]
    async fn test_skill_permission_with_restrictions() {
        let manager = PermissionManager::new();
        let checker = SkillPermissionChecker::new(manager);

        // Set up restrictions that only allow Read
        checker
            .set_skill_restrictions(
                "limited-skill",
                SkillToolRestrictions::with_allowed(vec!["Read".to_string()]),
            )
            .await;

        // Read should be restricted (denied) by the restriction check
        // because the skill isn't in the allowed tools
        let can_use_execute = checker
            .can_skill_use_tool("limited-skill", "Execute")
            .await
            .unwrap();
        assert!(!can_use_execute);

        // Read should pass the restriction check
        // (actual permission depends on permission manager state)
        let decisions = checker.get_decisions_for_skill("limited-skill").await;
        assert!(!decisions.is_empty());
    }
}
