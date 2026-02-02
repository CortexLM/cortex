//! Config command handler.

use anyhow::Result;
use std::collections::HashMap;
use std::collections::HashSet;

use crate::debug_cmd::commands::ConfigArgs;
use crate::debug_cmd::types::{ConfigDebugOutput, ConfigDiff, ConfigLocations, ResolvedConfig};
use crate::debug_cmd::utils::{is_sensitive_var_name, redact_sensitive_value};

/// Run the config debug command.
pub async fn run_config(args: ConfigArgs) -> Result<()> {
    // Use catch_unwind to handle potential panics from Config::default()
    // which can occur with malformed environment variables (#2006)
    let config = std::panic::catch_unwind(cortex_engine::Config::default).map_err(|_| {
        anyhow::anyhow!(
            "Failed to load configuration. This may be caused by:\n\
            - Invalid CORTEX_HOME path\n\
            - Malformed environment variables\n\
            - Corrupted config files\n\n\
            Try: unset CORTEX_HOME RUST_BACKTRACE && cortex debug config"
        )
    })?;

    let global_config = config.cortex_home.join("config.toml");
    let local_config = std::env::current_dir()
        .ok()
        .map(|d| d.join(".cortex/config.toml"));

    let resolved = ResolvedConfig {
        model: config.model.clone(),
        provider: config.model_provider_id.clone(),
        cwd: config.cwd.clone(),
        cortex_home: config.cortex_home.clone(),
    };

    let locations = ConfigLocations {
        global_config_exists: global_config.exists(),
        global_config,
        local_config_exists: local_config.as_ref().is_some_and(|p| p.exists()),
        local_config,
    };

    let environment = if args.env {
        let mut env_vars = HashMap::new();
        // Cortex environment variables
        let cortex_vars = [
            "CORTEX_HOME",
            "CORTEX_MODEL",
            "CORTEX_PROVIDER",
            "CORTEX_API_KEY",
            "CORTEX_DEBUG",
            "CORTEX_LOG_LEVEL",
            "CORTEX_AUTH_TOKEN",
            "CORTEX_API_URL",
            // Standard environment variables
            "EDITOR",
            "VISUAL",
            "SHELL",
        ];
        for var in cortex_vars {
            if let Ok(val) = std::env::var(var) {
                // Mask sensitive values (API keys, secrets, tokens, passwords, credentials)
                let display_val = if is_sensitive_var_name(var) {
                    redact_sensitive_value(&val)
                } else {
                    val
                };
                env_vars.insert(var.to_string(), display_val);
            }
        }
        Some(env_vars)
    } else {
        None
    };

    let output = ConfigDebugOutput {
        resolved,
        locations,
        environment,
    };

    if args.json {
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        println!("Resolved Configuration");
        println!("{}", "=".repeat(50));
        println!("  Model:       {}", output.resolved.model);
        // Clarify that 'cortex' provider means requests go through Cortex backend
        let provider_desc = if output.resolved.provider == "cortex" {
            "cortex (routes to model's underlying provider)"
        } else {
            &output.resolved.provider
        };
        println!("  Provider:    {}", provider_desc);
        println!("  CWD:         {}", output.resolved.cwd.display());
        println!("  Cortex Home: {}", output.resolved.cortex_home.display());
        println!();

        println!("Config File Locations");
        println!("{}", "-".repeat(40));
        println!(
            "  Global: {} {}",
            output.locations.global_config.display(),
            if output.locations.global_config_exists {
                "(exists)"
            } else {
                "(not found)"
            }
        );
        if let Some(ref local) = output.locations.local_config {
            println!(
                "  Local:  {} {}",
                local.display(),
                if output.locations.local_config_exists {
                    "(exists)"
                } else {
                    "(optional, not configured)"
                }
            );
        }

        if let Some(ref env) = output.environment {
            println!();
            println!("Environment Variables");
            println!("{}", "-".repeat(40));
            if env.is_empty() {
                println!("  (no Cortex-related environment variables set)");
            } else {
                for (key, val) in env {
                    println!("  {key}={val}");
                }
            }
        }

        // Show hints about available options
        println!();
        println!("Tip: Use --json for machine-readable output, --env for environment variables.");
    }

    // Handle --diff flag: compare local and global configs
    if args.diff {
        println!();
        println!("Config Diff (Global vs Local)");
        println!("{}", "=".repeat(50));

        let global_path = config.cortex_home.join("config.toml");
        let local_path = std::env::current_dir()
            .ok()
            .map(|d| d.join(".cortex/config.toml"));

        let global_content = if global_path.exists() {
            std::fs::read_to_string(&global_path).ok()
        } else {
            None
        };

        let local_content = local_path.as_ref().and_then(|p| {
            if p.exists() {
                std::fs::read_to_string(p).ok()
            } else {
                None
            }
        });

        match (global_content.as_ref(), local_content.as_ref()) {
            (None, None) => {
                println!("  No config files found.");
            }
            (Some(_global), None) => {
                println!("  Only global config exists.");
                if args.json {
                    let diff_output = serde_json::json!({
                        "global_only": true,
                        "local_only": false,
                        "differences": [],
                    });
                    println!("{}", serde_json::to_string_pretty(&diff_output)?);
                }
            }
            (None, Some(_local)) => {
                println!("  Only local config exists.");
                if args.json {
                    let diff_output = serde_json::json!({
                        "global_only": false,
                        "local_only": true,
                        "differences": [],
                    });
                    println!("{}", serde_json::to_string_pretty(&diff_output)?);
                }
            }
            (Some(global), Some(local)) => {
                if global == local {
                    println!("  Configs are identical.");
                } else {
                    let diff = compute_config_diff(global, local);
                    if args.json {
                        println!("{}", serde_json::to_string_pretty(&diff)?);
                    } else {
                        println!();
                        if !diff.only_in_global.is_empty() {
                            println!("Lines only in global config:");
                            for line in &diff.only_in_global {
                                println!("  - {}", line);
                            }
                            println!();
                        }
                        if !diff.only_in_local.is_empty() {
                            println!("Lines only in local config:");
                            for line in &diff.only_in_local {
                                println!("  + {}", line);
                            }
                            println!();
                        }
                        if !diff.unified_diff.is_empty() {
                            println!("Unified diff:");
                            println!("{}", diff.unified_diff);
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

/// Compute diff between two config file contents.
fn compute_config_diff(global: &str, local: &str) -> ConfigDiff {
    let global_lines: HashSet<&str> = global.lines().filter(|l| !l.trim().is_empty()).collect();
    let local_lines: HashSet<&str> = local.lines().filter(|l| !l.trim().is_empty()).collect();

    let only_in_global: Vec<String> = global_lines
        .difference(&local_lines)
        .map(|s| s.to_string())
        .collect();
    let only_in_local: Vec<String> = local_lines
        .difference(&global_lines)
        .map(|s| s.to_string())
        .collect();

    // Generate a simple unified diff
    let unified_diff = generate_unified_diff(global, local);

    ConfigDiff {
        only_in_global,
        only_in_local,
        unified_diff,
    }
}

/// Generate a simple unified diff output.
fn generate_unified_diff(old_content: &str, new_content: &str) -> String {
    let old_lines: Vec<&str> = old_content.lines().collect();
    let new_lines: Vec<&str> = new_content.lines().collect();

    let mut diff_output = String::new();
    diff_output.push_str("--- global/config.toml\n");
    diff_output.push_str("+++ local/config.toml\n");

    // Simple line-by-line comparison (not a proper LCS diff, but useful for config files)
    let max_lines = old_lines.len().max(new_lines.len());
    let mut has_changes = false;

    for i in 0..max_lines {
        let old_line = old_lines.get(i).copied();
        let new_line = new_lines.get(i).copied();

        match (old_line, new_line) {
            (Some(o), Some(n)) if o == n => {
                // Lines are the same, show context
                diff_output.push_str(&format!(" {}\n", o));
            }
            (Some(o), Some(n)) => {
                // Lines differ
                has_changes = true;
                diff_output.push_str(&format!("-{}\n", o));
                diff_output.push_str(&format!("+{}\n", n));
            }
            (Some(o), None) => {
                // Line only in old
                has_changes = true;
                diff_output.push_str(&format!("-{}\n", o));
            }
            (None, Some(n)) => {
                // Line only in new
                has_changes = true;
                diff_output.push_str(&format!("+{}\n", n));
            }
            (None, None) => break,
        }
    }

    if !has_changes {
        String::new()
    } else {
        diff_output
    }
}
