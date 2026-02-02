//! Handler for the `agent list` command.

use anyhow::{Result, bail};

use crate::agent_cmd::cli::ListArgs;
use crate::agent_cmd::loader::load_all_agents;
use crate::agent_cmd::types::AgentMode;
use crate::agent_cmd::utils::matches_pattern;

/// List agents command.
pub async fn run_list(args: ListArgs) -> Result<()> {
    // Validate mutually exclusive flags
    if args.primary && args.subagents {
        bail!(
            "Cannot specify both --primary and --subagents. Choose one filter or use neither for all agents."
        );
    }

    // Handle --remote flag
    if args.remote {
        println!("Remote agent registry:");
        println!("{}", "-".repeat(50));
        println!("Note: Remote agent registry is not yet implemented.");
        println!("Agents can be shared via:");
        println!("  - GitHub repositories with .cortex/agents/ directory");
        println!("  - Cortex Hub (coming soon)");
        println!("\nFor now, use local agents from:");
        println!("  - ~/.cortex/agents/ (personal)");
        println!("  - .cortex/agents/ (project)");
        return Ok(());
    }

    let agents = load_all_agents()?;

    // Filter agents
    let mut filtered: Vec<_> = agents
        .iter()
        .filter(|a| {
            // Filter by visibility
            if !args.all && a.hidden {
                return false;
            }
            // Filter by mode
            if args.primary && !matches!(a.mode, AgentMode::Primary | AgentMode::All) {
                return false;
            }
            if args.subagents && !matches!(a.mode, AgentMode::Subagent | AgentMode::All) {
                return false;
            }
            // Filter by pattern
            if let Some(ref pattern) = args.filter
                && !matches_pattern(&a.name, pattern)
            {
                return false;
            }
            true
        })
        .collect();

    // Sort by display_name (if present) or name for user-friendly ordering
    // This ensures agents are listed in the order users expect (by visible name)
    filtered.sort_by(|a, b| {
        let name_a = a.display_name.as_ref().unwrap_or(&a.name);
        let name_b = b.display_name.as_ref().unwrap_or(&b.name);
        name_a.to_lowercase().cmp(&name_b.to_lowercase())
    });

    // Output names only for shell completion
    if args.names_only {
        for agent in &filtered {
            println!("{}", agent.name);
        }
        return Ok(());
    }

    if args.json {
        let json = serde_json::to_string_pretty(&filtered)?;
        println!("{json}");
        return Ok(());
    }

    if filtered.is_empty() {
        println!("No agents found.");
        if !args.all {
            println!("Use --all to include hidden agents.");
        }
        return Ok(());
    }

    println!(
        "{:<20} {:<10} {:<10} {:<40}",
        "Name", "Mode", "Source", "Description"
    );
    println!("{}", "-".repeat(82));

    for agent in &filtered {
        let desc = agent
            .description
            .as_ref()
            .map(|d| {
                if d.len() > 38 {
                    format!("{}...", &d[..35])
                } else {
                    d.clone()
                }
            })
            .unwrap_or_else(|| "-".to_string());

        let mode = match agent.mode {
            AgentMode::Primary => "primary",
            AgentMode::Subagent => "subagent",
            AgentMode::All => "all",
        };

        println!(
            "{:<20} {:<10} {:<10} {:<40}",
            agent.name, mode, agent.source, desc
        );
    }

    let primary_count = filtered
        .iter()
        .filter(|a| matches!(a.mode, AgentMode::Primary | AgentMode::All))
        .count();
    let subagent_count = filtered
        .iter()
        .filter(|a| matches!(a.mode, AgentMode::Subagent | AgentMode::All))
        .count();

    // Calculate hidden agents count
    let hidden_count = if !args.all {
        agents.len() - filtered.len()
    } else {
        0
    };

    if hidden_count > 0 {
        println!(
            "\nShowing {} agents ({} primary, {} subagents, {} hidden - use --all to show all)",
            filtered.len(),
            primary_count,
            subagent_count,
            hidden_count
        );
    } else {
        println!(
            "\nTotal: {} agents ({} primary, {} subagents)",
            filtered.len(),
            primary_count,
            subagent_count
        );
    }
    println!("\nUse 'Cortex Agent show <name>' for details.");

    Ok(())
}
