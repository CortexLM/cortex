//! OAuth authentication functions for MCP commands.
//!
//! This module handles OAuth authentication, listing auth status,
//! and logout functionality for MCP servers.

use anyhow::{Result, bail};
use cortex_engine::create_default_client;

use super::config::{get_mcp_server, get_mcp_servers};
use super::types::{AuthCommand, AuthListArgs, AuthSubcommand, LogoutArgs};
use super::validation::validate_server_name;

/// Handle the auth command dispatch.
pub(crate) async fn run_auth_command(cmd: AuthCommand) -> Result<()> {
    match cmd.action {
        Some(AuthSubcommand::List(args)) => {
            // If subcommand is list, name should not be provided
            if cmd.name.is_some() {
                bail!("Cannot specify server name with 'list' subcommand");
            }
            run_auth_list(args).await
        }
        None => {
            // Direct auth with server name
            let name = cmd.name.ok_or_else(|| {
                anyhow::anyhow!(
                    "Server name required. Use 'cortex mcp auth <name>' to authenticate or 'cortex mcp auth list' to list auth status."
                )
            })?;
            run_auth(name, cmd.client_id, cmd.client_secret).await
        }
    }
}

/// List auth status for all servers.
async fn run_auth_list(args: AuthListArgs) -> Result<()> {
    let servers = get_mcp_servers()?;

    if servers.is_empty() {
        if args.json {
            println!("[]");
        } else {
            println!("No MCP servers configured.");
        }
        return Ok(());
    }

    let mut auth_statuses = Vec::new();

    for (name, server) in &servers {
        // Only check HTTP servers (stdio servers don't use OAuth)
        let transport = server.get("transport");
        let transport_type = transport
            .and_then(|t| t.get("type"))
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");

        if transport_type != "http" {
            continue;
        }

        let url = transport
            .and_then(|t| t.get("url"))
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let status = get_auth_status_for_display(name, url)
            .await
            .unwrap_or_else(|_| "Unknown".to_string());
        let has_oauth = check_server_supports_oauth(url).await.unwrap_or(false);

        auth_statuses.push(serde_json::json!({
            "name": name,
            "url": url,
            "oauth_supported": has_oauth,
            "status": status,
        }));
    }

    if args.json {
        let json = serde_json::to_string_pretty(&auth_statuses)?;
        println!("{json}");
        return Ok(());
    }

    if auth_statuses.is_empty() {
        println!("No HTTP MCP servers configured (OAuth is only for HTTP servers).");
        return Ok(());
    }

    println!("{:<20} {:<18} {:<40}", "Server", "Auth Status", "URL");
    println!("{}", "-".repeat(80));

    for status in &auth_statuses {
        let name = status["name"].as_str().unwrap_or("?");
        let url = status["url"].as_str().unwrap_or("?");
        let auth_status = status["status"].as_str().unwrap_or("Unknown");
        let oauth_supported = status["oauth_supported"].as_bool().unwrap_or(false);

        let display_status = if !oauth_supported {
            "N/A (no OAuth)"
        } else {
            auth_status
        };

        // Truncate URL if too long
        let display_url = if url.len() > 38 {
            format!("{}...", &url[..35])
        } else {
            url.to_string()
        };

        println!("{name:<20} {display_status:<18} {display_url:<40}");
    }

    println!("\nUse 'cortex mcp auth <name>' to authenticate with a server.");
    println!("Use 'cortex mcp logout <name>' to remove credentials.");

    Ok(())
}

/// Run OAuth authentication for a server.
async fn run_auth(
    name: String,
    client_id: Option<String>,
    client_secret: Option<String>,
) -> Result<()> {
    use cortex_engine::mcp;

    validate_server_name(&name)?;

    let server = get_mcp_server(&name)?.ok_or_else(|| {
        anyhow::anyhow!(
            "No MCP server named '{}' found. Add it first with `cortex mcp add`.",
            name
        )
    })?;

    // Get the server URL
    let transport = server
        .get("transport")
        .ok_or_else(|| anyhow::anyhow!("MCP server '{name}' has no transport configured"))?;

    let transport_type = transport
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    if transport_type != "http" {
        bail!(
            "OAuth authentication is only supported for HTTP MCP servers. Server '{name}' uses {transport_type} transport."
        );
    }

    let server_url = transport
        .get("url")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("HTTP MCP server '{name}' has no URL configured"))?;

    // Check current auth status
    let auth_status = mcp::get_auth_status(&name, server_url)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to check auth status: {e}"))?;

    match auth_status {
        mcp::AuthStatus::Authenticated => {
            println!("MCP server '{name}' is already authenticated.");
            println!("Use `cortex mcp logout {name}` first if you want to re-authenticate.");
            return Ok(());
        }
        mcp::AuthStatus::Expired => {
            println!("OAuth tokens for '{name}' have expired. Re-authenticating...");
        }
        mcp::AuthStatus::NotAuthenticated => {
            println!("Starting OAuth authentication for '{name}'...");
        }
    }

    // Run the OAuth flow
    println!("Opening browser for authentication...");
    println!("Waiting for authorization (timeout: 5 minutes)...");

    match mcp::run_oauth_flow(
        &name,
        server_url,
        client_id.as_deref(),
        client_secret.as_deref(),
    )
    .await
    {
        Ok(_tokens) => {
            println!("✓ Successfully authenticated with MCP server '{name}'!");
        }
        Err(e) => {
            bail!("OAuth authentication failed: {e}");
        }
    }

    Ok(())
}

/// Run logout for a server or all servers.
pub(crate) async fn run_logout(args: LogoutArgs) -> Result<()> {
    use cortex_engine::mcp;

    // Validate mutually exclusive flags
    if args.name.is_some() && args.all {
        bail!(
            "Cannot specify both --name and --all. Use --name for a specific server or --all for all servers."
        );
    }

    if args.all {
        // Logout from all servers
        let servers = get_mcp_servers()?;
        let mut count = 0;

        for (name, server) in &servers {
            // Only check HTTP servers
            let transport = server.get("transport");
            let transport_type = transport
                .and_then(|t| t.get("type"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");

            if transport_type != "http" {
                continue;
            }

            let has_tokens = mcp::has_stored_tokens(name).await.unwrap_or(false);

            if has_tokens {
                match mcp::remove_auth(name).await {
                    Ok(_) => {
                        println!("✓ Removed OAuth credentials for '{name}'.");
                        count += 1;
                    }
                    Err(e) => {
                        eprintln!("✗ Failed to remove credentials for '{name}': {e}");
                    }
                }
            }
        }

        if count == 0 {
            println!("No OAuth credentials found to remove.");
        } else {
            println!("\nRemoved credentials for {count} server(s).");
        }

        return Ok(());
    }

    let name = args
        .name
        .ok_or_else(|| anyhow::anyhow!("Server name required (or use --all)"))?;

    validate_server_name(&name)?;

    // Check if we have stored credentials
    let has_tokens = mcp::has_stored_tokens(&name)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to check stored tokens: {e}"))?;

    if !has_tokens {
        println!("No OAuth credentials found for MCP server '{name}'.");
        return Ok(());
    }

    // Remove the credentials
    mcp::remove_auth(&name)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to remove OAuth credentials: {e}"))?;

    println!("✓ Removed OAuth credentials for MCP server '{name}'.");

    Ok(())
}

/// Get auth status for display purposes.
pub(crate) async fn get_auth_status_for_display(name: &str, url: &str) -> Result<String> {
    use cortex_engine::mcp;

    let status = mcp::get_auth_status(name, url)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to get auth status: {}", e))?;

    Ok(match status {
        mcp::AuthStatus::Authenticated => "Authenticated".to_string(),
        mcp::AuthStatus::Expired => "Expired".to_string(),
        mcp::AuthStatus::NotAuthenticated => "Not Authenticated".to_string(),
    })
}

/// Check if server supports OAuth (by checking for .well-known endpoint).
async fn check_server_supports_oauth(url: &str) -> Result<bool> {
    // Try to parse URL and check for OAuth metadata
    let base_url = url.trim_end_matches('/');

    // Check for .well-known/oauth-authorization-server
    let well_known_url = format!("{}/.well-known/oauth-authorization-server", base_url);

    let client = create_default_client()?;

    match client.get(&well_known_url).send().await {
        Ok(resp) => Ok(resp.status().is_success()),
        Err(_) => Ok(false),
    }
}

/// Remove auth silently (returns true if credentials were removed).
pub(crate) async fn remove_auth_silent(name: &str) -> Result<bool> {
    use cortex_engine::mcp;

    let has_tokens = mcp::has_stored_tokens(name).await.unwrap_or(false);
    if has_tokens {
        mcp::remove_auth(name).await?;
        Ok(true)
    } else {
        Ok(false)
    }
}
