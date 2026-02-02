//! Handler for the `agent edit` command.

use anyhow::{Context, Result, bail};
use std::io::{self, BufRead, Write};

use crate::agent_cmd::cli::EditArgs;
use crate::agent_cmd::loader::{load_all_agents, parse_frontmatter};

/// Edit agent command.
///
/// Opens the agent file in the user's default editor, then validates the file
/// after editing. If validation fails, offers to re-open the editor to fix issues.
pub async fn run_edit(args: EditArgs) -> Result<()> {
    let agents = load_all_agents()?;

    let agent = agents
        .iter()
        .find(|a| a.name == args.name)
        .ok_or_else(|| anyhow::anyhow!("Agent '{}' not found", args.name))?;

    if agent.native {
        bail!(
            "Cannot edit built-in agent '{}'.\n\n\
            Built-in agents are part of the Cortex core and cannot be modified.\n\
            To customize this agent, create a copy:\n\
            cortex agent create my-{}",
            args.name,
            args.name
        );
    }

    let path = agent
        .path
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Agent '{}' has no file path", args.name))?;

    // Determine the editor to use
    let editor = args
        .editor
        .or_else(|| std::env::var("VISUAL").ok())
        .or_else(|| std::env::var("EDITOR").ok())
        .unwrap_or_else(|| {
            if cfg!(windows) {
                "notepad".to_string()
            } else {
                "vi".to_string()
            }
        });

    // Make a backup of the original file
    let backup_content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read agent file: {}", path.display()))?;

    loop {
        // Open the editor
        println!("Opening {} in {}...", path.display(), editor);
        let status = std::process::Command::new(&editor)
            .arg(path)
            .status()
            .with_context(|| format!("Failed to launch editor: {}", editor))?;

        if !status.success() {
            bail!("Editor exited with error");
        }

        // Read and validate the edited file
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read edited file: {}", path.display()))?;

        // Try to parse the frontmatter to validate
        match parse_frontmatter(&content) {
            Ok((frontmatter, _body)) => {
                // Validate required fields
                if frontmatter.name.trim().is_empty() {
                    eprintln!("\nError: Agent name cannot be empty.");
                } else if !frontmatter
                    .name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
                {
                    eprintln!(
                        "\nError: Agent name must contain only alphanumeric characters, hyphens, and underscores."
                    );
                } else {
                    // Validation passed
                    println!("\nAgent '{}' updated successfully!", frontmatter.name);
                    return Ok(());
                }
            }
            Err(e) => {
                eprintln!("\nError: Invalid agent configuration: {}", e);
            }
        }

        // Validation failed - offer to re-edit or rollback
        print!(
            "Would you like to (e)dit again, (r)ollback to original, or (k)eep invalid file? [e/r/k]: "
        );
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().lock().read_line(&mut input)?;

        match input.trim().to_lowercase().as_str() {
            "r" | "rollback" => {
                // Restore the backup
                std::fs::write(path, &backup_content)
                    .with_context(|| format!("Failed to restore backup: {}", path.display()))?;
                println!("Rolled back to original version.");
                return Ok(());
            }
            "k" | "keep" => {
                eprintln!("Warning: Keeping invalid configuration. The agent may fail to load.");
                return Ok(());
            }
            _ => {
                // Default: re-edit
                continue;
            }
        }
    }
}
