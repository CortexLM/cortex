//! Command dispatch logic for the executor.

use super::CommandExecutor;
use crate::commands::types::{CommandResult, ModalType, ParsedCommand};

impl CommandExecutor {
    /// Dispatch a parsed command to its handler.
    pub(super) fn dispatch(&self, cmd: &ParsedCommand) -> CommandResult {
        match cmd.name.as_str() {
            // ============ GENERAL ============
            "help" | "h" | "?" => self.cmd_help(cmd),
            "quit" | "q" | "exit" => CommandResult::Quit,
            "version" | "v" => self.cmd_version(),
            "upgrade" | "update" => CommandResult::OpenModal(ModalType::Upgrade),
            "settings" | "config" | "prefs" => CommandResult::OpenModal(ModalType::Settings),
            "reload-config" | "reload" => CommandResult::Async("config:reload".to_string()),
            "theme" => self.cmd_theme(cmd),
            "compact" => CommandResult::Toggle("compact".to_string()),
            "palette" | "cmd" => CommandResult::OpenModal(ModalType::CommandPalette),
            "init" => self.cmd_init(cmd),
            "commands" | "cmds" => CommandResult::Async("commands:list".to_string()),
            "agents" | "subagents" => CommandResult::OpenModal(ModalType::Agents),
            "tasks" | "bg" | "background" => CommandResult::OpenModal(ModalType::Tasks),
            "skills" | "sk" => CommandResult::OpenModal(ModalType::Skills),
            "skill" | "invoke" => self.cmd_skill(cmd),
            "skill-reload" | "sr" => CommandResult::Async("skills:reload".to_string()),
            "copy" | "cp" => CommandResult::Message(
                "To copy text from Cortex:\n\n\
                 - Hold SHIFT while selecting text with mouse\n\
                 - Then use your terminal's copy (Ctrl+Shift+C or right-click)\n\n\
                 This bypasses mouse capture and uses native selection."
                    .to_string(),
            ),

            // ============ AUTH ============
            "login" | "signin" => CommandResult::OpenModal(ModalType::Login),
            "logout" | "signout" => CommandResult::Async("auth:logout".to_string()),
            "account" | "whoami" | "me" => CommandResult::Async("auth:account".to_string()),

            // ============ BILLING ============
            "billing" | "plan" | "subscription" => {
                CommandResult::Async("billing:status".to_string())
            }
            "usage" | "stats" | "credits" => self.cmd_usage(cmd),
            "refresh" | "retry" => CommandResult::Async("billing:refresh".to_string()),

            // ============ SESSION ============
            "session" | "info" => CommandResult::Async("session:info".to_string()),
            "clear" | "cls" => CommandResult::Clear,
            "new" | "n" => CommandResult::NewSession,
            "resume" | "r" | "load" => self.cmd_resume(cmd),
            "sessions" | "list" | "ls-sessions" => {
                CommandResult::Async("sessions:list".to_string())
            }
            "fork" | "branch" => self.cmd_fork(cmd),
            "rename" | "mv" => self.cmd_rename(cmd),
            "favorite" | "fav" | "star" => CommandResult::Toggle("favorite".to_string()),
            "unfavorite" | "unfav" | "unstar" => {
                CommandResult::SetValue("favorite".to_string(), "false".to_string())
            }
            "export" | "save" => self.cmd_export(cmd),
            "share" => self.cmd_share(cmd),
            "timeline" | "tl" => CommandResult::OpenModal(ModalType::Timeline),
            "undo" | "u" => CommandResult::Async("undo".to_string()),
            "redo" => CommandResult::Async("redo".to_string()),
            "rewind" | "rw" => self.cmd_rewind(cmd),
            "delete" | "rm" => self.cmd_delete(cmd),

            // ============ NAVIGATION ============
            "diff" | "d" => self.cmd_diff(cmd),
            "transcript" | "tr" => CommandResult::Async("transcript".to_string()),
            "history" | "hist" => CommandResult::Async("history".to_string()),
            "scroll" => self.cmd_scroll(cmd),
            "goto" | "g" => self.cmd_goto(cmd),

            // ============ FILES ============
            "add" | "a" | "include" => self.cmd_add(cmd),
            "remove" | "rm-file" | "exclude" => self.cmd_remove(cmd),
            "search" | "find" | "grep" => self.cmd_search(cmd),
            "ls" | "dir" | "files" => self.cmd_ls(cmd),
            "mention" | "@" | "ref" => self.cmd_mention(cmd),
            "images" | "img" | "pics" => self.cmd_images(cmd),
            "tree" => self.cmd_tree(cmd),
            "context" | "ctx" => CommandResult::Async("context".to_string()),

            // ============ MODEL ============
            "model" | "m" => self.cmd_model(cmd),
            "models" | "lm" | "list-models" => {
                CommandResult::Async("models:fetch-and-pick".to_string())
            }
            "approval" | "approve" => self.cmd_approval(cmd),
            "sandbox" | "sb" => self.cmd_sandbox(cmd),
            "auto" | "autopilot" => self.cmd_auto(cmd),
            "provider" | "prov" => self.cmd_provider(cmd),
            "providers" | "lp" | "list-providers" => self.cmd_providers(),
            "temperature" | "temp" => self.cmd_temperature(cmd),
            "tokens" | "max-tokens" => self.cmd_tokens(cmd),

            // ============ MCP ============
            // All MCP commands redirect to the interactive panel
            "mcp" => self.cmd_mcp(cmd),
            "mcp-tools" | "tools" | "lt" => CommandResult::OpenModal(ModalType::McpManager),
            "mcp-auth" | "auth" => CommandResult::OpenModal(ModalType::McpManager),
            "mcp-reload" => CommandResult::OpenModal(ModalType::McpManager),
            "mcp-logs" => CommandResult::OpenModal(ModalType::McpManager),

            // ============ DEBUG ============
            "debug" | "dbg" => self.cmd_debug(cmd),
            "status" | "stat" => CommandResult::Async("status".to_string()),
            "cfg" => self.cmd_cfg(cmd),
            "logs" | "log" => self.cmd_logs(cmd),
            "dump" => self.cmd_dump(cmd),
            "metrics" | "perf" => CommandResult::Async("metrics".to_string()),

            // Hidden commands
            "crash" => CommandResult::Error("Crash test triggered".to_string()),
            "eval" => self.cmd_eval(cmd),

            // ============ DEVELOPMENT & TOOLS ============
            "plugins" | "plugin" => self.cmd_plugins(cmd),
            "cost" => CommandResult::Async("cost".to_string()),
            "bug" => self.cmd_bug(cmd),
            "delegates" => self.cmd_delegates(cmd),
            "spec" => self.cmd_spec(cmd),
            "bg-process" => self.cmd_bg_process(cmd),
            "ide" => CommandResult::Async("ide:status".to_string()),
            "install-github-app" => CommandResult::Async("github:install-app".to_string()),
            "review" => self.cmd_review(cmd),
            "experimental" | "exp" | "features" => self.cmd_experimental(cmd),
            "ratelimits" | "limits" | "quota" => CommandResult::Async("ratelimits".to_string()),
            "ghost" => self.cmd_ghost(cmd),
            "multiedit" | "sed" | "replace" => self.cmd_multiedit(cmd),
            "diagnostics" | "diag" | "lint" => self.cmd_diagnostics(cmd),
            "hooks" => self.cmd_hooks(cmd),
            "custom-commands" | "cc" => self.cmd_custom_commands(cmd),

            // Unknown (shouldn't reach here if registry check passed)
            _ => CommandResult::NotFound(format!("No handler for: /{}", cmd.name)),
        }
    }
}
