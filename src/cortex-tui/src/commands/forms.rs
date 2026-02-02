//! Form Registry for Slash Commands
//!
//! Provides form schemas for commands that require user input.
//! Each command maps to a `FormState` containing the appropriate fields.

use crate::widgets::form::{FormField, FormState};

/// Registry that provides form schemas for slash commands.
#[derive(Debug, Default)]
pub struct FormRegistry;

impl FormRegistry {
    /// Creates a new form registry.
    pub fn new() -> Self {
        Self
    }

    /// Returns the appropriate form schema for the given command.
    ///
    /// Returns `None` if the command doesn't require a form (e.g., simple toggles
    /// or commands with no arguments).
    pub fn get_form(&self, command: &str) -> Option<FormState> {
        match command {
            // ============================================================
            // SESSION COMMANDS
            // ============================================================
            "rename" => Some(FormState::new(
                "Rename Session",
                "rename",
                vec![
                    FormField::text("name", "New Session Name")
                        .required()
                        .with_placeholder("Enter new session name..."),
                ],
            )),

            "fork" => Some(FormState::new(
                "Fork Session",
                "fork",
                vec![
                    FormField::text("name", "Fork Name").with_placeholder("Optional fork name..."),
                ],
            )),

            "export" => Some(FormState::new(
                "Export Session",
                "export",
                vec![FormField::select(
                    "format",
                    "Format",
                    vec![
                        "markdown".to_string(),
                        "json".to_string(),
                        "txt".to_string(),
                    ],
                )],
            )),

            "delete" => Some(FormState::new(
                "Delete Session",
                "delete",
                vec![
                    FormField::text("session_id", "Session ID")
                        .required()
                        .with_placeholder("Enter session ID to delete..."),
                ],
            )),

            "resume" => Some(FormState::new(
                "Resume Session",
                "resume",
                vec![
                    FormField::text("session_id", "Session ID")
                        .with_placeholder("Optional session ID (latest if empty)..."),
                ],
            )),

            "rewind" => Some(FormState::new(
                "Rewind Session",
                "rewind",
                vec![
                    FormField::number("steps", "Steps")
                        .with_placeholder("1")
                        .with_value("1"),
                ],
            )),

            // ============================================================
            // NAVIGATION COMMANDS
            // ============================================================
            "goto" => Some(FormState::new(
                "Go To Message",
                "goto",
                vec![
                    FormField::number("message_number", "Message Number")
                        .required()
                        .with_placeholder("Enter message number..."),
                ],
            )),

            "scroll" => Some(FormState::new(
                "Scroll To",
                "scroll",
                vec![FormField::select(
                    "position",
                    "Position",
                    vec![
                        "top".to_string(),
                        "bottom".to_string(),
                        "10".to_string(),
                        "50".to_string(),
                    ],
                )],
            )),

            "search" => Some(FormState::new(
                "Search Messages",
                "search",
                vec![
                    FormField::text("pattern", "Search Pattern")
                        .required()
                        .with_placeholder("Enter search pattern..."),
                ],
            )),

            "diff" => Some(FormState::new(
                "Show Diff",
                "diff",
                vec![
                    FormField::text("file_path", "File Path")
                        .with_placeholder("Optional file path..."),
                ],
            )),

            // ============================================================
            // FILE COMMANDS
            // ============================================================
            "ls" => Some(FormState::new(
                "List Directory",
                "ls",
                vec![
                    FormField::text("directory", "Directory Path")
                        .with_placeholder(".")
                        .with_value("."),
                ],
            )),

            "tree" => Some(FormState::new(
                "Directory Tree",
                "tree",
                vec![
                    FormField::text("directory", "Directory Path")
                        .with_placeholder(".")
                        .with_value("."),
                ],
            )),

            "mention" => Some(FormState::new(
                "Mention File/Symbol",
                "mention",
                vec![
                    FormField::text("target", "File or Symbol")
                        .required()
                        .with_placeholder("Enter file path or symbol name..."),
                ],
            )),

            "add" => Some(FormState::new(
                "Add Files to Context",
                "add",
                vec![
                    FormField::text("files", "File Paths (space separated)")
                        .required()
                        .with_placeholder("path/to/file1 path/to/file2..."),
                ],
            )),

            "remove" => Some(FormState::new(
                "Remove Files from Context",
                "remove",
                vec![
                    FormField::text("files", "File Paths (space separated)")
                        .required()
                        .with_placeholder("path/to/file1 path/to/file2..."),
                ],
            )),

            // ============================================================
            // MODEL/CONFIG COMMANDS
            // ============================================================
            "temperature" => Some(FormState::new(
                "Set Temperature",
                "temperature",
                vec![
                    FormField::number("value", "Value (0.0-2.0)")
                        .required()
                        .with_placeholder("0.7"),
                ],
            )),

            "tokens" => Some(FormState::new(
                "Set Max Tokens",
                "tokens",
                vec![
                    FormField::number("max_tokens", "Max Tokens")
                        .required()
                        .with_placeholder("4096"),
                ],
            )),

            "approval" => Some(FormState::new(
                "Set Approval Mode",
                "approval",
                vec![FormField::select(
                    "mode",
                    "Mode",
                    vec![
                        "ask".to_string(),
                        "always".to_string(),
                        "session".to_string(),
                        "never".to_string(),
                    ],
                )],
            )),

            "sandbox" => Some(FormState::new(
                "Sandbox Mode",
                "sandbox",
                vec![FormField::toggle("enabled", "Enable Sandbox")],
            )),

            "auto" => Some(FormState::new(
                "Auto-Run Mode",
                "auto",
                vec![FormField::toggle("enabled", "Enable Auto-Run")],
            )),

            "provider" => Some(FormState::new(
                "Set Provider",
                "provider",
                vec![
                    FormField::text("provider", "Provider Name")
                        .required()
                        .with_placeholder("anthropic, openai, groq..."),
                ],
            )),

            "model" => Some(FormState::new(
                "Set Model",
                "model",
                vec![
                    FormField::text("model", "Model Name")
                        .required()
                        .with_placeholder("claude-sonnet-4-20250514, gpt-4..."),
                ],
            )),

            "theme" => Some(FormState::new(
                "Set Theme",
                "theme",
                vec![
                    FormField::text("theme", "Theme Name")
                        .with_placeholder("dark, light, custom..."),
                ],
            )),

            // ============================================================
            // MCP COMMANDS
            // ============================================================
            "mcp" => Some(FormState::new(
                "MCP Management",
                "mcp",
                vec![
                    FormField::select(
                        "action",
                        "Action",
                        vec![
                            "list".to_string(),
                            "status".to_string(),
                            "add".to_string(),
                            "remove".to_string(),
                        ],
                    ),
                    FormField::text("server_name", "Server Name")
                        .with_placeholder("Server name (for add/remove)..."),
                    FormField::text("command_args", "Command Args")
                        .with_placeholder("Additional arguments..."),
                ],
            )),

            "mcp-add" => Some(FormState::new(
                "Add MCP Server",
                "mcp-add",
                vec![
                    FormField::text("server_name", "Server Name")
                        .required()
                        .with_placeholder("Enter server name..."),
                    FormField::text("command", "Command")
                        .required()
                        .with_placeholder("Command to run (e.g., npx, uvx)..."),
                    FormField::text("args", "Arguments")
                        .with_placeholder("Optional command arguments..."),
                ],
            )),

            "mcp-remove" => Some(FormState::new(
                "Remove MCP Server",
                "mcp-remove",
                vec![
                    FormField::text("server_name", "Server Name")
                        .required()
                        .with_placeholder("Enter server name to remove..."),
                ],
            )),

            "mcp-auth" => Some(FormState::new(
                "MCP Authentication",
                "mcp-auth",
                vec![
                    FormField::text("server_name", "Server Name")
                        .required()
                        .with_placeholder("Enter server name..."),
                    FormField::secret("api_key", "API Key/Token")
                        .required()
                        .with_placeholder("Enter API key or token..."),
                ],
            )),

            "mcp-logs" => Some(FormState::new(
                "MCP Server Logs",
                "mcp-logs",
                vec![
                    FormField::text("server_name", "Server Name")
                        .with_placeholder("Optional server name (all if empty)..."),
                ],
            )),

            "mcp-reload" => Some(FormState::new(
                "Reload MCP Server",
                "mcp-reload",
                vec![
                    FormField::text("server_name", "Server Name")
                        .with_placeholder("Optional server name (all if empty)..."),
                ],
            )),

            // ============================================================
            // DEBUG COMMANDS
            // ============================================================
            "debug" => Some(FormState::new(
                "Debug Mode",
                "debug",
                vec![FormField::toggle("enabled", "Enable Debug")],
            )),

            "config" => Some(FormState::new(
                "View/Edit Config",
                "config",
                vec![
                    FormField::text("key", "Config Key")
                        .with_placeholder("Optional config key (all if empty)..."),
                ],
            )),

            "logs" => Some(FormState::new(
                "Set Log Level",
                "logs",
                vec![FormField::select(
                    "level",
                    "Level",
                    vec![
                        "info".to_string(),
                        "debug".to_string(),
                        "warn".to_string(),
                        "error".to_string(),
                    ],
                )],
            )),

            "dump" => Some(FormState::new(
                "Dump State",
                "dump",
                vec![
                    FormField::text("filename", "Output Filename")
                        .with_placeholder("Optional filename (stdout if empty)..."),
                ],
            )),

            "eval" => Some(FormState::new(
                "Evaluate Expression",
                "eval",
                vec![
                    FormField::text("expression", "Expression")
                        .required()
                        .with_placeholder("Enter expression to evaluate..."),
                ],
            )),

            // No form needed for this command
            _ => None,
        }
    }

    /// Returns true if the given command has an associated form.
    pub fn has_form(&self, command: &str) -> bool {
        self.get_form(command).is_some()
    }

    /// Returns a list of all commands that have forms.
    pub fn commands_with_forms(&self) -> Vec<&'static str> {
        vec![
            // Session commands
            "rename",
            "fork",
            "export",
            "delete",
            "resume",
            "rewind",
            // Navigation commands
            "goto",
            "scroll",
            "search",
            "diff",
            // File commands
            "ls",
            "tree",
            "mention",
            "add",
            "remove",
            // Model/Config commands
            "temperature",
            "tokens",
            "approval",
            "sandbox",
            "auto",
            "provider",
            "model",
            "theme",
            // MCP commands
            "mcp",
            "mcp-add",
            "mcp-remove",
            "mcp-auth",
            "mcp-logs",
            "mcp-reload",
            // Debug commands
            "debug",
            "config",
            "logs",
            "dump",
            "eval",
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::widgets::FieldKind;

    #[test]
    fn test_session_commands_have_forms() {
        let registry = FormRegistry::new();
        assert!(registry.has_form("rename"));
        assert!(registry.has_form("fork"));
        assert!(registry.has_form("export"));
        assert!(registry.has_form("delete"));
        assert!(registry.has_form("resume"));
        assert!(registry.has_form("rewind"));
    }

    #[test]
    fn test_navigation_commands_have_forms() {
        let registry = FormRegistry::new();
        assert!(registry.has_form("goto"));
        assert!(registry.has_form("scroll"));
        assert!(registry.has_form("search"));
        assert!(registry.has_form("diff"));
    }

    #[test]
    fn test_file_commands_have_forms() {
        let registry = FormRegistry::new();
        assert!(registry.has_form("ls"));
        assert!(registry.has_form("tree"));
        assert!(registry.has_form("mention"));
        assert!(registry.has_form("add"));
        assert!(registry.has_form("remove"));
    }

    #[test]
    fn test_model_config_commands_have_forms() {
        let registry = FormRegistry::new();
        assert!(registry.has_form("temperature"));
        assert!(registry.has_form("tokens"));
        assert!(registry.has_form("approval"));
        assert!(registry.has_form("sandbox"));
        assert!(registry.has_form("auto"));
        assert!(registry.has_form("provider"));
        assert!(registry.has_form("model"));
        assert!(registry.has_form("theme"));
    }

    #[test]
    fn test_mcp_commands_have_forms() {
        let registry = FormRegistry::new();
        assert!(registry.has_form("mcp"));
        assert!(registry.has_form("mcp-add"));
        assert!(registry.has_form("mcp-remove"));
        assert!(registry.has_form("mcp-auth"));
        assert!(registry.has_form("mcp-logs"));
        assert!(registry.has_form("mcp-reload"));
    }

    #[test]
    fn test_debug_commands_have_forms() {
        let registry = FormRegistry::new();
        assert!(registry.has_form("debug"));
        assert!(registry.has_form("config"));
        assert!(registry.has_form("logs"));
        assert!(registry.has_form("dump"));
        assert!(registry.has_form("eval"));
    }

    #[test]
    fn test_unknown_command_returns_none() {
        let registry = FormRegistry::new();
        assert!(!registry.has_form("unknown_command"));
        assert!(registry.get_form("unknown_command").is_none());
    }

    #[test]
    fn test_rename_form_has_required_field() {
        let registry = FormRegistry::new();
        let form = registry.get_form("rename").unwrap();
        assert_eq!(form.title, "Rename Session");
        assert_eq!(form.command, "rename");
        assert_eq!(form.fields.len(), 1);
        assert!(form.fields[0].required);
    }

    #[test]
    fn test_export_form_has_select_options() {
        let registry = FormRegistry::new();
        let form = registry.get_form("export").unwrap();
        assert_eq!(form.title, "Export Session");
        assert_eq!(form.fields.len(), 1);
        if let FieldKind::Select(options) = &form.fields[0].kind {
            assert_eq!(options.len(), 3);
            assert!(options.contains(&"markdown".to_string()));
            assert!(options.contains(&"json".to_string()));
            assert!(options.contains(&"txt".to_string()));
        } else {
            panic!("Expected Select field");
        }
    }

    #[test]
    fn test_mcp_auth_has_secret_field() {
        let registry = FormRegistry::new();
        let form = registry.get_form("mcp-auth").unwrap();
        assert_eq!(form.fields.len(), 2);
        // Second field should be a secret
        assert!(matches!(form.fields[1].kind, FieldKind::Secret));
    }

    #[test]
    fn test_sandbox_has_toggle_field() {
        let registry = FormRegistry::new();
        let form = registry.get_form("sandbox").unwrap();
        assert_eq!(form.fields.len(), 1);
        assert!(matches!(form.fields[0].kind, FieldKind::Toggle));
    }

    #[test]
    fn test_commands_with_forms_count() {
        let registry = FormRegistry::new();
        let commands = registry.commands_with_forms();
        // 6 session + 4 nav + 5 file + 8 model/config + 6 mcp + 5 debug = 34
        assert_eq!(commands.len(), 34);
    }

    #[test]
    fn test_ls_has_default_value() {
        let registry = FormRegistry::new();
        let form = registry.get_form("ls").unwrap();
        assert_eq!(form.fields[0].value, ".");
    }

    #[test]
    fn test_rewind_has_default_steps() {
        let registry = FormRegistry::new();
        let form = registry.get_form("rewind").unwrap();
        assert_eq!(form.fields[0].value, "1");
    }

    #[test]
    fn test_goto_uses_number_field() {
        let registry = FormRegistry::new();
        let form = registry.get_form("goto").unwrap();
        assert!(matches!(form.fields[0].kind, FieldKind::Number));
        assert!(form.fields[0].required);
    }

    #[test]
    fn test_temperature_uses_number_field() {
        let registry = FormRegistry::new();
        let form = registry.get_form("temperature").unwrap();
        assert!(matches!(form.fields[0].kind, FieldKind::Number));
        assert!(form.fields[0].required);
    }

    #[test]
    fn test_mcp_form_has_multiple_fields() {
        let registry = FormRegistry::new();
        let form = registry.get_form("mcp").unwrap();
        assert_eq!(form.fields.len(), 3);
        // First field is select
        assert!(matches!(form.fields[0].kind, FieldKind::Select(_)));
        // Second and third are text
        assert!(matches!(form.fields[1].kind, FieldKind::Text));
        assert!(matches!(form.fields[2].kind, FieldKind::Text));
    }

    #[test]
    fn test_approval_mode_options() {
        let registry = FormRegistry::new();
        let form = registry.get_form("approval").unwrap();
        if let FieldKind::Select(options) = &form.fields[0].kind {
            assert_eq!(options.len(), 4);
            assert!(options.contains(&"ask".to_string()));
            assert!(options.contains(&"always".to_string()));
            assert!(options.contains(&"session".to_string()));
            assert!(options.contains(&"never".to_string()));
        } else {
            panic!("Expected Select field");
        }
    }

    #[test]
    fn test_logs_level_options() {
        let registry = FormRegistry::new();
        let form = registry.get_form("logs").unwrap();
        if let FieldKind::Select(options) = &form.fields[0].kind {
            assert_eq!(options.len(), 4);
            assert!(options.contains(&"info".to_string()));
            assert!(options.contains(&"debug".to_string()));
            assert!(options.contains(&"warn".to_string()));
            assert!(options.contains(&"error".to_string()));
        } else {
            panic!("Expected Select field");
        }
    }

    #[test]
    fn test_mcp_add_form_has_required_fields() {
        let registry = FormRegistry::new();
        let form = registry.get_form("mcp-add").unwrap();
        assert_eq!(form.title, "Add MCP Server");
        assert_eq!(form.command, "mcp-add");
        assert_eq!(form.fields.len(), 3);
        // server_name and command are required
        assert!(form.fields[0].required);
        assert!(form.fields[1].required);
        // args is optional
        assert!(!form.fields[2].required);
    }

    #[test]
    fn test_mcp_remove_form_has_required_field() {
        let registry = FormRegistry::new();
        let form = registry.get_form("mcp-remove").unwrap();
        assert_eq!(form.title, "Remove MCP Server");
        assert_eq!(form.command, "mcp-remove");
        assert_eq!(form.fields.len(), 1);
        assert!(form.fields[0].required);
    }
}
