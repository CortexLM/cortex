//! Built-in /commands command.
//!
//! Lists all available custom commands with their descriptions.

use std::path::Path;

use thiserror::Error;

use crate::loader::CommandLoader;
use crate::registry::CommandRegistry;

/// Errors for the commands command.
#[derive(Debug, Error)]
pub enum CommandsError {
    /// Loader error.
    #[error("Loader error: {0}")]
    Loader(#[from] crate::loader::LoaderError),
}

/// Result of executing the /commands command.
#[derive(Debug)]
pub struct CommandsResult {
    /// List of command info.
    pub commands: Vec<CommandInfo>,
    /// Whether there are project-local commands.
    pub has_project_commands: bool,
    /// Whether there are global commands.
    pub has_global_commands: bool,
}

/// Information about a command for display.
#[derive(Debug, Clone)]
pub struct CommandInfo {
    /// Command name.
    pub name: String,
    /// Description.
    pub description: String,
    /// Whether it's a project-local command.
    pub is_project_local: bool,
    /// Source path.
    pub source: String,
    /// Hints/placeholders.
    pub hints: Vec<String>,
}

/// The /commands built-in command.
#[derive(Debug, Default)]
pub struct CommandsCommand;

impl CommandsCommand {
    /// Create a new /commands command.
    pub fn new() -> Self {
        Self
    }

    /// Execute the command synchronously.
    pub fn execute(&self, project_path: Option<&Path>) -> Result<CommandsResult, CommandsError> {
        let mut loader = CommandLoader::new();

        // Add project-local path if provided
        if let Some(project) = project_path {
            loader.prepend_dir(project.join(".cortex/commands"));
        }

        // Load commands synchronously
        let commands = crate::sync::load_all();

        let project_dir = project_path.map(|p| p.join(".cortex/commands"));
        let global_dir = dirs::config_dir().map(|d| d.join("cortex/commands"));

        let mut has_project = false;
        let mut has_global = false;

        let command_infos: Vec<CommandInfo> = commands
            .iter()
            .map(|cmd| {
                let source_parent = cmd.source_path.parent();
                let is_project = project_dir
                    .as_ref()
                    .is_some_and(|pd| source_parent == Some(pd.as_path()));

                if is_project {
                    has_project = true;
                } else if global_dir
                    .as_ref()
                    .is_some_and(|gd| source_parent == Some(gd.as_path()))
                {
                    has_global = true;
                }

                CommandInfo {
                    name: cmd.name.clone(),
                    description: cmd.description().to_string(),
                    is_project_local: is_project,
                    source: cmd.source_path.display().to_string(),
                    hints: cmd.hints(),
                }
            })
            .collect();

        Ok(CommandsResult {
            commands: command_infos,
            has_project_commands: has_project,
            has_global_commands: has_global,
        })
    }

    /// Execute the command asynchronously.
    pub async fn execute_async(
        &self,
        project_path: Option<&Path>,
    ) -> Result<CommandsResult, CommandsError> {
        let mut loader = CommandLoader::new();

        if let Some(project) = project_path {
            loader.prepend_dir(project.join(".cortex/commands"));
        }

        let registry = CommandRegistry::load_default().await?;

        let project_dir = project_path.map(|p| p.join(".cortex/commands"));
        let global_dir = dirs::config_dir().map(|d| d.join("cortex/commands"));

        let mut has_project = false;
        let mut has_global = false;

        let command_infos: Vec<CommandInfo> = registry
            .list()
            .iter()
            .map(|cmd| {
                let source_parent = cmd.source_path.parent();
                let is_project = project_dir
                    .as_ref()
                    .is_some_and(|pd| source_parent == Some(pd.as_path()));

                if is_project {
                    has_project = true;
                } else if global_dir
                    .as_ref()
                    .is_some_and(|gd| source_parent == Some(gd.as_path()))
                {
                    has_global = true;
                }

                CommandInfo {
                    name: cmd.name.clone(),
                    description: cmd.description().to_string(),
                    is_project_local: is_project,
                    source: cmd.source_path.display().to_string(),
                    hints: cmd.hints(),
                }
            })
            .collect();

        Ok(CommandsResult {
            commands: command_infos,
            has_project_commands: has_project,
            has_global_commands: has_global,
        })
    }

    /// Format the result for display.
    pub fn format_result(&self, result: &CommandsResult) -> String {
        let mut output = String::new();

        if result.commands.is_empty() {
            output.push_str("No custom commands found.\n\n");
            output.push_str("Create commands in:\n");
            output.push_str("  - .cortex/commands/  (project-local)\n");
            output.push_str("  - ~/.config/cortex/commands/  (global)\n");
            return output;
        }

        output.push_str(&format!(
            "Found {} custom command(s):\n\n",
            result.commands.len()
        ));

        // Group by project/global
        if result.has_project_commands {
            output.push_str("Project Commands:\n");
            for cmd in result.commands.iter().filter(|c| c.is_project_local) {
                output.push_str(&format!("  /{:<16} {}\n", cmd.name, cmd.description));
            }
            output.push('\n');
        }

        if result.has_global_commands {
            output.push_str("Global Commands:\n");
            for cmd in result.commands.iter().filter(|c| !c.is_project_local) {
                output.push_str(&format!("  /{:<16} {}\n", cmd.name, cmd.description));
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
    fn test_commands_command_new() {
        let cmd = CommandsCommand::new();
        assert_eq!(std::mem::size_of_val(&cmd), 0); // Zero-sized type
    }

    #[test]
    fn test_format_empty_result() {
        let cmd = CommandsCommand::new();
        let result = CommandsResult {
            commands: vec![],
            has_project_commands: false,
            has_global_commands: false,
        };

        let output = cmd.format_result(&result);
        assert!(output.contains("No custom commands found"));
    }

    #[test]
    fn test_format_with_commands() {
        let cmd = CommandsCommand::new();
        let result = CommandsResult {
            commands: vec![
                CommandInfo {
                    name: "build".to_string(),
                    description: "Build the project".to_string(),
                    is_project_local: true,
                    source: "/project/.cortex/commands/build.md".to_string(),
                    hints: vec!["$1".to_string()],
                },
                CommandInfo {
                    name: "deploy".to_string(),
                    description: "Deploy to server".to_string(),
                    is_project_local: false,
                    source: "~/.config/cortex/commands/deploy.md".to_string(),
                    hints: vec![],
                },
            ],
            has_project_commands: true,
            has_global_commands: true,
        };

        let output = cmd.format_result(&result);
        assert!(output.contains("Found 2 custom command(s)"));
        assert!(output.contains("Project Commands"));
        assert!(output.contains("Global Commands"));
        assert!(output.contains("/build"));
        assert!(output.contains("/deploy"));
    }

    #[test]
    fn test_execute_empty_directory() {
        let temp = TempDir::new().unwrap();
        let cmd = CommandsCommand::new();

        // Create empty command directories
        std::fs::create_dir_all(temp.path().join(".cortex/commands")).unwrap();

        let result = cmd.execute(Some(temp.path())).unwrap();
        assert!(result.commands.is_empty());
    }
}
