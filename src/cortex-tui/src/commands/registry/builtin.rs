//! Builtin command registration.
//!
//! This module contains all the built-in slash command definitions.

use crate::commands::types::{CommandCategory, CommandDef};

use super::core::CommandRegistry;

/// Registers all builtin commands in the registry.
pub fn register_builtin_commands(registry: &mut CommandRegistry) {
    // ========================================
    // GENERAL COMMANDS
    // ========================================

    registry.register(CommandDef::new(
        "help",
        &["h", "?"],
        "Show help information",
        "/help [topic]",
        CommandCategory::General,
        true,
    ));

    registry.register(CommandDef::new(
        "quit",
        &["q", "exit"],
        "Quit the application",
        "/quit",
        CommandCategory::General,
        false,
    ));

    registry.register(CommandDef::new(
        "version",
        &["v"],
        "Show version information",
        "/version",
        CommandCategory::General,
        false,
    ));

    registry.register(CommandDef::new(
        "upgrade",
        &["update"],
        "Check for and install updates",
        "/upgrade",
        CommandCategory::General,
        false,
    ));

    registry.register(CommandDef::new(
        "settings",
        &["config", "prefs"],
        "Open settings panel",
        "/settings",
        CommandCategory::General,
        false,
    ));

    // Config reload command (#2806)
    registry.register(CommandDef::new(
        "reload-config",
        &["reload"],
        "Reload configuration from disk",
        "/reload-config",
        CommandCategory::General,
        false,
    ));

    registry.register(CommandDef::new(
        "copy",
        &["cp"],
        "Show how to copy text",
        "/copy",
        CommandCategory::General,
        false,
    ));

    registry.register(CommandDef::new(
        "theme",
        &[],
        "Change color theme",
        "/theme [name]",
        CommandCategory::General,
        true,
    ));

    registry.register(CommandDef::new(
        "compact",
        &[],
        "Toggle compact display mode",
        "/compact",
        CommandCategory::General,
        false,
    ));

    registry.register(CommandDef::new(
        "palette",
        &["cmd"],
        "Open command palette",
        "/palette",
        CommandCategory::General,
        false,
    ));

    registry.register(CommandDef::new(
        "init",
        &[],
        "Initialize AGENTS.md in project directory",
        "/init [--force]",
        CommandCategory::General,
        true,
    ));

    registry.register(CommandDef::new(
        "commands",
        &["cmds"],
        "List all available custom commands",
        "/commands",
        CommandCategory::General,
        false,
    ));

    registry.register(CommandDef::new(
        "agents",
        &["subagents"],
        "List and manage custom agents",
        "/agents",
        CommandCategory::General,
        false,
    ));

    registry.register(CommandDef::new(
        "tasks",
        &["bg", "background"],
        "View and manage background tasks/agents",
        "/tasks",
        CommandCategory::General,
        false,
    ));

    registry.register(CommandDef::new(
        "skills",
        &["sk"],
        "List and manage skills",
        "/skills",
        CommandCategory::General,
        false,
    ));

    registry.register(CommandDef::new(
        "skill",
        &["invoke"],
        "Invoke a skill by name",
        "/skill <name> [args...]",
        CommandCategory::General,
        true,
    ));

    registry.register(CommandDef::new(
        "skill-reload",
        &["sr"],
        "Reload skills from disk",
        "/skill-reload",
        CommandCategory::General,
        false,
    ));

    registry.register(CommandDef::new(
        "share",
        &[],
        "Generate a share link for current session",
        "/share [duration]",
        CommandCategory::Session,
        true,
    ));

    // ========================================
    // AUTH COMMANDS
    // ========================================

    registry.register(CommandDef::new(
        "login",
        &["signin"],
        "Authenticate with Cortex",
        "/login",
        CommandCategory::Auth,
        false,
    ));

    registry.register(CommandDef::new(
        "logout",
        &["signout"],
        "Clear stored credentials",
        "/logout",
        CommandCategory::Auth,
        false,
    ));

    registry.register(CommandDef::new(
        "account",
        &["whoami", "me"],
        "Show account information",
        "/account",
        CommandCategory::Auth,
        false,
    ));

    // ========================================
    // BILLING COMMANDS
    // ========================================

    registry.register(CommandDef::new(
        "billing",
        &["plan", "subscription"],
        "Show billing status and credits",
        "/billing",
        CommandCategory::Billing,
        false,
    ));

    registry.register(CommandDef::new(
        "usage",
        &["stats", "credits"],
        "Show detailed usage breakdown",
        "/usage [--from YYYY-MM-DD] [--to YYYY-MM-DD]",
        CommandCategory::Billing,
        true,
    ));

    registry.register(CommandDef::new(
        "refresh",
        &["retry"],
        "Refresh billing status after adding payment method",
        "/refresh",
        CommandCategory::Billing,
        false,
    ));

    // ========================================
    // SESSION COMMANDS
    // ========================================

    registry.register(CommandDef::new(
        "session",
        &["info"],
        "Show current session info",
        "/session",
        CommandCategory::Session,
        false,
    ));

    registry.register(CommandDef::new(
        "clear",
        &["cls"],
        "Clear current conversation",
        "/clear",
        CommandCategory::Session,
        false,
    ));

    registry.register(CommandDef::new(
        "new",
        &["n"],
        "Start a new session",
        "/new",
        CommandCategory::Session,
        false,
    ));

    registry.register(CommandDef::new(
        "resume",
        &["r", "load"],
        "Resume a previous session",
        "/resume [session-id]",
        CommandCategory::Session,
        true,
    ));

    registry.register(CommandDef::new(
        "sessions",
        &["list", "ls-sessions"],
        "List all sessions",
        "/sessions",
        CommandCategory::Session,
        false,
    ));

    registry.register(CommandDef::new(
        "fork",
        &["branch"],
        "Fork current session",
        "/fork [name]",
        CommandCategory::Session,
        true,
    ));

    registry.register(CommandDef::new(
        "rename",
        &["mv"],
        "Rename current session",
        "/rename <name>",
        CommandCategory::Session,
        true,
    ));

    registry.register(CommandDef::new(
        "favorite",
        &["fav", "star"],
        "Mark session as favorite",
        "/favorite",
        CommandCategory::Session,
        false,
    ));

    registry.register(CommandDef::new(
        "unfavorite",
        &["unfav", "unstar"],
        "Remove favorite mark",
        "/unfavorite",
        CommandCategory::Session,
        false,
    ));

    registry.register(CommandDef::new(
        "export",
        &["save"],
        "Export session to file",
        "/export [format]",
        CommandCategory::Session,
        true,
    ));

    registry.register(CommandDef::new(
        "timeline",
        &["tl"],
        "View session timeline",
        "/timeline",
        CommandCategory::Session,
        false,
    ));

    registry.register(CommandDef::new(
        "rewind",
        &["rw"],
        "Rewind to a previous point",
        "/rewind [steps]",
        CommandCategory::Session,
        true,
    ));

    registry.register(CommandDef::new(
        "undo",
        &["u"],
        "Undo last action",
        "/undo",
        CommandCategory::Session,
        false,
    ));

    registry.register(CommandDef::new(
        "redo",
        &[],
        "Redo last undone action",
        "/redo",
        CommandCategory::Session,
        false,
    ));

    registry.register(CommandDef::new(
        "delete",
        &["rm"],
        "Delete a session",
        "/delete [session-id]",
        CommandCategory::Session,
        true,
    ));

    // ========================================
    // NAVIGATION COMMANDS
    // ========================================

    registry.register(CommandDef::new(
        "diff",
        &["d"],
        "Show file diff",
        "/diff [file]",
        CommandCategory::Navigation,
        true,
    ));

    registry.register(CommandDef::new(
        "transcript",
        &["tr"],
        "View session transcript",
        "/transcript",
        CommandCategory::Navigation,
        false,
    ));

    registry.register(CommandDef::new(
        "history",
        &["hist"],
        "View command history",
        "/history",
        CommandCategory::Navigation,
        false,
    ));

    registry.register(CommandDef::new(
        "scroll",
        &[],
        "Scroll to position",
        "/scroll <top|bottom|n>",
        CommandCategory::Navigation,
        true,
    ));

    registry.register(CommandDef::new(
        "goto",
        &["g"],
        "Go to message number",
        "/goto <n>",
        CommandCategory::Navigation,
        true,
    ));

    // ========================================
    // FILE COMMANDS
    // ========================================

    registry.register(CommandDef::new(
        "add",
        &["a", "include"],
        "Add file to context",
        "/add <file>...",
        CommandCategory::Files,
        true,
    ));

    registry.register(CommandDef::new(
        "remove",
        &["rm-file", "exclude"],
        "Remove file from context",
        "/remove <file>...",
        CommandCategory::Files,
        true,
    ));

    registry.register(CommandDef::new(
        "search",
        &["find", "grep"],
        "Search in files",
        "/search <pattern>",
        CommandCategory::Files,
        true,
    ));

    registry.register(CommandDef::new(
        "ls",
        &["dir", "files"],
        "List files in directory",
        "/ls [path]",
        CommandCategory::Files,
        true,
    ));

    registry.register(CommandDef::new(
        "mention",
        &["@", "ref"],
        "Mention a file or symbol",
        "/mention <file|symbol>",
        CommandCategory::Files,
        true,
    ));

    registry.register(CommandDef::new(
        "images",
        &["img", "pics"],
        "Add images to context",
        "/images <file>...",
        CommandCategory::Files,
        true,
    ));

    registry.register(CommandDef::new(
        "tree",
        &[],
        "Show directory tree",
        "/tree [path]",
        CommandCategory::Files,
        true,
    ));

    registry.register(CommandDef::new(
        "context",
        &["ctx"],
        "Show current context files",
        "/context",
        CommandCategory::Files,
        false,
    ));

    // ========================================
    // MODEL COMMANDS
    // ========================================

    registry.register(CommandDef::new(
        "model",
        &["m"],
        "Switch to a model",
        "/model <name>",
        CommandCategory::Model,
        true,
    ));

    registry.register(CommandDef::new(
        "models",
        &["lm", "list-models"],
        "List available models",
        "/models",
        CommandCategory::Model,
        false,
    ));

    registry.register(CommandDef::new(
        "approval",
        &["approve"],
        "Set tool approval mode",
        "/approval <ask|session|always>",
        CommandCategory::Model,
        true,
    ));

    registry.register(CommandDef::new(
        "sandbox",
        &["sb"],
        "Toggle sandbox mode",
        "/sandbox [on|off]",
        CommandCategory::Model,
        true,
    ));

    registry.register(CommandDef::new(
        "auto",
        &["autopilot"],
        "Toggle auto-approve mode",
        "/auto [on|off]",
        CommandCategory::Model,
        true,
    ));

    registry.register(CommandDef::new(
        "temperature",
        &["temp"],
        "Set temperature",
        "/temperature <0.0-2.0>",
        CommandCategory::Model,
        true,
    ));

    registry.register(CommandDef::new(
        "tokens",
        &["max-tokens"],
        "Set max tokens",
        "/tokens <n>",
        CommandCategory::Model,
        true,
    ));

    // ========================================
    // MCP COMMANDS
    // ========================================

    registry.register(CommandDef::new(
        "mcp",
        &[],
        "MCP server management (interactive)",
        "/mcp",
        CommandCategory::Mcp,
        false,
    ));

    registry.register(CommandDef::new(
        "mcp-tools",
        &["tools", "lt"],
        "List and manage MCP tools",
        "/mcp-tools",
        CommandCategory::Mcp,
        false,
    ));

    registry.register(CommandDef::new(
        "mcp-auth",
        &["auth"],
        "MCP authentication management",
        "/mcp-auth",
        CommandCategory::Mcp,
        false,
    ));

    registry.register(CommandDef::new(
        "mcp-reload",
        &[],
        "Reload MCP server configuration",
        "/mcp-reload",
        CommandCategory::Mcp,
        false,
    ));

    // ========================================
    // DEBUG COMMANDS
    // ========================================

    registry.register(CommandDef::new(
        "debug",
        &["dbg"],
        "Toggle debug mode",
        "/debug [on|off]",
        CommandCategory::Debug,
        true,
    ));

    registry.register(CommandDef::new(
        "status",
        &["stat"],
        "Show application status",
        "/status",
        CommandCategory::Debug,
        false,
    ));

    registry.register(CommandDef::new(
        "config",
        &["cfg"],
        "Show configuration",
        "/config [key]",
        CommandCategory::Debug,
        true,
    ));

    registry.register(CommandDef::new(
        "logs",
        &["log"],
        "View application logs",
        "/logs [level]",
        CommandCategory::Debug,
        true,
    ));

    registry.register(CommandDef::new(
        "dump",
        &[],
        "Dump session state",
        "/dump [file]",
        CommandCategory::Debug,
        true,
    ));

    registry.register(CommandDef::new(
        "metrics",
        &["perf"],
        "Show performance metrics",
        "/metrics",
        CommandCategory::Debug,
        false,
    ));

    // ========================================
    // DEVELOPMENT & TOOLS COMMANDS
    // ========================================

    registry.register(CommandDef::new(
        "plugins",
        &["plugin"],
        "Manage plugins (list, install, enable, disable, info, reload)",
        "/plugins [action] [plugin-id]",
        CommandCategory::General,
        true,
    ));

    registry.register(CommandDef::new(
        "cost",
        &[],
        "Show token usage and cost",
        "/cost",
        CommandCategory::General,
        false,
    ));

    registry.register(CommandDef::new(
        "bug",
        &[],
        "Report a bug",
        "/bug [description]",
        CommandCategory::General,
        true,
    ));

    registry.register(CommandDef::new(
        "delegates",
        &[],
        "Manage custom delegates (subagents)",
        "/delegates [action]",
        CommandCategory::General,
        true,
    ));

    registry.register(CommandDef::new(
        "spec",
        &[],
        "Toggle specification mode",
        "/spec [off]",
        CommandCategory::General,
        true,
    ));

    registry.register(CommandDef::new(
        "bg-process",
        &[],
        "Manage background processes",
        "/bg-process [action] [target]",
        CommandCategory::General,
        true,
    ));

    registry.register(CommandDef::new(
        "ide",
        &[],
        "Manage IDE integration (VS Code, Cursor)",
        "/ide",
        CommandCategory::General,
        false,
    ));

    registry.register(CommandDef::new(
        "install-github-app",
        &[],
        "Install the Cortex GitHub App",
        "/install-github-app",
        CommandCategory::General,
        false,
    ));

    registry.register(CommandDef::new(
        "review",
        &[],
        "Review code changes",
        "/review [target] [--base=branch]",
        CommandCategory::General,
        true,
    ));

    registry.register(CommandDef::new(
        "experimental",
        &["exp", "features"],
        "Manage experimental features",
        "/experimental [feature] [--enable|--disable]",
        CommandCategory::General,
        true,
    ));

    registry.register(CommandDef::new(
        "ratelimits",
        &["limits", "quota"],
        "Show API rate limits and usage",
        "/ratelimits",
        CommandCategory::General,
        false,
    ));

    registry.register(CommandDef::new(
        "ghost",
        &[],
        "Manage ghost commits for undo",
        "/ghost [action]",
        CommandCategory::General,
        true,
    ));

    registry.register(CommandDef::new(
        "multiedit",
        &["sed", "replace"],
        "Search and replace across multiple files",
        "/multiedit <pattern> <replacement> [--glob=pattern]",
        CommandCategory::Files,
        true,
    ));

    registry.register(CommandDef::new(
        "diagnostics",
        &["diag", "lint"],
        "Show LSP diagnostics for a file",
        "/diagnostics [file]",
        CommandCategory::Debug,
        true,
    ));

    registry.register(CommandDef::new(
        "hooks",
        &[],
        "Manage file hooks (formatters, linters)",
        "/hooks [action]",
        CommandCategory::General,
        true,
    ));

    registry.register(CommandDef::new(
        "custom-commands",
        &["cc"],
        "Manage custom commands",
        "/custom-commands [action]",
        CommandCategory::General,
        true,
    ));

    // ========================================
    // HIDDEN COMMANDS (for testing/debugging)
    // ========================================

    registry.register(CommandDef::hidden(
        "crash",
        &[],
        "Intentionally crash the application (debug only)",
        "/crash",
        CommandCategory::Debug,
        false,
    ));
}
