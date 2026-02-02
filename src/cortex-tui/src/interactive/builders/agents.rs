//! Builder for agents interactive selection.
//!
//! Provides builders for:
//! - Listing all agents (built-in + project + global)
//! - Creating new agents (project/global, AI/manual)

use std::path::Path;

use crate::interactive::state::{
    InlineFormField, InlineFormState, InteractiveAction, InteractiveItem, InteractiveState,
};

/// Agent type for grouping in the list
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AgentCategory {
    /// Built-in agents
    Builtin,
    /// Project-local agents (.cortex/agents/)
    Project,
    /// Global/user agents (~/.config/cortex/agents/)
    Global,
}

impl AgentCategory {
    fn label(&self) -> &'static str {
        match self {
            Self::Builtin => "Built-in Agents",
            Self::Project => "Project Agents",
            Self::Global => "Global Agents",
        }
    }
}

/// Information about an agent for display
#[derive(Debug, Clone)]
pub struct AgentDisplayInfo {
    /// Agent name/identifier
    pub name: String,
    /// Display name (optional)
    pub display_name: Option<String>,
    /// Description
    pub description: String,
    /// Category (builtin/project/global)
    pub category: AgentCategory,
    /// Model (if overridden)
    pub model: Option<String>,
    /// Whether it's a subagent
    pub is_subagent: bool,
    /// Source path for custom agents
    pub source: Option<String>,
}

impl AgentDisplayInfo {
    pub fn builtin(name: &str, description: &str, is_subagent: bool) -> Self {
        Self {
            name: name.to_string(),
            display_name: None,
            description: description.to_string(),
            category: AgentCategory::Builtin,
            model: None,
            is_subagent,
            source: None,
        }
    }

    pub fn project(name: &str, description: &str, source: &str) -> Self {
        Self {
            name: name.to_string(),
            display_name: None,
            description: description.to_string(),
            category: AgentCategory::Project,
            model: None,
            is_subagent: false,
            source: Some(source.to_string()),
        }
    }

    pub fn global(name: &str, description: &str, source: &str) -> Self {
        Self {
            name: name.to_string(),
            display_name: None,
            description: description.to_string(),
            category: AgentCategory::Global,
            model: None,
            is_subagent: false,
            source: Some(source.to_string()),
        }
    }

    pub fn with_model(mut self, model: &str) -> Self {
        self.model = Some(model.to_string());
        self
    }

    pub fn with_display_name(mut self, display_name: &str) -> Self {
        self.display_name = Some(display_name.to_string());
        self
    }
}

/// Load agents from project directory.
fn load_project_agents(project_path: Option<&Path>) -> Vec<AgentDisplayInfo> {
    let project_dir = match project_path {
        Some(p) => p.join(".cortex/agents"),
        None => {
            if let Ok(cwd) = std::env::current_dir() {
                cwd.join(".cortex/agents")
            } else {
                return Vec::new();
            }
        }
    };

    load_agents_from_dir(&project_dir, AgentCategory::Project)
}

/// Load agents from global directory.
fn load_global_agents() -> Vec<AgentDisplayInfo> {
    let global_dir = match dirs::config_dir() {
        Some(d) => d.join("cortex/agents"),
        None => return Vec::new(),
    };

    load_agents_from_dir(&global_dir, AgentCategory::Global)
}

/// Load agents from a directory.
fn load_agents_from_dir(dir: &Path, category: AgentCategory) -> Vec<AgentDisplayInfo> {
    let mut agents = Vec::new();

    if !dir.exists() {
        return agents;
    }

    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();

            // Support .md and .toml files
            let ext = path.extension().and_then(|e| e.to_str());
            if !matches!(ext, Some("md") | Some("toml")) {
                continue;
            }

            if let Ok(content) = std::fs::read_to_string(&path) {
                let agent = parse_agent_file(&content, &path, category);
                agents.push(agent);
            }
        }
    }

    agents
}

/// Parse agent file content (supports both TOML and Markdown frontmatter)
fn parse_agent_file(content: &str, path: &Path, category: AgentCategory) -> AgentDisplayInfo {
    let default_name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unnamed")
        .to_string();

    let source = path.display().to_string();

    // Try TOML first
    if path.extension().and_then(|e| e.to_str()) == Some("toml")
        && let Ok(parsed) = content.parse::<toml::Table>()
    {
        let name = parsed
            .get("name")
            .and_then(|v| v.as_str())
            .map(String::from)
            .unwrap_or(default_name.clone());
        let description = parsed
            .get("description")
            .and_then(|v| v.as_str())
            .map(String::from)
            .unwrap_or_default();
        let model = parsed
            .get("model")
            .and_then(|v| v.as_str())
            .map(String::from);
        let display_name = parsed
            .get("display_name")
            .and_then(|v| v.as_str())
            .map(String::from);

        let mut info = match category {
            AgentCategory::Project => AgentDisplayInfo::project(&name, &description, &source),
            AgentCategory::Global => AgentDisplayInfo::global(&name, &description, &source),
            AgentCategory::Builtin => AgentDisplayInfo::builtin(&name, &description, false),
        };

        if let Some(ref m) = model {
            info = info.with_model(m);
        }
        if let Some(ref dn) = display_name {
            info = info.with_display_name(dn);
        }

        return info;
    }

    // Try Markdown frontmatter
    let content_trimmed = content.trim();
    if content_trimmed.starts_with("---")
        && let Some(end_pos) = content_trimmed[3..].find("\n---")
    {
        let yaml_str = &content_trimmed[3..3 + end_pos];
        if let Ok(yaml) = serde_yaml::from_str::<serde_yaml::Value>(yaml_str) {
            let name = yaml
                .get("name")
                .and_then(serde_yaml::Value::as_str)
                .map(String::from)
                .unwrap_or(default_name.clone());
            let description = yaml
                .get("description")
                .and_then(serde_yaml::Value::as_str)
                .map(String::from)
                .unwrap_or_default();
            let model = yaml
                .get("model")
                .and_then(serde_yaml::Value::as_str)
                .map(String::from);
            let display_name = yaml
                .get("display_name")
                .and_then(serde_yaml::Value::as_str)
                .map(String::from);

            let mut info = match category {
                AgentCategory::Project => AgentDisplayInfo::project(&name, &description, &source),
                AgentCategory::Global => AgentDisplayInfo::global(&name, &description, &source),
                AgentCategory::Builtin => AgentDisplayInfo::builtin(&name, &description, false),
            };

            if let Some(ref m) = model {
                info = info.with_model(m);
            }
            if let Some(ref dn) = display_name {
                info = info.with_display_name(dn);
            }

            return info;
        }
    }

    // Fallback: use filename as name
    match category {
        AgentCategory::Project => AgentDisplayInfo::project(&default_name, "", &source),
        AgentCategory::Global => AgentDisplayInfo::global(&default_name, "", &source),
        AgentCategory::Builtin => AgentDisplayInfo::builtin(&default_name, "", false),
    }
}

/// Get built-in agents.
fn get_builtin_agents() -> Vec<AgentDisplayInfo> {
    vec![
        AgentDisplayInfo::builtin("general", "General-purpose agent for complex tasks", true),
        AgentDisplayInfo::builtin("explore", "Fast agent for codebase exploration", true),
        AgentDisplayInfo::builtin(
            "research",
            "Research agent for thorough investigation",
            true,
        ),
    ]
}

/// Build an interactive state for agents listing and management.
///
/// Shows all agents grouped by category with options to:
/// - View agent details
/// - Create new agent
///
/// # Arguments
/// * `project_path` - Optional project root path for loading project agents
/// * `terminal_height` - Optional terminal height for dynamic max_visible calculation
pub fn build_agents_selector(
    project_path: Option<&Path>,
    terminal_height: Option<u16>,
) -> InteractiveState {
    let mut items = Vec::new();

    // Add "Create New Agent" action at the top
    let create_item = InteractiveItem::new("__create__", "Create New Agent")
        .with_description("Create a new custom agent (AI-assisted or manual)")
        .with_shortcut('n');
    items.push(create_item);

    // Add separator
    items.push(
        InteractiveItem::new("__sep_actions__", "─────────────────────────────").as_separator(),
    );

    // Collect all agents
    let builtin_agents = get_builtin_agents();
    let project_agents = load_project_agents(project_path);
    let global_agents = load_global_agents();

    let mut current_category: Option<AgentCategory> = None;

    // Add built-in agents
    for agent in &builtin_agents {
        if current_category != Some(AgentCategory::Builtin) {
            current_category = Some(AgentCategory::Builtin);
            items.push(
                InteractiveItem::new(
                    format!("__cat_{}", AgentCategory::Builtin.label()),
                    AgentCategory::Builtin.label(),
                )
                .as_separator(),
            );
        }

        let suffix = if agent.is_subagent { " (subagent)" } else { "" };
        let description = format!("{}{}", agent.description, suffix);

        let item = InteractiveItem::new(format!("builtin:{}", agent.name), agent.name.clone())
            .with_description(description);
        items.push(item);
    }

    // Add project agents
    if !project_agents.is_empty() {
        let _current_category = Some(AgentCategory::Project);
        items.push(
            InteractiveItem::new(
                format!("__cat_{}", AgentCategory::Project.label()),
                AgentCategory::Project.label(),
            )
            .as_separator(),
        );

        for agent in &project_agents {
            let display_name = agent
                .display_name
                .as_ref()
                .unwrap_or(&agent.name)
                .to_string();
            let model_info = agent
                .model
                .as_ref()
                .map(|m| format!(" [{}]", m))
                .unwrap_or_default();
            let description = format!("{}{}", agent.description, model_info);

            let item =
                InteractiveItem::new(format!("project:{}", agent.name), display_name.clone())
                    .with_description(description);
            items.push(item);
        }
    }

    // Add global agents
    if !global_agents.is_empty() {
        let _current_category = Some(AgentCategory::Global);
        items.push(
            InteractiveItem::new(
                format!("__cat_{}", AgentCategory::Global.label()),
                AgentCategory::Global.label(),
            )
            .as_separator(),
        );

        for agent in &global_agents {
            let display_name = agent
                .display_name
                .as_ref()
                .unwrap_or(&agent.name)
                .to_string();
            let model_info = agent
                .model
                .as_ref()
                .map(|m| format!(" [{}]", m))
                .unwrap_or_default();
            let description = format!("{}{}", agent.description, model_info);

            let item = InteractiveItem::new(format!("global:{}", agent.name), display_name.clone())
                .with_description(description);
            items.push(item);
        }
    }

    // Add hint if no custom agents
    if project_agents.is_empty() && global_agents.is_empty() {
        items.push(InteractiveItem::new("__hint__", "").as_separator());
        items.push(
            InteractiveItem::new("__hint_no_custom__", "No custom agents found")
                .with_description("Press 'n' to create one")
                .with_disabled(true),
        );
    }

    // Calculate max_visible
    const UI_CHROME_HEIGHT: u16 = 6;
    let total_items = items.len();
    let max_visible = match terminal_height {
        Some(height) => {
            let available = height.saturating_sub(UI_CHROME_HEIGHT) as usize;
            available.clamp(8, total_items)
        }
        None => total_items.min(20),
    };

    InteractiveState::new("Agents", items, InteractiveAction::Custom("agents".into()))
        .with_search()
        .with_max_visible(max_visible)
        .with_hints(vec![
            ("↑↓".into(), "navigate".into()),
            ("Enter".into(), "select".into()),
            ("n".into(), "new agent".into()),
            ("Esc".into(), "close".into()),
        ])
}

/// Agent creation location
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum AgentLocation {
    /// Project-local (.cortex/agents/)
    #[default]
    Project,
    /// Global/user (~/.config/cortex/agents/)
    Global,
}

/// Agent creation method
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AgentCreationMethod {
    /// AI-assisted creation
    AI,
    /// Manual creation
    Manual,
}

/// Build an interactive state for agent creation - step 1: choose location
pub fn build_agent_location_selector() -> InteractiveState {
    let items = vec![
        InteractiveItem::new("project", "Project Agent")
            .with_description("Create in .cortex/agents/ - available only in this project")
            .with_shortcut('p'),
        InteractiveItem::new("global", "Global Agent")
            .with_description("Create in ~/.config/cortex/agents/ - available everywhere")
            .with_shortcut('g'),
    ];

    InteractiveState::new(
        "Where to create the agent?",
        items,
        InteractiveAction::Custom("agent_location".into()),
    )
    .with_hints(vec![
        ("p".into(), "project".into()),
        ("g".into(), "global".into()),
        ("Esc".into(), "cancel".into()),
    ])
}

/// Build an interactive state for agent creation - step 2: choose method
pub fn build_agent_method_selector(location: AgentLocation) -> InteractiveState {
    let location_str = match location {
        AgentLocation::Project => "project",
        AgentLocation::Global => "global",
    };

    let items = vec![
        InteractiveItem::new(format!("ai:{}", location_str), "AI-Assisted")
            .with_description("Describe what you want and AI will generate the agent configuration")
            .with_shortcut('a'),
        InteractiveItem::new(format!("manual:{}", location_str), "Manual")
            .with_description("Configure the agent settings manually")
            .with_shortcut('m'),
    ];

    InteractiveState::new(
        "How to create the agent?",
        items,
        InteractiveAction::Custom("agent_method".into()),
    )
    .with_hints(vec![
        ("a".into(), "AI-assisted".into()),
        ("m".into(), "manual".into()),
        ("Esc".into(), "cancel".into()),
    ])
}

/// Agent permission preset
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PermissionPreset {
    /// Read-only (no edit, no execute)
    ReadOnly,
    /// Standard (edit allowed, limited execute)
    Standard,
    /// Full access (all permissions)
    FullAccess,
    /// Custom (user-defined)
    Custom,
}

impl PermissionPreset {
    pub fn label(&self) -> &'static str {
        match self {
            Self::ReadOnly => "Read-only",
            Self::Standard => "Standard",
            Self::FullAccess => "Full Access",
            Self::Custom => "Custom",
        }
    }

    pub fn description(&self) -> &'static str {
        match self {
            Self::ReadOnly => "Can only read files, no modifications allowed",
            Self::Standard => "Can read and edit files, limited shell commands",
            Self::FullAccess => "All permissions enabled, including shell commands",
            Self::Custom => "Configure each permission individually",
        }
    }
}

/// Build an interactive state for permission selection during agent creation
pub fn build_permission_selector(suggested: Option<PermissionPreset>) -> InteractiveState {
    let items = vec![
        InteractiveItem::new("readonly", "Read-only")
            .with_description(PermissionPreset::ReadOnly.description())
            .with_current(suggested == Some(PermissionPreset::ReadOnly))
            .with_shortcut('r'),
        InteractiveItem::new("standard", "Standard")
            .with_description(PermissionPreset::Standard.description())
            .with_current(suggested == Some(PermissionPreset::Standard))
            .with_shortcut('s'),
        InteractiveItem::new("full", "Full Access")
            .with_description(PermissionPreset::FullAccess.description())
            .with_current(suggested == Some(PermissionPreset::FullAccess))
            .with_shortcut('f'),
        InteractiveItem::new("custom", "Custom")
            .with_description(PermissionPreset::Custom.description())
            .with_current(suggested == Some(PermissionPreset::Custom))
            .with_shortcut('c'),
    ];

    InteractiveState::new(
        "Select Permissions",
        items,
        InteractiveAction::Custom("agent_permissions".into()),
    )
    .with_hints(vec![
        ("r".into(), "read-only".into()),
        ("s".into(), "standard".into()),
        ("f".into(), "full".into()),
        ("Esc".into(), "cancel".into()),
    ])
}

/// Configuration for a new agent (used after AI generation or manual input)
#[derive(Debug, Clone, Default)]
pub struct NewAgentConfig {
    /// Agent name (identifier)
    pub name: String,
    /// Display name
    pub display_name: Option<String>,
    /// Description
    pub description: String,
    /// System prompt
    pub prompt: String,
    /// Model override
    pub model: Option<String>,
    /// Temperature
    pub temperature: Option<f32>,
    /// Permission preset
    pub permission_preset: Option<PermissionPreset>,
    /// Location (project or global)
    pub location: AgentLocation,
    /// Whether it's a subagent
    pub is_subagent: bool,
}

impl NewAgentConfig {
    pub fn new(name: &str, location: AgentLocation) -> Self {
        Self {
            name: name.to_string(),
            location,
            ..Default::default()
        }
    }

    /// Generate TOML content for the agent
    pub fn to_toml(&self) -> String {
        let mut content = String::new();

        content.push_str(&format!("name = \"{}\"\n", self.name));

        if let Some(ref dn) = self.display_name {
            content.push_str(&format!("display_name = \"{}\"\n", dn));
        }

        if !self.description.is_empty() {
            content.push_str(&format!("description = \"{}\"\n", self.description));
        }

        if let Some(ref model) = self.model {
            content.push_str(&format!("model = \"{}\"\n", model));
        }

        if let Some(temp) = self.temperature {
            content.push_str(&format!("temperature = {}\n", temp));
        }

        if self.is_subagent {
            content.push_str("mode = \"subagent\"\n");
        }

        // Permission section
        content.push_str("\n[permission]\n");
        match self.permission_preset {
            Some(PermissionPreset::ReadOnly) => {
                content.push_str("edit = \"deny\"\n");
                content.push_str("bash = \"deny\"\n");
            }
            Some(PermissionPreset::Standard) => {
                content.push_str("edit = \"allow\"\n");
                content.push_str("bash = \"ask\"\n");
            }
            Some(PermissionPreset::FullAccess) | None => {
                content.push_str("edit = \"allow\"\n");
                content.push_str("bash = \"allow\"\n");
            }
            Some(PermissionPreset::Custom) => {
                // Will be configured separately
                content.push_str("edit = \"ask\"\n");
                content.push_str("bash = \"ask\"\n");
            }
        }

        // Prompt section (if provided)
        if !self.prompt.is_empty() {
            content.push_str(&format!("\nprompt = '''\n{}\n'''\n", self.prompt));
        }

        content
    }

    /// Get the file path for this agent
    pub fn file_path(&self) -> std::path::PathBuf {
        let dir = match self.location {
            AgentLocation::Project => std::env::current_dir()
                .unwrap_or_default()
                .join(".cortex/agents"),
            AgentLocation::Global => dirs::config_dir().unwrap_or_default().join("cortex/agents"),
        };

        dir.join(format!("{}.toml", self.name))
    }
}

/// Build an inline form for AI-assisted agent creation description.
///
/// This form stays in the interactive TUI and allows the user to type
/// their agent description directly.
pub fn build_agent_ai_description_form(location: AgentLocation) -> InlineFormState {
    let location_str = match location {
        AgentLocation::Project => "project",
        AgentLocation::Global => "global",
    };

    InlineFormState::new(
        "Describe Your Agent",
        format!("agent-ai-create:{}", location_str),
    )
    .with_field(
        InlineFormField::new("description", "Description")
            .required()
            .with_placeholder("Describe what you want the agent to do..."),
    )
}

/// Build an interactive state for confirming a new agent configuration
pub fn build_agent_confirm_selector(config: &NewAgentConfig) -> InteractiveState {
    let mut items = Vec::new();

    // Show configuration summary
    items.push(
        InteractiveItem::new("__info_name__", format!("Name: {}", config.name)).as_separator(),
    );

    if let Some(ref dn) = config.display_name {
        items.push(
            InteractiveItem::new("__info_display__", format!("Display: {}", dn)).as_separator(),
        );
    }

    if !config.description.is_empty() {
        let desc = if config.description.len() > 50 {
            format!("{}...", &config.description[..47])
        } else {
            config.description.clone()
        };
        items.push(
            InteractiveItem::new("__info_desc__", format!("Description: {}", desc)).as_separator(),
        );
    }

    let perm_str = config
        .permission_preset
        .map(|p| p.label())
        .unwrap_or("Full Access");
    items.push(
        InteractiveItem::new("__info_perm__", format!("Permissions: {}", perm_str)).as_separator(),
    );

    let loc_str = match config.location {
        AgentLocation::Project => ".cortex/agents/",
        AgentLocation::Global => "~/.config/cortex/agents/",
    };
    items.push(
        InteractiveItem::new("__info_loc__", format!("Location: {}", loc_str)).as_separator(),
    );

    items.push(InteractiveItem::new("__sep__", "─────────────────────────────").as_separator());

    // Action items
    items.push(
        InteractiveItem::new("confirm", "Create Agent")
            .with_description("Save the agent configuration")
            .with_shortcut('y'),
    );

    items.push(
        InteractiveItem::new("edit_permissions", "Edit Permissions")
            .with_description("Modify the permission settings")
            .with_shortcut('p'),
    );

    items.push(
        InteractiveItem::new("cancel", "Cancel")
            .with_description("Discard and go back")
            .with_shortcut('n'),
    );

    InteractiveState::new(
        "Confirm Agent Creation",
        items,
        InteractiveAction::Custom("agent_confirm".into()),
    )
    .with_hints(vec![
        ("y".into(), "create".into()),
        ("p".into(), "permissions".into()),
        ("n".into(), "cancel".into()),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_agents_selector() {
        let state = build_agents_selector(None, None);
        assert!(!state.items.is_empty());
        assert_eq!(state.title, "Agents");

        // Should have create action at top
        assert_eq!(state.items[0].id, "__create__");
    }

    #[test]
    fn test_build_agent_location_selector() {
        let state = build_agent_location_selector();
        assert_eq!(state.items.len(), 2);
        assert_eq!(state.items[0].id, "project");
        assert_eq!(state.items[1].id, "global");
    }

    #[test]
    fn test_build_agent_method_selector() {
        let state = build_agent_method_selector(AgentLocation::Project);
        assert_eq!(state.items.len(), 2);
        assert!(state.items[0].id.starts_with("ai:"));
        assert!(state.items[1].id.starts_with("manual:"));
    }

    #[test]
    fn test_new_agent_config_to_toml() {
        let mut config = NewAgentConfig::new("test-agent", AgentLocation::Project);
        config.description = "A test agent".to_string();
        config.permission_preset = Some(PermissionPreset::ReadOnly);

        let toml = config.to_toml();
        assert!(toml.contains("name = \"test-agent\""));
        assert!(toml.contains("description = \"A test agent\""));
        assert!(toml.contains("edit = \"deny\""));
    }

    #[test]
    fn test_agent_display_info() {
        let builtin = AgentDisplayInfo::builtin("test", "description", true);
        assert_eq!(builtin.category, AgentCategory::Builtin);
        assert!(builtin.is_subagent);

        let project = AgentDisplayInfo::project("test", "description", "/path/to/agent.toml");
        assert_eq!(project.category, AgentCategory::Project);
        assert!(project.source.is_some());
    }

    #[test]
    fn test_build_agent_ai_description_form() {
        // Test for project location
        let form = build_agent_ai_description_form(AgentLocation::Project);
        assert_eq!(form.title, "Describe Your Agent");
        assert_eq!(form.action_id, "agent-ai-create:project");
        assert_eq!(form.fields.len(), 1);
        assert_eq!(form.fields[0].name, "description");
        assert!(form.fields[0].required);

        // Test for global location
        let form_global = build_agent_ai_description_form(AgentLocation::Global);
        assert_eq!(form_global.action_id, "agent-ai-create:global");
    }
}
