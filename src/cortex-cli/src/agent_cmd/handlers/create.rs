//! Handler for the `agent create` command.

use anyhow::{Context, Result, bail};
use std::io::{self, BufRead, Write};

use crate::agent_cmd::cli::CreateArgs;
use crate::agent_cmd::loader::get_agents_dir;
use crate::agent_cmd::types::AgentMode;
use crate::agent_cmd::utils::{AVAILABLE_TOOLS, RESERVED_NAMES, validate_model_name};

use super::generate::run_generate;

/// Create agent command (interactive wizard).
pub async fn run_create(args: CreateArgs) -> Result<()> {
    // Check if we're using AI generation
    if args.generate.is_some() {
        return run_generate(args).await;
    }

    let stdin = io::stdin();
    let mut stdout = io::stdout();

    // Helper to prompt for input
    fn prompt_input(
        stdin: &io::Stdin,
        stdout: &mut io::Stdout,
        prompt: &str,
        default: Option<&str>,
    ) -> Result<String> {
        if let Some(def) = default {
            print!("{prompt} [{def}]: ");
        } else {
            print!("{prompt}: ");
        }
        stdout.flush()?;

        let mut input = String::new();
        stdin.lock().read_line(&mut input)?;
        let input = input.trim().to_string();

        if input.is_empty() {
            Ok(default.map(String::from).unwrap_or_default())
        } else {
            Ok(input)
        }
    }

    // Only show banner in interactive mode
    if !args.non_interactive {
        println!("🤖 Cortex Agent Creator");
        println!("{}", "=".repeat(40));
        println!();
    }

    // Get agent name
    let name = if let Some(ref n) = args.name {
        n.clone()
    } else if args.non_interactive {
        bail!("Agent name is required in non-interactive mode (use --name)");
    } else {
        let input = prompt_input(&stdin, &mut stdout, "Agent name", None)?;
        if input.is_empty() {
            bail!("Agent name is required");
        }
        input
    };

    // Validate name is not empty or whitespace-only
    if name.trim().is_empty() {
        bail!("Error: Agent name cannot be empty");
    }

    // Validate name characters
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        bail!("Agent name must contain only alphanumeric characters, hyphens, and underscores");
    }

    // Check for reserved command names that would conflict with cortex CLI commands (#2450)
    let name_lower = name.to_lowercase();
    if RESERVED_NAMES.contains(&name_lower.as_str()) {
        bail!(
            "Error: '{}' is a reserved command name and cannot be used as an agent name.\n\
            Reserved names: {}",
            name,
            RESERVED_NAMES.join(", ")
        );
    }

    // Get description
    let description = if let Some(ref d) = args.description {
        d.clone()
    } else if args.non_interactive {
        format!("Custom agent: {name}")
    } else {
        prompt_input(
            &stdin,
            &mut stdout,
            "Description",
            Some(&format!("Custom agent: {name}")),
        )?
    };

    // Validate description length (Issue #1981)
    // Maximum description length: 1000 characters (reasonable for display and storage)
    const MAX_DESCRIPTION_LENGTH: usize = 1000;
    if description.len() > MAX_DESCRIPTION_LENGTH {
        bail!(
            "Description exceeds maximum length of {} characters ({} provided). \
            Please use a shorter description.",
            MAX_DESCRIPTION_LENGTH,
            description.len()
        );
    }

    // Get mode
    let mode = if let Some(ref m) = args.mode {
        m.parse::<AgentMode>().map_err(|e| anyhow::anyhow!(e))?
    } else if args.non_interactive {
        AgentMode::Primary
    } else {
        println!("\nAgent modes:");
        println!("  1. primary  - User-facing agent (default)");
        println!("  2. subagent - Invoked by other agents");
        println!("  3. all      - Both primary and subagent");
        let input = prompt_input(&stdin, &mut stdout, "Mode (1/2/3 or name)", Some("1"))?;
        match input.as_str() {
            "1" | "primary" => AgentMode::Primary,
            "2" | "subagent" | "sub" => AgentMode::Subagent,
            "3" | "all" | "both" => AgentMode::All,
            _ => {
                println!("Invalid mode, using 'primary'");
                AgentMode::Primary
            }
        }
    };

    // Get tool configuration
    let (allowed_tools, denied_tools) = if args.non_interactive {
        (None, Vec::new())
    } else {
        println!("\nTool Configuration:");
        println!("  Available tools: {}", AVAILABLE_TOOLS.join(", "));
        println!("\n  Press Enter to allow all tools, or specify configuration:");

        let allowed_input = prompt_input(
            &stdin,
            &mut stdout,
            "  Allowed tools (comma-separated, or 'all')",
            Some("all"),
        )?;
        let allowed: Option<Vec<String>> = if allowed_input == "all" || allowed_input.is_empty() {
            None
        } else {
            Some(
                allowed_input
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .collect(),
            )
        };

        let denied_input = prompt_input(
            &stdin,
            &mut stdout,
            "  Denied tools (comma-separated, or 'none')",
            Some("none"),
        )?;
        let denied = if denied_input == "none" || denied_input.is_empty() {
            Vec::new()
        } else {
            denied_input
                .split(',')
                .map(|s| s.trim().to_string())
                .collect()
        };

        (allowed, denied)
    };

    // Get system prompt
    let system_prompt = if args.non_interactive {
        format!("You are {name}, a helpful AI assistant.")
    } else {
        println!("\nSystem Prompt:");
        println!("  Enter the system prompt for this agent.");
        println!("  (Press Enter twice to finish, or type a single line for a simple prompt)");
        print!("  > ");
        stdout.flush()?;

        let mut lines = Vec::new();
        let mut empty_count = 0;

        loop {
            let mut line = String::new();
            stdin.lock().read_line(&mut line)?;

            if line.trim().is_empty() {
                empty_count += 1;
                if empty_count >= 2 || lines.is_empty() {
                    break;
                }
                lines.push(String::new());
            } else {
                empty_count = 0;
                lines.push(line.trim_end().to_string());
                print!("  > ");
                stdout.flush()?;
            }
        }

        if lines.is_empty() {
            format!("You are {name}, a helpful AI assistant.")
        } else {
            lines.join("\n")
        }
    };

    // Get optional settings
    let (temperature, model, color) = if args.non_interactive {
        (None, None, None)
    } else {
        println!("\nOptional Settings (press Enter to skip):");

        let temp_input = prompt_input(&stdin, &mut stdout, "  Temperature (0.0-2.0)", Some(""))?;
        let temperature = temp_input.parse::<f32>().ok();

        let model_input = prompt_input(&stdin, &mut stdout, "  Model override", Some(""))?;
        // Issue #2328: Validate model name if provided
        let model = if model_input.is_empty() {
            None
        } else {
            // Validate the model name to prevent typos from being accepted
            match validate_model_name(&model_input) {
                Ok(valid_model) => Some(valid_model),
                Err(e) => {
                    eprintln!("Warning: {}", e);
                    eprintln!(
                        "Using model name as-is. The agent may fail to run if the model doesn't exist."
                    );
                    Some(model_input)
                }
            }
        };

        let color = prompt_input(
            &stdin,
            &mut stdout,
            "  Color (hex, e.g., #22c55e)",
            Some(""),
        )?;
        let color = if color.is_empty() { None } else { Some(color) };

        (temperature, model, color)
    };

    // Create the agent file
    let agents_dir = get_agents_dir()?;
    std::fs::create_dir_all(&agents_dir)?;

    let agent_file = agents_dir.join(format!("{name}.md"));

    // Build frontmatter
    let mut frontmatter = format!(
        r#"---
name: {name}
description: "{description}"
mode: {mode}
"#
    );

    if let Some(temp) = temperature {
        frontmatter.push_str(&format!("temperature: {temp}\n"));
    }

    if let Some(ref m) = model {
        frontmatter.push_str(&format!("model: {m}\n"));
    }

    if let Some(ref c) = color {
        frontmatter.push_str(&format!("color: \"{c}\"\n"));
    }

    if let Some(ref allowed) = allowed_tools {
        frontmatter.push_str("allowed_tools:\n");
        for tool in allowed {
            frontmatter.push_str(&format!("  - {tool}\n"));
        }
    }

    if !denied_tools.is_empty() {
        frontmatter.push_str("denied_tools:\n");
        for tool in &denied_tools {
            frontmatter.push_str(&format!("  - {tool}\n"));
        }
    }

    frontmatter.push_str("---\n\n");

    let content = format!("{frontmatter}{system_prompt}\n");

    std::fs::write(&agent_file, &content)
        .with_context(|| format!("Failed to write agent file: {}", agent_file.display()))?;

    // In non-interactive mode, output only the path for scripting
    if args.non_interactive {
        println!("{}", agent_file.display());
    } else {
        println!("\nAgent '{name}' created successfully!");
        println!("   Location: {}", agent_file.display());
        println!("\n   Use 'Cortex Agent show {name}' to view details.");
    }

    Ok(())
}
