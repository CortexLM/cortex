//! Handler for the `agent remove` command.

use anyhow::{Context, Result, bail};
use std::io::{self, BufRead, Write};

use crate::agent_cmd::cli::RemoveArgs;
use crate::agent_cmd::loader::load_all_agents;

/// Remove agent command.
pub async fn run_remove(args: RemoveArgs) -> Result<()> {
    let agents = load_all_agents()?;

    let agent = agents
        .iter()
        .find(|a| a.name == args.name)
        .ok_or_else(|| anyhow::anyhow!("Agent '{}' not found", args.name))?;

    if agent.native {
        bail!(
            "Cannot remove built-in agent '{}'.\n\n\
            Built-in agents are part of the Cortex core and cannot be removed.\n\
            Alternative options:\n\
            1. Create a custom agent to replace it: cortex agent create my-{}\n\
            2. Disable specific tools on the built-in agent by creating a wrapper\n\
            3. Use a different agent with: cortex run --agent <other-agent>",
            args.name,
            args.name
        );
    }

    let path = agent
        .path
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Agent '{}' has no file path", args.name))?;

    if !args.force {
        print!(
            "Remove agent '{}' from {}? [y/N]: ",
            args.name,
            path.display()
        );
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().lock().read_line(&mut input)?;

        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Cancelled.");
            return Ok(());
        }
    }

    // Remove the file or directory
    if path.is_dir() {
        std::fs::remove_dir_all(path)
            .with_context(|| format!("Failed to remove directory: {}", path.display()))?;
    } else {
        std::fs::remove_file(path)
            .with_context(|| format!("Failed to remove file: {}", path.display()))?;
    }

    println!("Agent '{}' removed.", args.name);

    Ok(())
}
