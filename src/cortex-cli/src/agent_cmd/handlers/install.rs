//! Handler for the `agent install` command.

use anyhow::{Context, Result, bail};

use crate::agent_cmd::cli::InstallArgs;
use crate::agent_cmd::loader::get_agents_dir;

/// Install agent from registry.
pub async fn run_install(args: InstallArgs) -> Result<()> {
    let registry_url = args
        .registry
        .as_deref()
        .unwrap_or("https://registry.cortex.foundation");

    println!("Installing agent '{}' from registry...", args.name);
    println!("   Registry: {}", registry_url);

    // Construct the agent download URL
    let agent_url = format!("{}/agents/{}.md", registry_url, args.name);

    // Download the agent file
    println!("   Fetching: {}", agent_url);

    let response = reqwest::get(&agent_url).await;

    let content = match response {
        Ok(resp) => {
            if !resp.status().is_success() {
                bail!(
                    "Agent '{}' not found in registry (HTTP {}).\n\n\
                    Available options:\n\
                    1. Check the agent name is correct\n\
                    2. Use --registry to specify a different registry URL\n\
                    3. Create a custom agent with: cortex agent create {}",
                    args.name,
                    resp.status(),
                    args.name
                );
            }
            resp.text()
                .await
                .with_context(|| "Failed to read agent content")?
        }
        Err(e) => {
            bail!(
                "Failed to connect to registry: {}\n\n\
                Check your network connection or try a different registry with --registry",
                e
            );
        }
    };

    // Validate the content has proper frontmatter
    if !content.trim().starts_with("---") {
        bail!("Invalid agent file format. Expected markdown with YAML frontmatter.");
    }

    // Get target path
    let agents_dir = get_agents_dir()?;
    std::fs::create_dir_all(&agents_dir)?;

    let agent_file = agents_dir.join(format!("{}.md", args.name));

    // Check if already exists
    if agent_file.exists() && !args.force {
        bail!(
            "Agent '{}' already exists at {}.\n\
            Use --force to overwrite.",
            args.name,
            agent_file.display()
        );
    }

    // Write the agent file
    std::fs::write(&agent_file, &content)
        .with_context(|| format!("Failed to write agent file: {}", agent_file.display()))?;

    println!();
    println!("Agent '{}' installed successfully!", args.name);
    println!("   Location: {}", agent_file.display());
    println!();
    println!("   Use 'cortex agent show {}' to view details.", args.name);
    println!(
        "   Use 'cortex -a {}' to start a session with this agent.",
        args.name
    );

    Ok(())
}
