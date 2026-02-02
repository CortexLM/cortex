//! Handler for the `agent export` command.

use anyhow::{Context, Result};

use crate::agent_cmd::cli::ExportArgs;
use crate::agent_cmd::loader::load_all_agents;
use crate::agent_cmd::types::AgentInfo;

/// Export an agent definition to stdout or a file.
pub async fn run_export(args: ExportArgs) -> Result<()> {
    let agents = load_all_agents()?;

    let agent = agents
        .iter()
        .find(|a| a.name == args.name)
        .ok_or_else(|| anyhow::anyhow!("Agent '{}' not found", args.name))?;

    let output = if args.json {
        // Export as JSON
        serde_json::to_string_pretty(agent)?
    } else {
        // Export as markdown with YAML frontmatter
        export_agent_to_markdown(agent)
    };

    // Write to file or stdout
    if let Some(ref output_path) = args.output {
        std::fs::write(output_path, &output)
            .with_context(|| format!("Failed to write to: {}", output_path.display()))?;
        eprintln!(
            "Agent '{}' exported to: {}",
            args.name,
            output_path.display()
        );
    } else {
        print!("{}", output);
    }

    Ok(())
}

/// Export an agent to markdown format with YAML frontmatter.
fn export_agent_to_markdown(agent: &AgentInfo) -> String {
    let mut frontmatter = format!(
        r#"---
name: {}
"#,
        agent.name
    );

    if let Some(ref desc) = agent.description {
        frontmatter.push_str(&format!("description: \"{}\"\n", desc.replace('"', "\\\"")));
    }

    frontmatter.push_str(&format!("mode: {}\n", agent.mode));

    if let Some(ref display_name) = agent.display_name {
        frontmatter.push_str(&format!("display_name: \"{}\"\n", display_name));
    }

    if let Some(temp) = agent.temperature {
        frontmatter.push_str(&format!("temperature: {}\n", temp));
    }

    if let Some(top_p) = agent.top_p {
        frontmatter.push_str(&format!("top_p: {}\n", top_p));
    }

    if let Some(ref model) = agent.model {
        frontmatter.push_str(&format!("model: {}\n", model));
    }

    if let Some(ref color) = agent.color {
        frontmatter.push_str(&format!("color: \"{}\"\n", color));
    }

    if let Some(ref allowed) = agent.allowed_tools {
        frontmatter.push_str("allowed_tools:\n");
        for tool in allowed {
            frontmatter.push_str(&format!("  - {}\n", tool));
        }
    }

    if !agent.denied_tools.is_empty() {
        frontmatter.push_str("denied_tools:\n");
        for tool in &agent.denied_tools {
            frontmatter.push_str(&format!("  - {}\n", tool));
        }
    }

    if !agent.tags.is_empty() {
        frontmatter.push_str("tags:\n");
        for tag in &agent.tags {
            frontmatter.push_str(&format!("  - {}\n", tag));
        }
    }

    frontmatter.push_str(&format!("can_delegate: {}\n", agent.can_delegate));

    if let Some(max_turns) = agent.max_turns {
        frontmatter.push_str(&format!("max_turns: {}\n", max_turns));
    }

    frontmatter.push_str(&format!("hidden: {}\n", agent.hidden));
    frontmatter.push_str("---\n\n");

    if let Some(ref prompt) = agent.prompt {
        frontmatter.push_str(prompt);
        frontmatter.push('\n');
    }

    frontmatter
}
