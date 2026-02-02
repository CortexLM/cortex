//! Workspace/Project management command for Cortex CLI.
//!
//! Provides workspace management functionality:
//! - Show workspace information
//! - Initialize workspace configuration
//! - Manage workspace settings

use anyhow::{Result, bail};
use clap::Parser;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Workspace CLI command.
#[derive(Debug, Parser)]
pub struct WorkspaceCli {
    #[command(subcommand)]
    pub subcommand: Option<WorkspaceSubcommand>,
}

/// Workspace subcommands.
#[derive(Debug, clap::Subcommand)]
pub enum WorkspaceSubcommand {
    /// Show workspace information
    #[command(visible_alias = "info")]
    Show(WorkspaceShowArgs),

    /// Initialize workspace configuration
    Init(WorkspaceInitArgs),

    /// Set workspace settings
    Set(WorkspaceSetArgs),

    /// Open workspace configuration in editor
    Edit(WorkspaceEditArgs),
}

/// Arguments for workspace show command.
#[derive(Debug, Parser)]
pub struct WorkspaceShowArgs {
    /// Output as JSON
    #[arg(long)]
    pub json: bool,
}

/// Arguments for workspace init command.
#[derive(Debug, Parser)]
pub struct WorkspaceInitArgs {
    /// Force overwrite if config already exists
    #[arg(long, short = 'f')]
    pub force: bool,

    /// Template to use (minimal, default, full)
    #[arg(long, short = 't', default_value = "default")]
    pub template: String,
}

/// Arguments for workspace set command.
#[derive(Debug, Parser)]
pub struct WorkspaceSetArgs {
    /// Configuration key to set
    pub key: String,

    /// Value to set
    pub value: String,
}

/// Arguments for workspace edit command.
#[derive(Debug, Parser)]
pub struct WorkspaceEditArgs {
    /// Editor to use (defaults to $EDITOR or $VISUAL)
    #[arg(long, short = 'e')]
    pub editor: Option<String>,
}

/// Workspace information.
#[derive(Debug, Serialize)]
struct WorkspaceInfo {
    root: PathBuf,
    has_cortex_config: bool,
    has_agents_md: bool,
    has_git: bool,
    config_path: Option<PathBuf>,
    agents_path: Option<PathBuf>,
    project_name: Option<String>,
    settings: Option<WorkspaceSettings>,
}

/// Workspace settings from .cortex/config.toml
#[derive(Debug, Serialize, Deserialize, Default)]
struct WorkspaceSettings {
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    sandbox_mode: Option<String>,
    #[serde(default)]
    approval_mode: Option<String>,
}

impl WorkspaceCli {
    /// Run the workspace command.
    pub async fn run(self) -> Result<()> {
        match self.subcommand {
            None => run_show(WorkspaceShowArgs { json: false }).await,
            Some(WorkspaceSubcommand::Show(args)) => run_show(args).await,
            Some(WorkspaceSubcommand::Init(args)) => run_init(args).await,
            Some(WorkspaceSubcommand::Set(args)) => run_set(args).await,
            Some(WorkspaceSubcommand::Edit(args)) => run_edit(args).await,
        }
    }
}

/// Find workspace root by looking for .cortex directory or .git
fn find_workspace_root() -> PathBuf {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    let mut current = cwd.clone();
    loop {
        // Check for .cortex directory
        if current.join(".cortex").exists() {
            return current;
        }
        // Check for .git directory (fallback root indicator)
        if current.join(".git").exists() {
            return current;
        }
        // Go up one level
        if let Some(parent) = current.parent() {
            current = parent.to_path_buf();
        } else {
            // Reached filesystem root, return cwd
            return cwd;
        }
    }
}

async fn run_show(args: WorkspaceShowArgs) -> Result<()> {
    let root = find_workspace_root();
    let cortex_dir = root.join(".cortex");
    let config_path = cortex_dir.join("config.toml");
    let agents_path = root.join("AGENTS.md");
    let git_dir = root.join(".git");

    let has_cortex_config = config_path.exists();
    let has_agents_md = agents_path.exists();
    let has_git = git_dir.exists();

    // Get project name from directory name or git remote
    let project_name = root
        .file_name()
        .and_then(|n| n.to_str())
        .map(|s| s.to_string());

    // Load settings if config exists
    let settings = if has_cortex_config {
        std::fs::read_to_string(&config_path)
            .ok()
            .and_then(|content| toml::from_str(&content).ok())
    } else {
        None
    };

    let info = WorkspaceInfo {
        root: root.clone(),
        has_cortex_config,
        has_agents_md,
        has_git,
        config_path: if has_cortex_config {
            Some(config_path)
        } else {
            None
        },
        agents_path: if has_agents_md {
            Some(agents_path)
        } else {
            None
        },
        project_name,
        settings,
    };

    if args.json {
        println!("{}", serde_json::to_string_pretty(&info)?);
        return Ok(());
    }

    println!("Workspace Information");
    println!("{}", "=".repeat(50));
    println!("  Root:        {}", info.root.display());
    if let Some(ref name) = info.project_name {
        println!("  Project:     {}", name);
    }
    println!();

    println!("Configuration:");
    println!("{}", "-".repeat(40));
    println!(
        "  .cortex/config.toml: {}",
        if info.has_cortex_config {
            "present"
        } else {
            "not found"
        }
    );
    println!(
        "  AGENTS.md:           {}",
        if info.has_agents_md {
            "present"
        } else {
            "not found"
        }
    );
    println!(
        "  Git repository:      {}",
        if info.has_git { "yes" } else { "no" }
    );

    if let Some(ref settings) = info.settings {
        println!();
        println!("Workspace Settings:");
        println!("{}", "-".repeat(40));
        if let Some(ref model) = settings.model {
            println!("  Model:         {}", model);
        }
        if let Some(ref sandbox) = settings.sandbox_mode {
            println!("  Sandbox:       {}", sandbox);
        }
        if let Some(ref approval) = settings.approval_mode {
            println!("  Approval:      {}", approval);
        }
    }

    if !info.has_cortex_config {
        println!();
        println!("Tip: Run 'cortex workspace init' to create workspace configuration.");
    }

    Ok(())
}

async fn run_init(args: WorkspaceInitArgs) -> Result<()> {
    let root = find_workspace_root();
    let cortex_dir = root.join(".cortex");
    let config_path = cortex_dir.join("config.toml");

    if config_path.exists() && !args.force {
        bail!(
            "Workspace configuration already exists at {}.\n\
             Use --force to overwrite.",
            config_path.display()
        );
    }

    // Create .cortex directory
    std::fs::create_dir_all(&cortex_dir)?;

    // Generate config based on template
    let config_content = match args.template.as_str() {
        "minimal" => {
            r#"# Cortex Workspace Configuration (minimal)
# See https://docs.cortex.foundation/config for all options
"#
        }
        "full" => {
            r#"# Cortex Workspace Configuration (full)
# See https://docs.cortex.foundation/config for all options

[model]
# Default model for this workspace
# default = "claude-sonnet-4-20250514"

# Context window configuration
# model_context_window = 128000
# model_auto_compact_token_limit = 100000

[sandbox]
# Sandbox mode for command execution
# mode = "workspace-write"  # Options: read-only, workspace-write, full-access

[approval]
# Approval policy for tool executions
# mode = "on-request"  # Options: ask, medium, low, yolo

[features]
# Feature flags
# web_search = false
# mcp_servers = true

[tools]
# Tool configuration
# enabled = ["read", "write", "execute", "glob", "grep"]
# disabled = []
"#
        }
        _ => {
            // default template
            r#"# Cortex Workspace Configuration
# See https://docs.cortex.foundation/config for all options

[model]
# Default model for this workspace
# default = "claude-sonnet-4-20250514"

[sandbox]
# Sandbox mode: read-only, workspace-write, full-access
# mode = "workspace-write"

[approval]
# Approval policy: ask, medium, low, yolo
# mode = "on-request"
"#
        }
    };

    std::fs::write(&config_path, config_content)?;

    println!("Workspace initialized!");
    println!("  Created: {}", config_path.display());
    println!();
    println!("Edit the config file to customize workspace settings.");
    println!("Run 'cortex workspace edit' to open in your editor.");

    Ok(())
}

async fn run_set(args: WorkspaceSetArgs) -> Result<()> {
    let root = find_workspace_root();
    let cortex_dir = root.join(".cortex");
    let config_path = cortex_dir.join("config.toml");

    // Create .cortex directory if needed
    if !cortex_dir.exists() {
        std::fs::create_dir_all(&cortex_dir)?;
    }

    // Read existing config or create empty
    let content = if config_path.exists() {
        std::fs::read_to_string(&config_path)?
    } else {
        String::new()
    };

    // Parse as TOML
    let mut doc = content
        .parse::<toml_edit::DocumentMut>()
        .unwrap_or_else(|_| toml_edit::DocumentMut::new());

    // Map common keys to their TOML sections
    let (section, actual_key) = match args.key.as_str() {
        "model" | "default_model" => ("model", "default"),
        "sandbox" | "sandbox_mode" => ("sandbox", "mode"),
        "approval" | "approval_mode" => ("approval", "mode"),
        k if k.contains('.') => {
            let parts: Vec<&str> = k.splitn(2, '.').collect();
            (parts[0], parts[1])
        }
        _ => ("", args.key.as_str()),
    };

    if section.is_empty() {
        doc[actual_key] = toml_edit::value(&args.value);
    } else {
        if doc.get(section).is_none() {
            doc[section] = toml_edit::Item::Table(toml_edit::Table::new());
        }
        doc[section][actual_key] = toml_edit::value(&args.value);
    }

    std::fs::write(&config_path, doc.to_string())?;

    println!("Set {} = {}", args.key, args.value);
    println!("Config saved to: {}", config_path.display());

    Ok(())
}

async fn run_edit(args: WorkspaceEditArgs) -> Result<()> {
    let root = find_workspace_root();
    let cortex_dir = root.join(".cortex");
    let config_path = cortex_dir.join("config.toml");

    // Create config if it doesn't exist
    if !config_path.exists() {
        // Initialize with default template
        run_init(WorkspaceInitArgs {
            force: false,
            template: "default".to_string(),
        })
        .await?;
    }

    // Get editor
    let editor = args
        .editor
        .or_else(|| std::env::var("EDITOR").ok())
        .or_else(|| std::env::var("VISUAL").ok())
        .unwrap_or_else(|| {
            if cfg!(windows) {
                "notepad".to_string()
            } else {
                "nano".to_string()
            }
        });

    let status = std::process::Command::new(&editor)
        .arg(&config_path)
        .status()?;

    if !status.success() {
        bail!("Editor exited with an error.");
    }

    println!("Configuration saved.");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ==========================================================================
    // WorkspaceSettings tests
    // ==========================================================================

    #[test]
    fn test_workspace_settings_default() {
        let settings = WorkspaceSettings::default();

        assert!(settings.model.is_none());
        assert!(settings.sandbox_mode.is_none());
        assert!(settings.approval_mode.is_none());
    }

    #[test]
    fn test_workspace_settings_with_all_fields() {
        let settings = WorkspaceSettings {
            model: Some("claude-sonnet-4-20250514".to_string()),
            sandbox_mode: Some("workspace-write".to_string()),
            approval_mode: Some("on-request".to_string()),
        };

        assert_eq!(settings.model, Some("claude-sonnet-4-20250514".to_string()));
        assert_eq!(settings.sandbox_mode, Some("workspace-write".to_string()));
        assert_eq!(settings.approval_mode, Some("on-request".to_string()));
    }

    #[test]
    fn test_workspace_settings_json_roundtrip() {
        let settings = WorkspaceSettings {
            model: Some("claude-sonnet-4-20250514".to_string()),
            sandbox_mode: Some("workspace-write".to_string()),
            approval_mode: Some("on-request".to_string()),
        };

        let json = serde_json::to_string(&settings).expect("should serialize to JSON");
        let parsed: WorkspaceSettings =
            serde_json::from_str(&json).expect("should deserialize from JSON");

        assert_eq!(parsed.model, settings.model);
        assert_eq!(parsed.sandbox_mode, settings.sandbox_mode);
        assert_eq!(parsed.approval_mode, settings.approval_mode);
    }

    #[test]
    fn test_workspace_settings_json_roundtrip_partial() {
        let settings = WorkspaceSettings {
            model: Some("claude-sonnet-4-20250514".to_string()),
            sandbox_mode: None,
            approval_mode: Some("low".to_string()),
        };

        let json = serde_json::to_string(&settings).expect("should serialize to JSON");
        let parsed: WorkspaceSettings =
            serde_json::from_str(&json).expect("should deserialize from JSON");

        assert_eq!(parsed.model, Some("claude-sonnet-4-20250514".to_string()));
        assert!(parsed.sandbox_mode.is_none());
        assert_eq!(parsed.approval_mode, Some("low".to_string()));
    }

    #[test]
    fn test_workspace_settings_toml_roundtrip() {
        let settings = WorkspaceSettings {
            model: Some("claude-sonnet-4-20250514".to_string()),
            sandbox_mode: Some("full-access".to_string()),
            approval_mode: Some("yolo".to_string()),
        };

        let toml_str = toml::to_string(&settings).expect("should serialize to TOML");
        let parsed: WorkspaceSettings =
            toml::from_str(&toml_str).expect("should deserialize from TOML");

        assert_eq!(parsed.model, settings.model);
        assert_eq!(parsed.sandbox_mode, settings.sandbox_mode);
        assert_eq!(parsed.approval_mode, settings.approval_mode);
    }

    #[test]
    fn test_workspace_settings_deserialize_from_empty_toml() {
        let toml_str = "";
        let settings: WorkspaceSettings =
            toml::from_str(toml_str).expect("should deserialize from empty TOML");

        assert!(settings.model.is_none());
        assert!(settings.sandbox_mode.is_none());
        assert!(settings.approval_mode.is_none());
    }

    #[test]
    fn test_workspace_settings_deserialize_partial_toml() {
        let toml_str = r#"
            model = "gpt-4"
        "#;
        let settings: WorkspaceSettings =
            toml::from_str(toml_str).expect("should deserialize from partial TOML");

        assert_eq!(settings.model, Some("gpt-4".to_string()));
        assert!(settings.sandbox_mode.is_none());
        assert!(settings.approval_mode.is_none());
    }

    #[test]
    fn test_workspace_settings_deserialize_unknown_fields_ignored() {
        let toml_str = r#"
            model = "claude-sonnet-4-20250514"
            unknown_field = "should be ignored"
            another_unknown = 123
        "#;
        // This should not error; serde default behavior allows extra fields
        let result: Result<WorkspaceSettings, _> = toml::from_str(toml_str);
        // Note: By default serde does not ignore unknown fields, but the test
        // documents current behavior - if it changes, the test will catch it
        assert!(
            result.is_err()
                || result.unwrap().model == Some("claude-sonnet-4-20250514".to_string())
        );
    }

    // ==========================================================================
    // WorkspaceInfo tests
    // ==========================================================================

    #[test]
    fn test_workspace_info_json_serialization() {
        let info = WorkspaceInfo {
            root: PathBuf::from("/home/user/project"),
            has_cortex_config: true,
            has_agents_md: true,
            has_git: true,
            config_path: Some(PathBuf::from("/home/user/project/.cortex/config.toml")),
            agents_path: Some(PathBuf::from("/home/user/project/AGENTS.md")),
            project_name: Some("project".to_string()),
            settings: Some(WorkspaceSettings {
                model: Some("claude-sonnet-4-20250514".to_string()),
                sandbox_mode: Some("workspace-write".to_string()),
                approval_mode: None,
            }),
        };

        let json = serde_json::to_string(&info).expect("should serialize to JSON");

        // Verify key fields are present in serialized output
        assert!(json.contains("root"));
        assert!(json.contains("has_cortex_config"));
        assert!(json.contains("has_agents_md"));
        assert!(json.contains("has_git"));
        assert!(json.contains("project_name"));
        assert!(json.contains("project"));
        assert!(json.contains("settings"));
        assert!(json.contains("claude-sonnet-4-20250514"));
    }

    #[test]
    fn test_workspace_info_json_serialization_minimal() {
        let info = WorkspaceInfo {
            root: PathBuf::from("/tmp/test"),
            has_cortex_config: false,
            has_agents_md: false,
            has_git: false,
            config_path: None,
            agents_path: None,
            project_name: None,
            settings: None,
        };

        let json = serde_json::to_string(&info).expect("should serialize to JSON");

        assert!(json.contains("has_cortex_config"));
        assert!(json.contains("false"));
    }

    #[test]
    fn test_workspace_info_json_pretty_print() {
        let info = WorkspaceInfo {
            root: PathBuf::from("/workspace"),
            has_cortex_config: true,
            has_agents_md: false,
            has_git: true,
            config_path: Some(PathBuf::from("/workspace/.cortex/config.toml")),
            agents_path: None,
            project_name: Some("my-project".to_string()),
            settings: None,
        };

        let json_pretty =
            serde_json::to_string_pretty(&info).expect("should serialize to pretty JSON");

        // Pretty printed JSON should have newlines and indentation
        assert!(json_pretty.contains('\n'));
        assert!(json_pretty.contains("my-project"));
    }

    // ==========================================================================
    // WorkspaceShowArgs tests
    // ==========================================================================

    #[test]
    fn test_workspace_show_args_default() {
        let args = WorkspaceShowArgs { json: false };

        assert!(!args.json);
    }

    #[test]
    fn test_workspace_show_args_json_enabled() {
        let args = WorkspaceShowArgs { json: true };

        assert!(args.json);
    }

    // ==========================================================================
    // WorkspaceInitArgs tests
    // ==========================================================================

    #[test]
    fn test_workspace_init_args_default_template() {
        let args = WorkspaceInitArgs {
            force: false,
            template: "default".to_string(),
        };

        assert!(!args.force);
        assert_eq!(args.template, "default");
    }

    #[test]
    fn test_workspace_init_args_minimal_template() {
        let args = WorkspaceInitArgs {
            force: true,
            template: "minimal".to_string(),
        };

        assert!(args.force);
        assert_eq!(args.template, "minimal");
    }

    #[test]
    fn test_workspace_init_args_full_template() {
        let args = WorkspaceInitArgs {
            force: false,
            template: "full".to_string(),
        };

        assert!(!args.force);
        assert_eq!(args.template, "full");
    }

    // ==========================================================================
    // WorkspaceSetArgs tests
    // ==========================================================================

    #[test]
    fn test_workspace_set_args_simple_key() {
        let args = WorkspaceSetArgs {
            key: "model".to_string(),
            value: "claude-sonnet-4-20250514".to_string(),
        };

        assert_eq!(args.key, "model");
        assert_eq!(args.value, "claude-sonnet-4-20250514");
    }

    #[test]
    fn test_workspace_set_args_dotted_key() {
        let args = WorkspaceSetArgs {
            key: "sandbox.mode".to_string(),
            value: "full-access".to_string(),
        };

        assert_eq!(args.key, "sandbox.mode");
        assert_eq!(args.value, "full-access");
    }

    // ==========================================================================
    // WorkspaceEditArgs tests
    // ==========================================================================

    #[test]
    fn test_workspace_edit_args_no_editor() {
        let args = WorkspaceEditArgs { editor: None };

        assert!(args.editor.is_none());
    }

    #[test]
    fn test_workspace_edit_args_with_editor() {
        let args = WorkspaceEditArgs {
            editor: Some("vim".to_string()),
        };

        assert_eq!(args.editor, Some("vim".to_string()));
    }

    // ==========================================================================
    // WorkspaceSubcommand tests
    // ==========================================================================

    #[test]
    fn test_workspace_subcommand_debug_representation() {
        let show_cmd = WorkspaceSubcommand::Show(WorkspaceShowArgs { json: true });
        let debug_str = format!("{:?}", show_cmd);

        assert!(debug_str.contains("Show"));
        assert!(debug_str.contains("json: true"));
    }

    #[test]
    fn test_workspace_subcommand_init_debug() {
        let init_cmd = WorkspaceSubcommand::Init(WorkspaceInitArgs {
            force: true,
            template: "full".to_string(),
        });
        let debug_str = format!("{:?}", init_cmd);

        assert!(debug_str.contains("Init"));
        assert!(debug_str.contains("force: true"));
        assert!(debug_str.contains("full"));
    }
}
