//! Permission system for Cortex CLI.
//!
//! This module provides a comprehensive permission system for controlling
//! when and how tools can be executed. It is CRITICAL for safety.
//!
//! # Components
//!
//! - [`types`]: Core permission types (Permission, PermissionResponse, PermissionScope, etc.)
//! - [`patterns`]: Glob-style pattern matching for commands and paths
//! - [`storage`]: Persistence to `~/.cortex/permissions.json` and session memory
//! - [`prompts`]: Permission prompt types and formatting
//! - [`manager`]: Central PermissionManager that coordinates everything
//! - [`skill_permissions`]: Dedicated permission checker for skills
//!
//! # Usage
//!
//! ```rust,ignore
//! use cortex_engine::permission::{PermissionManager, PermissionContext, PermissionResponse};
//!
//! // Create a permission manager
//! let manager = PermissionManager::new();
//! manager.init().await?;
//!
//! // Check permission for a bash command
//! let response = manager.check_bash_permission("git push origin main").await;
//! match response {
//!     PermissionResponse::Allow => { /* Execute command */ }
//!     PermissionResponse::Deny => { /* Refuse execution */ }
//!     PermissionResponse::Ask => { /* Prompt user */ }
//! }
//!
//! // Grant a permission
//! manager.grant_permission("bash", "git push*", PermissionScope::Session).await?;
//! ```
//!
//! # Skill Permissions
//!
//! Skills have their own permission system for controlling which skills can be
//! loaded and what tools they can use:
//!
//! ```rust,ignore
//! use cortex_engine::permission::{PermissionManager, SkillPermissionChecker};
//!
//! let manager = PermissionManager::new();
//! let checker = SkillPermissionChecker::new(manager);
//!
//! // Check if a skill can be loaded
//! if checker.can_load_skill("my-skill").await? {
//!     // Check if the skill can use specific tools
//!     if checker.can_skill_use_tool("my-skill", "bash").await? {
//!         // Allow the tool usage
//!     }
//! }
//! ```
//!
//! # Integration with Tools
//!
//! Tools should use the permission manager before executing sensitive operations:
//!
//! ```rust,ignore
//! // In bash tool
//! let context = PermissionContext::for_command(&command)
//!     .with_risk(RiskLevel::Medium);
//! let response = permission_manager.request_permission("bash", &command, &context).await;
//!
//! if response != PermissionResponse::Allow {
//!     // Either denied or needs user confirmation
//!     return Err(...);
//! }
//! ```

pub mod manager;
pub mod patterns;
pub mod prompts;
pub mod skill_permissions;
pub mod storage;
pub mod types;

// Re-exports for convenience
pub use manager::{
    ConfigPermissions, PermissionManager, PermissionManagerConfig, global_manager,
    init_global_manager,
};
pub use patterns::{
    PatternMatcher, PatternSource, PermissionPattern, extract_command_prefix, extract_path_pattern,
    glob_match, matches_skill_pattern, skill_pattern,
};
pub use prompts::{PermissionPrompt, PromptResponse, format_permission_list};
pub use skill_permissions::{
    SkillPermissionChecker, SkillPermissionDecision, SkillToolRestrictions,
};
pub use storage::{PermissionStorage, PermissionStore};
pub use types::{
    Permission, PermissionCheckResult, PermissionContext, PermissionResponse, PermissionScope,
    RiskLevel, ToolCategory,
};
