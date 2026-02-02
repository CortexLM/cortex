//! Alias command for Cortex CLI.
//!
//! Provides user-defined command alias functionality:
//! - Set custom command aliases
//! - List all aliases
//! - Remove aliases
//! - Show alias details

use anyhow::{Result, bail};
use clap::Parser;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Alias CLI command.
#[derive(Debug, Parser)]
pub struct AliasCli {
    #[command(subcommand)]
    pub subcommand: AliasSubcommand,
}

/// Alias subcommands.
#[derive(Debug, clap::Subcommand)]
pub enum AliasSubcommand {
    /// Set a command alias
    Set(AliasSetArgs),

    /// List all aliases
    #[command(visible_alias = "ls")]
    List(AliasListArgs),

    /// Remove an alias
    #[command(visible_aliases = ["rm", "delete"])]
    Remove(AliasRemoveArgs),

    /// Show alias details
    #[command(visible_alias = "info")]
    Show(AliasShowArgs),
}

/// Arguments for alias set command.
#[derive(Debug, Parser)]
pub struct AliasSetArgs {
    /// Alias name (short name for the command)
    pub name: String,

    /// Command to alias (e.g., "exec --output-schema")
    pub command: String,

    /// Description of the alias
    #[arg(long, short = 'd')]
    pub description: Option<String>,

    /// Force overwrite if alias already exists
    #[arg(long, short = 'f')]
    pub force: bool,
}

/// Arguments for alias list command.
#[derive(Debug, Parser)]
pub struct AliasListArgs {
    /// Output as JSON
    #[arg(long)]
    pub json: bool,
}

/// Arguments for alias remove command.
#[derive(Debug, Parser)]
pub struct AliasRemoveArgs {
    /// Alias name to remove
    pub name: String,

    /// Skip confirmation prompt
    #[arg(long, short = 'y')]
    pub yes: bool,
}

/// Arguments for alias show command.
#[derive(Debug, Parser)]
pub struct AliasShowArgs {
    /// Alias name to show
    pub name: String,

    /// Output as JSON
    #[arg(long)]
    pub json: bool,
}

/// Alias definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AliasDefinition {
    pub name: String,
    pub command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Alias configuration.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct AliasConfig {
    #[serde(default)]
    pub aliases: HashMap<String, AliasDefinition>,
}

/// Get the aliases config file path.
fn get_aliases_config_path() -> PathBuf {
    dirs::home_dir()
        .map(|h| h.join(".cortex").join("aliases.toml"))
        .unwrap_or_else(|| PathBuf::from(".cortex/aliases.toml"))
}

/// Load aliases from config file.
fn load_aliases() -> Result<AliasConfig> {
    let config_path = get_aliases_config_path();
    if !config_path.exists() {
        return Ok(AliasConfig::default());
    }

    let content = std::fs::read_to_string(&config_path)?;
    let config: AliasConfig = toml::from_str(&content)?;
    Ok(config)
}

/// Save aliases to config file.
fn save_aliases(config: &AliasConfig) -> Result<()> {
    let config_path = get_aliases_config_path();

    // Create parent directory if needed
    if let Some(parent) = config_path.parent()
        && !parent.exists()
    {
        std::fs::create_dir_all(parent)?;
    }

    let content = toml::to_string_pretty(config)?;
    std::fs::write(&config_path, content)?;
    Ok(())
}

impl AliasCli {
    /// Run the alias command.
    pub async fn run(self) -> Result<()> {
        match self.subcommand {
            AliasSubcommand::Set(args) => run_set(args).await,
            AliasSubcommand::List(args) => run_list(args).await,
            AliasSubcommand::Remove(args) => run_remove(args).await,
            AliasSubcommand::Show(args) => run_show(args).await,
        }
    }
}

async fn run_set(args: AliasSetArgs) -> Result<()> {
    let mut config = load_aliases()?;

    // Check if alias already exists
    if config.aliases.contains_key(&args.name) && !args.force {
        bail!(
            "Alias '{}' already exists. Use --force to overwrite.",
            args.name
        );
    }

    // Create the alias definition
    let alias = AliasDefinition {
        name: args.name.clone(),
        command: args.command.clone(),
        description: args.description.clone(),
    };

    config.aliases.insert(args.name.clone(), alias);
    save_aliases(&config)?;

    println!("Alias '{}' set to: {}", args.name, args.command);
    if let Some(desc) = &args.description {
        println!("  Description: {}", desc);
    }

    Ok(())
}

async fn run_list(args: AliasListArgs) -> Result<()> {
    let config = load_aliases()?;

    if args.json {
        let aliases: Vec<&AliasDefinition> = config.aliases.values().collect();
        println!("{}", serde_json::to_string_pretty(&aliases)?);
        return Ok(());
    }

    if config.aliases.is_empty() {
        println!("No aliases defined.");
        println!("\nUse 'cortex alias set <name> <command>' to create an alias.");
        println!("Example: cortex alias set q \"exec --output-schema\"");
        return Ok(());
    }

    println!("Defined Aliases:");
    println!("{}", "-".repeat(60));

    let mut aliases: Vec<_> = config.aliases.values().collect();
    aliases.sort_by(|a, b| a.name.cmp(&b.name));

    for alias in aliases {
        println!("  {} = {}", alias.name, alias.command);
        if let Some(desc) = &alias.description {
            println!("      {}", desc);
        }
    }

    println!("\nTotal: {} alias(es)", config.aliases.len());
    Ok(())
}

async fn run_remove(args: AliasRemoveArgs) -> Result<()> {
    let mut config = load_aliases()?;

    if !config.aliases.contains_key(&args.name) {
        bail!("Alias '{}' does not exist.", args.name);
    }

    if !args.yes {
        println!(
            "Are you sure you want to remove alias '{}'? (y/N)",
            args.name
        );
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Aborted.");
            return Ok(());
        }
    }

    config.aliases.remove(&args.name);
    save_aliases(&config)?;

    println!("Alias '{}' removed.", args.name);
    Ok(())
}

async fn run_show(args: AliasShowArgs) -> Result<()> {
    let config = load_aliases()?;

    let alias = match config.aliases.get(&args.name) {
        Some(a) => a,
        None => {
            if args.json {
                let error = serde_json::json!({
                    "error": format!("Alias '{}' does not exist", args.name)
                });
                println!("{}", serde_json::to_string_pretty(&error)?);
                // Exit with error code but don't duplicate error message via bail!()
                std::process::exit(1);
            }
            bail!("Alias '{}' does not exist.", args.name);
        }
    };

    if args.json {
        println!("{}", serde_json::to_string_pretty(alias)?);
        return Ok(());
    }

    println!("Alias: {}", alias.name);
    println!("{}", "-".repeat(40));
    println!("  Command: {}", alias.command);
    if let Some(desc) = &alias.description {
        println!("  Description: {}", desc);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ==========================================================================
    // AliasDefinition tests
    // ==========================================================================

    #[test]
    fn test_alias_definition_with_description_json_roundtrip() {
        let alias = AliasDefinition {
            name: "q".to_string(),
            command: "exec --output-schema".to_string(),
            description: Some("Quick exec with schema output".to_string()),
        };

        let json = serde_json::to_string(&alias).expect("should serialize to JSON");
        let parsed: AliasDefinition =
            serde_json::from_str(&json).expect("should deserialize from JSON");

        assert_eq!(parsed.name, "q");
        assert_eq!(parsed.command, "exec --output-schema");
        assert_eq!(
            parsed.description,
            Some("Quick exec with schema output".to_string())
        );
    }

    #[test]
    fn test_alias_definition_without_description_json_roundtrip() {
        let alias = AliasDefinition {
            name: "ls".to_string(),
            command: "list --all".to_string(),
            description: None,
        };

        let json = serde_json::to_string(&alias).expect("should serialize to JSON");
        let parsed: AliasDefinition =
            serde_json::from_str(&json).expect("should deserialize from JSON");

        assert_eq!(parsed.name, "ls");
        assert_eq!(parsed.command, "list --all");
        assert!(parsed.description.is_none());
    }

    #[test]
    fn test_alias_definition_skips_none_description_in_serialization() {
        let alias = AliasDefinition {
            name: "test".to_string(),
            command: "run".to_string(),
            description: None,
        };

        let json = serde_json::to_string(&alias).expect("should serialize to JSON");

        // The description field should not appear in the JSON when None
        assert!(!json.contains("description"));
    }

    #[test]
    fn test_alias_definition_includes_description_when_present() {
        let alias = AliasDefinition {
            name: "test".to_string(),
            command: "run".to_string(),
            description: Some("A test alias".to_string()),
        };

        let json = serde_json::to_string(&alias).expect("should serialize to JSON");

        assert!(json.contains("description"));
        assert!(json.contains("A test alias"));
    }

    #[test]
    fn test_alias_definition_clone() {
        let original = AliasDefinition {
            name: "original".to_string(),
            command: "original cmd".to_string(),
            description: Some("original desc".to_string()),
        };

        let cloned = original.clone();

        assert_eq!(cloned.name, original.name);
        assert_eq!(cloned.command, original.command);
        assert_eq!(cloned.description, original.description);
    }

    // ==========================================================================
    // AliasConfig tests
    // ==========================================================================

    #[test]
    fn test_alias_config_default_is_empty() {
        let config = AliasConfig::default();

        assert!(config.aliases.is_empty());
    }

    #[test]
    fn test_alias_config_insert_and_retrieve() {
        let mut config = AliasConfig::default();

        let alias = AliasDefinition {
            name: "q".to_string(),
            command: "exec --quick".to_string(),
            description: None,
        };

        config.aliases.insert("q".to_string(), alias);

        assert_eq!(config.aliases.len(), 1);
        assert!(config.aliases.contains_key("q"));

        let retrieved = config.aliases.get("q").expect("alias should exist");
        assert_eq!(retrieved.name, "q");
        assert_eq!(retrieved.command, "exec --quick");
    }

    #[test]
    fn test_alias_config_multiple_aliases() {
        let mut config = AliasConfig::default();

        config.aliases.insert(
            "q".to_string(),
            AliasDefinition {
                name: "q".to_string(),
                command: "exec --quick".to_string(),
                description: Some("Quick execution".to_string()),
            },
        );

        config.aliases.insert(
            "ls".to_string(),
            AliasDefinition {
                name: "ls".to_string(),
                command: "list --all".to_string(),
                description: None,
            },
        );

        config.aliases.insert(
            "run".to_string(),
            AliasDefinition {
                name: "run".to_string(),
                command: "exec --verbose".to_string(),
                description: Some("Run with verbose output".to_string()),
            },
        );

        assert_eq!(config.aliases.len(), 3);
        assert!(config.aliases.contains_key("q"));
        assert!(config.aliases.contains_key("ls"));
        assert!(config.aliases.contains_key("run"));
    }

    #[test]
    fn test_alias_config_remove_alias() {
        let mut config = AliasConfig::default();

        config.aliases.insert(
            "q".to_string(),
            AliasDefinition {
                name: "q".to_string(),
                command: "exec".to_string(),
                description: None,
            },
        );

        assert!(config.aliases.contains_key("q"));

        config.aliases.remove("q");

        assert!(!config.aliases.contains_key("q"));
        assert!(config.aliases.is_empty());
    }

    #[test]
    fn test_alias_config_overwrite_alias() {
        let mut config = AliasConfig::default();

        config.aliases.insert(
            "q".to_string(),
            AliasDefinition {
                name: "q".to_string(),
                command: "old command".to_string(),
                description: Some("Old description".to_string()),
            },
        );

        config.aliases.insert(
            "q".to_string(),
            AliasDefinition {
                name: "q".to_string(),
                command: "new command".to_string(),
                description: Some("New description".to_string()),
            },
        );

        assert_eq!(config.aliases.len(), 1);
        let alias = config.aliases.get("q").expect("alias should exist");
        assert_eq!(alias.command, "new command");
        assert_eq!(alias.description, Some("New description".to_string()));
    }

    #[test]
    fn test_alias_config_toml_roundtrip() {
        let mut config = AliasConfig::default();

        config.aliases.insert(
            "q".to_string(),
            AliasDefinition {
                name: "q".to_string(),
                command: "exec --output-schema".to_string(),
                description: Some("Quick exec".to_string()),
            },
        );

        config.aliases.insert(
            "ls".to_string(),
            AliasDefinition {
                name: "ls".to_string(),
                command: "list --all".to_string(),
                description: None,
            },
        );

        let toml_str = toml::to_string(&config).expect("should serialize to TOML");
        let parsed: AliasConfig = toml::from_str(&toml_str).expect("should deserialize from TOML");

        assert_eq!(parsed.aliases.len(), 2);
        assert!(parsed.aliases.contains_key("q"));
        assert!(parsed.aliases.contains_key("ls"));

        let q_alias = parsed.aliases.get("q").expect("q alias should exist");
        assert_eq!(q_alias.command, "exec --output-schema");
        assert_eq!(q_alias.description, Some("Quick exec".to_string()));

        let ls_alias = parsed.aliases.get("ls").expect("ls alias should exist");
        assert_eq!(ls_alias.command, "list --all");
        assert!(ls_alias.description.is_none());
    }

    #[test]
    fn test_alias_config_empty_toml_roundtrip() {
        let config = AliasConfig::default();

        let toml_str = toml::to_string(&config).expect("should serialize empty config to TOML");
        let parsed: AliasConfig = toml::from_str(&toml_str).expect("should deserialize from TOML");

        assert!(parsed.aliases.is_empty());
    }

    #[test]
    fn test_alias_config_json_roundtrip() {
        let mut config = AliasConfig::default();

        config.aliases.insert(
            "test".to_string(),
            AliasDefinition {
                name: "test".to_string(),
                command: "run tests".to_string(),
                description: Some("Run the test suite".to_string()),
            },
        );

        let json = serde_json::to_string(&config).expect("should serialize to JSON");
        let parsed: AliasConfig =
            serde_json::from_str(&json).expect("should deserialize from JSON");

        assert_eq!(parsed.aliases.len(), 1);
        let alias = parsed.aliases.get("test").expect("test alias should exist");
        assert_eq!(alias.name, "test");
        assert_eq!(alias.command, "run tests");
        assert_eq!(alias.description, Some("Run the test suite".to_string()));
    }

    #[test]
    fn test_alias_definition_deserialize_from_json_missing_optional_field() {
        let json = r#"{"name": "q", "command": "exec"}"#;
        let alias: AliasDefinition =
            serde_json::from_str(json).expect("should deserialize with missing optional field");

        assert_eq!(alias.name, "q");
        assert_eq!(alias.command, "exec");
        assert!(alias.description.is_none());
    }

    #[test]
    fn test_alias_config_deserialize_from_toml_with_empty_aliases() {
        let toml_str = r#"
[aliases]
"#;
        let config: AliasConfig =
            toml::from_str(toml_str).expect("should deserialize TOML with empty aliases table");

        assert!(config.aliases.is_empty());
    }

    #[test]
    fn test_alias_config_deserialize_from_toml_with_aliases() {
        let toml_str = r#"
[aliases.q]
name = "q"
command = "exec --quick"
description = "Quick exec"

[aliases.ls]
name = "ls"
command = "list"
"#;
        let config: AliasConfig =
            toml::from_str(toml_str).expect("should deserialize TOML with aliases");

        assert_eq!(config.aliases.len(), 2);

        let q_alias = config.aliases.get("q").expect("q alias should exist");
        assert_eq!(q_alias.name, "q");
        assert_eq!(q_alias.command, "exec --quick");
        assert_eq!(q_alias.description, Some("Quick exec".to_string()));

        let ls_alias = config.aliases.get("ls").expect("ls alias should exist");
        assert_eq!(ls_alias.name, "ls");
        assert_eq!(ls_alias.command, "list");
        assert!(ls_alias.description.is_none());
    }

    #[test]
    fn test_alias_definition_special_characters_in_command() {
        let alias = AliasDefinition {
            name: "complex".to_string(),
            command: "exec --arg=\"value with spaces\" --flag".to_string(),
            description: Some("Command with special chars: <>&".to_string()),
        };

        let json = serde_json::to_string(&alias).expect("should serialize with special chars");
        let parsed: AliasDefinition =
            serde_json::from_str(&json).expect("should deserialize with special chars");

        assert_eq!(parsed.command, "exec --arg=\"value with spaces\" --flag");
        assert_eq!(
            parsed.description,
            Some("Command with special chars: <>&".to_string())
        );
    }
}
