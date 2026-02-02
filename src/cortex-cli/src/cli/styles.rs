//! CLI styling and formatting.
//!
//! Defines ANSI colors and formatting for the CLI help output.

use clap::builder::styling::{AnsiColor, Effects, Styles};

/// Cortex CLI styled help theme.
///
/// Uses a modern color scheme with cyan accents for a beautiful terminal experience.
pub fn get_styles() -> Styles {
    Styles::styled()
        // Headers (USAGE, COMMANDS, OPTIONS) - Bold cyan
        .header(AnsiColor::Cyan.on_default() | Effects::BOLD)
        // Usage line - Green
        .usage(AnsiColor::Green.on_default() | Effects::BOLD)
        // Literals (command names, flag names) - Bold green
        .literal(AnsiColor::Green.on_default() | Effects::BOLD)
        // Placeholders (<VALUE>, [ARGS]) - Yellow
        .placeholder(AnsiColor::Yellow.on_default())
        // Errors - Bold red
        .error(AnsiColor::Red.on_default() | Effects::BOLD)
        // Valid values - Cyan
        .valid(AnsiColor::Cyan.on_default())
        // Invalid values - Yellow
        .invalid(AnsiColor::Yellow.on_default())
}

/// After-help section with environment variables documentation.
pub const AFTER_HELP: &str = color_print::cstr!(
    r#"<cyan,bold>📚 QUICK START</>
    <green,bold>cortex</>                         Start interactive TUI
    <green,bold>cortex</> <dim>"fix the bug"</>          Start TUI with initial prompt
    <green,bold>cortex run</> <dim>"explain this"</>     Non-interactive single request
    <green,bold>cortex exec</> <dim>"run tests"</>       Headless execution for CI/CD
    <green,bold>cortex resume --last</>           Continue most recent session

<cyan,bold>🌍 ENVIRONMENT VARIABLES</>
    <yellow>CORTEX_HOME</>          Override config directory (default: ~/.config/cortex)
    <yellow>CORTEX_API_KEY</>       API key (alternative to --with-api-key)
    <yellow>CORTEX_MODEL</>         Default model (alternative to --model)
    <yellow>CORTEX_LOG_LEVEL</>     Log verbosity (error, warn, info, debug, trace)
    <yellow>NO_COLOR</>             Disable colored output (set to '1' or 'true')
    <yellow>VISUAL</>/<yellow>EDITOR</>        Editor for /edit command

<cyan,bold>📁 PATHS</>
    <dim>Config</>      ~/.config/cortex/config.toml
    <dim>Sessions</>    ~/.local/share/cortex/sessions/
    <dim>Logs</>        ~/.cache/cortex/logs/
    <dim>Agents</>      ~/.cortex/agents/ (personal), .cortex/agents/ (project)

<cyan,bold>🔗 LEARN MORE</>
    <blue,underline>https://docs.cortex.foundation</>       Documentation
    <blue,underline>https://github.com/CortexLM/cortex</>   Source & Issues"#
);

/// Before-help section with ASCII art banner.
pub const BEFORE_HELP: &str = color_print::cstr!(
    r#"<cyan,bold>  ╔═══════════════════════════════════════════════════╗
  ║   ██████╗ ██████╗ ██████╗ ████████╗███████╗██╗  ██╗║
  ║  ██╔════╝██╔═══██╗██╔══██╗╚══██╔══╝██╔════╝╚██╗██╔╝║
  ║  ██║     ██║   ██║██████╔╝   ██║   █████╗   ╚███╔╝ ║
  ║  ██║     ██║   ██║██╔══██╗   ██║   ██╔══╝   ██╔██╗ ║
  ║  ╚██████╗╚██████╔╝██║  ██║   ██║   ███████╗██╔╝ ██╗║
  ║   ╚═════╝ ╚═════╝ ╚═╝  ╚═╝   ╚═╝   ╚══════╝╚═╝  ╚═╝║
  ╚═══════════════════════════════════════════════════╝</>
<dim>                   AI-Powered Coding Agent</>"#
);

/// Command category display names for styled help output.
pub mod categories {
    pub const EXECUTION: &str = "🚀 Execution";
    pub const SESSION: &str = "📋 Session Management";
    pub const AUTH: &str = "🔐 Authentication";
    pub const EXTENSION: &str = "🔌 Extensibility";
    pub const CONFIG: &str = "⚙️ Configuration";
    pub const UTILITIES: &str = "🛠️ Utilities";
    pub const MAINTENANCE: &str = "🔧 Maintenance";
}

/// Custom help template with categorized commands.
///
/// Since clap doesn't natively support subcommand categories in help output,
/// we define a custom template that manually lists commands by category.
pub const HELP_TEMPLATE: &str = color_print::cstr!(
    r#"{before-help}
{about-with-newline}
{usage-heading} {usage}

<cyan,bold>🚀 Execution:</>
    <green,bold>run</>         Run Cortex non-interactively with advanced options <dim>[aliases: r]</>
    <green,bold>exec</>        Execute in headless mode (for CI/CD, scripts) <dim>[aliases: e]</>

<cyan,bold>📋 Session Management:</>
    <green,bold>resume</>      Resume a previous interactive session
    <green,bold>sessions</>    List previous sessions
    <green,bold>export</>      Export a session to JSON format
    <green,bold>import</>      Import a session from JSON file or URL
    <green,bold>delete</>      Delete a session

<cyan,bold>🔐 Authentication:</>
    <green,bold>login</>       Authenticate with Cortex API
    <green,bold>logout</>      Remove stored authentication credentials
    <green,bold>whoami</>      Show currently authenticated user

<cyan,bold>🔌 Extensibility:</>
    <green,bold>agent</>       Manage agents (list, create, show)
    <green,bold>mcp</>         Manage MCP (Model Context Protocol) servers
    <green,bold>acp</>         Start ACP server for IDE integration (e.g., Zed)

<cyan,bold>⚙️ Configuration:</>
    <green,bold>config</>      Show or edit configuration
    <green,bold>models</>      List available models
    <green,bold>features</>    Inspect feature flags
    <green,bold>init</>        Initialize AGENTS.md in the current directory

<cyan,bold>🛠️ Utilities:</>
    <green,bold>github</>      GitHub integration (actions, workflows) <dim>[aliases: gh]</>
    <green,bold>pr</>          Checkout a pull request
    <green,bold>scrape</>      Scrape web content to markdown/text/html
    <green,bold>stats</>       Show usage statistics
    <green,bold>completion</>  Generate shell completion scripts

<cyan,bold>🔧 Maintenance:</>
    <green,bold>upgrade</>     Check for and install updates
    <green,bold>uninstall</>   Uninstall Cortex CLI
    <green,bold>compact</>     Data compaction and cleanup <dim>[aliases: gc, cleanup]</>
    <green,bold>cache</>       Manage cache
    <green,bold>logs</>        View application logs
    <green,bold>feedback</>    Submit feedback and bug reports <dim>[aliases: report]</>
    <green,bold>lock</>        Lock/protect sessions from deletion <dim>[aliases: protect]</>
    <green,bold>alias</>       Manage command aliases <dim>[aliases: aliases]</>
    <green,bold>plugin</>      Manage plugins <dim>[aliases: plugins]</>

{options}{after-help}"#
);
