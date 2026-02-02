//! Plugin management command for Cortex CLI.
//!
//! Provides plugin management functionality:
//! - List installed plugins
//! - Install plugins
//! - Remove plugins
//! - Enable/disable plugins
//! - Show plugin info

use anyhow::{Result, bail};
use clap::Parser;
use serde::Serialize;
use std::path::PathBuf;

/// Plugin CLI command.
#[derive(Debug, Parser)]
pub struct PluginCli {
    #[command(subcommand)]
    pub subcommand: PluginSubcommand,
}

/// Plugin subcommands.
#[derive(Debug, clap::Subcommand)]
pub enum PluginSubcommand {
    /// List installed plugins
    #[command(visible_alias = "ls")]
    List(PluginListArgs),

    /// Install a plugin
    #[command(visible_alias = "add")]
    Install(PluginInstallArgs),

    /// Remove a plugin
    #[command(visible_aliases = ["rm", "uninstall"])]
    Remove(PluginRemoveArgs),

    /// Enable a plugin
    Enable(PluginEnableArgs),

    /// Disable a plugin
    Disable(PluginDisableArgs),

    /// Show plugin information
    #[command(visible_alias = "info")]
    Show(PluginShowArgs),
}

/// Arguments for plugin list command.
#[derive(Debug, Parser)]
pub struct PluginListArgs {
    /// Output as JSON
    #[arg(long)]
    pub json: bool,

    /// Show only enabled plugins
    #[arg(long)]
    pub enabled: bool,

    /// Show only disabled plugins
    #[arg(long)]
    pub disabled: bool,
}

/// Arguments for plugin install command.
#[derive(Debug, Parser)]
pub struct PluginInstallArgs {
    /// Plugin name or URL to install
    pub name: String,

    /// Plugin version (defaults to latest)
    #[arg(long, short = 'v')]
    pub version: Option<String>,

    /// Force reinstall if already installed
    #[arg(long, short = 'f')]
    pub force: bool,
}

/// Arguments for plugin remove command.
#[derive(Debug, Parser)]
pub struct PluginRemoveArgs {
    /// Plugin name to remove
    pub name: String,

    /// Skip confirmation prompt
    #[arg(long, short = 'y')]
    pub yes: bool,
}

/// Arguments for plugin enable command.
#[derive(Debug, Parser)]
pub struct PluginEnableArgs {
    /// Plugin name to enable
    pub name: String,
}

/// Arguments for plugin disable command.
#[derive(Debug, Parser)]
pub struct PluginDisableArgs {
    /// Plugin name to disable
    pub name: String,
}

/// Arguments for plugin show command.
#[derive(Debug, Parser)]
pub struct PluginShowArgs {
    /// Plugin name to show
    pub name: String,

    /// Output as JSON
    #[arg(long)]
    pub json: bool,
}

/// Plugin information for display.
#[derive(Debug, Serialize)]
struct PluginInfo {
    name: String,
    version: String,
    description: String,
    enabled: bool,
    path: PathBuf,
}

/// Get the plugins directory.
fn get_plugins_dir() -> PathBuf {
    dirs::home_dir()
        .map(|h| h.join(".cortex").join("plugins"))
        .unwrap_or_else(|| PathBuf::from(".cortex/plugins"))
}

impl PluginCli {
    /// Run the plugin command.
    pub async fn run(self) -> Result<()> {
        match self.subcommand {
            PluginSubcommand::List(args) => run_list(args).await,
            PluginSubcommand::Install(args) => run_install(args).await,
            PluginSubcommand::Remove(args) => run_remove(args).await,
            PluginSubcommand::Enable(args) => run_enable(args).await,
            PluginSubcommand::Disable(args) => run_disable(args).await,
            PluginSubcommand::Show(args) => run_show(args).await,
        }
    }
}

async fn run_list(args: PluginListArgs) -> Result<()> {
    let plugins_dir = get_plugins_dir();

    if !plugins_dir.exists() {
        if args.json {
            println!("[]");
        } else {
            println!("No plugins installed.");
            println!("\nPlugin directory: {}", plugins_dir.display());
            println!("Use 'cortex plugin install <name>' to install a plugin.");
        }
        return Ok(());
    }

    let mut plugins = Vec::new();

    // Scan plugins directory
    if let Ok(entries) = std::fs::read_dir(&plugins_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let manifest_path = path.join("plugin.toml");
                if manifest_path.exists()
                    && let Ok(content) = std::fs::read_to_string(&manifest_path)
                    && let Ok(manifest) = toml::from_str::<toml::Value>(&content)
                {
                    let name = manifest
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or_else(|| {
                            path.file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or("unknown")
                        })
                        .to_string();

                    let version = manifest
                        .get("version")
                        .and_then(|v| v.as_str())
                        .unwrap_or("0.0.0")
                        .to_string();

                    let description = manifest
                        .get("description")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();

                    let enabled = manifest
                        .get("enabled")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(true);

                    // Apply filters
                    if args.enabled && !enabled {
                        continue;
                    }
                    if args.disabled && enabled {
                        continue;
                    }

                    plugins.push(PluginInfo {
                        name,
                        version,
                        description,
                        enabled,
                        path,
                    });
                }
            }
        }
    }

    if args.json {
        println!("{}", serde_json::to_string_pretty(&plugins)?);
    } else if plugins.is_empty() {
        println!("No plugins installed.");
        println!("\nPlugin directory: {}", plugins_dir.display());
        println!("Use 'cortex plugin install <name>' to install a plugin.");
    } else {
        println!("Installed Plugins:");
        println!("{}", "-".repeat(60));
        for plugin in &plugins {
            let status = if plugin.enabled {
                "enabled"
            } else {
                "disabled"
            };
            println!("  {} v{} [{}]", plugin.name, plugin.version, status);
            if !plugin.description.is_empty() {
                println!("    {}", plugin.description);
            }
        }
        println!("\nTotal: {} plugin(s)", plugins.len());
    }

    Ok(())
}

async fn run_install(args: PluginInstallArgs) -> Result<()> {
    let plugins_dir = get_plugins_dir();

    // Create plugins directory if it doesn't exist
    if !plugins_dir.exists() {
        std::fs::create_dir_all(&plugins_dir)?;
    }

    let plugin_path = plugins_dir.join(&args.name);

    if plugin_path.exists() && !args.force {
        bail!(
            "Plugin '{}' is already installed. Use --force to reinstall.",
            args.name
        );
    }

    println!("Installing plugin: {}", args.name);
    if let Some(ref version) = args.version {
        println!("  Version: {}", version);
    }

    // For now, we support local directory installation or create a placeholder
    // In a full implementation, this would fetch from a plugin registry
    if std::path::Path::new(&args.name).exists() {
        // Install from local path
        let src_path = std::path::Path::new(&args.name);
        if src_path.is_dir() {
            // Copy directory
            copy_dir_recursive(src_path, &plugin_path)?;
            println!("Plugin installed from local directory.");
        } else {
            bail!("Source path is not a directory: {}", args.name);
        }
    } else {
        // Create placeholder plugin structure
        std::fs::create_dir_all(&plugin_path)?;

        let version = args.version.as_deref().unwrap_or("1.0.0");
        let manifest = format!(
            r#"# Plugin manifest
name = "{}"
version = "{}"
description = "Placeholder plugin - replace with actual implementation"
enabled = true
"#,
            args.name, version
        );

        std::fs::write(plugin_path.join("plugin.toml"), manifest)?;
        println!("Created placeholder plugin structure.");
        println!("Edit {} to configure your plugin.", plugin_path.display());
    }

    println!("Plugin '{}' installed successfully.", args.name);
    Ok(())
}

fn copy_dir_recursive(src: &std::path::Path, dst: &std::path::Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;

    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path)?;
        }
    }

    Ok(())
}

async fn run_remove(args: PluginRemoveArgs) -> Result<()> {
    let plugins_dir = get_plugins_dir();
    let plugin_path = plugins_dir.join(&args.name);

    if !plugin_path.exists() {
        bail!("Plugin '{}' is not installed.", args.name);
    }

    if !args.yes {
        println!(
            "Are you sure you want to remove plugin '{}'? (y/N)",
            args.name
        );
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Aborted.");
            return Ok(());
        }
    }

    std::fs::remove_dir_all(&plugin_path)?;
    println!("Plugin '{}' removed successfully.", args.name);
    Ok(())
}

async fn run_enable(args: PluginEnableArgs) -> Result<()> {
    let plugins_dir = get_plugins_dir();
    let plugin_path = plugins_dir.join(&args.name);
    let manifest_path = plugin_path.join("plugin.toml");

    if !manifest_path.exists() {
        bail!("Plugin '{}' is not installed.", args.name);
    }

    let content = std::fs::read_to_string(&manifest_path)?;
    let mut manifest: toml::Value = toml::from_str(&content)?;

    if let Some(table) = manifest.as_table_mut() {
        table.insert("enabled".to_string(), toml::Value::Boolean(true));
    }

    std::fs::write(&manifest_path, toml::to_string_pretty(&manifest)?)?;
    println!("Plugin '{}' enabled.", args.name);
    Ok(())
}

async fn run_disable(args: PluginDisableArgs) -> Result<()> {
    let plugins_dir = get_plugins_dir();
    let plugin_path = plugins_dir.join(&args.name);
    let manifest_path = plugin_path.join("plugin.toml");

    if !manifest_path.exists() {
        bail!("Plugin '{}' is not installed.", args.name);
    }

    let content = std::fs::read_to_string(&manifest_path)?;
    let mut manifest: toml::Value = toml::from_str(&content)?;

    if let Some(table) = manifest.as_table_mut() {
        table.insert("enabled".to_string(), toml::Value::Boolean(false));
    }

    std::fs::write(&manifest_path, toml::to_string_pretty(&manifest)?)?;
    println!("Plugin '{}' disabled.", args.name);
    Ok(())
}

async fn run_show(args: PluginShowArgs) -> Result<()> {
    let plugins_dir = get_plugins_dir();
    let plugin_path = plugins_dir.join(&args.name);
    let manifest_path = plugin_path.join("plugin.toml");

    if !manifest_path.exists() {
        bail!("Plugin '{}' is not installed.", args.name);
    }

    let content = std::fs::read_to_string(&manifest_path)?;
    let manifest: toml::Value = toml::from_str(&content)?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&manifest)?);
    } else {
        println!("Plugin: {}", args.name);
        println!("{}", "-".repeat(40));

        if let Some(version) = manifest.get("version").and_then(|v| v.as_str()) {
            println!("  Version:     {}", version);
        }

        if let Some(description) = manifest.get("description").and_then(|v| v.as_str()) {
            println!("  Description: {}", description);
        }

        if let Some(enabled) = manifest.get("enabled").and_then(|v| v.as_bool()) {
            println!("  Enabled:     {}", enabled);
        }

        if let Some(author) = manifest.get("author").and_then(|v| v.as_str()) {
            println!("  Author:      {}", author);
        }

        println!("  Path:        {}", plugin_path.display());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    // ==========================================================================
    // PluginInfo serialization tests
    // ==========================================================================

    #[test]
    fn test_plugin_info_serialization_json() {
        let info = PluginInfo {
            name: "test-plugin".to_string(),
            version: "1.2.3".to_string(),
            description: "A test plugin".to_string(),
            enabled: true,
            path: PathBuf::from("/home/user/.cortex/plugins/test-plugin"),
        };

        let json = serde_json::to_string(&info).expect("should serialize to JSON");

        assert!(json.contains("test-plugin"), "JSON should contain name");
        assert!(json.contains("1.2.3"), "JSON should contain version");
        assert!(
            json.contains("A test plugin"),
            "JSON should contain description"
        );
        assert!(json.contains("true"), "JSON should contain enabled status");
    }

    #[test]
    fn test_plugin_info_serialization_with_empty_description() {
        let info = PluginInfo {
            name: "minimal-plugin".to_string(),
            version: "0.1.0".to_string(),
            description: "".to_string(),
            enabled: false,
            path: PathBuf::from("/plugins/minimal"),
        };

        let json = serde_json::to_string(&info).expect("should serialize to JSON");

        assert!(json.contains("minimal-plugin"), "JSON should contain name");
        assert!(json.contains("0.1.0"), "JSON should contain version");
        assert!(
            json.contains("false"),
            "JSON should contain disabled status"
        );
    }

    #[test]
    fn test_plugin_info_serialization_pretty_json() {
        let info = PluginInfo {
            name: "pretty-plugin".to_string(),
            version: "2.0.0".to_string(),
            description: "Plugin for pretty output".to_string(),
            enabled: true,
            path: PathBuf::from("/path/to/plugin"),
        };

        let json = serde_json::to_string_pretty(&info).expect("should serialize to pretty JSON");

        assert!(json.contains('\n'), "Pretty JSON should have newlines");
        assert!(json.contains("pretty-plugin"), "JSON should contain name");
    }

    #[test]
    fn test_plugin_info_array_serialization() {
        let plugins = vec![
            PluginInfo {
                name: "plugin-a".to_string(),
                version: "1.0.0".to_string(),
                description: "First plugin".to_string(),
                enabled: true,
                path: PathBuf::from("/plugins/a"),
            },
            PluginInfo {
                name: "plugin-b".to_string(),
                version: "2.0.0".to_string(),
                description: "Second plugin".to_string(),
                enabled: false,
                path: PathBuf::from("/plugins/b"),
            },
        ];

        let json = serde_json::to_string(&plugins).expect("should serialize array to JSON");

        assert!(
            json.contains("plugin-a"),
            "JSON should contain first plugin name"
        );
        assert!(
            json.contains("plugin-b"),
            "JSON should contain second plugin name"
        );
        assert!(
            json.contains("1.0.0"),
            "JSON should contain first plugin version"
        );
        assert!(
            json.contains("2.0.0"),
            "JSON should contain second plugin version"
        );
    }

    #[test]
    fn test_plugin_info_empty_array_serialization() {
        let plugins: Vec<PluginInfo> = vec![];
        let json = serde_json::to_string(&plugins).expect("should serialize empty array to JSON");
        assert_eq!(json, "[]", "Empty array should serialize to []");
    }

    // ==========================================================================
    // CLI argument parsing tests - PluginListArgs
    // ==========================================================================

    #[test]
    fn test_plugin_list_args_default() {
        let args = PluginListArgs {
            json: false,
            enabled: false,
            disabled: false,
        };

        assert!(!args.json, "json should be false by default");
        assert!(!args.enabled, "enabled filter should be false by default");
        assert!(!args.disabled, "disabled filter should be false by default");
    }

    #[test]
    fn test_plugin_list_args_json_flag() {
        let args = PluginListArgs {
            json: true,
            enabled: false,
            disabled: false,
        };

        assert!(args.json, "json flag should be true when set");
    }

    #[test]
    fn test_plugin_list_args_enabled_filter() {
        let args = PluginListArgs {
            json: false,
            enabled: true,
            disabled: false,
        };

        assert!(args.enabled, "enabled filter should be true when set");
        assert!(!args.disabled, "disabled filter should be false");
    }

    #[test]
    fn test_plugin_list_args_disabled_filter() {
        let args = PluginListArgs {
            json: false,
            enabled: false,
            disabled: true,
        };

        assert!(!args.enabled, "enabled filter should be false");
        assert!(args.disabled, "disabled filter should be true when set");
    }

    // ==========================================================================
    // CLI argument parsing tests - PluginInstallArgs
    // ==========================================================================

    #[test]
    fn test_plugin_install_args_minimal() {
        let args = PluginInstallArgs {
            name: "my-plugin".to_string(),
            version: None,
            force: false,
        };

        assert_eq!(args.name, "my-plugin", "name should match");
        assert!(args.version.is_none(), "version should be None by default");
        assert!(!args.force, "force should be false by default");
    }

    #[test]
    fn test_plugin_install_args_with_version() {
        let args = PluginInstallArgs {
            name: "versioned-plugin".to_string(),
            version: Some("1.2.3".to_string()),
            force: false,
        };

        assert_eq!(args.name, "versioned-plugin", "name should match");
        assert_eq!(
            args.version,
            Some("1.2.3".to_string()),
            "version should be set"
        );
    }

    #[test]
    fn test_plugin_install_args_with_force() {
        let args = PluginInstallArgs {
            name: "forced-plugin".to_string(),
            version: None,
            force: true,
        };

        assert_eq!(args.name, "forced-plugin", "name should match");
        assert!(args.force, "force should be true when set");
    }

    #[test]
    fn test_plugin_install_args_full() {
        let args = PluginInstallArgs {
            name: "full-plugin".to_string(),
            version: Some("2.0.0-beta".to_string()),
            force: true,
        };

        assert_eq!(args.name, "full-plugin", "name should match");
        assert_eq!(
            args.version,
            Some("2.0.0-beta".to_string()),
            "version should match"
        );
        assert!(args.force, "force should be true");
    }

    // ==========================================================================
    // CLI argument parsing tests - PluginRemoveArgs
    // ==========================================================================

    #[test]
    fn test_plugin_remove_args_minimal() {
        let args = PluginRemoveArgs {
            name: "remove-me".to_string(),
            yes: false,
        };

        assert_eq!(args.name, "remove-me", "name should match");
        assert!(!args.yes, "yes should be false by default");
    }

    #[test]
    fn test_plugin_remove_args_with_yes() {
        let args = PluginRemoveArgs {
            name: "remove-confirmed".to_string(),
            yes: true,
        };

        assert_eq!(args.name, "remove-confirmed", "name should match");
        assert!(args.yes, "yes should be true when set");
    }

    // ==========================================================================
    // CLI argument parsing tests - PluginEnableArgs / PluginDisableArgs
    // ==========================================================================

    #[test]
    fn test_plugin_enable_args() {
        let args = PluginEnableArgs {
            name: "enable-me".to_string(),
        };

        assert_eq!(args.name, "enable-me", "name should match");
    }

    #[test]
    fn test_plugin_disable_args() {
        let args = PluginDisableArgs {
            name: "disable-me".to_string(),
        };

        assert_eq!(args.name, "disable-me", "name should match");
    }

    // ==========================================================================
    // CLI argument parsing tests - PluginShowArgs
    // ==========================================================================

    #[test]
    fn test_plugin_show_args_minimal() {
        let args = PluginShowArgs {
            name: "show-me".to_string(),
            json: false,
        };

        assert_eq!(args.name, "show-me", "name should match");
        assert!(!args.json, "json should be false by default");
    }

    #[test]
    fn test_plugin_show_args_with_json() {
        let args = PluginShowArgs {
            name: "show-json".to_string(),
            json: true,
        };

        assert_eq!(args.name, "show-json", "name should match");
        assert!(args.json, "json should be true when set");
    }

    // ==========================================================================
    // PluginCli command structure tests
    // ==========================================================================

    #[test]
    fn test_plugin_cli_command_exists() {
        let cmd = PluginCli::command();
        assert!(
            cmd.get_subcommands().count() > 0,
            "PluginCli should have subcommands"
        );
    }

    #[test]
    fn test_plugin_cli_has_list_subcommand() {
        let cmd = PluginCli::command();
        let list_cmd = cmd.get_subcommands().find(|c| c.get_name() == "list");
        assert!(
            list_cmd.is_some(),
            "PluginCli should have 'list' subcommand"
        );
    }

    #[test]
    fn test_plugin_cli_has_install_subcommand() {
        let cmd = PluginCli::command();
        let install_cmd = cmd.get_subcommands().find(|c| c.get_name() == "install");
        assert!(
            install_cmd.is_some(),
            "PluginCli should have 'install' subcommand"
        );
    }

    #[test]
    fn test_plugin_cli_has_remove_subcommand() {
        let cmd = PluginCli::command();
        let remove_cmd = cmd.get_subcommands().find(|c| c.get_name() == "remove");
        assert!(
            remove_cmd.is_some(),
            "PluginCli should have 'remove' subcommand"
        );
    }

    #[test]
    fn test_plugin_cli_has_enable_subcommand() {
        let cmd = PluginCli::command();
        let enable_cmd = cmd.get_subcommands().find(|c| c.get_name() == "enable");
        assert!(
            enable_cmd.is_some(),
            "PluginCli should have 'enable' subcommand"
        );
    }

    #[test]
    fn test_plugin_cli_has_disable_subcommand() {
        let cmd = PluginCli::command();
        let disable_cmd = cmd.get_subcommands().find(|c| c.get_name() == "disable");
        assert!(
            disable_cmd.is_some(),
            "PluginCli should have 'disable' subcommand"
        );
    }

    #[test]
    fn test_plugin_cli_has_show_subcommand() {
        let cmd = PluginCli::command();
        let show_cmd = cmd.get_subcommands().find(|c| c.get_name() == "show");
        assert!(
            show_cmd.is_some(),
            "PluginCli should have 'show' subcommand"
        );
    }

    #[test]
    fn test_plugin_cli_list_has_ls_alias() {
        let cmd = PluginCli::command();
        let list_cmd = cmd.get_subcommands().find(|c| c.get_name() == "list");
        if let Some(list) = list_cmd {
            let aliases: Vec<_> = list.get_visible_aliases().collect();
            assert!(
                aliases.contains(&"ls"),
                "list command should have 'ls' alias"
            );
        }
    }

    #[test]
    fn test_plugin_cli_install_has_add_alias() {
        let cmd = PluginCli::command();
        let install_cmd = cmd.get_subcommands().find(|c| c.get_name() == "install");
        if let Some(install) = install_cmd {
            let aliases: Vec<_> = install.get_visible_aliases().collect();
            assert!(
                aliases.contains(&"add"),
                "install command should have 'add' alias"
            );
        }
    }

    #[test]
    fn test_plugin_cli_remove_has_rm_alias() {
        let cmd = PluginCli::command();
        let remove_cmd = cmd.get_subcommands().find(|c| c.get_name() == "remove");
        if let Some(remove) = remove_cmd {
            let aliases: Vec<_> = remove.get_visible_aliases().collect();
            assert!(
                aliases.contains(&"rm"),
                "remove command should have 'rm' alias"
            );
        }
    }

    #[test]
    fn test_plugin_cli_remove_has_uninstall_alias() {
        let cmd = PluginCli::command();
        let remove_cmd = cmd.get_subcommands().find(|c| c.get_name() == "remove");
        if let Some(remove) = remove_cmd {
            let aliases: Vec<_> = remove.get_visible_aliases().collect();
            assert!(
                aliases.contains(&"uninstall"),
                "remove command should have 'uninstall' alias"
            );
        }
    }

    #[test]
    fn test_plugin_cli_show_has_info_alias() {
        let cmd = PluginCli::command();
        let show_cmd = cmd.get_subcommands().find(|c| c.get_name() == "show");
        if let Some(show) = show_cmd {
            let aliases: Vec<_> = show.get_visible_aliases().collect();
            assert!(
                aliases.contains(&"info"),
                "show command should have 'info' alias"
            );
        }
    }

    // ==========================================================================
    // PluginSubcommand variant tests
    // ==========================================================================

    #[test]
    fn test_plugin_subcommand_list_variant() {
        let args = PluginListArgs {
            json: true,
            enabled: false,
            disabled: false,
        };
        let subcmd = PluginSubcommand::List(args);

        match subcmd {
            PluginSubcommand::List(list_args) => {
                assert!(list_args.json, "List variant should contain correct args");
            }
            _ => panic!("Expected List variant"),
        }
    }

    #[test]
    fn test_plugin_subcommand_install_variant() {
        let args = PluginInstallArgs {
            name: "test".to_string(),
            version: Some("1.0.0".to_string()),
            force: true,
        };
        let subcmd = PluginSubcommand::Install(args);

        match subcmd {
            PluginSubcommand::Install(install_args) => {
                assert_eq!(
                    install_args.name, "test",
                    "Install variant should contain correct args"
                );
                assert_eq!(install_args.version, Some("1.0.0".to_string()));
                assert!(install_args.force);
            }
            _ => panic!("Expected Install variant"),
        }
    }

    #[test]
    fn test_plugin_subcommand_remove_variant() {
        let args = PluginRemoveArgs {
            name: "remove-test".to_string(),
            yes: true,
        };
        let subcmd = PluginSubcommand::Remove(args);

        match subcmd {
            PluginSubcommand::Remove(remove_args) => {
                assert_eq!(
                    remove_args.name, "remove-test",
                    "Remove variant should contain correct args"
                );
                assert!(remove_args.yes);
            }
            _ => panic!("Expected Remove variant"),
        }
    }

    #[test]
    fn test_plugin_subcommand_enable_variant() {
        let args = PluginEnableArgs {
            name: "enable-test".to_string(),
        };
        let subcmd = PluginSubcommand::Enable(args);

        match subcmd {
            PluginSubcommand::Enable(enable_args) => {
                assert_eq!(
                    enable_args.name, "enable-test",
                    "Enable variant should contain correct args"
                );
            }
            _ => panic!("Expected Enable variant"),
        }
    }

    #[test]
    fn test_plugin_subcommand_disable_variant() {
        let args = PluginDisableArgs {
            name: "disable-test".to_string(),
        };
        let subcmd = PluginSubcommand::Disable(args);

        match subcmd {
            PluginSubcommand::Disable(disable_args) => {
                assert_eq!(
                    disable_args.name, "disable-test",
                    "Disable variant should contain correct args"
                );
            }
            _ => panic!("Expected Disable variant"),
        }
    }

    #[test]
    fn test_plugin_subcommand_show_variant() {
        let args = PluginShowArgs {
            name: "show-test".to_string(),
            json: true,
        };
        let subcmd = PluginSubcommand::Show(args);

        match subcmd {
            PluginSubcommand::Show(show_args) => {
                assert_eq!(
                    show_args.name, "show-test",
                    "Show variant should contain correct args"
                );
                assert!(show_args.json);
            }
            _ => panic!("Expected Show variant"),
        }
    }

    // ==========================================================================
    // Debug trait tests
    // ==========================================================================

    #[test]
    fn test_plugin_list_args_debug() {
        let args = PluginListArgs {
            json: true,
            enabled: false,
            disabled: true,
        };
        let debug_output = format!("{:?}", args);

        assert!(
            debug_output.contains("PluginListArgs"),
            "Debug should include type name"
        );
        assert!(
            debug_output.contains("json"),
            "Debug should include json field"
        );
        assert!(
            debug_output.contains("enabled"),
            "Debug should include enabled field"
        );
        assert!(
            debug_output.contains("disabled"),
            "Debug should include disabled field"
        );
    }

    #[test]
    fn test_plugin_install_args_debug() {
        let args = PluginInstallArgs {
            name: "test-plugin".to_string(),
            version: Some("1.0.0".to_string()),
            force: true,
        };
        let debug_output = format!("{:?}", args);

        assert!(
            debug_output.contains("PluginInstallArgs"),
            "Debug should include type name"
        );
        assert!(
            debug_output.contains("test-plugin"),
            "Debug should include name"
        );
        assert!(
            debug_output.contains("1.0.0"),
            "Debug should include version"
        );
    }

    #[test]
    fn test_plugin_subcommand_debug() {
        let subcmd = PluginSubcommand::Enable(PluginEnableArgs {
            name: "test".to_string(),
        });
        let debug_output = format!("{:?}", subcmd);

        assert!(
            debug_output.contains("Enable"),
            "Debug should include variant name"
        );
        assert!(
            debug_output.contains("test"),
            "Debug should include contained name"
        );
    }

    #[test]
    fn test_plugin_cli_debug() {
        let cli = PluginCli {
            subcommand: PluginSubcommand::List(PluginListArgs {
                json: false,
                enabled: false,
                disabled: false,
            }),
        };
        let debug_output = format!("{:?}", cli);

        assert!(
            debug_output.contains("PluginCli"),
            "Debug should include type name"
        );
        assert!(
            debug_output.contains("List"),
            "Debug should include subcommand variant"
        );
    }
}
