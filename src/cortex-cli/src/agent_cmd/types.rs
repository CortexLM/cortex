//! Agent type definitions.
//!
//! Contains core data structures for agent management.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Agent operation mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AgentMode {
    /// Primary agent (user-facing).
    #[default]
    Primary,
    /// Sub-agent (invoked by other agents).
    Subagent,
    /// Available as both primary and sub-agent.
    All,
}

impl std::fmt::Display for AgentMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AgentMode::Primary => write!(f, "primary"),
            AgentMode::Subagent => write!(f, "subagent"),
            AgentMode::All => write!(f, "all"),
        }
    }
}

impl std::str::FromStr for AgentMode {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "primary" => Ok(AgentMode::Primary),
            "subagent" | "sub" => Ok(AgentMode::Subagent),
            "all" | "both" => Ok(AgentMode::All),
            _ => Err(format!("Invalid mode: {s}. Use: primary, subagent, or all")),
        }
    }
}

/// Source of an agent definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentSource {
    /// Built-in agent.
    Builtin,
    /// Personal agent from ~/.cortex/agents/.
    Personal,
    /// Project agent from .cortex/agents/.
    Project,
}

impl std::fmt::Display for AgentSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Builtin => write!(f, "builtin"),
            Self::Personal => write!(f, "personal"),
            Self::Project => write!(f, "project"),
        }
    }
}

fn default_can_delegate() -> bool {
    true
}

/// Agent frontmatter configuration (from YAML in markdown files).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentFrontmatter {
    /// Agent name (unique identifier).
    pub name: String,
    /// Description of what this agent does.
    #[serde(default)]
    pub description: Option<String>,
    /// Agent mode.
    #[serde(default)]
    pub mode: AgentMode,
    /// Model to use (overrides default).
    #[serde(default)]
    pub model: Option<String>,
    /// Temperature setting.
    #[serde(default)]
    pub temperature: Option<f32>,
    /// Top-p setting.
    #[serde(default)]
    pub top_p: Option<f32>,
    /// Maximum tokens for response.
    #[serde(default)]
    pub max_tokens: Option<u32>,
    /// Allowed tools (None means all tools).
    #[serde(default, alias = "allowed-tools")]
    pub allowed_tools: Option<Vec<String>>,
    /// Denied tools.
    #[serde(default, alias = "denied-tools")]
    pub denied_tools: Vec<String>,
    /// Tags for categorization.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Whether agent can spawn sub-agents.
    #[serde(default = "default_can_delegate")]
    pub can_delegate: bool,
    /// Maximum number of turns.
    #[serde(default)]
    pub max_turns: Option<u32>,
    /// Display name (for UI).
    #[serde(default, alias = "display-name")]
    pub display_name: Option<String>,
    /// Color for UI (hex).
    #[serde(default)]
    pub color: Option<String>,
    /// Whether agent is hidden from UI.
    #[serde(default)]
    pub hidden: bool,
    /// Additional tools configuration (tool_name -> enabled).
    #[serde(default)]
    pub tools: HashMap<String, bool>,
}

/// A loaded agent definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentInfo {
    /// Agent name.
    pub name: String,
    /// Display name.
    pub display_name: Option<String>,
    /// Description.
    pub description: Option<String>,
    /// Agent mode.
    pub mode: AgentMode,
    /// Whether this is a built-in agent.
    pub native: bool,
    /// Whether agent is hidden from UI.
    pub hidden: bool,
    /// Custom system prompt.
    pub prompt: Option<String>,
    /// Temperature for generation.
    pub temperature: Option<f32>,
    /// Top-P for generation.
    pub top_p: Option<f32>,
    /// Color for UI (hex).
    pub color: Option<String>,
    /// Model override.
    pub model: Option<String>,
    /// Tools configuration (tool_name -> enabled).
    /// Empty means no tool-specific overrides.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub tools: HashMap<String, bool>,
    /// Allowed tools.
    pub allowed_tools: Option<Vec<String>>,
    /// Denied tools.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub denied_tools: Vec<String>,
    /// Maximum turns.
    pub max_turns: Option<u32>,
    /// Can delegate to sub-agents.
    pub can_delegate: bool,
    /// Tags.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Source of the agent.
    pub source: AgentSource,
    /// Path to agent definition file.
    pub path: Option<PathBuf>,
}

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // AgentMode tests
    // =========================================================================

    #[test]
    fn test_agent_mode_default() {
        let mode = AgentMode::default();
        assert_eq!(mode, AgentMode::Primary);
    }

    #[test]
    fn test_agent_mode_display() {
        assert_eq!(AgentMode::Primary.to_string(), "primary");
        assert_eq!(AgentMode::Subagent.to_string(), "subagent");
        assert_eq!(AgentMode::All.to_string(), "all");
    }

    #[test]
    fn test_agent_mode_from_str_valid() {
        assert_eq!("primary".parse::<AgentMode>().unwrap(), AgentMode::Primary);
        assert_eq!(
            "subagent".parse::<AgentMode>().unwrap(),
            AgentMode::Subagent
        );
        assert_eq!("sub".parse::<AgentMode>().unwrap(), AgentMode::Subagent);
        assert_eq!("all".parse::<AgentMode>().unwrap(), AgentMode::All);
        assert_eq!("both".parse::<AgentMode>().unwrap(), AgentMode::All);
    }

    #[test]
    fn test_agent_mode_from_str_case_insensitive() {
        assert_eq!("PRIMARY".parse::<AgentMode>().unwrap(), AgentMode::Primary);
        assert_eq!(
            "SUBAGENT".parse::<AgentMode>().unwrap(),
            AgentMode::Subagent
        );
        assert_eq!("SUB".parse::<AgentMode>().unwrap(), AgentMode::Subagent);
        assert_eq!("ALL".parse::<AgentMode>().unwrap(), AgentMode::All);
        assert_eq!("Both".parse::<AgentMode>().unwrap(), AgentMode::All);
    }

    #[test]
    fn test_agent_mode_from_str_invalid() {
        assert!("invalid".parse::<AgentMode>().is_err());
        assert!("".parse::<AgentMode>().is_err());
        assert!("prima".parse::<AgentMode>().is_err());
    }

    #[test]
    fn test_agent_mode_serialize() {
        let json = serde_json::to_string(&AgentMode::Primary).unwrap();
        assert_eq!(json, "\"primary\"");

        let json = serde_json::to_string(&AgentMode::Subagent).unwrap();
        assert_eq!(json, "\"subagent\"");

        let json = serde_json::to_string(&AgentMode::All).unwrap();
        assert_eq!(json, "\"all\"");
    }

    #[test]
    fn test_agent_mode_deserialize() {
        let mode: AgentMode = serde_json::from_str("\"primary\"").unwrap();
        assert_eq!(mode, AgentMode::Primary);

        let mode: AgentMode = serde_json::from_str("\"subagent\"").unwrap();
        assert_eq!(mode, AgentMode::Subagent);

        let mode: AgentMode = serde_json::from_str("\"all\"").unwrap();
        assert_eq!(mode, AgentMode::All);
    }

    #[test]
    fn test_agent_mode_equality() {
        assert_eq!(AgentMode::Primary, AgentMode::Primary);
        assert_ne!(AgentMode::Primary, AgentMode::Subagent);
        assert_ne!(AgentMode::Subagent, AgentMode::All);
    }

    #[test]
    fn test_agent_mode_clone() {
        let mode = AgentMode::Subagent;
        let cloned = mode;
        assert_eq!(mode, cloned);
    }

    #[test]
    fn test_agent_mode_copy() {
        let mode = AgentMode::All;
        let copied = mode;
        assert_eq!(mode, copied);
    }

    // =========================================================================
    // AgentSource tests
    // =========================================================================

    #[test]
    fn test_agent_source_display() {
        assert_eq!(AgentSource::Builtin.to_string(), "builtin");
        assert_eq!(AgentSource::Personal.to_string(), "personal");
        assert_eq!(AgentSource::Project.to_string(), "project");
    }

    #[test]
    fn test_agent_source_serialize() {
        let json = serde_json::to_string(&AgentSource::Builtin).unwrap();
        assert_eq!(json, "\"builtin\"");

        let json = serde_json::to_string(&AgentSource::Personal).unwrap();
        assert_eq!(json, "\"personal\"");

        let json = serde_json::to_string(&AgentSource::Project).unwrap();
        assert_eq!(json, "\"project\"");
    }

    #[test]
    fn test_agent_source_deserialize() {
        let source: AgentSource = serde_json::from_str("\"builtin\"").unwrap();
        assert_eq!(source, AgentSource::Builtin);

        let source: AgentSource = serde_json::from_str("\"personal\"").unwrap();
        assert_eq!(source, AgentSource::Personal);

        let source: AgentSource = serde_json::from_str("\"project\"").unwrap();
        assert_eq!(source, AgentSource::Project);
    }

    #[test]
    fn test_agent_source_equality() {
        assert_eq!(AgentSource::Builtin, AgentSource::Builtin);
        assert_ne!(AgentSource::Builtin, AgentSource::Personal);
        assert_ne!(AgentSource::Personal, AgentSource::Project);
    }

    // =========================================================================
    // AgentFrontmatter tests
    // =========================================================================

    #[test]
    fn test_agent_frontmatter_minimal_deserialize() {
        let yaml = r#"
name: test-agent
"#;
        let frontmatter: AgentFrontmatter = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(frontmatter.name, "test-agent");
        assert!(frontmatter.description.is_none());
        assert_eq!(frontmatter.mode, AgentMode::Primary);
        assert!(frontmatter.model.is_none());
        assert!(frontmatter.temperature.is_none());
        assert!(frontmatter.allowed_tools.is_none());
        assert!(frontmatter.denied_tools.is_empty());
        assert!(frontmatter.tags.is_empty());
        assert!(frontmatter.can_delegate); // default is true
        assert!(!frontmatter.hidden);
    }

    #[test]
    fn test_agent_frontmatter_full_deserialize() {
        let yaml = "
name: full-agent
description: A fully configured agent
mode: subagent
model: gpt-4o
temperature: 0.7
top_p: 0.9
max_tokens: 4096
allowed_tools:
  - read
  - write
denied_tools:
  - execute
tags:
  - coding
  - review
can_delegate: false
max_turns: 10
display_name: Full Agent
color: '#FF5733'
hidden: true
";
        let frontmatter: AgentFrontmatter = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(frontmatter.name, "full-agent");
        assert_eq!(
            frontmatter.description,
            Some("A fully configured agent".to_string())
        );
        assert_eq!(frontmatter.mode, AgentMode::Subagent);
        assert_eq!(frontmatter.model, Some("gpt-4o".to_string()));
        assert_eq!(frontmatter.temperature, Some(0.7));
        assert_eq!(frontmatter.top_p, Some(0.9));
        assert_eq!(frontmatter.max_tokens, Some(4096));
        assert_eq!(
            frontmatter.allowed_tools,
            Some(vec!["read".to_string(), "write".to_string()])
        );
        assert_eq!(frontmatter.denied_tools, vec!["execute".to_string()]);
        assert_eq!(
            frontmatter.tags,
            vec!["coding".to_string(), "review".to_string()]
        );
        assert!(!frontmatter.can_delegate);
        assert_eq!(frontmatter.max_turns, Some(10));
        assert_eq!(frontmatter.display_name, Some("Full Agent".to_string()));
        assert_eq!(frontmatter.color, Some("#FF5733".to_string()));
        assert!(frontmatter.hidden);
    }

    #[test]
    fn test_agent_frontmatter_allowed_tools_alias() {
        let yaml = r#"
name: test
allowed-tools:
  - tool1
  - tool2
"#;
        let frontmatter: AgentFrontmatter = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(
            frontmatter.allowed_tools,
            Some(vec!["tool1".to_string(), "tool2".to_string()])
        );
    }

    #[test]
    fn test_agent_frontmatter_denied_tools_alias() {
        let yaml = r#"
name: test
denied-tools:
  - dangerous_tool
"#;
        let frontmatter: AgentFrontmatter = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(frontmatter.denied_tools, vec!["dangerous_tool".to_string()]);
    }

    #[test]
    fn test_agent_frontmatter_display_name_alias() {
        let yaml = r#"
name: test
display-name: Test Display Name
"#;
        let frontmatter: AgentFrontmatter = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(
            frontmatter.display_name,
            Some("Test Display Name".to_string())
        );
    }

    #[test]
    fn test_agent_frontmatter_tools_map() {
        let yaml = r#"
name: test
tools:
  read: true
  write: false
  execute: true
"#;
        let frontmatter: AgentFrontmatter = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(frontmatter.tools.get("read"), Some(&true));
        assert_eq!(frontmatter.tools.get("write"), Some(&false));
        assert_eq!(frontmatter.tools.get("execute"), Some(&true));
    }

    // =========================================================================
    // AgentInfo tests
    // =========================================================================

    #[test]
    fn test_agent_info_serialize() {
        let info = AgentInfo {
            name: "test-agent".to_string(),
            display_name: Some("Test Agent".to_string()),
            description: Some("A test agent".to_string()),
            mode: AgentMode::Primary,
            native: true,
            hidden: false,
            prompt: Some("You are a helpful assistant.".to_string()),
            temperature: Some(0.7),
            top_p: None,
            color: Some("#00FF00".to_string()),
            model: Some("gpt-4o".to_string()),
            tools: HashMap::new(),
            allowed_tools: None,
            denied_tools: vec![],
            max_turns: Some(20),
            can_delegate: true,
            tags: vec!["test".to_string()],
            source: AgentSource::Builtin,
            path: None,
        };

        let json = serde_json::to_string(&info).expect("Should serialize");
        assert!(json.contains("test-agent"));
        assert!(json.contains("Test Agent"));
        assert!(json.contains("primary"));
        assert!(json.contains("builtin"));
    }

    #[test]
    fn test_agent_info_deserialize() {
        let json = r##"{
            "name": "deserialized-agent",
            "display_name": null,
            "description": "An agent from JSON",
            "mode": "subagent",
            "native": false,
            "hidden": true,
            "prompt": "Be helpful.",
            "temperature": 0.5,
            "top_p": 0.95,
            "color": "#FF0000",
            "model": "claude-sonnet",
            "allowed_tools": ["read", "write"],
            "denied_tools": ["delete"],
            "max_turns": 15,
            "can_delegate": false,
            "tags": ["utility"],
            "source": "project",
            "path": "/path/to/agent.md"
        }"##;

        let info: AgentInfo = serde_json::from_str(json).expect("Should deserialize");
        assert_eq!(info.name, "deserialized-agent");
        assert!(info.display_name.is_none());
        assert_eq!(info.description, Some("An agent from JSON".to_string()));
        assert_eq!(info.mode, AgentMode::Subagent);
        assert!(!info.native);
        assert!(info.hidden);
        assert_eq!(info.prompt, Some("Be helpful.".to_string()));
        assert_eq!(info.temperature, Some(0.5));
        assert_eq!(info.top_p, Some(0.95));
        assert_eq!(info.color, Some("#FF0000".to_string()));
        assert_eq!(info.model, Some("claude-sonnet".to_string()));
        assert_eq!(
            info.allowed_tools,
            Some(vec!["read".to_string(), "write".to_string()])
        );
        assert_eq!(info.denied_tools, vec!["delete".to_string()]);
        assert_eq!(info.max_turns, Some(15));
        assert!(!info.can_delegate);
        assert_eq!(info.tags, vec!["utility".to_string()]);
        assert_eq!(info.source, AgentSource::Project);
        assert_eq!(info.path, Some(PathBuf::from("/path/to/agent.md")));
    }

    #[test]
    fn test_agent_info_skip_serializing_empty() {
        let info = AgentInfo {
            name: "minimal".to_string(),
            display_name: None,
            description: None,
            mode: AgentMode::Primary,
            native: false,
            hidden: false,
            prompt: None,
            temperature: None,
            top_p: None,
            color: None,
            model: None,
            tools: HashMap::new(), // empty, should be skipped
            allowed_tools: None,
            denied_tools: vec![], // empty, should be skipped
            max_turns: None,
            can_delegate: true,
            tags: vec![], // empty, should be skipped
            source: AgentSource::Personal,
            path: None,
        };

        let json = serde_json::to_string(&info).expect("Should serialize");
        // Empty collections should be skipped
        assert!(!json.contains("\"tools\""));
        assert!(!json.contains("\"denied_tools\""));
        assert!(!json.contains("\"tags\""));
    }

    #[test]
    fn test_agent_info_roundtrip() {
        let original = AgentInfo {
            name: "roundtrip-test".to_string(),
            display_name: Some("Roundtrip Test".to_string()),
            description: Some("Testing roundtrip".to_string()),
            mode: AgentMode::All,
            native: true,
            hidden: false,
            prompt: Some("System prompt".to_string()),
            temperature: Some(0.8),
            top_p: Some(0.9),
            color: Some("#AABBCC".to_string()),
            model: Some("test-model".to_string()),
            tools: {
                let mut m = HashMap::new();
                m.insert("tool1".to_string(), true);
                m
            },
            allowed_tools: Some(vec!["allowed".to_string()]),
            denied_tools: vec!["denied".to_string()],
            max_turns: Some(25),
            can_delegate: false,
            tags: vec!["tag1".to_string(), "tag2".to_string()],
            source: AgentSource::Builtin,
            path: Some(PathBuf::from("/test/path.md")),
        };

        let json = serde_json::to_string(&original).expect("Should serialize");
        let deserialized: AgentInfo = serde_json::from_str(&json).expect("Should deserialize");

        assert_eq!(original.name, deserialized.name);
        assert_eq!(original.display_name, deserialized.display_name);
        assert_eq!(original.description, deserialized.description);
        assert_eq!(original.mode, deserialized.mode);
        assert_eq!(original.native, deserialized.native);
        assert_eq!(original.hidden, deserialized.hidden);
        assert_eq!(original.prompt, deserialized.prompt);
        assert_eq!(original.temperature, deserialized.temperature);
        assert_eq!(original.top_p, deserialized.top_p);
        assert_eq!(original.color, deserialized.color);
        assert_eq!(original.model, deserialized.model);
        assert_eq!(original.allowed_tools, deserialized.allowed_tools);
        assert_eq!(original.denied_tools, deserialized.denied_tools);
        assert_eq!(original.max_turns, deserialized.max_turns);
        assert_eq!(original.can_delegate, deserialized.can_delegate);
        assert_eq!(original.tags, deserialized.tags);
        assert_eq!(original.source, deserialized.source);
        assert_eq!(original.path, deserialized.path);
    }

    // =========================================================================
    // default_can_delegate tests
    // =========================================================================

    #[test]
    fn test_default_can_delegate() {
        assert!(default_can_delegate());
    }
}
