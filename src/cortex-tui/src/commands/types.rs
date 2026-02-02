//! Command system types for cortex-tui slash commands.
//!
//! This module defines the core types used throughout the command system,
//! including command categories, execution results, modal types, and
//! command definitions.

use std::fmt;

// ============================================================
// COMMAND CATEGORY
// ============================================================

/// Command category for grouping in help and command palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CommandCategory {
    /// General commands (help, quit, version, etc.)
    General,
    /// Authentication commands (login, logout, account)
    Auth,
    /// Billing and usage commands (billing, usage)
    Billing,
    /// Session management commands (new, clear, fork, etc.)
    Session,
    /// Navigation commands (diff, transcript, history)
    Navigation,
    /// File-related commands (add, search, ls, etc.)
    Files,
    /// Model and provider commands (model, models, etc.)
    Model,
    /// MCP (Model Context Protocol) commands
    Mcp,
    /// Debug and diagnostics commands
    Debug,
}

impl CommandCategory {
    /// Returns the human-readable name for this category.
    pub fn name(&self) -> &'static str {
        match self {
            CommandCategory::General => "General",
            CommandCategory::Auth => "Auth",
            CommandCategory::Billing => "Billing",
            CommandCategory::Session => "Session",
            CommandCategory::Navigation => "Navigation",
            CommandCategory::Files => "Files",
            CommandCategory::Model => "Model",
            CommandCategory::Mcp => "MCP",
            CommandCategory::Debug => "Debug",
        }
    }

    /// Returns a short description for this category.
    pub fn description(&self) -> &'static str {
        match self {
            CommandCategory::General => "General application commands",
            CommandCategory::Auth => "Authentication and account management",
            CommandCategory::Billing => "Billing, usage, and subscription information",
            CommandCategory::Session => "Session and conversation management",
            CommandCategory::Navigation => "Navigate through content and history",
            CommandCategory::Files => "File and context management",
            CommandCategory::Model => "Model and provider configuration",
            CommandCategory::Mcp => "MCP server and tool management",
            CommandCategory::Debug => "Debugging and diagnostics",
        }
    }

    /// Returns all available categories in display order.
    pub fn all() -> &'static [CommandCategory] {
        &[
            CommandCategory::General,
            CommandCategory::Auth,
            CommandCategory::Billing,
            CommandCategory::Session,
            CommandCategory::Navigation,
            CommandCategory::Files,
            CommandCategory::Model,
            CommandCategory::Mcp,
            CommandCategory::Debug,
        ]
    }
}

impl fmt::Display for CommandCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name())
    }
}

// ============================================================
// VIEW TYPE
// ============================================================

/// View types that commands can switch to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewType {
    /// Main session view
    Session,
    /// Settings view
    Settings,
    /// Help view
    Help,
}

impl ViewType {
    /// Returns the name of this view.
    pub fn name(&self) -> &'static str {
        match self {
            ViewType::Session => "Session",
            ViewType::Settings => "Settings",
            ViewType::Help => "Help",
        }
    }
}

// ============================================================
// MODAL TYPE
// ============================================================

/// Modal types that commands can open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModalType {
    /// Help modal with optional topic
    Help(Option<String>),
    /// Settings modal
    Settings,
    /// Session list/picker modal
    Sessions,
    /// Command palette modal
    CommandPalette,
    /// Fork session modal
    Fork,
    /// Export session modal with optional format
    Export(Option<String>),
    /// Timeline/history browser modal
    Timeline,
    /// Theme picker modal
    ThemePicker,
    // ProviderPicker removed: provider is now always "cortex"
    /// Model picker modal
    ModelPicker,
    /// File picker modal
    FilePicker,
    /// Confirmation dialog with message
    Confirm(String),
    /// Form modal with command name (e.g., "rename", "temperature")
    Form(String),
    /// Approval mode picker modal
    ApprovalPicker,
    /// Log level picker modal
    LogLevelPicker,
    /// MCP Server manager modal
    McpManager,
    /// Login modal for device code authentication
    Login,
    /// Upgrade modal for self-update
    Upgrade,
    /// Agents manager modal for listing and creating agents
    Agents,
    /// Background tasks manager modal
    Tasks,
    /// Skills manager modal for listing and invoking skills
    Skills,
}

impl ModalType {
    /// Returns the title for this modal.
    pub fn title(&self) -> &'static str {
        match self {
            ModalType::Help(_) => "Help",
            ModalType::Settings => "Settings",
            ModalType::Sessions => "Sessions",
            ModalType::CommandPalette => "Command Palette",
            ModalType::Fork => "Fork Session",
            ModalType::Export(_) => "Export",
            ModalType::Timeline => "Timeline",
            ModalType::ThemePicker => "Theme",
            // ProviderPicker removed: provider is now always "cortex"
            ModalType::ModelPicker => "Model",
            ModalType::FilePicker => "Files",
            ModalType::Confirm(_) => "Confirm",
            ModalType::Form(_) => "Form",
            ModalType::ApprovalPicker => "Approval Mode",
            ModalType::LogLevelPicker => "Log Level",
            ModalType::McpManager => "MCP Servers",
            ModalType::Login => "Login",
            ModalType::Upgrade => "Upgrade",
            ModalType::Agents => "Agents",
            ModalType::Tasks => "Background Tasks",
            ModalType::Skills => "Skills",
        }
    }
}

// ============================================================
// COMMAND RESULT
// ============================================================

/// Result of command execution.
#[derive(Debug, Clone)]
pub enum CommandResult {
    /// Command executed successfully with no output.
    Success,
    /// Command executed successfully with a message to display.
    Message(String),
    /// Command failed with an error message.
    Error(String),
    /// Command requests application quit.
    Quit,
    /// Command requests clearing the current view/session.
    Clear,
    /// Command requests opening a modal.
    OpenModal(ModalType),
    /// Command requests switching to a view.
    SwitchView(ViewType),
    /// Command requests starting a new session.
    NewSession,
    /// Command requests resuming a session by ID.
    ResumeSession(String),
    /// Command requests toggling a feature.
    Toggle(String),
    /// Command requests setting a value.
    SetValue(String, String),
    /// Command requires async execution (returns future identifier).
    Async(String),
    /// Command was not found.
    NotFound(String),
    /// Command needs more arguments.
    NeedsArgs(String),
}

impl CommandResult {
    /// Returns true if this result indicates success.
    pub fn is_success(&self) -> bool {
        matches!(
            self,
            CommandResult::Success
                | CommandResult::Message(_)
                | CommandResult::Clear
                | CommandResult::OpenModal(_)
                | CommandResult::SwitchView(_)
                | CommandResult::NewSession
                | CommandResult::ResumeSession(_)
                | CommandResult::Toggle(_)
                | CommandResult::SetValue(_, _)
                | CommandResult::Async(_)
        )
    }

    /// Returns true if this result indicates an error.
    pub fn is_error(&self) -> bool {
        matches!(
            self,
            CommandResult::Error(_) | CommandResult::NotFound(_) | CommandResult::NeedsArgs(_)
        )
    }

    /// Returns true if this result requests quit.
    pub fn is_quit(&self) -> bool {
        matches!(self, CommandResult::Quit)
    }

    /// Returns the error message if this is an error result.
    pub fn error_message(&self) -> Option<&str> {
        match self {
            CommandResult::Error(msg) => Some(msg),
            CommandResult::NotFound(cmd) => Some(cmd),
            CommandResult::NeedsArgs(msg) => Some(msg),
            _ => None,
        }
    }
}

// ============================================================
// PARSED COMMAND
// ============================================================

/// A parsed command with name and arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedCommand {
    /// The command name (without the leading /).
    pub name: String,
    /// The command arguments.
    pub args: Vec<String>,
    /// The original raw input string.
    pub raw: String,
}

impl ParsedCommand {
    /// Creates a new parsed command.
    pub fn new(name: String, args: Vec<String>, raw: String) -> Self {
        Self { name, args, raw }
    }

    /// Returns true if the command has any arguments.
    pub fn has_args(&self) -> bool {
        !self.args.is_empty()
    }

    /// Returns the first argument, if any.
    pub fn first_arg(&self) -> Option<&str> {
        self.args.first().map(|s| s.as_str())
    }

    /// Returns all arguments joined by space.
    pub fn args_string(&self) -> String {
        self.args.join(" ")
    }

    /// Returns all arguments joined by space, with proper quoting for args containing spaces.
    ///
    /// This is useful for passing arguments through an intermediate string representation
    /// while preserving the ability to correctly parse them back using `CommandParser::split_args`.
    pub fn args_string_quoted(&self) -> String {
        self.args
            .iter()
            .map(|arg| {
                if arg.contains(' ') || arg.contains('"') || arg.contains('\'') {
                    // Escape existing double quotes and wrap in double quotes
                    format!("\"{}\"", arg.replace('\\', "\\\\").replace('"', "\\\""))
                } else {
                    arg.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Returns the argument at the given index.
    pub fn arg(&self, index: usize) -> Option<&str> {
        self.args.get(index).map(|s| s.as_str())
    }

    /// Returns the number of arguments.
    pub fn arg_count(&self) -> usize {
        self.args.len()
    }
}

impl fmt::Display for ParsedCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "/{}", self.name)?;
        for arg in &self.args {
            if arg.contains(' ') {
                write!(f, " \"{}\"", arg)?;
            } else {
                write!(f, " {}", arg)?;
            }
        }
        Ok(())
    }
}

// ============================================================
// COMMAND DEFINITION
// ============================================================

/// Definition of a slash command.
#[derive(Debug, Clone)]
pub struct CommandDef {
    /// The primary command name (without /).
    pub name: &'static str,
    /// Alternative names/aliases for the command.
    pub aliases: &'static [&'static str],
    /// Short description shown in help.
    pub description: &'static str,
    /// Usage example (e.g., "/help [topic]").
    pub usage: &'static str,
    /// Category for grouping in help/palette.
    pub category: CommandCategory,
    /// Whether the command accepts arguments.
    pub has_args: bool,
    /// Whether the command is hidden from help/completion.
    pub hidden: bool,
}

impl CommandDef {
    /// Creates a new command definition.
    pub const fn new(
        name: &'static str,
        aliases: &'static [&'static str],
        description: &'static str,
        usage: &'static str,
        category: CommandCategory,
        has_args: bool,
    ) -> Self {
        Self {
            name,
            aliases,
            description,
            usage,
            category,
            has_args,
            hidden: false,
        }
    }

    /// Creates a hidden command definition.
    pub const fn hidden(
        name: &'static str,
        aliases: &'static [&'static str],
        description: &'static str,
        usage: &'static str,
        category: CommandCategory,
        has_args: bool,
    ) -> Self {
        Self {
            name,
            aliases,
            description,
            usage,
            category,
            has_args,
            hidden: true,
        }
    }

    /// Returns all names for this command (primary + aliases).
    pub fn all_names(&self) -> impl Iterator<Item = &'static str> {
        std::iter::once(self.name).chain(self.aliases.iter().copied())
    }

    /// Returns true if the given name matches this command.
    pub fn matches(&self, name: &str) -> bool {
        self.name.eq_ignore_ascii_case(name)
            || self.aliases.iter().any(|a| a.eq_ignore_ascii_case(name))
    }
}

impl fmt::Display for CommandDef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "/{}", self.name)
    }
}

// ============================================================
// TESTS
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_category_all() {
        let categories = CommandCategory::all();
        assert_eq!(categories.len(), 9);
        assert_eq!(categories[0], CommandCategory::General);
        assert!(categories.contains(&CommandCategory::Billing));
    }

    #[test]
    fn test_command_category_name() {
        assert_eq!(CommandCategory::General.name(), "General");
        assert_eq!(CommandCategory::Mcp.name(), "MCP");
    }

    #[test]
    fn test_parsed_command() {
        let cmd = ParsedCommand::new(
            "help".to_string(),
            vec!["topic".to_string()],
            "/help topic".to_string(),
        );
        assert_eq!(cmd.name, "help");
        assert!(cmd.has_args());
        assert_eq!(cmd.first_arg(), Some("topic"));
        assert_eq!(cmd.arg_count(), 1);
    }

    #[test]
    fn test_parsed_command_display() {
        let cmd = ParsedCommand::new(
            "search".to_string(),
            vec!["hello world".to_string()],
            "/search \"hello world\"".to_string(),
        );
        assert_eq!(cmd.to_string(), "/search \"hello world\"");
    }

    #[test]
    fn test_command_result_is_success() {
        assert!(CommandResult::Success.is_success());
        assert!(CommandResult::Message("ok".to_string()).is_success());
        assert!(!CommandResult::Error("fail".to_string()).is_success());
    }

    #[test]
    fn test_command_result_is_error() {
        assert!(CommandResult::Error("fail".to_string()).is_error());
        assert!(CommandResult::NotFound("foo".to_string()).is_error());
        assert!(!CommandResult::Success.is_error());
    }

    #[test]
    fn test_command_def_matches() {
        let def = CommandDef::new(
            "help",
            &["h", "?"],
            "Show help",
            "/help [topic]",
            CommandCategory::General,
            true,
        );
        assert!(def.matches("help"));
        assert!(def.matches("HELP"));
        assert!(def.matches("h"));
        assert!(def.matches("?"));
        assert!(!def.matches("foo"));
    }

    #[test]
    fn test_command_def_all_names() {
        let def = CommandDef::new(
            "quit",
            &["q", "exit"],
            "Quit",
            "/quit",
            CommandCategory::General,
            false,
        );
        let names: Vec<_> = def.all_names().collect();
        assert_eq!(names, vec!["quit", "q", "exit"]);
    }

    #[test]
    fn test_modal_type_title() {
        assert_eq!(ModalType::Help(None).title(), "Help");
        assert_eq!(ModalType::Settings.title(), "Settings");
        assert_eq!(ModalType::CommandPalette.title(), "Command Palette");
    }

    #[test]
    fn test_view_type_name() {
        assert_eq!(ViewType::Session.name(), "Session");
        assert_eq!(ViewType::Help.name(), "Help");
    }

    #[test]
    fn test_args_string_quoted_simple() {
        // Simple args without spaces should not be quoted
        let cmd = ParsedCommand::new(
            "remove".to_string(),
            vec!["file1.txt".to_string(), "file2.txt".to_string()],
            "/remove file1.txt file2.txt".to_string(),
        );
        assert_eq!(cmd.args_string_quoted(), "file1.txt file2.txt");
    }

    #[test]
    fn test_args_string_quoted_with_spaces() {
        // Args with spaces should be quoted
        let cmd = ParsedCommand::new(
            "remove".to_string(),
            vec!["my file.txt".to_string(), "other.txt".to_string()],
            "/remove \"my file.txt\" other.txt".to_string(),
        );
        assert_eq!(cmd.args_string_quoted(), "\"my file.txt\" other.txt");
    }

    #[test]
    fn test_args_string_quoted_with_internal_quotes() {
        // Args with internal quotes should be escaped
        let cmd = ParsedCommand::new(
            "remove".to_string(),
            vec!["file \"with\" quotes.txt".to_string()],
            "/remove".to_string(),
        );
        assert_eq!(cmd.args_string_quoted(), "\"file \\\"with\\\" quotes.txt\"");
    }

    #[test]
    fn test_args_string_quoted_empty() {
        // No args should return empty string
        let cmd = ParsedCommand::new("remove".to_string(), vec![], "/remove".to_string());
        assert_eq!(cmd.args_string_quoted(), "");
    }
}
