//! Custom agent configuration types.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Errors that can occur when working with custom agents.
#[derive(Debug, Error)]
pub enum CustomAgentError {
    /// IO error.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// YAML parsing error.
    #[error("YAML error: {0}")]
    Yaml(#[from] serde_yaml::Error),

    /// Invalid frontmatter.
    #[error("Invalid frontmatter: {0}")]
    InvalidFrontmatter(String),

    /// Invalid configuration.
    #[error("Invalid configuration: {0}")]
    InvalidConfig(String),

    /// Agent not found.
    #[error("Agent not found: {0}")]
    NotFound(String),
}

/// Configuration for a Custom Agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomAgentConfig {
    /// Unique name of the agent.
    #[serde(default)]
    pub name: String,

    /// Description for display.
    #[serde(default)]
    pub description: String,

    /// Model to use (or "inherit" for session model).
    #[serde(default = "default_model")]
    pub model: String,

    /// Effort of reasoning (low, medium, high).
    #[serde(default)]
    pub reasoning_effort: ReasoningEffort,

    /// Tools configuration (list or category).
    #[serde(default)]
    pub tools: ToolsConfig,

    /// System prompt (loaded from body).
    #[serde(skip)]
    pub prompt: String,

    /// Temperature for generation.
    #[serde(default)]
    pub temperature: Option<f32>,

    /// Maximum steps for this agent.
    #[serde(default)]
    pub max_steps: Option<usize>,

    /// Color for UI display (hex).
    #[serde(default)]
    pub color: Option<String>,

    /// Whether this agent is hidden from listings.
    #[serde(default)]
    pub hidden: bool,
}

fn default_model() -> String {
    "inherit".to_string()
}

impl Default for CustomAgentConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            description: String::new(),
            model: default_model(),
            reasoning_effort: ReasoningEffort::default(),
            tools: ToolsConfig::default(),
            prompt: String::new(),
            temperature: None,
            max_steps: None,
            color: None,
            hidden: false,
        }
    }
}

impl CustomAgentConfig {
    /// Create a new agent config with a name.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Default::default()
        }
    }

    /// Set the description.
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = description.into();
        self
    }

    /// Set the model.
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// Set the reasoning effort.
    pub fn with_reasoning_effort(mut self, effort: ReasoningEffort) -> Self {
        self.reasoning_effort = effort;
        self
    }

    /// Set the tools configuration.
    pub fn with_tools(mut self, tools: ToolsConfig) -> Self {
        self.tools = tools;
        self
    }

    /// Set the prompt.
    pub fn with_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.prompt = prompt.into();
        self
    }

    /// Set the temperature.
    pub fn with_temperature(mut self, temp: f32) -> Self {
        self.temperature = Some(temp);
        self
    }

    /// Set max steps.
    pub fn with_max_steps(mut self, steps: usize) -> Self {
        self.max_steps = Some(steps);
        self
    }

    /// Set the color.
    pub fn with_color(mut self, color: impl Into<String>) -> Self {
        self.color = Some(color.into());
        self
    }

    /// Mark as hidden.
    pub fn hidden(mut self) -> Self {
        self.hidden = true;
        self
    }

    /// Check if this agent inherits the model.
    pub fn inherits_model(&self) -> bool {
        self.model == "inherit" || self.model.is_empty()
    }
}

/// Reasoning effort level.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    /// Low effort - fast responses.
    Low,
    /// Medium effort - balanced.
    #[default]
    Medium,
    /// High effort - thorough analysis.
    High,
}

impl ReasoningEffort {
    /// Get the suggested temperature for this effort level.
    pub fn suggested_temperature(&self) -> f32 {
        match self {
            ReasoningEffort::Low => 0.3,
            ReasoningEffort::Medium => 0.5,
            ReasoningEffort::High => 0.7,
        }
    }

    /// Get the suggested max steps for this effort level.
    pub fn suggested_max_steps(&self) -> usize {
        match self {
            ReasoningEffort::Low => 10,
            ReasoningEffort::Medium => 20,
            ReasoningEffort::High => 30,
        }
    }
}

impl std::fmt::Display for ReasoningEffort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReasoningEffort::Low => write!(f, "low"),
            ReasoningEffort::Medium => write!(f, "medium"),
            ReasoningEffort::High => write!(f, "high"),
        }
    }
}

impl std::str::FromStr for ReasoningEffort {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "low" => Ok(ReasoningEffort::Low),
            "medium" => Ok(ReasoningEffort::Medium),
            "high" => Ok(ReasoningEffort::High),
            _ => Err(format!("Invalid reasoning effort: {}", s)),
        }
    }
}

/// Tool category for predefined tool sets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ToolCategory {
    /// Read-only tools: Read, LS, Grep, Glob.
    ReadOnly,
    /// Edit tools: Create, Edit, ApplyPatch.
    Edit,
    /// Execute tools: Execute (shell).
    Execute,
    /// Web tools: WebSearch, FetchUrl.
    Web,
    /// MCP dynamic tools.
    Mcp,
    /// All available tools.
    All,
}

impl ToolCategory {
    /// Get the tools in this category.
    pub fn tools(&self) -> Vec<&'static str> {
        match self {
            ToolCategory::ReadOnly => vec!["Read", "LS", "Grep", "Glob"],
            ToolCategory::Edit => vec!["Create", "Edit", "ApplyPatch"],
            ToolCategory::Execute => vec!["Execute"],
            ToolCategory::Web => vec!["WebSearch", "FetchUrl"],
            ToolCategory::Mcp => vec![], // Dynamic, filled at runtime
            ToolCategory::All => vec![
                "Read",
                "LS",
                "Grep",
                "Glob",
                "Create",
                "Edit",
                "ApplyPatch",
                "Execute",
                "WebSearch",
                "FetchUrl",
            ],
        }
    }
}

impl std::fmt::Display for ToolCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ToolCategory::ReadOnly => write!(f, "read-only"),
            ToolCategory::Edit => write!(f, "edit"),
            ToolCategory::Execute => write!(f, "execute"),
            ToolCategory::Web => write!(f, "web"),
            ToolCategory::Mcp => write!(f, "mcp"),
            ToolCategory::All => write!(f, "all"),
        }
    }
}

impl std::str::FromStr for ToolCategory {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().replace('_', "-").as_str() {
            "read-only" | "readonly" => Ok(ToolCategory::ReadOnly),
            "edit" => Ok(ToolCategory::Edit),
            "execute" => Ok(ToolCategory::Execute),
            "web" => Ok(ToolCategory::Web),
            "mcp" => Ok(ToolCategory::Mcp),
            "all" => Ok(ToolCategory::All),
            _ => Err(format!("Invalid tool category: {}", s)),
        }
    }
}

/// Tools configuration - either a list or a category.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ToolsConfig {
    /// Explicit list of tools.
    List(Vec<String>),
    /// Predefined category.
    Category(ToolCategory),
}

impl Default for ToolsConfig {
    fn default() -> Self {
        ToolsConfig::Category(ToolCategory::ReadOnly)
    }
}

impl ToolsConfig {
    /// Get the list of allowed tools.
    pub fn allowed_tools(&self) -> Vec<String> {
        match self {
            ToolsConfig::List(tools) => tools.clone(),
            ToolsConfig::Category(cat) => cat.tools().iter().map(|s| s.to_string()).collect(),
        }
    }

    /// Check if a tool is allowed.
    pub fn is_tool_allowed(&self, tool: &str) -> bool {
        match self {
            ToolsConfig::List(tools) => tools.iter().any(|t| t.eq_ignore_ascii_case(tool)),
            ToolsConfig::Category(cat) => cat.tools().iter().any(|t| t.eq_ignore_ascii_case(tool)),
        }
    }

    /// Create a read-only tools config.
    pub fn read_only() -> Self {
        ToolsConfig::Category(ToolCategory::ReadOnly)
    }

    /// Create an edit tools config.
    pub fn edit() -> Self {
        ToolsConfig::Category(ToolCategory::Edit)
    }

    /// Create an all-tools config.
    pub fn all() -> Self {
        ToolsConfig::Category(ToolCategory::All)
    }

    /// Create a custom tools list.
    pub fn custom(tools: impl IntoIterator<Item = impl Into<String>>) -> Self {
        ToolsConfig::List(tools.into_iter().map(Into::into).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_config_default() {
        let config = CustomAgentConfig::default();

        assert!(config.name.is_empty());
        assert_eq!(config.model, "inherit");
        assert_eq!(config.reasoning_effort, ReasoningEffort::Medium);
        assert!(config.inherits_model());
    }

    #[test]
    fn test_agent_config_builder() {
        let config = CustomAgentConfig::new("test-agent")
            .with_description("A test agent")
            .with_model("gpt-4")
            .with_reasoning_effort(ReasoningEffort::High)
            .with_tools(ToolsConfig::read_only())
            .with_temperature(0.7)
            .with_max_steps(25)
            .with_color("#ff0000");

        assert_eq!(config.name, "test-agent");
        assert_eq!(config.description, "A test agent");
        assert_eq!(config.model, "gpt-4");
        assert_eq!(config.reasoning_effort, ReasoningEffort::High);
        assert!(!config.inherits_model());
        assert_eq!(config.temperature, Some(0.7));
        assert_eq!(config.max_steps, Some(25));
        assert_eq!(config.color, Some("#ff0000".to_string()));
    }

    #[test]
    fn test_agent_config_parsing() {
        let yaml = r#"
name: test-agent
description: Test agent
model: claude-3.5
tools: read-only
"#;
        let config: CustomAgentConfig = serde_yaml::from_str(yaml).unwrap();

        assert_eq!(config.name, "test-agent");
        assert_eq!(config.model, "claude-3.5");
        assert!(matches!(
            config.tools,
            ToolsConfig::Category(ToolCategory::ReadOnly)
        ));
    }

    #[test]
    fn test_tool_category_tools() {
        let read_only = ToolCategory::ReadOnly;
        let tools = read_only.tools();

        assert!(tools.contains(&"Read"));
        assert!(tools.contains(&"Grep"));
        assert!(!tools.contains(&"Execute"));
        assert!(!tools.contains(&"Edit"));
    }

    #[test]
    fn test_tool_category_expansion() {
        let cat = ToolCategory::ReadOnly;
        let tools = cat.tools();

        assert!(tools.contains(&"Read"));
        assert!(!tools.contains(&"Execute"));
    }

    #[test]
    fn test_tools_config_allowed() {
        let config = ToolsConfig::read_only();
        let allowed = config.allowed_tools();

        assert!(allowed.contains(&"Read".to_string()));
        assert!(!allowed.contains(&"Execute".to_string()));
    }

    #[test]
    fn test_tools_config_is_allowed() {
        let config = ToolsConfig::read_only();

        assert!(config.is_tool_allowed("Read"));
        assert!(config.is_tool_allowed("read")); // Case insensitive
        assert!(!config.is_tool_allowed("Execute"));
    }

    #[test]
    fn test_tools_config_custom() {
        let config = ToolsConfig::custom(["Read", "Execute"]);
        let allowed = config.allowed_tools();

        assert_eq!(allowed.len(), 2);
        assert!(config.is_tool_allowed("Read"));
        assert!(config.is_tool_allowed("Execute"));
        assert!(!config.is_tool_allowed("Edit"));
    }

    #[test]
    fn test_reasoning_effort_from_str() {
        assert_eq!(
            "low".parse::<ReasoningEffort>().unwrap(),
            ReasoningEffort::Low
        );
        assert_eq!(
            "MEDIUM".parse::<ReasoningEffort>().unwrap(),
            ReasoningEffort::Medium
        );
        assert_eq!(
            "High".parse::<ReasoningEffort>().unwrap(),
            ReasoningEffort::High
        );
        assert!("invalid".parse::<ReasoningEffort>().is_err());
    }

    #[test]
    fn test_tool_category_from_str() {
        assert_eq!(
            "read-only".parse::<ToolCategory>().unwrap(),
            ToolCategory::ReadOnly
        );
        assert_eq!(
            "readonly".parse::<ToolCategory>().unwrap(),
            ToolCategory::ReadOnly
        );
        assert_eq!("edit".parse::<ToolCategory>().unwrap(), ToolCategory::Edit);
        assert_eq!("all".parse::<ToolCategory>().unwrap(), ToolCategory::All);
        assert!("invalid".parse::<ToolCategory>().is_err());
    }

    #[test]
    fn test_reasoning_effort_suggestions() {
        assert!(
            ReasoningEffort::Low.suggested_temperature()
                < ReasoningEffort::High.suggested_temperature()
        );
        assert!(
            ReasoningEffort::Low.suggested_max_steps()
                < ReasoningEffort::High.suggested_max_steps()
        );
    }
}
