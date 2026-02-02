use crate::error::{CortexError, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// Permission level for a tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolPermission {
    /// Tool is allowed to run without asking.
    Allow,
    /// User must be asked before running the tool.
    Ask,
    /// Tool is explicitly denied.
    Deny,
}

/// Model overrides for a specific agent profile.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelOverrides {
    /// Model name override.
    pub model: Option<String>,
    /// Provider name override.
    pub provider: Option<String>,
    /// Temperature override.
    pub temperature: Option<f32>,
    /// Max context tokens override.
    pub max_context_tokens: Option<u32>,
    /// Max output tokens override.
    pub max_output_tokens: Option<u32>,
}

/// Agent profile definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentProfile {
    /// Unique name of the profile.
    pub name: String,
    /// Description of the agent's role.
    pub description: String,
    /// Model overrides for this profile.
    #[serde(default)]
    pub model_overrides: ModelOverrides,
    /// Map of tool names to permissions.
    #[serde(default)]
    pub tool_permissions: HashMap<String, ToolPermission>,
    /// Custom system prompt for this profile.
    pub system_prompt: Option<String>,
}

/// State that can accumulate during a session and needs to be reset on profile switch.
#[derive(Debug, Clone, Default)]
#[allow(dead_code)]
pub struct ProfileSessionState {
    /// Cached tool registrations from previous profile.
    pub tool_cache_dirty: bool,
    /// Indicates MCP connections need to be refreshed.
    pub mcp_connections_dirty: bool,
    /// Previous profile name (for detecting switches).
    pub previous_profile: Option<String>,
}

#[allow(dead_code)]
impl ProfileSessionState {
    /// Create new session state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Check if a profile switch occurred and mark state dirty.
    pub fn on_profile_switch(&mut self, new_profile: &str) -> bool {
        let switched = self
            .previous_profile
            .as_ref()
            .map(|p| p != new_profile)
            .unwrap_or(true);

        if switched {
            self.tool_cache_dirty = true;
            self.mcp_connections_dirty = true;
            self.previous_profile = Some(new_profile.to_string());
        }

        switched
    }

    /// Clear all dirty flags after reset is complete.
    pub fn mark_clean(&mut self) {
        self.tool_cache_dirty = false;
        self.mcp_connections_dirty = false;
    }

    /// Check if any state needs to be refreshed.
    pub fn needs_refresh(&self) -> bool {
        self.tool_cache_dirty || self.mcp_connections_dirty
    }
}

impl AgentProfile {
    /// Load all profiles from the project and user configuration.
    pub fn load_all() -> Result<HashMap<String, AgentProfile>> {
        let mut profiles = Self::defaults();

        // Load from user home directory first (lower precedence than local)
        if let Some(home_dir) = dirs::home_dir() {
            let user_config = home_dir.join(".cortex").join("agents.toml");
            if user_config.exists() {
                match Self::load_from_file(&user_config) {
                    Ok(user_profiles) => {
                        for (name, profile) in user_profiles {
                            profiles.insert(name, profile);
                        }
                    }
                    Err(e) => tracing::warn!("Failed to load user agent profiles: {}", e),
                }
            }
        }

        // Load from .cortex/agents.toml in current directory (highest precedence)
        let local_config = Path::new(".cortex/agents.toml");
        if local_config.exists() {
            let local_profiles = Self::load_from_file(local_config)?;
            for (name, profile) in local_profiles {
                profiles.insert(name, profile);
            }
        }

        Ok(profiles)
    }

    /// Load profiles from a specific TOML file.
    pub fn load_from_file(path: &Path) -> Result<HashMap<String, AgentProfile>> {
        let content = fs::read_to_string(path)?;
        let config: AgentProfilesConfig = toml::from_str(&content).map_err(|e| {
            CortexError::Config(format!("Failed to parse {}: {}", path.display(), e))
        })?;

        Ok(config.agents)
    }

    /// Get default internal profiles.
    pub fn defaults() -> HashMap<String, AgentProfile> {
        let mut profiles = HashMap::new();

        profiles.insert(
            "default".to_string(),
            AgentProfile {
                name: "default".to_string(),
                description: "Default general-purpose agent.".to_string(),
                model_overrides: ModelOverrides::default(),
                tool_permissions: HashMap::new(),
                system_prompt: None,
            },
        );

        profiles.insert(
            "build".to_string(),
            AgentProfile {
                name: "build".to_string(),
                description: "Full access agent for building and implementing features.".to_string(),
                model_overrides: ModelOverrides::default(),
                tool_permissions: {
                    let mut perms = HashMap::new();
                    perms.insert("*".to_string(), ToolPermission::Allow);
                    perms
                },
                system_prompt: Some("You are an expert software engineer with full access to the codebase. Your goal is to implement features, fix bugs, and build high-quality software.".to_string()),
            },
        );

        profiles.insert(
            "plan".to_string(),
            AgentProfile {
                name: "plan".to_string(),
                description: "Read-only agent for planning and analysis. Denies write/edit tools.".to_string(),
                model_overrides: ModelOverrides::default(),
                tool_permissions: {
                    let mut perms = HashMap::new();
                    // Allow read tools
                    perms.insert("Read".to_string(), ToolPermission::Allow);
                    perms.insert("LS".to_string(), ToolPermission::Allow);
                    perms.insert("Grep".to_string(), ToolPermission::Allow);
                    perms.insert("Glob".to_string(), ToolPermission::Allow);
                    perms.insert("SearchFiles".to_string(), ToolPermission::Allow);
                    perms.insert("WebSearch".to_string(), ToolPermission::Allow);
                    perms.insert("ViewImage".to_string(), ToolPermission::Allow);

                    // Deny write/edit tools
                    perms.insert("Edit".to_string(), ToolPermission::Deny);
                    perms.insert("Create".to_string(), ToolPermission::Deny);
                    perms.insert("ApplyPatch".to_string(), ToolPermission::Deny);
                    perms.insert("Execute".to_string(), ToolPermission::Deny);
                    perms
                },
                system_prompt: Some("You are a software architect focused on planning and analysis. You have read-only access to the codebase. Your goal is to analyze requirements, research the codebase, and provide detailed implementation plans without making any changes.".to_string()),
            },
        );

        profiles.insert(
            "explore".to_string(),
            AgentProfile {
                name: "explore".to_string(),
                description: "Fast, search-focused agent for exploring and understanding the codebase.".to_string(),
                model_overrides: ModelOverrides {
                    model: Some("gpt-4o-mini".to_string()),
                    ..ModelOverrides::default()
                },
                tool_permissions: {
                    let mut perms = HashMap::new();
                    perms.insert("Grep".to_string(), ToolPermission::Allow);
                    perms.insert("Glob".to_string(), ToolPermission::Allow);
                    perms.insert("LS".to_string(), ToolPermission::Allow);
                    perms.insert("Read".to_string(), ToolPermission::Allow);
                    perms.insert("SearchFiles".to_string(), ToolPermission::Allow);

                    // Deny others for speed and focus
                    perms.insert("Edit".to_string(), ToolPermission::Deny);
                    perms.insert("Create".to_string(), ToolPermission::Deny);
                    perms.insert("ApplyPatch".to_string(), ToolPermission::Deny);
                    perms.insert("Execute".to_string(), ToolPermission::Deny);
                    perms
                },
                system_prompt: Some("You are a code exploration specialist. Your goal is to quickly find information and understand the codebase structure using search and navigation tools.".to_string()),
            },
        );

        profiles
    }

    /// Check if a tool is allowed for this profile.
    pub fn can_use_tool(&self, tool_name: &str) -> ToolPermission {
        if let Some(perm) = self.tool_permissions.get(tool_name) {
            *perm
        } else if let Some(perm) = self.tool_permissions.get("*") {
            *perm
        } else {
            // Default to Ask for unspecified tools
            ToolPermission::Ask
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_profiles() {
        let defaults = AgentProfile::defaults();
        assert!(defaults.contains_key("build"));
        assert!(defaults.contains_key("plan"));
        assert!(defaults.contains_key("explore"));

        let build = defaults.get("build").unwrap();
        assert_eq!(build.can_use_tool("Read"), ToolPermission::Allow);
        assert_eq!(build.can_use_tool("Execute"), ToolPermission::Allow);

        let plan = defaults.get("plan").unwrap();
        assert_eq!(plan.can_use_tool("Read"), ToolPermission::Allow);
        assert_eq!(plan.can_use_tool("Edit"), ToolPermission::Deny);
        assert_eq!(plan.can_use_tool("Execute"), ToolPermission::Deny);

        let explore = defaults.get("explore").unwrap();
        assert_eq!(explore.can_use_tool("Grep"), ToolPermission::Allow);
        assert_eq!(explore.can_use_tool("Edit"), ToolPermission::Deny);
        assert_eq!(
            explore.model_overrides.model,
            Some("gpt-4o-mini".to_string())
        );
    }
}

/// Helper struct for parsing agents.toml.
#[derive(Debug, Deserialize)]
struct AgentProfilesConfig {
    #[serde(default)]
    agents: HashMap<String, AgentProfile>,
}
