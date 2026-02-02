//! Per-project configuration support.
//!
//! This module provides functionality to:
//! - Discover project-level config files (`.cortex/config.toml`, `cortex.toml`, or JSON variants)
//! - Load and parse project configurations (TOML and JSONC formats)
//! - Merge global, project, and CLI configurations with proper precedence

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::config_discovery::{find_project_root, find_up};
use super::loader::{ConfigFormat, parse_config_content};
use super::types::ConfigToml;
use tracing::{debug, trace};

/// Project config file names to search for (in priority order).
pub const PROJECT_CONFIG_NAMES: &[&str] = &[
    ".cortex/config.toml",  // Standard TOML location
    ".cortex/config.json",  // JSON location
    ".cortex/config.jsonc", // JSONC location
    "cortex.toml",          // Alternative TOML at project root
    "cortex.json",          // Alternative JSON at project root
    "cortex.jsonc",         // Alternative JSONC at project root
];

/// Find the project configuration file by walking up the directory tree.
///
/// Searches for `.cortex/config.toml` or `cortex.toml` starting from the
/// given directory, walking up to the project root (git root or filesystem root).
///
/// # Arguments
/// * `start_dir` - Directory to start searching from
///
/// # Returns
/// * `Some(PathBuf)` - Path to the found config file
/// * `None` - No project config found
pub fn find_project_config(start_dir: &Path) -> Option<PathBuf> {
    trace!(start_dir = %start_dir.display(), "Searching for project config");

    // Try each config name in priority order
    for config_name in PROJECT_CONFIG_NAMES {
        if let Some(config_path) = find_up(start_dir, config_name) {
            debug!(path = %config_path.display(), "Found project config");
            return Some(config_path);
        }
    }

    // Also check for .cortex directory at project root (all formats)
    if let Some(project_root) = find_project_root(start_dir) {
        let cortex_dir = project_root.join(".cortex");
        for config_name in &["config.toml", "config.json", "config.jsonc"] {
            let cortex_dir_config = cortex_dir.join(config_name);
            if cortex_dir_config.exists() {
                debug!(path = %cortex_dir_config.display(), "Found project config in .cortex directory");
                return Some(cortex_dir_config);
            }
        }
    }

    debug!("No project config found");
    None
}

/// Load project configuration from the given path.
///
/// Supports both TOML and JSONC formats based on file extension.
///
/// # Arguments
/// * `config_path` - Path to the project config file
///
/// # Returns
/// * `Ok(ConfigToml)` - Parsed configuration
/// * `Err` - If the file cannot be read or parsed
pub fn load_project_config(config_path: &Path) -> std::io::Result<ConfigToml> {
    debug!(path = %config_path.display(), "Loading project config");

    let content = std::fs::read_to_string(config_path)?;
    let format = ConfigFormat::from_path(config_path);

    parse_config_content(&content, format).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "Failed to parse project config at {}: {e}",
                config_path.display()
            ),
        )
    })
}

/// Load project configuration asynchronously.
///
/// Supports both TOML and JSONC formats based on file extension.
pub async fn load_project_config_async(config_path: &Path) -> std::io::Result<ConfigToml> {
    debug!(path = %config_path.display(), "Loading project config (async)");

    let content = tokio::fs::read_to_string(config_path).await?;
    let format = ConfigFormat::from_path(config_path);

    parse_config_content(&content, format).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "Failed to parse project config at {}: {e}",
                config_path.display()
            ),
        )
    })
}

/// Merge configurations with proper precedence: global → project → CLI.
///
/// Values are merged with later sources taking precedence over earlier ones.
/// HashMap fields (like `mcp_servers`) are merged additively.
///
/// # Arguments
/// * `global` - Global configuration from `~/.cortex/config.toml`
/// * `project` - Project configuration from `.cortex/config.toml` or `cortex.toml`
///
/// # Returns
/// * Merged `ConfigToml` with project values overriding global values
pub fn merge_configs(global: ConfigToml, project: Option<ConfigToml>) -> ConfigToml {
    let Some(project) = project else {
        return global;
    };

    debug!("Merging global and project configs");

    ConfigToml {
        // Simple fields: project overrides global
        model: project.model.or(global.model),
        model_provider: project.model_provider.or(global.model_provider),
        model_context_window: project.model_context_window.or(global.model_context_window),
        model_auto_compact_token_limit: project
            .model_auto_compact_token_limit
            .or(global.model_auto_compact_token_limit),
        approval_policy: project.approval_policy.or(global.approval_policy),
        sandbox_mode: project.sandbox_mode.or(global.sandbox_mode),
        sandbox_workspace_write: project
            .sandbox_workspace_write
            .or(global.sandbox_workspace_write),
        instructions: merge_instructions(global.instructions, project.instructions),
        history: project.history.or(global.history),
        model_reasoning_effort: project
            .model_reasoning_effort
            .or(global.model_reasoning_effort),
        model_reasoning_summary: project
            .model_reasoning_summary
            .or(global.model_reasoning_summary),
        hide_agent_reasoning: project.hide_agent_reasoning.or(global.hide_agent_reasoning),
        show_raw_agent_reasoning: project
            .show_raw_agent_reasoning
            .or(global.show_raw_agent_reasoning),
        check_for_update_on_startup: project
            .check_for_update_on_startup
            .or(global.check_for_update_on_startup),
        disable_paste_burst: project.disable_paste_burst.or(global.disable_paste_burst),
        tui: project.tui.or(global.tui),
        current_agent: project.current_agent.or(global.current_agent),

        // Additive merge for HashMaps
        mcp_servers: merge_hash_maps(global.mcp_servers, project.mcp_servers),
        profiles: merge_hash_maps(global.profiles, project.profiles),

        // Additive merge for Vec
        trusted_directories: merge_vecs(global.trusted_directories, project.trusted_directories),

        // Permission config: project overrides global fields
        permission: if project.permission != Default::default() {
            project.permission
        } else {
            global.permission
        },

        // Custom commands: additive merge
        custom_commands: {
            let mut cmds = global.custom_commands;
            cmds.extend(project.custom_commands);
            cmds
        },

        // Plugin configurations: additive merge
        plugins: {
            let mut plugins = global.plugins;
            plugins.extend(project.plugins);
            plugins
        },

        // Plugin directories: additive merge
        plugin_dirs: merge_vecs(global.plugin_dirs, project.plugin_dirs),

        // Plugin settings: project overrides global
        plugin_settings: project.plugin_settings.or(global.plugin_settings),

        // Small model for lightweight tasks: project overrides global
        small_model: project.small_model.or(global.small_model),

        // Model aliases: additive merge (project overrides global for same alias)
        model_aliases: merge_hash_maps(global.model_aliases, project.model_aliases),

        // Custom providers: additive merge (project overrides global for same provider ID)
        providers: merge_hash_maps(global.providers, project.providers),

        // Execution config: project overrides global
        execution: if project.execution != Default::default() {
            project.execution
        } else {
            global.execution
        },
    }
}

/// Merge two HashMaps, with the second taking precedence for duplicate keys.
fn merge_hash_maps<K, V>(mut base: HashMap<K, V>, override_map: HashMap<K, V>) -> HashMap<K, V>
where
    K: std::hash::Hash + Eq,
{
    for (key, value) in override_map {
        base.insert(key, value);
    }
    base
}

/// Merge two Vecs, deduplicating values.
fn merge_vecs<T>(mut base: Vec<T>, additions: Vec<T>) -> Vec<T>
where
    T: PartialEq,
{
    for item in additions {
        if !base.contains(&item) {
            base.push(item);
        }
    }
    base
}

/// Merge instructions, combining both if present.
fn merge_instructions(global: Option<String>, project: Option<String>) -> Option<String> {
    match (global, project) {
        (Some(g), Some(p)) => Some(format!("{g}\n\n{p}")),
        (Some(g), None) => Some(g),
        (None, Some(p)) => Some(p),
        (None, None) => None,
    }
}

/// Get the project config path if it exists, for a given working directory.
pub fn get_project_config_path(cwd: &Path) -> Option<PathBuf> {
    find_project_config(cwd)
}

/// Get the directory containing the project config.
pub fn get_project_config_dir(config_path: &Path) -> PathBuf {
    // If config is .cortex/config.toml, return parent of .cortex
    // If config is cortex.toml, return its parent
    let parent = config_path.parent().unwrap_or(config_path);
    if parent.ends_with(".cortex") {
        parent.parent().unwrap_or(parent).to_path_buf()
    } else {
        parent.to_path_buf()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_find_project_config_cortex_dir() {
        let temp_dir = TempDir::new().unwrap();
        let cortex_dir = temp_dir.path().join(".cortex");
        std::fs::create_dir_all(&cortex_dir).unwrap();
        std::fs::write(cortex_dir.join("config.toml"), "model = \"test\"").unwrap();

        let found = find_project_config(temp_dir.path());
        assert!(found.is_some());
        assert!(found.unwrap().ends_with(".cortex/config.toml"));
    }

    #[test]
    fn test_find_project_config_root() {
        let temp_dir = TempDir::new().unwrap();
        std::fs::write(temp_dir.path().join("cortex.toml"), "model = \"test\"").unwrap();

        let found = find_project_config(temp_dir.path());
        assert!(found.is_some());
        assert!(found.unwrap().ends_with("cortex.toml"));
    }

    #[test]
    fn test_find_project_config_none() {
        let temp_dir = TempDir::new().unwrap();
        let found = find_project_config(temp_dir.path());
        assert!(found.is_none());
    }

    #[test]
    fn test_merge_configs_basic() {
        let global = ConfigToml {
            model: Some("global-model".to_string()),
            model_provider: Some("global-provider".to_string()),
            ..Default::default()
        };

        let project = ConfigToml {
            model: Some("project-model".to_string()),
            ..Default::default()
        };

        let merged = merge_configs(global, Some(project));
        assert_eq!(merged.model, Some("project-model".to_string()));
        assert_eq!(merged.model_provider, Some("global-provider".to_string()));
    }

    #[test]
    fn test_merge_configs_mcp_additive() {
        use super::super::types::McpServerConfig;

        let mut global_mcp = HashMap::new();
        global_mcp.insert(
            "server1".to_string(),
            McpServerConfig {
                command: "cmd1".to_string(),
                args: vec![],
                env: HashMap::new(),
                timeout_seconds: None,
            },
        );

        let mut project_mcp = HashMap::new();
        project_mcp.insert(
            "server2".to_string(),
            McpServerConfig {
                command: "cmd2".to_string(),
                args: vec![],
                env: HashMap::new(),
                timeout_seconds: None,
            },
        );

        let global = ConfigToml {
            mcp_servers: global_mcp,
            ..Default::default()
        };

        let project = ConfigToml {
            mcp_servers: project_mcp,
            ..Default::default()
        };

        let merged = merge_configs(global, Some(project));
        assert_eq!(merged.mcp_servers.len(), 2);
        assert!(merged.mcp_servers.contains_key("server1"));
        assert!(merged.mcp_servers.contains_key("server2"));
    }

    #[test]
    fn test_merge_instructions() {
        let result = merge_instructions(Some("Global".to_string()), Some("Project".to_string()));
        assert_eq!(result, Some("Global\n\nProject".to_string()));

        let result = merge_instructions(Some("Global".to_string()), None);
        assert_eq!(result, Some("Global".to_string()));

        let result = merge_instructions(None, Some("Project".to_string()));
        assert_eq!(result, Some("Project".to_string()));

        let result = merge_instructions(None, None);
        assert_eq!(result, None);
    }

    #[test]
    fn test_get_project_config_dir() {
        let path = PathBuf::from("/project/.cortex/config.toml");
        let dir = get_project_config_dir(&path);
        assert_eq!(dir, PathBuf::from("/project"));

        let path = PathBuf::from("/project/cortex.toml");
        let dir = get_project_config_dir(&path);
        assert_eq!(dir, PathBuf::from("/project"));
    }
}
