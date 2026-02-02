//! Handler for AI-powered agent generation.

use anyhow::{Context, Result, bail};
use std::io::{self, BufRead, Write};

use crate::agent_cmd::cli::CreateArgs;
use crate::agent_cmd::loader::get_agents_dir;
use crate::agent_cmd::utils::validate_model_name;

/// Generate agent using AI.
pub async fn run_generate(args: CreateArgs) -> Result<()> {
    use cortex_engine::agent::{AgentGenerator, GeneratedAgent};

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

    // Determine if we should run in fully automated mode
    // When description is provided via --generate, run non-interactively
    let fully_automated = args.generate.is_some() || args.non_interactive;

    if !fully_automated {
        println!("Cortex AI Agent Generator");
        println!("{}", "=".repeat(40));
        println!();
    }

    // Get description - either from args or interactively
    let description = if let Some(ref desc) = args.generate {
        desc.clone()
    } else if args.non_interactive {
        bail!("Description is required in non-interactive mode (use --generate \"description\")");
    } else {
        println!("Describe the agent you want to create.");
        println!("Be specific about its purpose, expertise, and when it should be used.");
        println!();

        let input = prompt_input(&stdin, &mut stdout, "Description", None)?;
        if input.is_empty() {
            bail!("Description is required");
        }
        input
    };

    // Validate model argument
    let model_arg = args.model.trim();
    if model_arg.is_empty() {
        bail!("Error: Model name cannot be empty");
    }

    // Validate the model name format and existence
    let valid_model = validate_model_name(model_arg)?;

    if !fully_automated {
        println!();
        println!("Generating agent configuration...");
        println!("   Using model: {}", valid_model);
        println!();
    }

    // Create generator and generate
    let generator = AgentGenerator::new().with_model(&valid_model);

    let generated = generator
        .generate(&description)
        .await
        .with_context(|| "Failed to generate agent configuration")?;

    // Display the generated configuration (only in interactive mode)
    if !fully_automated {
        println!("Generated Agent Configuration:");
        println!("{}", "-".repeat(40));
        println!();
        println!("  Name:        {}", generated.identifier);
        println!("  Display:     {}", generated.display_name);
        println!("  Mode:        {}", generated.mode);
        if let Some(temp) = generated.temperature {
            println!("  Temperature: {}", temp);
        }
        println!("  Tools:       {}", generated.tools.join(", "));
        if !generated.tags.is_empty() {
            println!("  Tags:        {}", generated.tags.join(", "));
        }
        println!(
            "  Delegate:    {}",
            if generated.can_delegate { "yes" } else { "no" }
        );
        println!();
        println!("  When to use:");
        println!("    {}", generated.when_to_use);
        println!();
        println!("  System Prompt:");
        println!("  {}", "-".repeat(36));

        // Show truncated prompt
        let prompt_preview = if generated.system_prompt.len() > 500 {
            format!(
                "{}...\n\n    (truncated, {} chars total)",
                &generated.system_prompt[..500],
                generated.system_prompt.len()
            )
        } else {
            generated.system_prompt.clone()
        };
        for line in prompt_preview.lines() {
            println!("    {}", line);
        }
        println!();

        // Confirm or allow edits (only in interactive mode)
        let confirm = prompt_input(
            &stdin,
            &mut stdout,
            "Save this agent? (y/n/edit)",
            Some("y"),
        )?;

        match confirm.to_lowercase().as_str() {
            "n" | "no" => {
                println!("Cancelled.");
                return Ok(());
            }
            "e" | "edit" => {
                // Allow editing the name
                let new_name = prompt_input(
                    &stdin,
                    &mut stdout,
                    &format!("Agent name [{}]", generated.identifier),
                    Some(&generated.identifier),
                )?;

                // Validate that new name is not empty or whitespace-only
                if new_name.trim().is_empty() {
                    bail!("Error: Agent name cannot be empty");
                }

                let final_agent = if new_name != generated.identifier {
                    GeneratedAgent {
                        identifier: new_name.to_lowercase().replace([' ', '-'], "_"),
                        ..generated
                    }
                } else {
                    generated
                };

                save_generated_agent(&final_agent, false)?;
                return Ok(());
            }
            _ => {
                // Save as-is
            }
        }
        save_generated_agent(&generated, false)?;
    } else {
        // Fully automated mode: save directly without prompts
        save_generated_agent(&generated, true)?;
    }

    Ok(())
}

/// Save a generated agent to disk.
///
/// # Arguments
/// * `agent` - The generated agent configuration to save
/// * `force` - If true, overwrite existing agent without confirmation prompt
fn save_generated_agent(agent: &cortex_engine::agent::GeneratedAgent, force: bool) -> Result<()> {
    // Validate agent identifier is not empty
    if agent.identifier.trim().is_empty() {
        bail!("Error: Agent name cannot be empty");
    }

    let agents_dir = get_agents_dir()?;
    std::fs::create_dir_all(&agents_dir)?;

    let agent_file = agents_dir.join(agent.filename());

    // Check if file already exists (only prompt if not in force mode)
    if agent_file.exists() && !force {
        print!(
            "Agent '{}' already exists. Overwrite? [y/N]: ",
            agent.identifier
        );
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().lock().read_line(&mut input)?;

        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Cancelled.");
            return Ok(());
        }
    }

    let content = agent.to_markdown();
    std::fs::write(&agent_file, &content)
        .with_context(|| format!("Failed to write agent file: {}", agent_file.display()))?;

    println!();
    println!("Agent '{}' created successfully!", agent.identifier);
    println!("   Location: {}", agent_file.display());
    println!();
    println!(
        "   Use 'Cortex Agent show {}' to view details.",
        agent.identifier
    );
    println!(
        "   Use 'cortex -a {}' to start a session with this agent.",
        agent.identifier
    );

    Ok(())
}
