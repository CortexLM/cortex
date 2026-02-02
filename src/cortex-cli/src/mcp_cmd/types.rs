//! CLI command definitions and argument types for MCP commands.
//!
//! This module contains all the clap-derived structs for the MCP subcommands.

use clap::{ArgGroup, Parser};
use cortex_common::CliConfigOverrides;

use super::validation::parse_env_pair;

/// MCP management CLI.
#[derive(Debug, Parser)]
pub struct McpCli {
    #[clap(flatten)]
    pub config_overrides: CliConfigOverrides,

    #[command(subcommand)]
    pub subcommand: McpSubcommand,
}

/// MCP subcommands.
#[derive(Debug, clap::Subcommand)]
pub enum McpSubcommand {
    /// List configured MCP servers.
    List(ListArgs),

    /// List configured MCP servers (alias for list).
    #[command(visible_alias = "ls")]
    Ls(ListArgs),

    /// Show details for a configured MCP server.
    Get(GetArgs),

    /// Add a global MCP server entry.
    Add(AddArgs),

    /// Remove a global MCP server entry.
    #[command(visible_alias = "rm")]
    Remove(RemoveArgs),

    /// Enable a disabled MCP server.
    Enable(EnableArgs),

    /// Disable an MCP server without removing it.
    Disable(DisableArgs),

    /// Rename an MCP server.
    Rename(RenameArgs),

    /// Authenticate with an OAuth-enabled MCP server.
    Auth(AuthCommand),

    /// Remove OAuth credentials for an MCP server.
    Logout(LogoutArgs),

    /// Debug and test an MCP server connection.
    Debug(DebugArgs),
}

/// Arguments for list command.
#[derive(Debug, Parser)]
pub struct ListArgs {
    /// Output the configured servers as JSON.
    #[arg(long)]
    pub json: bool,

    /// Show all servers including disabled ones.
    #[arg(long)]
    pub all: bool,
}

/// Arguments for get command.
#[derive(Debug, Parser)]
pub struct GetArgs {
    /// Name of the MCP server to display.
    pub name: String,

    /// Output the server configuration as JSON.
    #[arg(long)]
    pub json: bool,
}

/// Arguments for add command.
#[derive(Debug, Parser)]
#[command(
    override_usage = "cortex mcp add [OPTIONS] <NAME> (--url <URL> | -- <COMMAND>...)",
    after_help = "IMPORTANT: Use '--' to separate the MCP server command from cortex options.\n\
This is required when the command or its arguments start with a dash (-).\n\n\
Examples:\n  \
cortex mcp add myserver -- npx @example/server\n  \
cortex mcp add myserver -- python -m my_server\n  \
cortex mcp add myserver -- node server.js -v       # -v goes to server, not cortex\n  \
cortex mcp add myserver --url https://example.com/mcp\n\n\
Without '--', arguments like '-v' or '-m' may be interpreted as cortex flags."
)]
pub struct AddArgs {
    /// Name for the MCP server configuration.
    pub name: String,

    /// Overwrite existing server configuration if it exists.
    #[arg(long, short = 'f')]
    pub force: bool,

    /// Allow localhost and private network URLs (for local development).
    /// By default, URLs containing localhost, 127.0.0.1, or private network
    /// addresses are blocked for security. Use this flag to enable local
    /// development servers.
    #[arg(long)]
    pub allow_local: bool,

    #[command(flatten)]
    pub transport_args: AddMcpTransportArgs,
}

/// Transport arguments for add command.
#[derive(Debug, clap::Args)]
#[command(
    group(
        ArgGroup::new("transport")
            .args(["command", "url", "sse"])
            .required(true)
            .multiple(false)
    )
)]
pub struct AddMcpTransportArgs {
    #[command(flatten)]
    pub stdio: Option<AddMcpStdioArgs>,

    #[command(flatten)]
    pub streamable_http: Option<AddMcpStreamableHttpArgs>,

    #[command(flatten)]
    pub sse_transport: Option<AddMcpSseArgs>,
}

/// Stdio transport arguments.
#[derive(Debug, clap::Args)]
pub struct AddMcpStdioArgs {
    /// Command to launch the MCP server.
    #[arg(trailing_var_arg = true, num_args = 0..)]
    pub command: Vec<String>,

    /// Environment variables to set when launching the server.
    #[arg(long, value_parser = parse_env_pair, value_name = "KEY=VALUE")]
    pub env: Vec<(String, String)>,
}

/// HTTP transport arguments.
#[derive(Debug, clap::Args)]
pub struct AddMcpStreamableHttpArgs {
    /// URL for a streamable HTTP MCP server.
    #[arg(long)]
    pub url: String,

    /// Name of the environment variable containing a bearer token (not the token itself).
    /// The CLI will read the token value from this env var at runtime.
    /// Example: --bearer-token-env-var MY_API_TOKEN (where MY_API_TOKEN env var contains the token)
    #[arg(
        long = "bearer-token-env-var",
        value_name = "ENV_VAR",
        requires = "url"
    )]
    pub bearer_token_env_var: Option<String>,
}

/// SSE (Server-Sent Events) transport arguments.
#[derive(Debug, clap::Args)]
pub struct AddMcpSseArgs {
    /// URL for an SSE (Server-Sent Events) MCP server.
    /// Use this for MCP servers that communicate via SSE transport.
    #[arg(long = "sse", value_name = "URL")]
    pub sse_url: Option<String>,

    /// Optional environment variable to read for a bearer token.
    #[arg(
        long = "sse-bearer-token-env-var",
        value_name = "ENV_VAR",
        requires = "sse"
    )]
    pub sse_bearer_token_env_var: Option<String>,
}

/// Arguments for remove command.
#[derive(Debug, Parser)]
pub struct RemoveArgs {
    /// Name of the MCP server configuration to remove.
    pub name: String,

    /// Skip confirmation prompt.
    /// Aliases: --force, -f (for compatibility)
    #[arg(short = 'y', long = "yes", visible_aliases = ["force"], short_alias = 'f')]
    pub yes: bool,
}

/// Arguments for enable command.
#[derive(Debug, Parser)]
pub struct EnableArgs {
    /// Name of the MCP server to enable.
    pub name: String,
}

/// Arguments for disable command.
#[derive(Debug, Parser)]
pub struct DisableArgs {
    /// Name of the MCP server to disable.
    pub name: String,
}

/// Arguments for rename command.
#[derive(Debug, Parser)]
pub struct RenameArgs {
    /// Current name of the MCP server.
    pub old_name: String,

    /// New name for the MCP server.
    pub new_name: String,
}

/// Auth command with subcommands.
#[derive(Debug, Parser)]
pub struct AuthCommand {
    #[command(subcommand)]
    pub action: Option<AuthSubcommand>,

    /// Name of the MCP server to authenticate with (if no subcommand).
    pub name: Option<String>,

    /// Client ID for OAuth (if not using dynamic registration).
    #[arg(long)]
    pub client_id: Option<String>,

    /// Client secret for OAuth (if required).
    #[arg(long)]
    pub client_secret: Option<String>,
}

/// Auth subcommands.
#[derive(Debug, clap::Subcommand)]
pub enum AuthSubcommand {
    /// List OAuth status for all servers.
    List(AuthListArgs),
}

/// Arguments for auth list command.
#[derive(Debug, Parser)]
pub struct AuthListArgs {
    /// Output as JSON.
    #[arg(long)]
    pub json: bool,
}

/// Arguments for logout command.
#[derive(Debug, Parser)]
pub struct LogoutArgs {
    /// Name of the MCP server to remove credentials for.
    #[arg(required_unless_present = "all")]
    pub name: Option<String>,

    /// Remove OAuth credentials for all servers.
    #[arg(long)]
    pub all: bool,
}

/// Arguments for debug command.
#[derive(Debug, Parser)]
pub struct DebugArgs {
    /// Name of the MCP server to debug.
    pub name: String,

    /// Output as JSON.
    #[arg(long)]
    pub json: bool,

    /// Test OAuth authentication if configured.
    #[arg(long)]
    pub test_auth: bool,

    /// Timeout in seconds for connection test.
    #[arg(long, default_value = "30")]
    pub timeout: u64,

    /// Force fresh health check, bypassing any cache.
    /// By default, health checks may be cached for performance.
    /// Use this flag to ensure you get the current status after making configuration changes.
    #[arg(long)]
    pub no_cache: bool,

    /// Show cache information when displaying cached results.
    /// Displays the age of cached health status if available.
    #[arg(long)]
    pub show_cache_info: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // ListArgs tests
    // =========================================================================

    #[test]
    fn test_list_args_defaults() {
        let args = ListArgs {
            json: false,
            all: false,
        };
        assert!(!args.json, "JSON should be false by default");
        assert!(!args.all, "All should be false by default");
    }

    #[test]
    fn test_list_args_json_enabled() {
        let args = ListArgs {
            json: true,
            all: false,
        };
        assert!(args.json, "JSON flag should be settable to true");
    }

    #[test]
    fn test_list_args_all_enabled() {
        let args = ListArgs {
            json: false,
            all: true,
        };
        assert!(args.all, "All flag should be settable to true");
    }

    #[test]
    fn test_list_args_both_flags() {
        let args = ListArgs {
            json: true,
            all: true,
        };
        assert!(args.json, "JSON should be true");
        assert!(args.all, "All should be true");
    }

    // =========================================================================
    // GetArgs tests
    // =========================================================================

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

    // =========================================================================
    // AddArgs tests
    // =========================================================================

    #[test]
    fn test_add_args_defaults() {
        let args = AddArgs {
            name: "myserver".to_string(),
            force: false,
            allow_local: false,
            transport_args: AddMcpTransportArgs {
                stdio: None,
                streamable_http: None,
                sse_transport: None,
            },
        };
        assert_eq!(args.name, "myserver");
        assert!(!args.force, "Force should be false by default");
        assert!(!args.allow_local, "Allow local should be false by default");
    }

    #[test]
    fn test_add_args_with_force() {
        let args = AddArgs {
            name: "server".to_string(),
            force: true,
            allow_local: false,
            transport_args: AddMcpTransportArgs {
                stdio: None,
                streamable_http: None,
                sse_transport: None,
            },
        };
        assert!(args.force, "Force should be true");
    }

    #[test]
    fn test_add_args_with_allow_local() {
        let args = AddArgs {
            name: "local-server".to_string(),
            force: false,
            allow_local: true,
            transport_args: AddMcpTransportArgs {
                stdio: None,
                streamable_http: None,
                sse_transport: None,
            },
        };
        assert!(args.allow_local, "Allow local should be true");
    }

    // =========================================================================
    // AddMcpTransportArgs tests
    // =========================================================================

    #[test]
    fn test_transport_args_empty() {
        let args = AddMcpTransportArgs {
            stdio: None,
            streamable_http: None,
            sse_transport: None,
        };
        assert!(args.stdio.is_none());
        assert!(args.streamable_http.is_none());
        assert!(args.sse_transport.is_none());
    }

    #[test]
    fn test_transport_args_with_stdio() {
        let stdio = AddMcpStdioArgs {
            command: vec!["npx".to_string(), "@example/server".to_string()],
            env: vec![],
        };
        let args = AddMcpTransportArgs {
            stdio: Some(stdio),
            streamable_http: None,
            sse_transport: None,
        };
        assert!(args.stdio.is_some());
        let stdio = args.stdio.unwrap();
        assert_eq!(stdio.command.len(), 2);
        assert_eq!(stdio.command[0], "npx");
    }

    #[test]
    fn test_transport_args_with_http() {
        let http = AddMcpStreamableHttpArgs {
            url: "https://api.example.com".to_string(),
            bearer_token_env_var: None,
        };
        let args = AddMcpTransportArgs {
            stdio: None,
            streamable_http: Some(http),
            sse_transport: None,
        };
        assert!(args.streamable_http.is_some());
        let http = args.streamable_http.unwrap();
        assert_eq!(http.url, "https://api.example.com");
    }

    #[test]
    fn test_transport_args_with_sse() {
        let sse = AddMcpSseArgs {
            sse_url: Some("https://sse.example.com".to_string()),
            sse_bearer_token_env_var: None,
        };
        let args = AddMcpTransportArgs {
            stdio: None,
            streamable_http: None,
            sse_transport: Some(sse),
        };
        assert!(args.sse_transport.is_some());
        let sse = args.sse_transport.unwrap();
        assert_eq!(sse.sse_url, Some("https://sse.example.com".to_string()));
    }

    // =========================================================================
    // AddMcpStdioArgs tests
    // =========================================================================

    #[test]
    fn test_stdio_args_empty_command() {
        let args = AddMcpStdioArgs {
            command: vec![],
            env: vec![],
        };
        assert!(args.command.is_empty());
        assert!(args.env.is_empty());
    }

    #[test]
    fn test_stdio_args_with_command() {
        let args = AddMcpStdioArgs {
            command: vec![
                "python".to_string(),
                "-m".to_string(),
                "my_server".to_string(),
            ],
            env: vec![],
        };
        assert_eq!(args.command.len(), 3);
        assert_eq!(args.command[0], "python");
        assert_eq!(args.command[1], "-m");
        assert_eq!(args.command[2], "my_server");
    }

    #[test]
    fn test_stdio_args_with_env_vars() {
        let args = AddMcpStdioArgs {
            command: vec!["server".to_string()],
            env: vec![
                ("API_KEY".to_string(), "secret123".to_string()),
                ("DEBUG".to_string(), "true".to_string()),
            ],
        };
        assert_eq!(args.env.len(), 2);
        assert!(
            args.env
                .contains(&("API_KEY".to_string(), "secret123".to_string()))
        );
        assert!(
            args.env
                .contains(&("DEBUG".to_string(), "true".to_string()))
        );
    }

    // =========================================================================
    // AddMcpStreamableHttpArgs tests
    // =========================================================================

    #[test]
    fn test_http_args_basic() {
        let args = AddMcpStreamableHttpArgs {
            url: "https://example.com/mcp".to_string(),
            bearer_token_env_var: None,
        };
        assert_eq!(args.url, "https://example.com/mcp");
        assert!(args.bearer_token_env_var.is_none());
    }

    #[test]
    fn test_http_args_with_bearer_token() {
        let args = AddMcpStreamableHttpArgs {
            url: "https://api.example.com".to_string(),
            bearer_token_env_var: Some("MY_TOKEN".to_string()),
        };
        assert_eq!(args.url, "https://api.example.com");
        assert_eq!(args.bearer_token_env_var, Some("MY_TOKEN".to_string()));
    }

    // =========================================================================
    // AddMcpSseArgs tests
    // =========================================================================

    #[test]
    fn test_sse_args_empty() {
        let args = AddMcpSseArgs {
            sse_url: None,
            sse_bearer_token_env_var: None,
        };
        assert!(args.sse_url.is_none());
        assert!(args.sse_bearer_token_env_var.is_none());
    }

    #[test]
    fn test_sse_args_with_url() {
        let args = AddMcpSseArgs {
            sse_url: Some("https://sse.example.com/events".to_string()),
            sse_bearer_token_env_var: None,
        };
        assert_eq!(
            args.sse_url,
            Some("https://sse.example.com/events".to_string())
        );
    }

    #[test]
    fn test_sse_args_with_bearer_token() {
        let args = AddMcpSseArgs {
            sse_url: Some("https://sse.example.com".to_string()),
            sse_bearer_token_env_var: Some("SSE_TOKEN".to_string()),
        };
        assert_eq!(args.sse_bearer_token_env_var, Some("SSE_TOKEN".to_string()));
    }

    // =========================================================================
    // RemoveArgs tests
    // =========================================================================

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

    // =========================================================================
    // EnableArgs tests
    // =========================================================================

    #[test]
    fn test_enable_args_construction() {
        let args = EnableArgs {
            name: "server-to-enable".to_string(),
        };
        assert_eq!(args.name, "server-to-enable");
    }

    #[test]
    fn test_enable_args_empty_name() {
        let args = EnableArgs {
            name: String::new(),
        };
        assert!(args.name.is_empty());
    }

    // =========================================================================
    // DisableArgs tests
    // =========================================================================

    #[test]
    fn test_disable_args_construction() {
        let args = DisableArgs {
            name: "server-to-disable".to_string(),
        };
        assert_eq!(args.name, "server-to-disable");
    }

    #[test]
    fn test_disable_args_empty_name() {
        let args = DisableArgs {
            name: String::new(),
        };
        assert!(args.name.is_empty());
    }

    // =========================================================================
    // RenameArgs tests
    // =========================================================================

    #[test]
    fn test_rename_args_construction() {
        let args = RenameArgs {
            old_name: "old-server".to_string(),
            new_name: "new-server".to_string(),
        };
        assert_eq!(args.old_name, "old-server");
        assert_eq!(args.new_name, "new-server");
    }

    #[test]
    fn test_rename_args_same_name() {
        let args = RenameArgs {
            old_name: "server".to_string(),
            new_name: "server".to_string(),
        };
        assert_eq!(args.old_name, args.new_name);
    }

    // =========================================================================
    // AuthCommand tests
    // =========================================================================

    #[test]
    fn test_auth_command_with_name_only() {
        let cmd = AuthCommand {
            action: None,
            name: Some("my-server".to_string()),
            client_id: None,
            client_secret: None,
        };
        assert!(cmd.action.is_none());
        assert_eq!(cmd.name, Some("my-server".to_string()));
        assert!(cmd.client_id.is_none());
        assert!(cmd.client_secret.is_none());
    }

    #[test]
    fn test_auth_command_with_credentials() {
        let cmd = AuthCommand {
            action: None,
            name: Some("server".to_string()),
            client_id: Some("my-client-id".to_string()),
            client_secret: Some("my-secret".to_string()),
        };
        assert_eq!(cmd.client_id, Some("my-client-id".to_string()));
        assert_eq!(cmd.client_secret, Some("my-secret".to_string()));
    }

    #[test]
    fn test_auth_command_with_list_action() {
        let cmd = AuthCommand {
            action: Some(AuthSubcommand::List(AuthListArgs { json: false })),
            name: None,
            client_id: None,
            client_secret: None,
        };
        assert!(matches!(cmd.action, Some(AuthSubcommand::List(_))));
    }

    // =========================================================================
    // AuthListArgs tests
    // =========================================================================

    #[test]
    fn test_auth_list_args_default() {
        let args = AuthListArgs { json: false };
        assert!(!args.json);
    }

    #[test]
    fn test_auth_list_args_json_enabled() {
        let args = AuthListArgs { json: true };
        assert!(args.json);
    }

    // =========================================================================
    // LogoutArgs tests
    // =========================================================================

    #[test]
    fn test_logout_args_with_name() {
        let args = LogoutArgs {
            name: Some("server".to_string()),
            all: false,
        };
        assert_eq!(args.name, Some("server".to_string()));
        assert!(!args.all);
    }

    #[test]
    fn test_logout_args_with_all() {
        let args = LogoutArgs {
            name: None,
            all: true,
        };
        assert!(args.name.is_none());
        assert!(args.all);
    }

    #[test]
    fn test_logout_args_both_name_and_all() {
        let args = LogoutArgs {
            name: Some("server".to_string()),
            all: true,
        };
        assert!(args.name.is_some());
        assert!(args.all);
    }

    // =========================================================================
    // DebugArgs tests
    // =========================================================================

    #[test]
    fn test_debug_args_defaults() {
        let args = DebugArgs {
            name: "my-server".to_string(),
            json: false,
            test_auth: false,
            timeout: 30,
            no_cache: false,
            show_cache_info: false,
        };
        assert_eq!(args.name, "my-server");
        assert!(!args.json);
        assert!(!args.test_auth);
        assert_eq!(args.timeout, 30, "Default timeout should be 30");
        assert!(!args.no_cache);
        assert!(!args.show_cache_info);
    }

    #[test]
    fn test_debug_args_with_json() {
        let args = DebugArgs {
            name: "server".to_string(),
            json: true,
            test_auth: false,
            timeout: 30,
            no_cache: false,
            show_cache_info: false,
        };
        assert!(args.json);
    }

    #[test]
    fn test_debug_args_with_test_auth() {
        let args = DebugArgs {
            name: "server".to_string(),
            json: false,
            test_auth: true,
            timeout: 30,
            no_cache: false,
            show_cache_info: false,
        };
        assert!(args.test_auth);
    }

    #[test]
    fn test_debug_args_with_custom_timeout() {
        let args = DebugArgs {
            name: "server".to_string(),
            json: false,
            test_auth: false,
            timeout: 120,
            no_cache: false,
            show_cache_info: false,
        };
        assert_eq!(args.timeout, 120);
    }

    #[test]
    fn test_debug_args_with_no_cache() {
        let args = DebugArgs {
            name: "server".to_string(),
            json: false,
            test_auth: false,
            timeout: 30,
            no_cache: true,
            show_cache_info: false,
        };
        assert!(args.no_cache);
    }

    #[test]
    fn test_debug_args_with_show_cache_info() {
        let args = DebugArgs {
            name: "server".to_string(),
            json: false,
            test_auth: false,
            timeout: 30,
            no_cache: false,
            show_cache_info: true,
        };
        assert!(args.show_cache_info);
    }

    #[test]
    fn test_debug_args_all_flags_enabled() {
        let args = DebugArgs {
            name: "server".to_string(),
            json: true,
            test_auth: true,
            timeout: 60,
            no_cache: true,
            show_cache_info: true,
        };
        assert!(args.json);
        assert!(args.test_auth);
        assert_eq!(args.timeout, 60);
        assert!(args.no_cache);
        assert!(args.show_cache_info);
    }

    // =========================================================================
    // McpSubcommand enum variant tests
    // =========================================================================

    #[test]
    fn test_subcommand_list_variant() {
        let subcommand = McpSubcommand::List(ListArgs {
            json: false,
            all: false,
        });
        assert!(matches!(subcommand, McpSubcommand::List(_)));
    }

    #[test]
    fn test_subcommand_ls_variant() {
        let subcommand = McpSubcommand::Ls(ListArgs {
            json: false,
            all: false,
        });
        assert!(matches!(subcommand, McpSubcommand::Ls(_)));
    }

    #[test]
    fn test_subcommand_get_variant() {
        let subcommand = McpSubcommand::Get(GetArgs {
            name: "server".to_string(),
            json: false,
        });
        assert!(matches!(subcommand, McpSubcommand::Get(_)));
    }

    #[test]
    fn test_subcommand_add_variant() {
        let subcommand = McpSubcommand::Add(AddArgs {
            name: "server".to_string(),
            force: false,
            allow_local: false,
            transport_args: AddMcpTransportArgs {
                stdio: None,
                streamable_http: None,
                sse_transport: None,
            },
        });
        assert!(matches!(subcommand, McpSubcommand::Add(_)));
    }

    #[test]
    fn test_subcommand_remove_variant() {
        let subcommand = McpSubcommand::Remove(RemoveArgs {
            name: "server".to_string(),
            yes: false,
        });
        assert!(matches!(subcommand, McpSubcommand::Remove(_)));
    }

    #[test]
    fn test_subcommand_enable_variant() {
        let subcommand = McpSubcommand::Enable(EnableArgs {
            name: "server".to_string(),
        });
        assert!(matches!(subcommand, McpSubcommand::Enable(_)));
    }

    #[test]
    fn test_subcommand_disable_variant() {
        let subcommand = McpSubcommand::Disable(DisableArgs {
            name: "server".to_string(),
        });
        assert!(matches!(subcommand, McpSubcommand::Disable(_)));
    }

    #[test]
    fn test_subcommand_rename_variant() {
        let subcommand = McpSubcommand::Rename(RenameArgs {
            old_name: "old".to_string(),
            new_name: "new".to_string(),
        });
        assert!(matches!(subcommand, McpSubcommand::Rename(_)));
    }

    #[test]
    fn test_subcommand_auth_variant() {
        let subcommand = McpSubcommand::Auth(AuthCommand {
            action: None,
            name: Some("server".to_string()),
            client_id: None,
            client_secret: None,
        });
        assert!(matches!(subcommand, McpSubcommand::Auth(_)));
    }

    #[test]
    fn test_subcommand_logout_variant() {
        let subcommand = McpSubcommand::Logout(LogoutArgs {
            name: Some("server".to_string()),
            all: false,
        });
        assert!(matches!(subcommand, McpSubcommand::Logout(_)));
    }

    #[test]
    fn test_subcommand_debug_variant() {
        let subcommand = McpSubcommand::Debug(DebugArgs {
            name: "server".to_string(),
            json: false,
            test_auth: false,
            timeout: 30,
            no_cache: false,
            show_cache_info: false,
        });
        assert!(matches!(subcommand, McpSubcommand::Debug(_)));
    }

    // =========================================================================
    // McpCli construction tests
    // =========================================================================

    #[test]
    fn test_mcp_cli_with_list_subcommand() {
        let cli = McpCli {
            config_overrides: CliConfigOverrides::default(),
            subcommand: McpSubcommand::List(ListArgs {
                json: false,
                all: false,
            }),
        };
        assert!(matches!(cli.subcommand, McpSubcommand::List(_)));
    }

    #[test]
    fn test_mcp_cli_with_get_subcommand() {
        let cli = McpCli {
            config_overrides: CliConfigOverrides::default(),
            subcommand: McpSubcommand::Get(GetArgs {
                name: "test-server".to_string(),
                json: true,
            }),
        };
        match cli.subcommand {
            McpSubcommand::Get(args) => {
                assert_eq!(args.name, "test-server");
                assert!(args.json);
            }
            _ => panic!("Expected Get subcommand"),
        }
    }
}
