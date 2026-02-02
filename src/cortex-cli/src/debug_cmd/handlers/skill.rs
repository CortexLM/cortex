//! Skill command handler.

use anyhow::Result;
use std::path::PathBuf;

use crate::debug_cmd::commands::SkillArgs;
use crate::debug_cmd::types::{SkillDebugOutput, SkillDefinition};
use crate::debug_cmd::utils::get_cortex_home;

/// Run the skill debug command.
pub async fn run_skill(args: SkillArgs) -> Result<()> {
    let cortex_home = get_cortex_home();
    let skills_dir = cortex_home.join("skills");

    // Try to find the skill
    let skill_path = if PathBuf::from(&args.name).exists() {
        Some(PathBuf::from(&args.name))
    } else if skills_dir.join(&args.name).exists() {
        Some(skills_dir.join(&args.name))
    } else if skills_dir.join(format!("{}.yaml", &args.name)).exists() {
        Some(skills_dir.join(format!("{}.yaml", &args.name)))
    } else if skills_dir.join(format!("{}.yml", &args.name)).exists() {
        Some(skills_dir.join(format!("{}.yml", &args.name)))
    } else if skills_dir.join(format!("{}.toml", &args.name)).exists() {
        Some(skills_dir.join(format!("{}.toml", &args.name)))
    } else {
        None
    };

    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    let mut definition = None;
    let mut valid = false;

    if let Some(ref path) = skill_path
        && path.exists()
    {
        match std::fs::read_to_string(path) {
            Ok(content) => {
                // Try YAML first, then TOML
                let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                let parse_result: Result<SkillDefinition, String> = match ext {
                    "yaml" | "yml" => serde_yaml::from_str(&content).map_err(|e| e.to_string()),
                    "toml" => toml::from_str(&content).map_err(|e| e.to_string()),
                    _ => {
                        // Try YAML first, then TOML
                        serde_yaml::from_str(&content)
                            .map_err(|e| e.to_string())
                            .or_else(|_| toml::from_str(&content).map_err(|e| e.to_string()))
                    }
                };

                match parse_result {
                    Ok(def) => {
                        // Validate the definition
                        if def.name.is_empty() {
                            warnings.push("Skill name is empty".to_string());
                        }
                        if def.description.is_empty() {
                            warnings.push("Skill description is empty".to_string());
                        }
                        if def.commands.is_empty() {
                            warnings.push("Skill has no commands".to_string());
                        }
                        for cmd in &def.commands {
                            if cmd.command.is_none() {
                                warnings
                                    .push(format!("Command '{}' has no command defined", cmd.name));
                            }
                        }
                        valid = errors.is_empty();
                        definition = Some(def);
                    }
                    Err(e) => {
                        errors.push(format!("Parse error: {}", e));
                    }
                }
            }
            Err(e) => {
                errors.push(format!("Read error: {}", e));
            }
        }
    }

    let output = SkillDebugOutput {
        name: args.name.clone(),
        path: skill_path.clone(),
        found: skill_path.is_some(),
        valid,
        definition,
        errors,
        warnings,
    };

    if args.json {
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        println!("Skill Debug: {}", output.name);
        println!("{}", "=".repeat(50));
        println!("  Found: {}", output.found);
        if let Some(ref path) = output.path {
            println!("  Path:  {}", path.display());
        }
        println!("  Valid: {}", output.valid);

        if let Some(ref def) = output.definition {
            println!();
            println!("Definition");
            println!("{}", "-".repeat(40));
            println!("  Name:        {}", def.name);
            println!("  Description: {}", def.description);
            if let Some(ref version) = def.version {
                println!("  Version:     {}", version);
            }
            if let Some(ref author) = def.author {
                println!("  Author:      {}", author);
            }
            println!("  Commands:    {}", def.commands.len());
            for cmd in &def.commands {
                println!("    - {}", cmd.name);
            }
        }

        if !output.errors.is_empty() {
            println!();
            println!("Errors");
            println!("{}", "-".repeat(40));
            for error in &output.errors {
                println!("  \u{2717} {}", error);
            }
        }

        if !output.warnings.is_empty() {
            println!();
            println!("Warnings");
            println!("{}", "-".repeat(40));
            for warning in &output.warnings {
                println!("  \u{26A0} {}", warning);
            }
        }
    }

    Ok(())
}
