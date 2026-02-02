//! Built-in /agents command.
//!
//! Lists all available custom agents (subagents) with their descriptions.

use std::path::Path;

use thiserror::Error;

/// Errors for the agents command.
#[derive(Debug, Error)]
pub enum AgentsError {
    /// IO error.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// YAML parsing error.
    #[error("YAML error: {0}")]
    Yaml(#[from] serde_yaml::Error),
}

/// Result of executing the /agents command.
#[derive(Debug)]
pub struct AgentsResult {
    /// List of custom agent info.
    pub agents: Vec<CustomAgentInfo>,
    /// Whether there are project-local agents.
    pub has_project_agents: bool,
    /// Whether there are global agents.
    pub has_global_agents: bool,
    /// Built-in agents.
    pub builtin_agents: Vec<BuiltinAgentInfo>,
}

/// Information about a custom agent for display.
#[derive(Debug, Clone)]
pub struct CustomAgentInfo {
    /// Agent name.
    pub name: String,
    /// Description.
    pub description: String,
    /// Model (or "inherit").
    pub model: String,
    /// Reasoning effort level.
    pub reasoning_effort: String,
    /// Tools configuration.
    pub tools: String,
    /// Whether it's a project-local agent.
    pub is_project_local: bool,
    /// Source path.
    pub source: String,
}

/// Information about a built-in agent.
#[derive(Debug, Clone)]
pub struct BuiltinAgentInfo {
    /// Agent name.
    pub name: String,
    /// Description.
    pub description: String,
    /// Whether it's a subagent.
    pub is_subagent: bool,
}

/// The /agents built-in command.
#[derive(Debug, Default)]
pub struct AgentsCommand;

impl AgentsCommand {
    /// Create a new /agents command.
    pub fn new() -> Self {
        Self
    }

    /// Execute the command synchronously.
    pub fn execute(&self, project_path: Option<&Path>) -> Result<AgentsResult, AgentsError> {
        let mut agents = Vec::new();
        let mut has_project = false;
        let mut has_global = false;

        // Search paths
        let project_dir = project_path.map(|p| p.join(".cortex/agents"));
        let global_dir = dirs::config_dir().map(|d| d.join("cortex/agents"));

        // Load from project directory
        if let Some(ref dir) = project_dir
            && dir.exists()
            && let Ok(project_agents) = self.load_from_dir(dir)
        {
            has_project = !project_agents.is_empty();
            for mut agent in project_agents {
                agent.is_project_local = true;
                agents.push(agent);
            }
        }

        // Load from global directory
        if let Some(ref dir) = global_dir
            && dir.exists()
            && let Ok(global_agents) = self.load_from_dir(dir)
        {
            for agent in global_agents {
                // Skip if already loaded from project
                if !agents.iter().any(|d| d.name == agent.name) {
                    has_global = true;
                    agents.push(agent);
                }
            }
        }

        // Built-in agents
        let builtin_agents = vec![
            BuiltinAgentInfo {
                name: "general".to_string(),
                description: "General-purpose agent for complex tasks".to_string(),
                is_subagent: true,
            },
            BuiltinAgentInfo {
                name: "explore".to_string(),
                description: "Fast agent for codebase exploration".to_string(),
                is_subagent: true,
            },
            BuiltinAgentInfo {
                name: "research".to_string(),
                description: "Research agent for thorough investigation".to_string(),
                is_subagent: true,
            },
        ];

        Ok(AgentsResult {
            agents,
            has_project_agents: has_project,
            has_global_agents: has_global,
            builtin_agents,
        })
    }

    /// Load custom agents from a directory.
    fn load_from_dir(&self, dir: &Path) -> Result<Vec<CustomAgentInfo>, AgentsError> {
        let mut agents = Vec::new();

        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.extension().is_some_and(|e| e == "md")
                && let Ok(agent) = self.load_agent_info(&path)
            {
                agents.push(agent);
            }
        }

        Ok(agents)
    }

    /// Load custom agent info from a file.
    fn load_agent_info(&self, path: &Path) -> Result<CustomAgentInfo, AgentsError> {
        let content = std::fs::read_to_string(path)?;

        // Parse frontmatter
        let (name, description, model, reasoning_effort, tools) =
            self.parse_agent_frontmatter(&content, path)?;

        Ok(CustomAgentInfo {
            name,
            description,
            model,
            reasoning_effort,
            tools,
            is_project_local: false,
            source: path.display().to_string(),
        })
    }

    /// Parse custom agent frontmatter for display info.
    fn parse_agent_frontmatter(
        &self,
        content: &str,
        path: &Path,
    ) -> Result<(String, String, String, String, String), AgentsError> {
        let content = content.trim();

        // Default values
        let default_name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unnamed")
            .to_string();

        if !content.starts_with("---") {
            return Ok((
                default_name,
                String::new(),
                "inherit".to_string(),
                "medium".to_string(),
                "read-only".to_string(),
            ));
        }

        // Find closing delimiter
        let rest = &content[3..];
        if let Some(end_pos) = rest.find("\n---") {
            let yaml_str = &rest[..end_pos];
            let yaml: serde_yaml::Value = serde_yaml::from_str(yaml_str)?;

            let name = yaml
                .get("name")
                .and_then(|v| v.as_str())
                .map(String::from)
                .unwrap_or(default_name);

            let description = yaml
                .get("description")
                .and_then(|v| v.as_str())
                .map(String::from)
                .unwrap_or_default();

            let model = yaml
                .get("model")
                .and_then(|v| v.as_str())
                .map(String::from)
                .unwrap_or_else(|| "inherit".to_string());

            let reasoning_effort = yaml
                .get("reasoning_effort")
                .and_then(|v| v.as_str())
                .map(String::from)
                .unwrap_or_else(|| "medium".to_string());

            let tools = yaml
                .get("tools")
                .map(|v| match v {
                    serde_yaml::Value::String(s) => s.clone(),
                    serde_yaml::Value::Sequence(seq) => {
                        let items: Vec<String> = seq
                            .iter()
                            .filter_map(|i| i.as_str().map(String::from))
                            .collect();
                        format!("[{}]", items.join(", "))
                    }
                    _ => "read-only".to_string(),
                })
                .unwrap_or_else(|| "read-only".to_string());

            Ok((name, description, model, reasoning_effort, tools))
        } else {
            Ok((
                default_name,
                String::new(),
                "inherit".to_string(),
                "medium".to_string(),
                "read-only".to_string(),
            ))
        }
    }

    /// Format the result for display.
    pub fn format_result(&self, result: &AgentsResult) -> String {
        let mut output = String::new();

        // Built-in agents
        output.push_str("Built-in Agents:\n");
        for agent in &result.builtin_agents {
            let suffix = if agent.is_subagent { " (subagent)" } else { "" };
            output.push_str(&format!(
                "  @{:<16} {}{}\n",
                agent.name, agent.description, suffix
            ));
        }
        output.push('\n');

        if result.agents.is_empty() {
            output.push_str("No custom agents found.\n\n");
            output.push_str("Create agents in:\n");
            output.push_str("  - .cortex/agents/  (project-local)\n");
            output.push_str("  - ~/.config/cortex/agents/  (global)\n");
            return output;
        }

        output.push_str(&format!(
            "Found {} custom agent(s):\n\n",
            result.agents.len()
        ));

        // Group by project/global
        if result.has_project_agents {
            output.push_str("Project Agents:\n");
            for agent in result.agents.iter().filter(|d| d.is_project_local) {
                output.push_str(&format!(
                    "  @{:<16} {} ({})\n",
                    agent.name, agent.description, agent.tools
                ));
            }
            output.push('\n');
        }

        if result.has_global_agents {
            output.push_str("Global Agents:\n");
            for agent in result.agents.iter().filter(|d| !d.is_project_local) {
                output.push_str(&format!(
                    "  @{:<16} {} ({})\n",
                    agent.name, agent.description, agent.tools
                ));
            }
        }

        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_agents_command_new() {
        let cmd = AgentsCommand::new();
        assert_eq!(std::mem::size_of_val(&cmd), 0);
    }

    #[test]
    fn test_format_empty_result() {
        let cmd = AgentsCommand::new();
        let result = AgentsResult {
            agents: vec![],
            has_project_agents: false,
            has_global_agents: false,
            builtin_agents: vec![],
        };

        let output = cmd.format_result(&result);
        assert!(output.contains("No custom agents found"));
    }

    #[test]
    fn test_format_with_agents() {
        let cmd = AgentsCommand::new();
        let result = AgentsResult {
            agents: vec![
                CustomAgentInfo {
                    name: "reviewer".to_string(),
                    description: "Code reviewer".to_string(),
                    model: "gpt-4".to_string(),
                    reasoning_effort: "high".to_string(),
                    tools: "read-only".to_string(),
                    is_project_local: true,
                    source: "/project/.cortex/agents/reviewer.md".to_string(),
                },
                CustomAgentInfo {
                    name: "helper".to_string(),
                    description: "General helper".to_string(),
                    model: "inherit".to_string(),
                    reasoning_effort: "medium".to_string(),
                    tools: "all".to_string(),
                    is_project_local: false,
                    source: "~/.config/cortex/agents/helper.md".to_string(),
                },
            ],
            has_project_agents: true,
            has_global_agents: true,
            builtin_agents: vec![BuiltinAgentInfo {
                name: "general".to_string(),
                description: "General purpose".to_string(),
                is_subagent: true,
            }],
        };

        let output = cmd.format_result(&result);
        assert!(output.contains("Found 2 custom agent(s)"));
        assert!(output.contains("Project Agents"));
        assert!(output.contains("Global Agents"));
        assert!(output.contains("@reviewer"));
        assert!(output.contains("@helper"));
        assert!(output.contains("Built-in Agents"));
        assert!(output.contains("@general"));
    }

    #[test]
    fn test_execute_with_agents() {
        let temp = TempDir::new().unwrap();
        let agents_dir = temp.path().join(".cortex/agents");
        std::fs::create_dir_all(&agents_dir).unwrap();

        std::fs::write(
            agents_dir.join("test-agent.md"),
            r#"---
name: test-agent
description: A test agent
model: gpt-4
reasoning_effort: high
tools: read-only
---

You are a test agent."#,
        )
        .unwrap();

        let cmd = AgentsCommand::new();
        let result = cmd.execute(Some(temp.path())).unwrap();

        assert_eq!(result.agents.len(), 1);
        assert!(result.has_project_agents);

        let agent = &result.agents[0];
        assert_eq!(agent.name, "test-agent");
        assert_eq!(agent.description, "A test agent");
        assert_eq!(agent.model, "gpt-4");
        assert_eq!(agent.reasoning_effort, "high");
        assert_eq!(agent.tools, "read-only");
    }

    #[test]
    fn test_parse_agent_frontmatter_no_yaml() {
        let cmd = AgentsCommand::new();
        let content = "Just a prompt without frontmatter.";
        let path = Path::new("/test/my-agent.md");

        let (name, desc, model, effort, tools) =
            cmd.parse_agent_frontmatter(content, path).unwrap();

        assert_eq!(name, "my-agent");
        assert!(desc.is_empty());
        assert_eq!(model, "inherit");
        assert_eq!(effort, "medium");
        assert_eq!(tools, "read-only");
    }
}
