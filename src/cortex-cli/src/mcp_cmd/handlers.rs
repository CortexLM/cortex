//! Command handler functions for MCP commands.
//!
//! This module contains the implementation of all MCP command handlers
//! (list, get, add, remove, enable, disable, rename).

use anyhow::{Context, Result, bail};
use cortex_engine::config::find_cortex_home;
use std::io::{self, BufRead, Write};

use super::auth::{get_auth_status_for_display, remove_auth_silent};
use super::config::{get_mcp_server, get_mcp_servers};
use super::types::{
    AddArgs, AddMcpSseArgs, AddMcpStreamableHttpArgs, AddMcpTransportArgs, DisableArgs, EnableArgs,
    GetArgs, ListArgs, RemoveArgs, RenameArgs,
};
use super::validation::{
    MAX_ENV_VARS, validate_bearer_token_env_var, validate_command_args, validate_env_var_name,
    validate_env_var_value, validate_server_name, validate_url, validate_url_internal,
};

/// Run the list command.
pub(crate) async fn run_list(args: ListArgs) -> Result<()> {
    let servers = get_mcp_servers()?;

    if args.json {
        // Sort servers by name for consistent JSON output
        let mut sorted_servers: Vec<_> = servers.iter().collect();
        sorted_servers.sort_by(|a, b| a.0.cmp(b.0));
        let sorted_map: toml::map::Map<String, toml::Value> = sorted_servers
            .into_iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let json = serde_json::to_string_pretty(&sorted_map)?;
        println!("{json}");
        return Ok(());
    }

    if servers.is_empty() {
        println!("No MCP servers configured yet.");
        println!("\nTo add a server:");
        println!("  cortex mcp add my-tool -- my-command      # Local stdio server");
        println!("  cortex mcp add my-api --url https://...   # Remote HTTP server");
        return Ok(());
    }

    println!(
        "{:<20} {:<12} {:<8} {:<18} {:<30}",
        "Name", "Status", "Tools", "Auth", "Transport"
    );
    println!("{}", "-".repeat(90));

    // Sort servers alphabetically by name for deterministic output
    let mut sorted_servers: Vec<_> = servers.iter().collect();
    sorted_servers.sort_by(|a, b| a.0.cmp(b.0));

    for (name, server) in sorted_servers {
        let enabled = server
            .get("enabled")
            .and_then(toml::Value::as_bool)
            .unwrap_or(true);
        let status = if enabled { "enabled" } else { "disabled" };

        // Get tools count if available (would come from cached server info)
        let tools_count = server
            .get("_cached_tools_count")
            .and_then(|v| v.as_integer())
            .map(|n| n.to_string())
            .unwrap_or_else(|| "unknown".to_string());

        // Determine transport type and auth status
        let (transport_info, auth_status) = if let Some(transport) = server.get("transport") {
            let transport_type = transport
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            match transport_type {
                "stdio" => {
                    let cmd = transport
                        .get("command")
                        .and_then(|v| v.as_str())
                        .unwrap_or("?");
                    // Truncate long commands with ellipsis indicator
                    let transport_str = if cmd.len() > 22 {
                        format!("stdio: {}...", &cmd[..19])
                    } else {
                        format!("stdio: {cmd}")
                    };
                    (transport_str, "N/A".to_string())
                }
                "http" => {
                    let url = transport.get("url").and_then(|v| v.as_str()).unwrap_or("?");
                    // Check OAuth status for HTTP servers
                    let auth = get_auth_status_for_display(name, url)
                        .await
                        .unwrap_or_else(|_| "Unknown".to_string());
                    // Truncate URL if too long with ellipsis indicator
                    let transport_str = if url.len() > 25 {
                        format!("http: {}...", &url[..22])
                    } else {
                        format!("http: {url}")
                    };
                    (transport_str, auth)
                }
                _ => (transport_type.to_string(), "N/A".to_string()),
            }
        } else {
            ("unknown".to_string(), "N/A".to_string())
        };

        println!("{name:<20} {status:<12} {tools_count:<8} {auth_status:<18} {transport_info:<30}");
    }

    println!("\nTotal: {} server(s)", servers.len());
    println!("Note: Status shows config state (enabled/disabled), not connection health.");
    println!("\nUse 'cortex mcp get <name>' for details.");
    println!("Use 'cortex mcp debug <name>' to test connection and verify server health.");

    Ok(())
}

/// Run the get command.
pub(crate) async fn run_get(args: GetArgs) -> Result<()> {
    let server = get_mcp_server(&args.name)?
        .ok_or_else(|| anyhow::anyhow!("No MCP server named '{}' found", args.name))?;

    if args.json {
        let json = serde_json::to_string_pretty(&server)?;
        println!("{json}");
        return Ok(());
    }

    println!("MCP Server: {}", args.name);
    println!("{}", "=".repeat(40));

    let enabled = server
        .get("enabled")
        .and_then(toml::Value::as_bool)
        .unwrap_or(true);
    println!("Enabled: {enabled}");

    if let Some(transport) = server.get("transport") {
        let transport_type = transport
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        println!("\nTransport: {transport_type}");

        match transport_type {
            "stdio" => {
                if let Some(cmd) = transport.get("command").and_then(|v| v.as_str()) {
                    // Quote command if it contains spaces
                    let cmd_display = if cmd.contains(' ') {
                        format!("\"{}\"", cmd)
                    } else {
                        cmd.to_string()
                    };
                    println!("  Command: {cmd_display}");
                }
                if let Some(args) = transport.get("args").and_then(|v| v.as_array()) {
                    let args_str: Vec<_> = args
                        .iter()
                        .filter_map(|v| v.as_str())
                        .map(|arg| {
                            // Quote arguments containing spaces or special characters for copy-paste
                            if arg.contains(' ') || arg.contains('"') || arg.contains('\'') {
                                format!("\"{}\"", arg.replace('"', "\\\""))
                            } else {
                                arg.to_string()
                            }
                        })
                        .collect();
                    if !args_str.is_empty() {
                        println!("  Args: {}", args_str.join(" "));
                    }
                }
                if let Some(env) = transport.get("env").and_then(|v| v.as_table())
                    && !env.is_empty()
                {
                    println!("  Environment:");
                    for (key, value) in env {
                        let val_str = value.as_str().unwrap_or("***");
                        // Mask sensitive values
                        let masked = if key.to_lowercase().contains("key")
                            || key.to_lowercase().contains("secret")
                            || key.to_lowercase().contains("token")
                            || key.to_lowercase().contains("password")
                        {
                            "***"
                        } else {
                            val_str
                        };
                        println!("    {key}={masked}");
                    }
                }
            }
            "http" => {
                if let Some(url) = transport.get("url").and_then(|v| v.as_str()) {
                    println!("  URL: {url}");
                }
                if let Some(token_var) = transport
                    .get("bearer_token_env_var")
                    .and_then(|v| v.as_str())
                {
                    println!("  Bearer Token Env Var: {token_var}");
                }
            }
            _ => {}
        }
    }

    // Show OAuth status if HTTP
    if let Some(transport) = server.get("transport") {
        let transport_type = transport
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        if transport_type == "http"
            && let Some(url) = transport.get("url").and_then(|v| v.as_str())
        {
            println!("\nOAuth Status:");
            match get_auth_status_for_display(&args.name, url).await {
                Ok(status) => println!("  {status}"),
                Err(_) => println!("  Unknown"),
            }
        }
    }

    Ok(())
}

/// Run the add command.
pub(crate) async fn run_add(args: AddArgs) -> Result<()> {
    let AddArgs {
        name,
        force,
        allow_local,
        transport_args,
    } = args;

    validate_server_name(&name)?;

    // Check if server already exists
    let server_exists = get_mcp_server(&name)?.is_some();
    if server_exists && !force {
        bail!(
            "MCP server '{}' already exists. Use --force to overwrite, or 'cortex mcp remove {}' first.",
            name,
            name
        );
    }

    // Validate that bearer token is not used with stdio transport
    if let Some(ref http_args) = transport_args.streamable_http
        && http_args.bearer_token_env_var.is_some()
        && let Some(ref stdio_args) = transport_args.stdio
        && !stdio_args.command.is_empty()
    {
        bail!(
            "Error: --bearer-token-env-var is only supported for HTTP transport.\n\
                         You cannot use bearer tokens with stdio transport (command execution).\n\n\
                         To use bearer token authentication:\n\
                         • Remove the command arguments (-- <COMMAND>)\n\
                         • Use --url instead to specify an HTTP MCP server\n\n\
                         Example: cortex mcp add {name} --url https://api.example.com --bearer-token-env-var MY_TOKEN"
        );
    }

    let cortex_home =
        find_cortex_home().map_err(|e| anyhow::anyhow!("Failed to find cortex home: {}", e))?;
    let config_path = cortex_home.join("config.toml");

    // If force mode and server exists, remove it first
    if server_exists && force {
        // Parse, remove the server, and re-serialize
        if config_path.exists() {
            let content = std::fs::read_to_string(&config_path)
                .with_context(|| format!("failed to read config: {}", config_path.display()))?;
            let mut config: toml::Value =
                toml::from_str(&content).with_context(|| "failed to parse config")?;

            if let Some(mcp_servers) = config.get_mut("mcp_servers").and_then(|v| v.as_table_mut())
            {
                mcp_servers.remove(&name);
            }

            // Write back the config without the old server
            let updated_content =
                toml::to_string_pretty(&config).with_context(|| "failed to serialize config")?;
            std::fs::write(&config_path, &updated_content)
                .with_context(|| format!("failed to write config: {}", config_path.display()))?;

            println!("✓ Removed existing MCP server '{name}' (--force)");
        }
    }

    // Load existing config or create new
    let mut config_content = if config_path.exists() {
        std::fs::read_to_string(&config_path)
            .with_context(|| format!("failed to read config: {}", config_path.display()))?
    } else {
        String::new()
    };

    // Create MCP server entry
    // Returns (toml_content, success_message) - message is deferred until config write succeeds
    let (transport_toml, success_msg) = match transport_args {
        AddMcpTransportArgs {
            stdio: Some(stdio), ..
        } => {
            // Validate command arguments
            validate_command_args(&stdio.command)?;

            // Validate environment variables
            if stdio.env.len() > MAX_ENV_VARS {
                bail!(
                    "Too many environment variables ({}). Maximum allowed is {}",
                    stdio.env.len(),
                    MAX_ENV_VARS
                );
            }
            for (key, value) in &stdio.env {
                validate_env_var_name(key)?;
                validate_env_var_value(value)?;
            }

            let mut command_parts = stdio.command.into_iter();
            let command_bin_raw = command_parts
                .next()
                .ok_or_else(|| anyhow::anyhow!("command is required"))?;

            // Expand tilde in command path (e.g., ~/script.sh -> /home/user/script.sh) (#2452)
            let command_bin = if let Some(suffix) = command_bin_raw.strip_prefix("~/") {
                if let Some(home) = dirs::home_dir() {
                    home.join(suffix).to_string_lossy().to_string()
                } else {
                    tracing::warn!(
                        "Could not expand ~ in command path (home directory not found): {}",
                        command_bin_raw
                    );
                    command_bin_raw
                }
            } else {
                command_bin_raw
            };

            // Also expand tilde in command arguments
            let command_args: Vec<String> = command_parts
                .map(|arg| {
                    if let Some(suffix) = arg.strip_prefix("~/") {
                        if let Some(home) = dirs::home_dir() {
                            home.join(suffix).to_string_lossy().to_string()
                        } else {
                            arg
                        }
                    } else {
                        arg
                    }
                })
                .collect();

            // Sanitize command_bin for TOML (escape special characters)
            let command_bin_escaped = command_bin.replace('\\', "\\\\").replace('"', "\\\"");

            let mut toml = format!(
                r#"
[mcp_servers.{name}]
enabled = true
[mcp_servers.{name}.transport]
type = "stdio"
command = "{command_bin_escaped}"
"#
            );

            if !command_args.is_empty() {
                toml.push_str(&format!(
                    "args = [{}]\n",
                    command_args
                        .iter()
                        .map(|a| {
                            // Escape special characters in args
                            let escaped = a.replace('\\', "\\\\").replace('"', "\\\"");
                            format!("\"{escaped}\"")
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }

            if !stdio.env.is_empty() {
                toml.push_str("[mcp_servers.");
                toml.push_str(&name);
                toml.push_str(".transport.env]\n");
                for (key, value) in &stdio.env {
                    // Escape special characters in env values
                    let escaped_value = value.replace('\\', "\\\\").replace('"', "\\\"");
                    toml.push_str(&format!("{key} = \"{escaped_value}\"\n"));
                }
            }

            // Build success message but defer printing until config write succeeds
            let mut success_msg = format!("✓ Added stdio MCP server '{name}'\n");
            success_msg.push_str(&format!("  Command: {command_bin}\n"));
            if !command_args.is_empty() {
                success_msg.push_str(&format!("  Args: {}\n", command_args.join(" ")));
            }

            (toml, success_msg)
        }
        AddMcpTransportArgs {
            streamable_http:
                Some(AddMcpStreamableHttpArgs {
                    url,
                    bearer_token_env_var,
                }),
            ..
        } => {
            // Validate URL format and safety (allow_local bypasses localhost/private network check)
            validate_url_internal(&url, allow_local)?;

            // Validate bearer token env var if provided
            if let Some(ref token_var) = bearer_token_env_var {
                validate_bearer_token_env_var(token_var)?;
            }

            // Escape URL for TOML
            let url_escaped = url.replace('\\', "\\\\").replace('"', "\\\"");

            let mut toml = format!(
                r#"
[mcp_servers.{name}]
enabled = true
[mcp_servers.{name}.transport]
type = "http"
url = "{url_escaped}"
"#
            );

            if let Some(ref token_var) = bearer_token_env_var {
                toml.push_str(&format!("bearer_token_env_var = \"{token_var}\"\n"));
            }

            // Build success message but defer printing until config write succeeds
            let mut success_msg = format!("✓ Added HTTP MCP server '{name}'\n");
            success_msg.push_str(&format!("  URL: {url}\n"));
            if let Some(ref token_var) = bearer_token_env_var {
                success_msg.push_str(&format!("  Bearer Token Env Var: {token_var}\n"));
            }

            (toml, success_msg)
        }
        AddMcpTransportArgs {
            sse_transport:
                Some(AddMcpSseArgs {
                    sse_url: Some(url),
                    sse_bearer_token_env_var,
                }),
            ..
        } => {
            // Validate URL format and safety
            validate_url(&url)?;

            // Validate bearer token env var if provided
            if let Some(ref token_var) = sse_bearer_token_env_var {
                validate_bearer_token_env_var(token_var)?;
            }

            // Escape URL for TOML
            let url_escaped = url.replace('\\', "\\\\").replace('"', "\\\"");

            let mut toml = format!(
                r#"
[mcp_servers.{name}]
enabled = true
[mcp_servers.{name}.transport]
type = "sse"
url = "{url_escaped}"
"#
            );

            if let Some(ref token_var) = sse_bearer_token_env_var {
                toml.push_str(&format!("bearer_token_env_var = \"{token_var}\"\n"));
            }

            // Build success message but defer printing until config write succeeds
            let mut success_msg = format!("✓ Added SSE MCP server '{name}'\n");
            success_msg.push_str(&format!("  URL: {url}\n"));
            if let Some(ref token_var) = sse_bearer_token_env_var {
                success_msg.push_str(&format!("  Bearer Token Env Var: {token_var}\n"));
            }

            (toml, success_msg)
        }
        _ => bail!("exactly one of --command, --url, or --sse must be provided"),
    };

    // Append to config
    config_content.push_str(&transport_toml);

    // Ensure directory exists
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Write config file - only print success message after this succeeds
    std::fs::write(&config_path, &config_content)
        .with_context(|| format!("failed to write config: {}", config_path.display()))?;

    // Now that config was successfully written, print the success message
    print!("{success_msg}");
    println!("\nUse 'cortex mcp get {name}' to view the configuration.");
    println!("Use 'cortex mcp debug {name}' to test the connection.");

    Ok(())
}

/// Run the remove command.
pub(crate) async fn run_remove(args: RemoveArgs) -> Result<()> {
    let RemoveArgs { name, yes } = args;

    validate_server_name(&name)?;

    // Check if server exists
    if get_mcp_server(&name)?.is_none() {
        bail!("No MCP server named '{}' found", name);
    }

    // Confirm removal if not skipped via --yes/-y/--force/-f
    if !yes {
        print!("Remove MCP server '{name}'? [y/N]: ");
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().lock().read_line(&mut input)?;

        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Cancelled.");
            return Ok(());
        }
    }

    let cortex_home =
        find_cortex_home().map_err(|e| anyhow::anyhow!("Failed to find cortex home: {}", e))?;
    let config_path = cortex_home.join("config.toml");

    if !config_path.exists() {
        bail!("Config file not found");
    }

    let content = std::fs::read_to_string(&config_path)
        .with_context(|| format!("failed to read config: {}", config_path.display()))?;

    // Parse the TOML and remove the server section
    let mut config: toml::Value =
        toml::from_str(&content).with_context(|| "failed to parse config")?;

    if let Some(mcp_servers) = config.get_mut("mcp_servers").and_then(|v| v.as_table_mut()) {
        mcp_servers.remove(&name);
    }

    // Write back the config
    let new_content =
        toml::to_string_pretty(&config).with_context(|| "failed to serialize config")?;

    std::fs::write(&config_path, new_content)
        .with_context(|| format!("failed to write config: {}", config_path.display()))?;

    println!("✓ Removed MCP server '{name}'.");

    // Also remove OAuth credentials if any
    match remove_auth_silent(&name).await {
        Ok(true) => println!("✓ Removed OAuth credentials for '{name}'."),
        Ok(false) => {}
        Err(_) => {}
    }

    Ok(())
}

/// Enable an MCP server.
pub(crate) async fn run_enable(args: EnableArgs) -> Result<()> {
    validate_server_name(&args.name)?;

    // Check if server exists
    if get_mcp_server(&args.name)?.is_none() {
        bail!("No MCP server named '{}' found", args.name);
    }

    let cortex_home =
        find_cortex_home().map_err(|e| anyhow::anyhow!("Failed to find cortex home: {}", e))?;
    let config_path = cortex_home.join("config.toml");

    if !config_path.exists() {
        bail!("Config file not found");
    }

    let content = std::fs::read_to_string(&config_path)
        .with_context(|| format!("failed to read config: {}", config_path.display()))?;

    let mut config: toml::Value =
        toml::from_str(&content).with_context(|| "failed to parse config")?;

    if let Some(mcp_servers) = config.get_mut("mcp_servers").and_then(|v| v.as_table_mut())
        && let Some(server) = mcp_servers.get_mut(&args.name)
        && let Some(table) = server.as_table_mut()
    {
        // Check current state
        let already_enabled = table
            .get("enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        if already_enabled {
            println!("MCP server '{}' is already enabled.", args.name);
            return Ok(());
        }

        table.insert("enabled".to_string(), toml::Value::Boolean(true));
    }

    let new_content =
        toml::to_string_pretty(&config).with_context(|| "failed to serialize config")?;

    std::fs::write(&config_path, new_content)
        .with_context(|| format!("failed to write config: {}", config_path.display()))?;

    println!("✓ Enabled MCP server '{}'.", args.name);

    Ok(())
}

/// Disable an MCP server.
pub(crate) async fn run_disable(args: DisableArgs) -> Result<()> {
    validate_server_name(&args.name)?;

    // Check if server exists
    if get_mcp_server(&args.name)?.is_none() {
        bail!("No MCP server named '{}' found", args.name);
    }

    let cortex_home =
        find_cortex_home().map_err(|e| anyhow::anyhow!("Failed to find cortex home: {}", e))?;
    let config_path = cortex_home.join("config.toml");

    if !config_path.exists() {
        bail!("Config file not found");
    }

    let content = std::fs::read_to_string(&config_path)
        .with_context(|| format!("failed to read config: {}", config_path.display()))?;

    let mut config: toml::Value =
        toml::from_str(&content).with_context(|| "failed to parse config")?;

    if let Some(mcp_servers) = config.get_mut("mcp_servers").and_then(|v| v.as_table_mut())
        && let Some(server) = mcp_servers.get_mut(&args.name)
        && let Some(table) = server.as_table_mut()
    {
        // Check current state
        let already_disabled = table
            .get("enabled")
            .and_then(|v| v.as_bool())
            .map(|v| !v)
            .unwrap_or(false);

        if already_disabled {
            println!("MCP server '{}' is already disabled.", args.name);
            return Ok(());
        }

        table.insert("enabled".to_string(), toml::Value::Boolean(false));
    }

    let new_content =
        toml::to_string_pretty(&config).with_context(|| "failed to serialize config")?;

    std::fs::write(&config_path, new_content)
        .with_context(|| format!("failed to write config: {}", config_path.display()))?;

    println!("✓ Disabled MCP server '{}'.", args.name);
    println!(
        "  The server configuration is preserved. Use 'cortex mcp enable {}' to re-enable.",
        args.name
    );

    Ok(())
}

/// Rename an MCP server.
pub(crate) async fn run_rename(args: RenameArgs) -> Result<()> {
    validate_server_name(&args.old_name)?;
    validate_server_name(&args.new_name)?;

    // Check if old server exists
    if get_mcp_server(&args.old_name)?.is_none() {
        bail!("No MCP server named '{}' found", args.old_name);
    }

    // Check if new name already exists
    if get_mcp_server(&args.new_name)?.is_some() {
        bail!(
            "MCP server '{}' already exists. Remove it first or choose a different name.",
            args.new_name
        );
    }

    let cortex_home =
        find_cortex_home().map_err(|e| anyhow::anyhow!("Failed to find cortex home: {}", e))?;
    let config_path = cortex_home.join("config.toml");

    if !config_path.exists() {
        bail!("Config file not found");
    }

    let content = std::fs::read_to_string(&config_path)
        .with_context(|| format!("failed to read config: {}", config_path.display()))?;

    let mut config: toml::Value =
        toml::from_str(&content).with_context(|| "failed to parse config")?;

    if let Some(mcp_servers) = config.get_mut("mcp_servers").and_then(|v| v.as_table_mut())
        && let Some(server_config) = mcp_servers.remove(&args.old_name)
    {
        mcp_servers.insert(args.new_name.clone(), server_config);
    }

    let new_content =
        toml::to_string_pretty(&config).with_context(|| "failed to serialize config")?;

    std::fs::write(&config_path, new_content)
        .with_context(|| format!("failed to write config: {}", config_path.display()))?;

    println!(
        "✓ Renamed MCP server '{}' to '{}'.",
        args.old_name, args.new_name
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::types::AddMcpStdioArgs;
    use super::*;

    // ========================================================================
    // ListArgs tests
    // ========================================================================

    #[test]
    fn test_list_args_defaults() {
        let args = ListArgs {
            json: false,
            all: false,
        };
        assert!(!args.json, "json should default to false");
        assert!(!args.all, "all should default to false");
    }

    #[test]
    fn test_list_args_json_enabled() {
        let args = ListArgs {
            json: true,
            all: false,
        };
        assert!(args.json, "json flag should be settable to true");
    }

    #[test]
    fn test_list_args_all_enabled() {
        let args = ListArgs {
            json: false,
            all: true,
        };
        assert!(args.all, "all flag should be settable to true");
    }

    // ========================================================================
    // GetArgs tests
    // ========================================================================

    #[test]
    fn test_get_args_construction() {
        let args = GetArgs {
            name: "test-server".to_string(),
            json: false,
        };
        assert_eq!(args.name, "test-server");
        assert!(!args.json);
    }

    #[test]
    fn test_get_args_with_json() {
        let args = GetArgs {
            name: "my-mcp-server".to_string(),
            json: true,
        };
        assert_eq!(args.name, "my-mcp-server");
        assert!(args.json);
    }

    #[test]
    fn test_get_args_empty_name() {
        let args = GetArgs {
            name: String::new(),
            json: false,
        };
        assert!(args.name.is_empty());
    }

    // ========================================================================
    // RemoveArgs tests
    // ========================================================================

    #[test]
    fn test_remove_args_construction() {
        let args = RemoveArgs {
            name: "server-to-remove".to_string(),
            yes: false,
        };
        assert_eq!(args.name, "server-to-remove");
        assert!(!args.yes, "yes should default to false for confirmation");
    }

    #[test]
    fn test_remove_args_with_yes() {
        let args = RemoveArgs {
            name: "server".to_string(),
            yes: true,
        };
        assert!(args.yes, "yes flag should skip confirmation");
    }

    // ========================================================================
    // EnableArgs tests
    // ========================================================================

    #[test]
    fn test_enable_args_construction() {
        let args = EnableArgs {
            name: "my-server".to_string(),
        };
        assert_eq!(args.name, "my-server");
    }

    // ========================================================================
    // DisableArgs tests
    // ========================================================================

    #[test]
    fn test_disable_args_construction() {
        let args = DisableArgs {
            name: "disabled-server".to_string(),
        };
        assert_eq!(args.name, "disabled-server");
    }

    // ========================================================================
    // RenameArgs tests
    // ========================================================================

    #[test]
    fn test_rename_args_construction() {
        let args = RenameArgs {
            old_name: "old-name".to_string(),
            new_name: "new-name".to_string(),
        };
        assert_eq!(args.old_name, "old-name");
        assert_eq!(args.new_name, "new-name");
    }

    #[test]
    fn test_rename_args_same_name() {
        let args = RenameArgs {
            old_name: "same".to_string(),
            new_name: "same".to_string(),
        };
        assert_eq!(args.old_name, args.new_name);
    }

    // ========================================================================
    // AddArgs tests
    // ========================================================================

    #[test]
    fn test_add_args_force_flag() {
        let args = AddArgs {
            name: "new-server".to_string(),
            force: true,
            allow_local: false,
            transport_args: AddMcpTransportArgs {
                stdio: None,
                streamable_http: Some(AddMcpStreamableHttpArgs {
                    url: "https://example.com/mcp".to_string(),
                    bearer_token_env_var: None,
                }),
                sse_transport: None,
            },
        };
        assert!(args.force, "force flag should be settable");
        assert!(!args.allow_local, "allow_local should default to false");
    }

    #[test]
    fn test_add_args_allow_local_flag() {
        let args = AddArgs {
            name: "local-server".to_string(),
            force: false,
            allow_local: true,
            transport_args: AddMcpTransportArgs {
                stdio: None,
                streamable_http: Some(AddMcpStreamableHttpArgs {
                    url: "http://localhost:8080/mcp".to_string(),
                    bearer_token_env_var: None,
                }),
                sse_transport: None,
            },
        };
        assert!(
            args.allow_local,
            "allow_local should be settable for local dev"
        );
    }

    // ========================================================================
    // AddMcpStreamableHttpArgs tests
    // ========================================================================

    #[test]
    fn test_add_mcp_http_args_url_only() {
        let args = AddMcpStreamableHttpArgs {
            url: "https://api.example.com/mcp".to_string(),
            bearer_token_env_var: None,
        };
        assert_eq!(args.url, "https://api.example.com/mcp");
        assert!(args.bearer_token_env_var.is_none());
    }

    #[test]
    fn test_add_mcp_http_args_with_bearer_token() {
        let args = AddMcpStreamableHttpArgs {
            url: "https://api.example.com/mcp".to_string(),
            bearer_token_env_var: Some("MY_API_TOKEN".to_string()),
        };
        assert_eq!(args.bearer_token_env_var, Some("MY_API_TOKEN".to_string()));
    }

    // ========================================================================
    // AddMcpStdioArgs tests
    // ========================================================================

    #[test]
    fn test_add_mcp_stdio_args_simple_command() {
        let args = AddMcpStdioArgs {
            command: vec!["npx".to_string(), "@example/server".to_string()],
            env: vec![],
        };
        assert_eq!(args.command.len(), 2);
        assert_eq!(args.command[0], "npx");
        assert!(args.env.is_empty());
    }

    #[test]
    fn test_add_mcp_stdio_args_with_env_vars() {
        let args = AddMcpStdioArgs {
            command: vec![
                "python".to_string(),
                "-m".to_string(),
                "myserver".to_string(),
            ],
            env: vec![
                ("API_KEY".to_string(), "secret123".to_string()),
                ("DEBUG".to_string(), "true".to_string()),
            ],
        };
        assert_eq!(args.command.len(), 3);
        assert_eq!(args.env.len(), 2);
        assert_eq!(args.env[0].0, "API_KEY");
        assert_eq!(args.env[1].0, "DEBUG");
    }

    // ========================================================================
    // AddMcpSseArgs tests
    // ========================================================================

    #[test]
    fn test_add_mcp_sse_args_with_url() {
        let args = AddMcpSseArgs {
            sse_url: Some("https://api.example.com/sse".to_string()),
            sse_bearer_token_env_var: None,
        };
        assert_eq!(
            args.sse_url,
            Some("https://api.example.com/sse".to_string())
        );
        assert!(args.sse_bearer_token_env_var.is_none());
    }

    #[test]
    fn test_add_mcp_sse_args_with_bearer_token() {
        let args = AddMcpSseArgs {
            sse_url: Some("https://api.example.com/sse".to_string()),
            sse_bearer_token_env_var: Some("SSE_TOKEN".to_string()),
        };
        assert_eq!(args.sse_bearer_token_env_var, Some("SSE_TOKEN".to_string()));
    }

    // ========================================================================
    // AddMcpTransportArgs tests
    // ========================================================================

    #[test]
    fn test_transport_args_stdio_variant() {
        let args = AddMcpTransportArgs {
            stdio: Some(AddMcpStdioArgs {
                command: vec!["node".to_string(), "server.js".to_string()],
                env: vec![],
            }),
            streamable_http: None,
            sse_transport: None,
        };
        assert!(args.stdio.is_some());
        assert!(args.streamable_http.is_none());
        assert!(args.sse_transport.is_none());
    }

    #[test]
    fn test_transport_args_http_variant() {
        let args = AddMcpTransportArgs {
            stdio: None,
            streamable_http: Some(AddMcpStreamableHttpArgs {
                url: "https://example.com".to_string(),
                bearer_token_env_var: None,
            }),
            sse_transport: None,
        };
        assert!(args.stdio.is_none());
        assert!(args.streamable_http.is_some());
        assert!(args.sse_transport.is_none());
    }

    #[test]
    fn test_transport_args_sse_variant() {
        let args = AddMcpTransportArgs {
            stdio: None,
            streamable_http: None,
            sse_transport: Some(AddMcpSseArgs {
                sse_url: Some("https://example.com/sse".to_string()),
                sse_bearer_token_env_var: None,
            }),
        };
        assert!(args.stdio.is_none());
        assert!(args.streamable_http.is_none());
        assert!(args.sse_transport.is_some());
    }
}
