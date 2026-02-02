//! Wait command handler.

use anyhow::{Result, bail};
use cortex_engine::create_default_client;
use std::time::Duration;

use crate::debug_cmd::commands::WaitArgs;
use crate::debug_cmd::types::WaitResult;
use crate::debug_cmd::utils::check_command_installed;

/// Run the wait debug command.
pub async fn run_wait(args: WaitArgs) -> Result<()> {
    let start = std::time::Instant::now();
    let timeout = Duration::from_secs(args.timeout);

    // Validate interval: minimum 100ms to prevent high CPU usage
    // Treat 0 as default (500ms)
    let interval_ms = if args.interval == 0 {
        500
    } else if args.interval < 100 {
        100
    } else {
        args.interval
    };
    let interval = Duration::from_millis(interval_ms);

    let (condition, success, error) = if args.lsp_ready {
        // Wait for LSP - check if rust-analyzer or similar is available
        let condition = "lsp_ready".to_string();
        let mut success = false;
        let mut error = None;

        while start.elapsed() < timeout {
            let (available, _, _) = check_command_installed("rust-analyzer").await;
            if available {
                success = true;
                break;
            }
            tokio::time::sleep(interval).await;
        }

        if !success {
            error = Some("Timeout waiting for LSP".to_string());
        }

        (condition, success, error)
    } else if args.server_ready {
        // Wait for HTTP server
        let condition = format!("server_ready ({})", args.server_url);
        let mut success = false;
        let mut error = None;

        let client = create_default_client()?;

        while start.elapsed() < timeout {
            match client.get(&args.server_url).send().await {
                Ok(response)
                    if response.status().is_success() || response.status().is_client_error() =>
                {
                    // Server is responding (even 4xx means it's up)
                    success = true;
                    break;
                }
                _ => {
                    tokio::time::sleep(interval).await;
                }
            }
        }

        if !success {
            error = Some(format!("Timeout waiting for server at {}", args.server_url));
        }

        (condition, success, error)
    } else if let Some(port) = args.port {
        // Wait for TCP port to be available
        let condition = format!("port_ready ({}:{})", args.host, port);
        let mut success = false;
        let mut error = None;

        let addr = format!("{}:{}", args.host, port);

        while start.elapsed() < timeout {
            match tokio::net::TcpStream::connect(&addr).await {
                Ok(_) => {
                    // Port is open and accepting connections
                    success = true;
                    break;
                }
                Err(_) => {
                    tokio::time::sleep(interval).await;
                }
            }
        }

        if !success {
            error = Some(format!(
                "Timeout waiting for port {} on {}",
                port, args.host
            ));
        }

        (condition, success, error)
    } else {
        let error_message = "No condition specified. Use --lsp-ready, --server-ready, or --port";
        if args.json {
            let error = serde_json::json!({
                "error": error_message,
                "success": false
            });
            println!("{}", serde_json::to_string_pretty(&error)?);
        }
        bail!("{}", error_message);
    };

    let waited_ms = start.elapsed().as_millis() as u64;

    let result = WaitResult {
        condition,
        success,
        waited_ms,
        error,
    };

    if args.json {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        println!("Wait Result");
        println!("{}", "=".repeat(40));
        println!("  Condition: {}", result.condition);
        println!(
            "  Success:   {} ({})",
            result.success,
            if result.success {
                format!("{} is ready", result.condition)
            } else {
                "condition not met".to_string()
            }
        );
        // Display waited time in seconds (consistent with --timeout which is in seconds)
        println!("  Waited:    {:.2}s", result.waited_ms as f64 / 1000.0);
        if let Some(ref err) = result.error {
            println!("  Error:     {}", err);
        }
    }

    if !result.success {
        std::process::exit(1);
    }

    Ok(())
}
