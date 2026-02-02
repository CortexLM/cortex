//! Permission request hooks.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::types::HookPriority;
use crate::Result;

/// Input for permission.ask hook.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionAskInput {
    /// Session ID
    pub session_id: String,
    /// Permission type
    pub permission: String,
    /// Resource being accessed
    pub resource: String,
    /// Reason for the permission request
    pub reason: Option<String>,
}

/// Output for permission.ask hook (mutable).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionAskOutput {
    /// Permission decision
    pub decision: PermissionDecision,
    /// Reason for the decision
    pub reason: Option<String>,
}

impl PermissionAskOutput {
    /// Create a new output that defers to the user.
    pub fn ask() -> Self {
        Self {
            decision: PermissionDecision::Ask,
            reason: None,
        }
    }
}

/// Permission decision.
///
/// # Security
///
/// The `Allow` variant enables automatic permission grants without user interaction.
/// This is a **security-sensitive capability** that should only be used by:
///
/// 1. **Trusted system plugins** that are part of the core Cortex distribution
/// 2. **Internal permission policies** configured by administrators
///
/// **Third-party plugins should NEVER return `Allow`** as this enables privilege
/// escalation attacks. Third-party plugins should only return:
/// - `Ask` - to prompt the user for a decision (safest)
/// - `Deny` - to automatically deny the permission
///
/// ## Security Recommendations
///
/// - Audit any plugin that returns `Allow` carefully
/// - Consider implementing a plugin signing system to restrict `Allow` to signed plugins
/// - Log all `Allow` decisions for security monitoring
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PermissionDecision {
    /// Ask the user (default - safe for all plugins)
    Ask,
    /// Automatically allow.
    ///
    /// # Security Warning
    ///
    /// **ONLY use from trusted system plugins.** Third-party plugins using this
    /// variant can bypass user consent and enable privilege escalation attacks.
    ///
    /// When processing `Allow` decisions from plugins:
    /// 1. Verify the plugin is trusted/signed
    /// 2. Log the automatic grant for audit purposes
    /// 3. Consider requiring additional confirmation for sensitive permissions
    #[doc(hidden)]
    Allow,
    /// Automatically deny (safe for all plugins)
    Deny,
}

impl PermissionDecision {
    /// Check if this decision requires elevated trust.
    ///
    /// Returns `true` for `Allow`, which should only be used by trusted system plugins.
    pub fn requires_elevated_trust(&self) -> bool {
        matches!(self, Self::Allow)
    }

    /// Validate that this decision is safe for a third-party plugin.
    ///
    /// Returns an error if the decision is `Allow`, which third-party plugins
    /// should not be permitted to make.
    pub fn validate_for_third_party(&self) -> std::result::Result<(), &'static str> {
        if matches!(self, Self::Allow) {
            return Err("Third-party plugins cannot auto-grant permissions (security restriction)");
        }
        Ok(())
    }
}

/// Handler for permission.ask hook.
#[async_trait]
pub trait PermissionAskHook: Send + Sync {
    /// Get the priority of this hook.
    fn priority(&self) -> HookPriority {
        HookPriority::default()
    }

    /// Execute the hook.
    async fn execute(
        &self,
        input: &PermissionAskInput,
        output: &mut PermissionAskOutput,
    ) -> Result<()>;
}
