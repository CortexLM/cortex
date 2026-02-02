//! Handler for the `agent show` command.

use anyhow::Result;

use crate::agent_cmd::cli::ShowArgs;
use crate::agent_cmd::loader::load_all_agents;
use crate::agent_cmd::utils::format_color_preview;

/// Show agent details command.
pub async fn run_show(args: ShowArgs) -> Result<()> {
    let agents = load_all_agents()?;

    let agent = agents
        .iter()
        .find(|a| a.name == args.name)
        .ok_or_else(|| anyhow::anyhow!("Agent '{}' not found", args.name))?;

    // Warn if the agent is hidden
    if agent.hidden {
        eprintln!(
            "Note: '{}' is a hidden agent (not shown in default listings).",
            agent.name
        );
        eprintln!();
    }

    if args.json {
        // Create a custom JSON value to show "builtin" instead of null for native agents
        let mut json_value = serde_json::to_value(agent)?;
        if let serde_json::Value::Object(ref mut map) = json_value {
            if agent.native && agent.path.is_none() {
                map.insert(
                    "path".to_string(),
                    serde_json::Value::String("builtin".to_string()),
                );
            }
            // Add model override if provided via --model flag
            if let Some(ref model_override) = args.model {
                map.insert(
                    "model_override".to_string(),
                    serde_json::Value::String(model_override.clone()),
                );
            }
        }
        let json = serde_json::to_string_pretty(&json_value)?;
        println!("{json}");
        return Ok(());
    }

    println!("Agent: {}", agent.name);
    println!("{}", "=".repeat(40));

    if let Some(ref display_name) = agent.display_name {
        println!("Display Name: {display_name}");
    }

    if let Some(ref desc) = agent.description {
        println!("Description: {desc}");
    }

    println!("Mode: {}", agent.mode);
    println!("Source: {}", agent.source);
    println!(
        "Native: {} ({})",
        if agent.native { "yes" } else { "no" },
        if agent.native {
            "built-in agent bundled with Cortex"
        } else {
            "user-defined agent"
        }
    );
    println!("Hidden: {}", agent.hidden);
    println!(
        "Can Delegate: {} ({})",
        if agent.can_delegate { "Yes" } else { "No" },
        if agent.can_delegate {
            "can spawn sub-agents for parallel tasks"
        } else {
            "runs tasks sequentially"
        }
    );

    // Show model - either from --model override or agent's configured model
    if let Some(ref model_override) = args.model {
        println!("Model: {model_override} (override via --model)");
    } else if let Some(ref model) = agent.model {
        println!("Model: {model}");
    }

    if let Some(temp) = agent.temperature {
        println!("Temperature: {temp}");
    }

    if let Some(top_p) = agent.top_p {
        println!("Top-P: {top_p}");
    }

    if let Some(max_turns) = agent.max_turns {
        println!("Max Turns: {max_turns}");
    }

    if let Some(ref color) = agent.color {
        // Display color with preview (colored block using ANSI escape codes)
        let color_preview = format_color_preview(color);
        println!("Color: {color} {color_preview}");
    }

    if !agent.tags.is_empty() {
        println!("Tags: {}", agent.tags.join(", "));
    }

    if let Some(ref allowed) = agent.allowed_tools {
        println!("Allowed Tools: {}", allowed.join(", "));
    }

    if !agent.denied_tools.is_empty() {
        println!("Denied Tools: {}", agent.denied_tools.join(", "));
    }

    if !agent.tools.is_empty() {
        println!("Tools Configuration:");
        for (tool, enabled) in &agent.tools {
            println!(
                "  {tool}: {}",
                if *enabled { "enabled" } else { "disabled" }
            );
        }
    }

    match &agent.path {
        Some(path) => println!("Path: {}", path.display()),
        None if agent.native => println!("Path: (builtin)"),
        None => {} // Non-native with no path shouldn't happen, but handle gracefully
    }

    if let Some(ref prompt) = agent.prompt {
        println!("\nSystem Prompt:");
        println!("{}", "-".repeat(40));
        let preview = if prompt.len() > 500 {
            format!(
                "{}...\n\n(truncated, {} chars total)",
                &prompt[..500],
                prompt.len()
            )
        } else {
            prompt.clone()
        };
        // Display each line with proper indentation to handle multi-line prompts
        for line in preview.lines() {
            println!("  {}", line);
        }
    }

    Ok(())
}
