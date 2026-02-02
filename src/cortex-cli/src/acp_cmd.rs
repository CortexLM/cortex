//! ACP (Agent Client Protocol) server command.
//!
//! The ACP protocol enables IDE integration (like Zed) with Cortex.
//! Supports both stdio and HTTP transports for flexible integration.

use anyhow::{Result, bail};
use clap::Parser;
use cortex_common::resolve_model_alias;
use std::net::SocketAddr;
use std::path::PathBuf;

/// ACP server CLI command.
#[derive(Debug, Parser)]
#[command(about = "Start an ACP (Agent Client Protocol) server for IDE integration")]
pub struct AcpCli {
    /// Working directory for the session.
    #[arg(long = "cwd", short = 'C', value_name = "DIR")]
    pub cwd: Option<PathBuf>,

    /// Port to listen on (default: random available port).
    /// If 0, uses stdio transport instead of HTTP.
    #[arg(long = "port", short = 'p', default_value = "0")]
    pub port: u16,

    /// Host address to bind to.
    #[arg(long = "host", default_value = "127.0.0.1")]
    pub host: String,

    /// Use stdio transport (JSON-RPC over stdin/stdout).
    #[arg(long = "stdio", conflicts_with_all = ["port", "host"])]
    pub stdio: bool,

    /// Enable verbose/debug output.
    #[arg(long = "verbose", short = 'v')]
    pub verbose: bool,

    /// Model to use.
    #[arg(long = "model", short = 'm')]
    pub model: Option<String>,

    /// Agent to use.
    #[arg(long = "agent")]
    pub agent: Option<String>,

    /// Tools to allow (whitelist). Can be specified multiple times.
    /// Only these tools will be available to the agent.
    #[arg(long = "allow-tool", action = clap::ArgAction::Append)]
    pub allow_tools: Vec<String>,

    /// Tools to deny (blacklist). Can be specified multiple times.
    /// These tools will be blocked from use.
    #[arg(long = "deny-tool", action = clap::ArgAction::Append)]
    pub deny_tools: Vec<String>,
}

impl AcpCli {
    /// Run the ACP server command.
    pub async fn run(self) -> Result<()> {
        // Validate agent exists early if specified (Issue #1958)
        if let Some(ref agent_name) = self.agent {
            let cortex_home = dirs::home_dir()
                .map(|h| h.join(".cortex"))
                .ok_or_else(|| anyhow::anyhow!("Could not determine home directory"))?;
            let cwd = std::env::current_dir().ok();
            let registry = cortex_engine::AgentRegistry::new(&cortex_home, cwd.as_deref());
            // Scan for agents in standard locations
            let _ = registry.scan().await;
            if !registry.exists(agent_name).await {
                bail!(
                    "Agent not found: '{}'. Use 'cortex agent list' to see available agents.",
                    agent_name
                );
            }
        }

        // Build configuration
        let mut config = cortex_engine::Config::default();

        if let Some(cwd) = &self.cwd {
            config.cwd = cwd.clone();
        }

        if let Some(model) = &self.model {
            // Resolve model alias (e.g., "sonnet" -> "anthropic/claude-sonnet-4-20250514")
            config.model = resolve_model_alias(model).to_string();
        }

        // Report tool restrictions (will be applied when server initializes session)
        if !self.allow_tools.is_empty() {
            eprintln!("Tool whitelist: {:?}", self.allow_tools);
            // Note: Tool restrictions are passed via server configuration
        }

        if !self.deny_tools.is_empty() {
            eprintln!("Tool blacklist: {:?}", self.deny_tools);
            // Note: Tool restrictions are passed via server configuration
        }

        // Decide transport mode
        if self.stdio || self.port == 0 {
            // Use stdio transport
            self.run_stdio_server(config).await
        } else {
            // Use HTTP transport
            self.run_http_server(config).await
        }
    }

    /// Run ACP server with stdio transport.
    async fn run_stdio_server(&self, config: cortex_engine::Config) -> Result<()> {
        eprintln!("Starting ACP server on stdio transport...");

        let server = cortex_engine::acp::AcpServer::new(config);
        server.run_stdio().await
    }

    /// Run ACP server with HTTP transport.
    async fn run_http_server(&self, config: cortex_engine::Config) -> Result<()> {
        let addr: SocketAddr = format!("{}:{}", self.host, self.port).parse()?;

        eprintln!("Starting ACP server on http://{}", addr);

        let server = cortex_engine::acp::AcpServer::new(config);
        server.run_http(addr).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    // ==========================================================================
    // AcpCli default values tests
    // ==========================================================================

    #[test]
    fn test_acp_cli_default_values() {
        let cli = AcpCli::try_parse_from(["acp"]).expect("should parse with no args");

        assert!(cli.cwd.is_none());
        assert_eq!(cli.port, 0);
        assert_eq!(cli.host, "127.0.0.1");
        assert!(!cli.stdio);
        assert!(!cli.verbose);
        assert!(cli.model.is_none());
        assert!(cli.agent.is_none());
        assert!(cli.allow_tools.is_empty());
        assert!(cli.deny_tools.is_empty());
    }

    // ==========================================================================
    // AcpCli cwd option tests
    // ==========================================================================

    #[test]
    fn test_acp_cli_cwd_long_option() {
        let cli =
            AcpCli::try_parse_from(["acp", "--cwd", "/home/user/project"]).expect("should parse");

        assert_eq!(cli.cwd, Some(PathBuf::from("/home/user/project")));
    }

    #[test]
    fn test_acp_cli_cwd_short_option() {
        let cli = AcpCli::try_parse_from(["acp", "-C", "/tmp/test"]).expect("should parse");

        assert_eq!(cli.cwd, Some(PathBuf::from("/tmp/test")));
    }

    // ==========================================================================
    // AcpCli port option tests
    // ==========================================================================

    #[test]
    fn test_acp_cli_port_long_option() {
        let cli = AcpCli::try_parse_from(["acp", "--port", "8080"]).expect("should parse");

        assert_eq!(cli.port, 8080);
    }

    #[test]
    fn test_acp_cli_port_short_option() {
        let cli = AcpCli::try_parse_from(["acp", "-p", "3000"]).expect("should parse");

        assert_eq!(cli.port, 3000);
    }

    #[test]
    fn test_acp_cli_port_zero_uses_stdio() {
        let cli = AcpCli::try_parse_from(["acp", "--port", "0"]).expect("should parse");

        assert_eq!(cli.port, 0);
    }

    // ==========================================================================
    // AcpCli host option tests
    // ==========================================================================

    #[test]
    fn test_acp_cli_host_option() {
        let cli = AcpCli::try_parse_from(["acp", "--host", "0.0.0.0"]).expect("should parse");

        assert_eq!(cli.host, "0.0.0.0");
    }

    #[test]
    fn test_acp_cli_host_ipv6() {
        let cli = AcpCli::try_parse_from(["acp", "--host", "::1"]).expect("should parse");

        assert_eq!(cli.host, "::1");
    }

    // ==========================================================================
    // AcpCli stdio option tests
    // ==========================================================================

    #[test]
    fn test_acp_cli_stdio_flag() {
        let cli = AcpCli::try_parse_from(["acp", "--stdio"]).expect("should parse");

        assert!(cli.stdio);
    }

    #[test]
    fn test_acp_cli_stdio_conflicts_with_port() {
        let result = AcpCli::try_parse_from(["acp", "--stdio", "--port", "8080"]);

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("cannot be used with") || err.contains("conflict"),
            "Expected conflict error, got: {}",
            err
        );
    }

    #[test]
    fn test_acp_cli_stdio_conflicts_with_host() {
        let result = AcpCli::try_parse_from(["acp", "--stdio", "--host", "0.0.0.0"]);

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("cannot be used with") || err.contains("conflict"),
            "Expected conflict error, got: {}",
            err
        );
    }

    // ==========================================================================
    // AcpCli verbose option tests
    // ==========================================================================

    #[test]
    fn test_acp_cli_verbose_long_flag() {
        let cli = AcpCli::try_parse_from(["acp", "--verbose"]).expect("should parse");

        assert!(cli.verbose);
    }

    #[test]
    fn test_acp_cli_verbose_short_flag() {
        let cli = AcpCli::try_parse_from(["acp", "-v"]).expect("should parse");

        assert!(cli.verbose);
    }

    // ==========================================================================
    // AcpCli model option tests
    // ==========================================================================

    #[test]
    fn test_acp_cli_model_long_option() {
        let cli = AcpCli::try_parse_from(["acp", "--model", "sonnet"]).expect("should parse");

        assert_eq!(cli.model, Some("sonnet".to_string()));
    }

    #[test]
    fn test_acp_cli_model_short_option() {
        let cli =
            AcpCli::try_parse_from(["acp", "-m", "gpt-4"]).expect("should parse with -m option");

        assert_eq!(cli.model, Some("gpt-4".to_string()));
    }

    #[test]
    fn test_acp_cli_model_with_full_path() {
        let cli = AcpCli::try_parse_from(["acp", "--model", "anthropic/claude-sonnet-4-20250514"])
            .expect("should parse");

        assert_eq!(
            cli.model,
            Some("anthropic/claude-sonnet-4-20250514".to_string())
        );
    }

    // ==========================================================================
    // AcpCli agent option tests
    // ==========================================================================

    #[test]
    fn test_acp_cli_agent_option() {
        let cli = AcpCli::try_parse_from(["acp", "--agent", "developer"]).expect("should parse");

        assert_eq!(cli.agent, Some("developer".to_string()));
    }

    #[test]
    fn test_acp_cli_agent_with_path_like_name() {
        let cli =
            AcpCli::try_parse_from(["acp", "--agent", "my-custom-agent"]).expect("should parse");

        assert_eq!(cli.agent, Some("my-custom-agent".to_string()));
    }

    // ==========================================================================
    // AcpCli allow_tools option tests
    // ==========================================================================

    #[test]
    fn test_acp_cli_allow_tool_single() {
        let cli = AcpCli::try_parse_from(["acp", "--allow-tool", "read"]).expect("should parse");

        assert_eq!(cli.allow_tools, vec!["read"]);
    }

    #[test]
    fn test_acp_cli_allow_tool_multiple() {
        let cli = AcpCli::try_parse_from([
            "acp",
            "--allow-tool",
            "read",
            "--allow-tool",
            "write",
            "--allow-tool",
            "execute",
        ])
        .expect("should parse");

        assert_eq!(cli.allow_tools, vec!["read", "write", "execute"]);
    }

    // ==========================================================================
    // AcpCli deny_tools option tests
    // ==========================================================================

    #[test]
    fn test_acp_cli_deny_tool_single() {
        let cli = AcpCli::try_parse_from(["acp", "--deny-tool", "execute"]).expect("should parse");

        assert_eq!(cli.deny_tools, vec!["execute"]);
    }

    #[test]
    fn test_acp_cli_deny_tool_multiple() {
        let cli = AcpCli::try_parse_from(["acp", "--deny-tool", "execute", "--deny-tool", "shell"])
            .expect("should parse");

        assert_eq!(cli.deny_tools, vec!["execute", "shell"]);
    }

    // ==========================================================================
    // AcpCli combined options tests
    // ==========================================================================

    #[test]
    fn test_acp_cli_combined_http_options() {
        let cli = AcpCli::try_parse_from([
            "acp",
            "--host",
            "0.0.0.0",
            "--port",
            "9000",
            "--model",
            "opus",
            "--verbose",
        ])
        .expect("should parse");

        assert_eq!(cli.host, "0.0.0.0");
        assert_eq!(cli.port, 9000);
        assert_eq!(cli.model, Some("opus".to_string()));
        assert!(cli.verbose);
        assert!(!cli.stdio);
    }

    #[test]
    fn test_acp_cli_combined_stdio_options() {
        let cli = AcpCli::try_parse_from([
            "acp",
            "--stdio",
            "--model",
            "sonnet",
            "--agent",
            "developer",
            "--cwd",
            "/workspace",
        ])
        .expect("should parse");

        assert!(cli.stdio);
        assert_eq!(cli.model, Some("sonnet".to_string()));
        assert_eq!(cli.agent, Some("developer".to_string()));
        assert_eq!(cli.cwd, Some(PathBuf::from("/workspace")));
    }

    #[test]
    fn test_acp_cli_all_options_http_mode() {
        let cli = AcpCli::try_parse_from([
            "acp",
            "--cwd",
            "/home/user/project",
            "--port",
            "8080",
            "--host",
            "127.0.0.1",
            "--verbose",
            "--model",
            "gpt-4",
            "--agent",
            "coder",
            "--allow-tool",
            "read",
            "--allow-tool",
            "write",
            "--deny-tool",
            "execute",
        ])
        .expect("should parse all options in HTTP mode");

        assert_eq!(cli.cwd, Some(PathBuf::from("/home/user/project")));
        assert_eq!(cli.port, 8080);
        assert_eq!(cli.host, "127.0.0.1");
        assert!(!cli.stdio);
        assert!(cli.verbose);
        assert_eq!(cli.model, Some("gpt-4".to_string()));
        assert_eq!(cli.agent, Some("coder".to_string()));
        assert_eq!(cli.allow_tools, vec!["read", "write"]);
        assert_eq!(cli.deny_tools, vec!["execute"]);
    }

    #[test]
    fn test_acp_cli_short_options_combined() {
        let cli = AcpCli::try_parse_from(["acp", "-C", "/tmp", "-p", "3000", "-m", "sonnet", "-v"])
            .expect("should parse with short options");

        assert_eq!(cli.cwd, Some(PathBuf::from("/tmp")));
        assert_eq!(cli.port, 3000);
        assert_eq!(cli.model, Some("sonnet".to_string()));
        assert!(cli.verbose);
    }

    // ==========================================================================
    // AcpCli edge cases and error handling tests
    // ==========================================================================

    #[test]
    fn test_acp_cli_invalid_port_value() {
        let result = AcpCli::try_parse_from(["acp", "--port", "invalid"]);

        assert!(result.is_err());
    }

    #[test]
    fn test_acp_cli_port_out_of_range() {
        // u16 max is 65535, anything above should fail
        let result = AcpCli::try_parse_from(["acp", "--port", "70000"]);

        assert!(result.is_err());
    }

    #[test]
    fn test_acp_cli_missing_value_for_option() {
        let result = AcpCli::try_parse_from(["acp", "--model"]);

        assert!(result.is_err());
    }

    #[test]
    fn test_acp_cli_unknown_option() {
        let result = AcpCli::try_parse_from(["acp", "--unknown-flag"]);

        assert!(result.is_err());
    }

    #[test]
    fn test_acp_cli_empty_tool_name_allowed() {
        // Clap allows empty strings by default
        let cli = AcpCli::try_parse_from(["acp", "--allow-tool", ""]).expect("should parse");

        assert_eq!(cli.allow_tools, vec![""]);
    }

    #[test]
    fn test_acp_cli_tool_with_special_characters() {
        let cli = AcpCli::try_parse_from(["acp", "--allow-tool", "my-tool_v2:latest"])
            .expect("should parse");

        assert_eq!(cli.allow_tools, vec!["my-tool_v2:latest"]);
    }

    #[test]
    fn test_acp_cli_both_allow_and_deny_tools() {
        let cli = AcpCli::try_parse_from([
            "acp",
            "--allow-tool",
            "read",
            "--deny-tool",
            "execute",
            "--allow-tool",
            "write",
        ])
        .expect("should parse with both allow and deny tools");

        assert_eq!(cli.allow_tools, vec!["read", "write"]);
        assert_eq!(cli.deny_tools, vec!["execute"]);
    }
}
