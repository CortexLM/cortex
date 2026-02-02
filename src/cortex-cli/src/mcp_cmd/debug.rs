//! Debug and connection testing for MCP servers.
//!
//! This module provides the debug command and connection testing utilities
//! for both stdio and HTTP MCP servers.

use anyhow::{Context, Result, bail};
use cortex_engine::create_default_client;
use std::io::Write;

use super::auth::get_auth_status_for_display;
use super::config::get_mcp_server;
use super::macros::safe_println;
use super::types::DebugArgs;
use super::validation::validate_server_name;

/// Server info returned from connection tests.
pub(crate) struct ServerInfo {
    pub capabilities: serde_json::Value,
    pub tools: serde_json::Value,
    pub resources: serde_json::Value,
    pub prompts: serde_json::Value,
}

/// Run the debug command.
pub(crate) async fn run_debug(args: DebugArgs) -> Result<()> {
    let DebugArgs {
        name,
        json,
        test_auth,
        timeout,
        no_cache,
        show_cache_info,
    } = args;

    // Issue #2319: Display cache status information
    // When --no-cache is used, always perform fresh checks
    // When --show-cache-info is used, display cache age if results are cached
    let cache_status = if no_cache {
        "fresh (--no-cache)"
    } else {
        "fresh" // All checks are currently fresh, but flag reserved for future caching
    };
    let _ = show_cache_info; // Reserved for future use when caching is implemented

    validate_server_name(&name)?;

    let server = get_mcp_server(&name)?
        .ok_or_else(|| anyhow::anyhow!("No MCP server named '{}' found", name))?;

    let enabled = server
        .get("enabled")
        .and_then(toml::Value::as_bool)
        .unwrap_or(true);

    let transport = server
        .get("transport")
        .ok_or_else(|| anyhow::anyhow!("MCP server '{name}' has no transport configured"))?;

    let transport_type = transport
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    // Record check timestamp to indicate freshness of results
    let check_timestamp = chrono::Utc::now().to_rfc3339();

    let mut debug_result = serde_json::json!({
        "name": name,
        "enabled": enabled,
        "transport_type": transport_type,
        "connection": {
            "success": serde_json::Value::Null,
            "error": serde_json::Value::Null
        },
        "capabilities": null,
        "tools": [],
        "resources": [],
        "prompts": [],
        "auth_status": null,
        "errors": [],
        "checked_at": check_timestamp,
    });

    let mut errors: Vec<String> = Vec::new();

    // Use safe_println! to avoid SIGPIPE crashes when output is piped (Issue #1989)
    if !json {
        safe_println!("Debugging MCP Server: {name}");
        safe_println!("{}", "=".repeat(50));
        // Issue #2319: Show cache status to indicate freshness of results
        safe_println!("Checked at: {} ({})", check_timestamp, cache_status);
        safe_println!();
        safe_println!("Configuration:");
        safe_println!("  Enabled: {enabled}");
        safe_println!("  Transport: {transport_type}");
    }

    // Transport-specific info
    match transport_type {
        "stdio" => {
            let cmd = transport
                .get("command")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            let args = transport
                .get("args")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str())
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .unwrap_or_default();

            debug_result["command"] = serde_json::json!(cmd);
            debug_result["args"] = serde_json::json!(args);

            if !json {
                safe_println!("  Command: {cmd}");
                if !args.is_empty() {
                    safe_println!("  Args: {args}");
                }
            }

            // Test if command exists (check if it's in PATH)
            let cmd_exists = std::process::Command::new(cmd)
                .arg("--version")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok();
            debug_result["command_exists"] = serde_json::json!(cmd_exists);

            if !json {
                safe_println!();
                safe_println!("Command Check:");
                if cmd_exists {
                    safe_println!("  ✓ Command '{cmd}' found in PATH");
                } else {
                    safe_println!("  ✗ Command '{cmd}' not found in PATH");
                    errors.push(format!("Command '{}' not found in PATH", cmd));
                }
            }

            // Try to connect and get capabilities
            if !json {
                safe_println!();
                safe_println!("Connection Test:");
            }

            match test_stdio_connection(&name, cmd, &args, timeout).await {
                Ok(info) => {
                    debug_result["connection"] = serde_json::json!({
                        "success": true
                    });
                    debug_result["capabilities"] = info.capabilities.clone();
                    debug_result["tools"] = info.tools.clone();
                    debug_result["resources"] = info.resources.clone();
                    debug_result["prompts"] = info.prompts.clone();

                    if !json {
                        safe_println!("  ✓ Connected successfully");
                        safe_println!();
                        safe_println!("Server Capabilities:");
                        if let Some(caps) = info.capabilities.as_object() {
                            for (key, value) in caps {
                                safe_println!("  • {key}: {value}");
                            }
                        } else {
                            safe_println!("  (none reported)");
                        }
                        safe_println!();
                        safe_println!(
                            "Available Tools: {}",
                            info.tools.as_array().map(|a| a.len()).unwrap_or(0)
                        );
                        if let Some(tools) = info.tools.as_array() {
                            for tool in tools.iter().take(10) {
                                if let Some(name) = tool.get("name").and_then(|v| v.as_str()) {
                                    let desc = tool
                                        .get("description")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("");
                                    let desc_short = if desc.len() > 50 {
                                        format!("{}...", &desc[..47])
                                    } else {
                                        desc.to_string()
                                    };
                                    safe_println!("  • {name}: {desc_short}");
                                }
                            }
                            if tools.len() > 10 {
                                safe_println!("  ... and {} more", tools.len() - 10);
                            }
                        }
                        safe_println!();
                        let resources_count =
                            info.resources.as_array().map(|a| a.len()).unwrap_or(0);
                        let prompts_count = info.prompts.as_array().map(|a| a.len()).unwrap_or(0);
                        safe_println!(
                            "Available Resources: {}{}",
                            resources_count,
                            if resources_count == 0 {
                                " (server does not expose any resources)"
                            } else {
                                ""
                            }
                        );
                        safe_println!(
                            "Available Prompts: {}{}",
                            prompts_count,
                            if prompts_count == 0 {
                                " (server does not expose any prompts)"
                            } else {
                                ""
                            }
                        );
                    }
                }
                Err(e) => {
                    let error_msg = format!("Connection failed: {}", e);
                    debug_result["connection"] = serde_json::json!({
                        "success": false,
                        "error": error_msg.clone()
                    });
                    errors.push(error_msg);

                    if !json {
                        safe_println!("  ✗ Connection failed: {e}");
                    }
                }
            }
        }
        "http" => {
            let url = transport.get("url").and_then(|v| v.as_str()).unwrap_or("?");

            debug_result["url"] = serde_json::json!(url);

            if !json {
                safe_println!("  URL: {url}");
            }

            // Test HTTP connection
            if !json {
                safe_println!();
                safe_println!("Connection Test:");
            }

            match test_http_connection(&name, url, timeout).await {
                Ok(info) => {
                    debug_result["connection"] = serde_json::json!({
                        "success": true
                    });
                    debug_result["capabilities"] = info.capabilities.clone();
                    debug_result["tools"] = info.tools.clone();
                    debug_result["resources"] = info.resources.clone();
                    debug_result["prompts"] = info.prompts.clone();

                    if !json {
                        safe_println!("  ✓ Connected successfully");
                        safe_println!();
                        safe_println!("Server Capabilities:");
                        if let Some(caps) = info.capabilities.as_object() {
                            for (key, value) in caps {
                                safe_println!("  • {key}: {value}");
                            }
                        } else {
                            safe_println!("  (none reported)");
                        }
                        safe_println!();
                        safe_println!(
                            "Available Tools: {}",
                            info.tools.as_array().map(|a| a.len()).unwrap_or(0)
                        );
                        if let Some(tools) = info.tools.as_array() {
                            for tool in tools.iter().take(10) {
                                if let Some(name) = tool.get("name").and_then(|v| v.as_str()) {
                                    let desc = tool
                                        .get("description")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("");
                                    let desc_short = if desc.len() > 50 {
                                        format!("{}...", &desc[..47])
                                    } else {
                                        desc.to_string()
                                    };
                                    safe_println!("  • {name}: {desc_short}");
                                }
                            }
                            if tools.len() > 10 {
                                safe_println!("  ... and {} more", tools.len() - 10);
                            }
                        }
                        safe_println!();
                        let resources_count =
                            info.resources.as_array().map(|a| a.len()).unwrap_or(0);
                        let prompts_count = info.prompts.as_array().map(|a| a.len()).unwrap_or(0);
                        safe_println!(
                            "Available Resources: {}{}",
                            resources_count,
                            if resources_count == 0 {
                                " (server does not expose any resources)"
                            } else {
                                ""
                            }
                        );
                        safe_println!(
                            "Available Prompts: {}{}",
                            prompts_count,
                            if prompts_count == 0 {
                                " (server does not expose any prompts)"
                            } else {
                                ""
                            }
                        );
                    }
                }
                Err(e) => {
                    let error_msg = format!("Connection failed: {}", e);
                    debug_result["connection"] = serde_json::json!({
                        "success": false,
                        "error": error_msg.clone()
                    });
                    errors.push(error_msg);

                    if !json {
                        safe_println!("  ✗ Connection failed: {e}");
                    }
                }
            }

            // Check OAuth status
            if test_auth {
                if !json {
                    safe_println!();
                    safe_println!("OAuth Status:");
                }

                let auth_status = get_auth_status_for_display(&name, url)
                    .await
                    .unwrap_or_else(|_| "Unknown".to_string());
                debug_result["auth_status"] = serde_json::json!(auth_status);

                if !json {
                    safe_println!("  {auth_status}");

                    if auth_status == "Not Authenticated" {
                        safe_println!("\n  Use 'cortex mcp auth {name}' to authenticate.");
                    }
                }
            }
        }
        _ => {
            errors.push(format!("Unknown transport type: {}", transport_type));
            if !json {
                safe_println!("  ✗ Unknown transport type: {transport_type}");
            }
        }
    }

    // Only include errors field if there are errors
    if !errors.is_empty() {
        debug_result["errors"] = serde_json::json!(errors);
    }

    if json {
        let output = serde_json::to_string_pretty(&debug_result)?;
        safe_println!("{output}");
    } else if !errors.is_empty() {
        safe_println!();
        safe_println!("Errors:");
        for err in &errors {
            safe_println!("  ✗ {err}");
        }
    }

    Ok(())
}

/// Test connection to a stdio MCP server.
async fn test_stdio_connection(
    _name: &str,
    command: &str,
    _args: &str,
    timeout_secs: u64,
) -> Result<ServerInfo> {
    use std::process::Stdio;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::process::Command;
    use tokio::time::{Duration, timeout};

    // Parse args
    let args_vec: Vec<&str> = if _args.is_empty() {
        vec![]
    } else {
        _args.split_whitespace().collect()
    };

    // Spawn the process
    let mut child = Command::new(command)
        .args(&args_vec)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("Failed to spawn command: {}", command))?;

    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| anyhow::anyhow!("Failed to get stdin"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("Failed to get stdout"))?;
    let mut reader = BufReader::new(stdout);

    // Send initialize request
    let init_request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {
                "name": "Cortex",
                "version": "1.0.0"
            }
        }
    });

    let request_str = serde_json::to_string(&init_request)? + "\n";
    stdin.write_all(request_str.as_bytes()).await?;
    stdin.flush().await?;

    // Read response with timeout
    let mut response_line = String::new();
    let read_result = timeout(
        Duration::from_secs(timeout_secs),
        reader.read_line(&mut response_line),
    )
    .await
    .map_err(|_| anyhow::anyhow!("Connection timed out after {} seconds", timeout_secs))?
    .with_context(|| "Failed to read response")?;

    if read_result == 0 {
        bail!("Server closed connection without response");
    }

    let response: serde_json::Value =
        serde_json::from_str(&response_line).with_context(|| "Failed to parse server response")?;

    let capabilities = response
        .get("result")
        .and_then(|r| r.get("capabilities"))
        .cloned()
        .unwrap_or(serde_json::json!({}));

    // Send initialized notification
    let initialized = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    });
    let notif_str = serde_json::to_string(&initialized)? + "\n";
    stdin.write_all(notif_str.as_bytes()).await?;
    stdin.flush().await?;

    // Request tools list
    let tools_request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/list",
        "params": {}
    });
    let tools_str = serde_json::to_string(&tools_request)? + "\n";
    stdin.write_all(tools_str.as_bytes()).await?;
    stdin.flush().await?;

    let mut tools_line = String::new();
    let _ = timeout(Duration::from_secs(5), reader.read_line(&mut tools_line)).await;
    let tools = if !tools_line.is_empty() {
        serde_json::from_str::<serde_json::Value>(&tools_line)
            .ok()
            .and_then(|v| v.get("result").and_then(|r| r.get("tools")).cloned())
            .unwrap_or(serde_json::json!([]))
    } else {
        serde_json::json!([])
    };

    // Request resources list
    let resources_request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "resources/list",
        "params": {}
    });
    let resources_str = serde_json::to_string(&resources_request)? + "\n";
    stdin.write_all(resources_str.as_bytes()).await?;
    stdin.flush().await?;

    let mut resources_line = String::new();
    let _ = timeout(
        Duration::from_secs(5),
        reader.read_line(&mut resources_line),
    )
    .await;
    let resources = if !resources_line.is_empty() {
        serde_json::from_str::<serde_json::Value>(&resources_line)
            .ok()
            .and_then(|v| v.get("result").and_then(|r| r.get("resources")).cloned())
            .unwrap_or(serde_json::json!([]))
    } else {
        serde_json::json!([])
    };

    // Request prompts list
    let prompts_request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "prompts/list",
        "params": {}
    });
    let prompts_str = serde_json::to_string(&prompts_request)? + "\n";
    stdin.write_all(prompts_str.as_bytes()).await?;
    stdin.flush().await?;

    let mut prompts_line = String::new();
    let _ = timeout(Duration::from_secs(5), reader.read_line(&mut prompts_line)).await;
    let prompts = if !prompts_line.is_empty() {
        serde_json::from_str::<serde_json::Value>(&prompts_line)
            .ok()
            .and_then(|v| v.get("result").and_then(|r| r.get("prompts")).cloned())
            .unwrap_or(serde_json::json!([]))
    } else {
        serde_json::json!([])
    };

    // Kill the process
    let _ = child.kill().await;

    Ok(ServerInfo {
        capabilities,
        tools,
        resources,
        prompts,
    })
}

/// Test connection to an HTTP MCP server.
async fn test_http_connection(_name: &str, url: &str, timeout_secs: u64) -> Result<ServerInfo> {
    use tokio::time::{Duration, timeout};

    let client = create_default_client().context("Failed to create HTTP client")?;

    // Send initialize request
    let init_request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {
                "name": "Cortex",
                "version": "1.0.0"
            }
        }
    });

    let response = timeout(
        Duration::from_secs(timeout_secs),
        client
            .post(url)
            .header("Content-Type", "application/json")
            .json(&init_request)
            .send(),
    )
    .await
    .map_err(|_| anyhow::anyhow!("Connection timed out after {} seconds", timeout_secs))?
    .map_err(|e| anyhow::anyhow!("HTTP request failed: {}", e))?;

    if !response.status().is_success() {
        bail!("Server returned error: {}", response.status());
    }

    let response_json: serde_json::Value = response
        .json()
        .await
        .with_context(|| "Failed to parse server response")?;

    let capabilities = response_json
        .get("result")
        .and_then(|r| r.get("capabilities"))
        .cloned()
        .unwrap_or(serde_json::json!({}));

    // Send initialized notification
    let initialized = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    });
    let _ = client
        .post(url)
        .header("Content-Type", "application/json")
        .json(&initialized)
        .send()
        .await;

    // Request tools list
    let tools_request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/list",
        "params": {}
    });
    let tools = match client
        .post(url)
        .header("Content-Type", "application/json")
        .json(&tools_request)
        .send()
        .await
    {
        Ok(resp) => resp
            .json::<serde_json::Value>()
            .await
            .ok()
            .and_then(|v| v.get("result").and_then(|r| r.get("tools")).cloned())
            .unwrap_or(serde_json::json!([])),
        Err(_) => serde_json::json!([]),
    };

    // Request resources list
    let resources_request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "resources/list",
        "params": {}
    });
    let resources = match client
        .post(url)
        .header("Content-Type", "application/json")
        .json(&resources_request)
        .send()
        .await
    {
        Ok(resp) => resp
            .json::<serde_json::Value>()
            .await
            .ok()
            .and_then(|v| v.get("result").and_then(|r| r.get("resources")).cloned())
            .unwrap_or(serde_json::json!([])),
        Err(_) => serde_json::json!([]),
    };

    // Request prompts list
    let prompts_request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "prompts/list",
        "params": {}
    });
    let prompts = match client
        .post(url)
        .header("Content-Type", "application/json")
        .json(&prompts_request)
        .send()
        .await
    {
        Ok(resp) => resp
            .json::<serde_json::Value>()
            .await
            .ok()
            .and_then(|v| v.get("result").and_then(|r| r.get("prompts")).cloned())
            .unwrap_or(serde_json::json!([])),
        Err(_) => serde_json::json!([]),
    };

    Ok(ServerInfo {
        capabilities,
        tools,
        resources,
        prompts,
    })
}
