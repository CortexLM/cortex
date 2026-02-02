//! Handler for the `agent copy` command.

use anyhow::{Context, Result, bail};

use crate::agent_cmd::cli::CopyArgs;
use crate::agent_cmd::loader::{
    get_agents_dir, load_all_agents, parse_frontmatter, read_file_with_encoding,
};

/// Copy/clone an existing agent with a new name.
pub async fn run_copy(args: CopyArgs) -> Result<()> {
    let agents = load_all_agents()?;

    // Find the source agent
    let source_agent = agents
        .iter()
        .find(|a| a.name == args.source)
        .ok_or_else(|| anyhow::anyhow!("Agent '{}' not found", args.source))?;

    // Validate destination name
    if args.destination.trim().is_empty() {
        bail!("Destination agent name cannot be empty");
    }

    if !args
        .destination
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        bail!("Agent name must contain only alphanumeric characters, hyphens, and underscores");
    }

    // Check if destination already exists
    let dest_exists = agents.iter().any(|a| a.name == args.destination);
    if dest_exists && !args.force {
        bail!(
            "Agent '{}' already exists. Use --force to overwrite.",
            args.destination
        );
    }

    // Get the agents directory
    let agents_dir = get_agents_dir()?;
    std::fs::create_dir_all(&agents_dir)?;

    let dest_file = agents_dir.join(format!("{}.md", args.destination));

    // Generate the agent content
    let content = if source_agent.native {
        // For built-in agents, create a new file from scratch
        let mut frontmatter = format!(
            r#"---
name: {}
description: "{}"
mode: {}
"#,
            args.destination,
            source_agent
                .description
                .as_ref()
                .map(|d| format!("Copy of {}: {}", args.source, d))
                .unwrap_or_else(|| format!("Copy of {} agent", args.source)),
            source_agent.mode
        );

        if let Some(temp) = source_agent.temperature {
            frontmatter.push_str(&format!("temperature: {}\n", temp));
        }

        if let Some(ref model) = source_agent.model {
            frontmatter.push_str(&format!("model: {}\n", model));
        }

        if let Some(ref color) = source_agent.color {
            frontmatter.push_str(&format!("color: \"{}\"\n", color));
        }

        if let Some(ref allowed) = source_agent.allowed_tools {
            frontmatter.push_str("allowed_tools:\n");
            for tool in allowed {
                frontmatter.push_str(&format!("  - {}\n", tool));
            }
        }

        if !source_agent.denied_tools.is_empty() {
            frontmatter.push_str("denied_tools:\n");
            for tool in &source_agent.denied_tools {
                frontmatter.push_str(&format!("  - {}\n", tool));
            }
        }

        frontmatter.push_str(&format!("can_delegate: {}\n", source_agent.can_delegate));
        frontmatter.push_str("---\n\n");

        if let Some(ref prompt) = source_agent.prompt {
            frontmatter.push_str(prompt);
            frontmatter.push('\n');
        }

        frontmatter
    } else if let Some(ref path) = source_agent.path {
        // For custom agents, read the file and update the name
        let content = read_file_with_encoding(path)?;
        let (mut fm, body) = parse_frontmatter(&content)?;
        fm.name = args.destination.clone();

        // Rebuild the file
        let yaml = serde_yaml::to_string(&fm)?;
        format!("---\n{}---\n\n{}\n", yaml, body)
    } else {
        bail!("Agent '{}' has no source file", args.source);
    };

    // Write the new agent file
    std::fs::write(&dest_file, &content)
        .with_context(|| format!("Failed to write agent file: {}", dest_file.display()))?;

    println!("Agent '{}' copied to '{}'", args.source, args.destination);
    println!("   Location: {}", dest_file.display());
    println!();
    println!(
        "   Use 'cortex agent show {}' to view details.",
        args.destination
    );

    Ok(())
}
