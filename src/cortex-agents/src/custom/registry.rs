//! Custom agent registry for managing loaded agents.

use std::collections::HashMap;

use crate::permission::PermissionConfig;
use crate::AgentInfo;
use crate::AgentMode;

use super::config::{CustomAgentConfig, ToolCategory, ToolsConfig};

/// Registry of custom agents.
pub struct CustomAgentRegistry {
    /// Map of agent names to configurations.
    agents: HashMap<String, CustomAgentConfig>,
}

impl Default for CustomAgentRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl CustomAgentRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self {
            agents: HashMap::new(),
        }
    }

    /// Create a registry from a list of custom agents.
    pub fn from_agents(agents: impl IntoIterator<Item = CustomAgentConfig>) -> Self {
        let mut registry = Self::new();
        for agent in agents {
            registry.register(agent);
        }
        registry
    }

    /// Register a custom agent.
    pub fn register(&mut self, agent: CustomAgentConfig) {
        self.agents.insert(agent.name.clone(), agent);
    }

    /// Unregister a custom agent by name.
    pub fn unregister(&mut self, name: &str) -> Option<CustomAgentConfig> {
        self.agents.remove(name)
    }

    /// Get a custom agent by name.
    pub fn get(&self, name: &str) -> Option<&CustomAgentConfig> {
        self.agents.get(name)
    }

    /// Get a mutable reference to a custom agent by name.
    pub fn get_mut(&mut self, name: &str) -> Option<&mut CustomAgentConfig> {
        self.agents.get_mut(name)
    }

    /// Check if a custom agent exists.
    pub fn contains(&self, name: &str) -> bool {
        self.agents.contains_key(name)
    }

    /// List all custom agents.
    pub fn list(&self) -> impl Iterator<Item = &CustomAgentConfig> {
        self.agents.values()
    }

    /// List visible custom agents (not hidden).
    pub fn list_visible(&self) -> impl Iterator<Item = &CustomAgentConfig> {
        self.agents.values().filter(|d| !d.hidden)
    }

    /// Get the number of custom agents.
    pub fn len(&self) -> usize {
        self.agents.len()
    }

    /// Check if the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.agents.is_empty()
    }

    /// Get custom agent names.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.agents.keys().map(String::as_str)
    }

    /// Clear all custom agents.
    pub fn clear(&mut self) {
        self.agents.clear();
    }

    /// Find custom agents by prefix (for autocompletion).
    pub fn find_by_prefix(&self, prefix: &str) -> Vec<&CustomAgentConfig> {
        let prefix_lower = prefix.to_lowercase();
        self.agents
            .values()
            .filter(|d| d.name.to_lowercase().starts_with(&prefix_lower))
            .collect()
    }

    /// Search custom agents by name or description.
    pub fn search(&self, query: &str) -> Vec<&CustomAgentConfig> {
        let query_lower = query.to_lowercase();
        self.agents
            .values()
            .filter(|d| {
                d.name.to_lowercase().contains(&query_lower)
                    || d.description.to_lowercase().contains(&query_lower)
            })
            .collect()
    }

    /// Convert a custom agent to AgentInfo for use with the agent system.
    pub fn to_agent_info(&self, name: &str) -> Option<AgentInfo> {
        self.get(name).map(custom_agent_to_agent_info)
    }

    /// Convert all custom agents to AgentInfo.
    pub fn to_agent_infos(&self) -> Vec<AgentInfo> {
        self.agents
            .values()
            .map(custom_agent_to_agent_info)
            .collect()
    }
}

/// Convert a CustomAgentConfig to AgentInfo.
fn custom_agent_to_agent_info(agent: &CustomAgentConfig) -> AgentInfo {
    let mut info = AgentInfo::new(&agent.name)
        .with_display_name(&agent.name)
        .with_mode(AgentMode::Subagent);

    // Description
    if !agent.description.is_empty() {
        info = info.with_description(&agent.description);
    }

    // Prompt
    if !agent.prompt.is_empty() {
        info = info.with_prompt(&agent.prompt);
    }

    // Model (only if not inheriting)
    if !agent.inherits_model() {
        info = info.with_model(&agent.model);
    }

    // Temperature
    let temp = agent
        .temperature
        .unwrap_or_else(|| agent.reasoning_effort.suggested_temperature());
    info = info.with_temperature(temp);

    // Max steps
    let max_steps = agent
        .max_steps
        .unwrap_or_else(|| agent.reasoning_effort.suggested_max_steps());
    info = info.with_max_steps(max_steps);

    // Color
    if let Some(ref color) = agent.color {
        info = info.with_color(color);
    }

    // Hidden
    if agent.hidden {
        info = info.hidden();
    }

    // Permission config based on tools
    let permission = tools_to_permission(&agent.tools);
    info = info.with_permission(permission);

    // Disable tools not in the allowed list
    let allowed = agent.tools.allowed_tools();
    let all_tools = ToolCategory::All.tools();

    for tool in all_tools {
        if !allowed.iter().any(|t| t.eq_ignore_ascii_case(tool)) {
            info = info.disable_tool(tool.to_lowercase());
        }
    }

    info
}

/// Convert tools config to permission config.
fn tools_to_permission(tools: &ToolsConfig) -> PermissionConfig {
    match tools {
        ToolsConfig::Category(cat) => match cat {
            ToolCategory::ReadOnly => PermissionConfig::read_only(),
            ToolCategory::Edit | ToolCategory::Execute | ToolCategory::All => {
                PermissionConfig::full_access()
            }
            ToolCategory::Web | ToolCategory::Mcp => PermissionConfig::read_only(),
        },
        ToolsConfig::List(list) => {
            // Check if any edit/execute tools are in the list
            let has_edit = list.iter().any(|t| {
                let t_lower = t.to_lowercase();
                t_lower == "create" || t_lower == "edit" || t_lower == "applypatch"
            });
            let has_execute = list.iter().any(|t| t.to_lowercase() == "execute");

            if has_edit || has_execute {
                PermissionConfig::full_access()
            } else {
                PermissionConfig::read_only()
            }
        }
    }
}

impl IntoIterator for CustomAgentRegistry {
    type Item = CustomAgentConfig;
    type IntoIter = std::collections::hash_map::IntoValues<String, CustomAgentConfig>;

    fn into_iter(self) -> Self::IntoIter {
        self.agents.into_values()
    }
}

impl<'a> IntoIterator for &'a CustomAgentRegistry {
    type Item = &'a CustomAgentConfig;
    type IntoIter = std::collections::hash_map::Values<'a, String, CustomAgentConfig>;

    fn into_iter(self) -> Self::IntoIter {
        self.agents.values()
    }
}

impl FromIterator<CustomAgentConfig> for CustomAgentRegistry {
    fn from_iter<I: IntoIterator<Item = CustomAgentConfig>>(iter: I) -> Self {
        Self::from_agents(iter)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::custom::ReasoningEffort;

    fn make_agent(name: &str) -> CustomAgentConfig {
        CustomAgentConfig::new(name)
            .with_description(format!("Description for {}", name))
            .with_prompt(format!("Prompt for {}", name))
    }

    #[test]
    fn test_registry_new() {
        let registry = CustomAgentRegistry::new();
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
    }

    #[test]
    fn test_registry_register() {
        let mut registry = CustomAgentRegistry::new();
        registry.register(make_agent("test"));

        assert!(!registry.is_empty());
        assert_eq!(registry.len(), 1);
        assert!(registry.contains("test"));
    }

    #[test]
    fn test_registry_get() {
        let mut registry = CustomAgentRegistry::new();
        registry.register(make_agent("test"));

        let agent = registry.get("test");
        assert!(agent.is_some());
        assert_eq!(agent.unwrap().name, "test");

        assert!(registry.get("nonexistent").is_none());
    }

    #[test]
    fn test_registry_unregister() {
        let mut registry = CustomAgentRegistry::new();
        registry.register(make_agent("test"));

        let removed = registry.unregister("test");
        assert!(removed.is_some());
        assert!(registry.is_empty());
    }

    #[test]
    fn test_registry_list() {
        let mut registry = CustomAgentRegistry::new();
        registry.register(make_agent("a"));
        registry.register(make_agent("b"));
        registry.register(make_agent("c"));

        let list: Vec<_> = registry.list().collect();
        assert_eq!(list.len(), 3);
    }

    #[test]
    fn test_registry_list_visible() {
        let mut registry = CustomAgentRegistry::new();
        registry.register(make_agent("visible"));
        registry.register(make_agent("hidden").hidden());

        let visible: Vec<_> = registry.list_visible().collect();
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].name, "visible");
    }

    #[test]
    fn test_registry_find_by_prefix() {
        let mut registry = CustomAgentRegistry::new();
        registry.register(make_agent("build"));
        registry.register(make_agent("bump"));
        registry.register(make_agent("test"));

        let found = registry.find_by_prefix("bu");
        assert_eq!(found.len(), 2);

        let found = registry.find_by_prefix("te");
        assert_eq!(found.len(), 1);
    }

    #[test]
    fn test_registry_search() {
        let mut registry = CustomAgentRegistry::new();
        registry
            .register(CustomAgentConfig::new("reviewer").with_description("Code review helper"));
        registry
            .register(CustomAgentConfig::new("builder").with_description("Build system helper"));

        let found = registry.search("review");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "reviewer");

        let found = registry.search("helper");
        assert_eq!(found.len(), 2);
    }

    #[test]
    fn test_to_agent_info() {
        let mut registry = CustomAgentRegistry::new();
        registry.register(
            CustomAgentConfig::new("test-agent")
                .with_description("A test agent")
                .with_prompt("You are a test agent")
                .with_model("gpt-4")
                .with_reasoning_effort(ReasoningEffort::High)
                .with_tools(ToolsConfig::read_only())
                .with_temperature(0.8)
                .with_max_steps(25)
                .with_color("#ff0000"),
        );

        let info = registry.to_agent_info("test-agent");
        assert!(info.is_some());

        let info = info.unwrap();
        assert_eq!(info.name, "test-agent");
        assert_eq!(info.description.as_deref(), Some("A test agent"));
        assert_eq!(info.prompt.as_deref(), Some("You are a test agent"));
        assert_eq!(info.model.as_deref(), Some("gpt-4"));
        assert_eq!(info.temperature, Some(0.8));
        assert_eq!(info.max_steps, Some(25));
        assert_eq!(info.color.as_deref(), Some("#ff0000"));
        assert_eq!(info.mode, AgentMode::Subagent);
    }

    #[test]
    fn test_to_agent_info_inherit_model() {
        let mut registry = CustomAgentRegistry::new();
        registry.register(CustomAgentConfig::new("test").with_model("inherit"));

        let info = registry.to_agent_info("test").unwrap();
        assert!(info.model.is_none()); // Should not have a model set
    }

    #[test]
    fn test_tools_to_permission() {
        // Read-only should have read_only permission
        let perm = tools_to_permission(&ToolsConfig::read_only());
        assert!(perm.edit.is_denied());

        // All should have full_access
        let perm = tools_to_permission(&ToolsConfig::all());
        assert!(perm.edit.is_allowed());

        // Custom with edit should have full_access
        let perm = tools_to_permission(&ToolsConfig::custom(["Read", "Edit"]));
        assert!(perm.edit.is_allowed());

        // Custom without edit should be read_only
        let perm = tools_to_permission(&ToolsConfig::custom(["Read", "Grep"]));
        assert!(perm.edit.is_denied());
    }

    #[test]
    fn test_from_iterator() {
        let agents = vec![make_agent("a"), make_agent("b")];
        let registry: CustomAgentRegistry = agents.into_iter().collect();

        assert_eq!(registry.len(), 2);
        assert!(registry.contains("a"));
        assert!(registry.contains("b"));
    }
}
