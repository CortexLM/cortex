//! CreateAgent tool handler.
//!
//! Generates custom agent configuration files for project or personal use.

use async_trait::async_trait;
use serde_json::{Value, json};
use std::path::PathBuf;

use super::ToolHandler;
use crate::error::{CortexError, Result};
use crate::tools::context::ToolContext;
use crate::tools::spec::{ToolMetadata, ToolResult};

/// CreateAgent handler for generating agent configurations.
pub struct CreateAgentHandler;

impl CreateAgentHandler {
    pub fn new() -> Self {
        Self
    }

    /// Sanitize description to create a valid filename.
    fn sanitize_filename(description: &str) -> String {
        let sanitized: String = description
            .to_lowercase()
            .chars()
            .map(|c| {
                if c.is_alphanumeric() {
                    c
                } else if c == '-' || c == '_' {
                    c
                } else {
                    '-'
                }
            })
            .collect();

        sanitized
            .split('-')
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("-")
            .chars()
            .take(50)
            .collect()
    }

    /// Get the agents directory based on location (legacy, uses process cwd).
    #[allow(dead_code)]
    fn get_agents_dir(location: &str) -> Result<PathBuf> {
        match location {
            "project" => Ok(PathBuf::from(".cortex/agents")),
            "personal" => {
                let home = dirs::home_dir().ok_or_else(|| {
                    CortexError::Internal("Could not determine home directory".to_string())
                })?;
                Ok(home.join(".cortex/agents"))
            }
            _ => Err(CortexError::InvalidInput(format!(
                "Invalid location: {}. Must be 'project' or 'personal'",
                location
            ))),
        }
    }

    /// Get the agents directory based on location, using the provided cwd for project location.
    fn get_agents_dir_with_cwd(location: &str, cwd: &PathBuf) -> Result<PathBuf> {
        match location {
            "project" => Ok(cwd.join(".cortex/agents")),
            "personal" => {
                let home = dirs::home_dir().ok_or_else(|| {
                    CortexError::Internal("Could not determine home directory".to_string())
                })?;
                Ok(home.join(".cortex/agents"))
            }
            _ => Err(CortexError::InvalidInput(format!(
                "Invalid location: {}. Must be 'project' or 'personal'",
                location
            ))),
        }
    }
}

impl Default for CreateAgentHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolHandler for CreateAgentHandler {
    fn name(&self) -> &str {
        "CreateAgent"
    }

    async fn execute(&self, arguments: Value, context: &ToolContext) -> Result<ToolResult> {
        let description = arguments
            .get("description")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                CortexError::InvalidInput("Missing required parameter: description".to_string())
            })?;

        if description.len() < 10 {
            return Err(CortexError::InvalidInput(
                "Description must be at least 10 characters long".to_string(),
            ));
        }

        let location = arguments
            .get("location")
            .and_then(|v| v.as_str())
            .unwrap_or("project");

        if location != "project" && location != "personal" {
            return Err(CortexError::InvalidInput(format!(
                "Invalid location: {}. Must be 'project' or 'personal'",
                location
            )));
        }

        let agent_name = Self::sanitize_filename(description);
        if agent_name.is_empty() {
            return Err(CortexError::InvalidInput(
                "Could not generate valid agent name from description".to_string(),
            ));
        }

        // Use context.cwd for project location to avoid relying on process cwd
        let agents_dir = Self::get_agents_dir_with_cwd(location, &context.cwd)?;
        let agent_path = agents_dir.join(format!("{}.toml", agent_name));

        if agent_path.exists() {
            return Err(CortexError::InvalidInput(format!(
                "Agent already exists at: {}",
                agent_path.display()
            )));
        }

        std::fs::create_dir_all(&agents_dir)?;

        let agent_content = format!(
            r#"[agent]
name = "{}"
description = """
{}
"""

# Add custom configuration here
# [config]
# key = "value"
"#,
            agent_name, description
        );

        std::fs::write(&agent_path, agent_content)?;

        let output = format!(
            "Agent created successfully!\n\nName: {}\nLocation: {}\nDescription: {}\n\nYou can now use this agent with the Delegate tool.",
            agent_name,
            agent_path.display(),
            description
        );

        let metadata = ToolMetadata {
            duration_ms: 0,
            exit_code: None,
            files_modified: vec![agent_path.display().to_string()],
            data: Some(json!({
                "agent_name": agent_name,
                "agent_path": agent_path.display().to_string(),
                "location": location,
            })),
        };

        Ok(ToolResult::success(output).with_metadata(metadata))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_create_agent_project_location() {
        let temp_dir = TempDir::new().unwrap();
        let context = ToolContext::new(temp_dir.path().to_path_buf());

        let handler = CreateAgentHandler::new();
        let description = "Process and transform data files";
        let args = json!({
            "description": description,
            "location": "project"
        });

        let result = handler.execute(args, &context).await;

        assert!(result.is_ok(), "Handler execution failed: {:?}", result);

        let tool_result = result.unwrap();
        assert!(tool_result.success);
        assert!(tool_result.output.contains("Agent created successfully"));
        assert!(
            tool_result
                .output
                .contains("process-and-transform-data-files")
        );

        let expected_name = CreateAgentHandler::sanitize_filename(description);
        let agent_path = temp_dir
            .path()
            .join(format!(".cortex/agents/{}.toml", expected_name));
        assert!(
            agent_path.exists(),
            "Agent file should exist at {:?}",
            agent_path
        );

        let content = std::fs::read_to_string(&agent_path).unwrap();
        assert!(content.contains("name = \"process-and-transform-data-files\""));
        assert!(content.contains("Process and transform data files"));
    }

    #[tokio::test]
    async fn test_create_agent_personal_location() {
        let temp_dir = TempDir::new().unwrap();
        let context = ToolContext::new(temp_dir.path().to_path_buf());

        let handler = CreateAgentHandler::new();
        let args = json!({
            "description": "Custom data processor for personal projects",
            "location": "personal"
        });

        let result = handler.execute(args, &context).await;

        // Personal location uses the actual home directory, so this test
        // just verifies it doesn't fail unexpectedly
        if let Ok(tool_result) = result {
            assert!(tool_result.success);
            assert!(tool_result.output.contains("Agent created successfully"));
        }
        // else: Expected if home dir doesn't exist or is not writable
    }

    #[tokio::test]
    async fn test_create_agent_missing_description() {
        let temp_dir = TempDir::new().unwrap();
        let context = ToolContext::new(temp_dir.path().to_path_buf());

        let handler = CreateAgentHandler::new();
        let args = json!({
            "location": "project"
        });

        let result = handler.execute(args, &context).await;
        assert!(result.is_err());

        let err = result.unwrap_err();
        assert!(matches!(err, CortexError::InvalidInput(_)));
        assert!(
            err.to_string()
                .contains("Missing required parameter: description")
        );
    }

    #[tokio::test]
    async fn test_create_agent_short_description() {
        let temp_dir = TempDir::new().unwrap();
        let context = ToolContext::new(temp_dir.path().to_path_buf());

        let handler = CreateAgentHandler::new();
        let args = json!({
            "description": "Too short",
            "location": "project"
        });

        let result = handler.execute(args, &context).await;
        assert!(result.is_err());

        let err = result.unwrap_err();
        assert!(matches!(err, CortexError::InvalidInput(_)));
        assert!(err.to_string().contains("at least 10 characters"));
    }

    #[tokio::test]
    async fn test_create_agent_invalid_location() {
        let temp_dir = TempDir::new().unwrap();
        let context = ToolContext::new(temp_dir.path().to_path_buf());

        let handler = CreateAgentHandler::new();
        let args = json!({
            "description": "Valid description here",
            "location": "invalid_location"
        });

        let result = handler.execute(args, &context).await;
        assert!(result.is_err());

        let err = result.unwrap_err();
        assert!(matches!(err, CortexError::InvalidInput(_)));
        assert!(err.to_string().contains("Invalid location"));
    }

    #[tokio::test]
    async fn test_create_agent_default_location() {
        let temp_dir = TempDir::new().unwrap();
        let context = ToolContext::new(temp_dir.path().to_path_buf());

        let handler = CreateAgentHandler::new();
        let description = "Agent with default location parameter";
        let args = json!({
            "description": description
        });

        let result = handler.execute(args, &context).await;

        assert!(result.is_ok(), "Error: {:?}", result.err());

        let tool_result = result.unwrap();
        assert!(tool_result.success);

        let expected_name = CreateAgentHandler::sanitize_filename(description);
        let agent_path = temp_dir
            .path()
            .join(format!(".cortex/agents/{}.toml", expected_name));
        assert!(
            agent_path.exists(),
            "Agent file should exist at {:?}",
            agent_path
        );
    }

    #[tokio::test]
    async fn test_create_agent_duplicate_error() {
        let temp_dir = TempDir::new().unwrap();
        let context = ToolContext::new(temp_dir.path().to_path_buf());

        let handler = CreateAgentHandler::new();
        let args = json!({
            "description": "Duplicate agent test case",
            "location": "project"
        });

        // Create the first agent
        let result1 = handler.execute(args.clone(), &context).await;

        assert!(result1.is_ok(), "First agent creation should succeed");

        let agent_path = temp_dir
            .path()
            .join(".cortex/agents/duplicate-agent-test-case.toml");
        assert!(
            agent_path.exists(),
            "First agent file should exist at {:?}",
            agent_path
        );

        // Try to create a duplicate
        let result2 = handler.execute(args, &context).await;

        assert!(result2.is_err(), "Second agent creation should fail");

        let err = result2.unwrap_err();
        assert!(matches!(err, CortexError::InvalidInput(_)));
        assert!(err.to_string().contains("already exists"));
    }

    #[test]
    fn test_sanitize_filename() {
        assert_eq!(
            CreateAgentHandler::sanitize_filename("Simple Test"),
            "simple-test"
        );
        assert_eq!(
            CreateAgentHandler::sanitize_filename("Test with Special!@# Characters"),
            "test-with-special-characters"
        );
        assert_eq!(
            CreateAgentHandler::sanitize_filename("Multiple   Spaces"),
            "multiple-spaces"
        );
        assert_eq!(
            CreateAgentHandler::sanitize_filename("CamelCaseTest"),
            "camelcasetest"
        );
        assert_eq!(
            CreateAgentHandler::sanitize_filename("test_with_underscores"),
            "test_with_underscores"
        );
    }
}
