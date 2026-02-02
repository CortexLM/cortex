//! LSP command handler.

use anyhow::Result;

use crate::debug_cmd::commands::LspArgs;
use crate::debug_cmd::types::{LspConnectionTest, LspDebugOutput, LspServerInfo};
use crate::debug_cmd::utils::check_command_installed;

/// Run the LSP debug command.
pub async fn run_lsp(args: LspArgs) -> Result<()> {
    // Known LSP servers
    let known_servers = vec![
        ("rust-analyzer", "Rust", "rust-analyzer"),
        (
            "typescript-language-server",
            "TypeScript/JavaScript",
            "typescript-language-server",
        ),
        ("pyright", "Python", "pyright-langserver"),
        ("pylsp", "Python", "pylsp"),
        ("gopls", "Go", "gopls"),
        ("clangd", "C/C++", "clangd"),
        ("lua-language-server", "Lua", "lua-language-server"),
        ("marksman", "Markdown", "marksman"),
        ("yaml-language-server", "YAML", "yaml-language-server"),
        (
            "vscode-json-language-server",
            "JSON",
            "vscode-json-language-server",
        ),
        ("bash-language-server", "Bash", "bash-language-server"),
        ("taplo", "TOML", "taplo"),
        ("zls", "Zig", "zls"),
    ];

    let mut servers = Vec::new();

    for (name, language, command) in known_servers {
        let (installed, path, version) = check_command_installed(command).await;
        servers.push(LspServerInfo {
            name: name.to_string(),
            language: language.to_string(),
            command: command.to_string(),
            installed,
            version,
            path,
        });
    }

    // Filter if specific server requested
    if let Some(ref server_name) = args.server {
        servers.retain(|s| s.name.to_lowercase().contains(&server_name.to_lowercase()));
    }

    // Filter by language if specified
    if let Some(ref lang) = args.language {
        servers.retain(|s| s.language.to_lowercase().contains(&lang.to_lowercase()));
    }

    // Connection test placeholder (actual implementation would require LSP client)
    let connection_test = if args.server.is_some() || args.file.is_some() {
        let server = args.server.as_deref().unwrap_or("auto-detect");
        Some(LspConnectionTest {
            server: server.to_string(),
            success: false,
            latency_ms: None,
            error: Some("LSP connection testing not yet implemented".to_string()),
        })
    } else {
        None
    };

    let output = LspDebugOutput {
        servers,
        connection_test,
    };

    if args.json {
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        println!("LSP Servers");
        println!("{}", "=".repeat(60));
        println!("{:<30} {:<15} {:<10}", "Server", "Language", "Status");
        println!("{}", "-".repeat(60));

        for server in &output.servers {
            let status = if server.installed {
                "installed"
            } else {
                "not found"
            };
            println!("{:<30} {:<15} {:<10}", server.name, server.language, status);
            if let Some(ref path) = server.path {
                println!("    Path: {}", path.display());
            }
            if let Some(ref version) = server.version {
                println!("    Version: {}", version);
            }
        }

        if let Some(ref test) = output.connection_test {
            println!();
            println!("Connection Test: {}", test.server);
            println!("{}", "-".repeat(40));
            println!("  Success: {}", test.success);
            if let Some(latency) = test.latency_ms {
                println!("  Latency: {}ms", latency);
            }
            if let Some(ref error) = test.error {
                println!("  Error: {}", error);
            }
        }
    }

    Ok(())
}
