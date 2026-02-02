//! Help content types and section definitions.

// ============================================================
// HELP CONTENT TYPES
// ============================================================

/// Content type for help sections.
#[derive(Debug, Clone)]
pub enum HelpContent {
    /// Section title (bold heading)
    Title(String),
    /// Paragraph of text (word-wrapped)
    Paragraph(String),
    /// Key binding: key -> description
    KeyBinding { key: String, description: String },
    /// Command: name, description, usage
    Command {
        name: String,
        description: String,
        usage: String,
    },
    /// Bullet list
    List(Vec<String>),
    /// Code snippet
    Code(String),
    /// Horizontal separator
    Separator,
}

// ============================================================
// HELP SECTION
// ============================================================

/// A documentation section with id, title, and content.
#[derive(Debug, Clone)]
pub struct HelpSection {
    /// Unique identifier for navigation
    pub id: &'static str,
    /// Display title in sidebar
    pub title: &'static str,
    /// Section content
    pub content: Vec<HelpContent>,
}

impl HelpSection {
    /// Creates a new help section.
    ///
    /// # Arguments
    /// * `id` - Unique section identifier
    /// * `title` - Display title for sidebar
    pub fn new(id: &'static str, title: &'static str) -> Self {
        Self {
            id,
            title,
            content: Vec::new(),
        }
    }

    /// Sets the section content.
    ///
    /// # Arguments
    /// * `content` - Vector of help content items
    pub fn with_content(mut self, content: Vec<HelpContent>) -> Self {
        self.content = content;
        self
    }
}

// ============================================================
// DEFAULT HELP SECTIONS
// ============================================================

/// Returns the default help sections.
pub fn get_help_sections() -> Vec<HelpSection> {
    vec![
        HelpSection::new("getting-started", "Getting Started").with_content(vec![
            HelpContent::Title("Welcome to Cortex".to_string()),
            HelpContent::Paragraph(
                "Cortex is an AI-powered coding assistant that helps you write, \
                understand, and debug code through natural conversation."
                    .to_string(),
            ),
            HelpContent::Separator,
            HelpContent::Title("Quick Start".to_string()),
            HelpContent::List(vec![
                "Type a message and press Enter to send".to_string(),
                "Use /help to see available commands".to_string(),
                "Press ? for keyboard shortcuts".to_string(),
                "Use Tab to cycle through UI panels".to_string(),
            ]),
            HelpContent::Separator,
            HelpContent::Title("Getting Help".to_string()),
            HelpContent::Paragraph(
                "Use the sidebar to navigate between help topics. Press Tab to \
                switch between the sidebar and content pane."
                    .to_string(),
            ),
        ]),
        HelpSection::new("keyboard", "Keyboard Shortcuts").with_content(vec![
            HelpContent::Title("Navigation".to_string()),
            HelpContent::KeyBinding {
                key: "Tab".to_string(),
                description: "Cycle focus".to_string(),
            },
            HelpContent::KeyBinding {
                key: "Shift+Tab".to_string(),
                description: "Cycle focus (reverse)".to_string(),
            },
            HelpContent::KeyBinding {
                key: "Ctrl+P".to_string(),
                description: "Command palette".to_string(),
            },
            HelpContent::KeyBinding {
                key: "Ctrl+B".to_string(),
                description: "Toggle sidebar".to_string(),
            },
            HelpContent::KeyBinding {
                key: "?".to_string(),
                description: "Show help".to_string(),
            },
            HelpContent::Separator,
            HelpContent::Title("Session".to_string()),
            HelpContent::KeyBinding {
                key: "Ctrl+S".to_string(),
                description: "Open sessions".to_string(),
            },
            HelpContent::KeyBinding {
                key: "Ctrl+N".to_string(),
                description: "New session".to_string(),
            },
            HelpContent::KeyBinding {
                key: "Ctrl+Z".to_string(),
                description: "Undo".to_string(),
            },
            HelpContent::KeyBinding {
                key: "Ctrl+Y".to_string(),
                description: "Redo".to_string(),
            },
            HelpContent::Separator,
            HelpContent::Title("Input".to_string()),
            HelpContent::KeyBinding {
                key: "Enter".to_string(),
                description: "Send message".to_string(),
            },
            HelpContent::KeyBinding {
                key: "Shift+Enter".to_string(),
                description: "New line".to_string(),
            },
            HelpContent::KeyBinding {
                key: "Up/Down".to_string(),
                description: "History navigation".to_string(),
            },
            HelpContent::Separator,
            HelpContent::Title("View".to_string()),
            HelpContent::KeyBinding {
                key: "j/k".to_string(),
                description: "Scroll down/up".to_string(),
            },
            HelpContent::KeyBinding {
                key: "g/G".to_string(),
                description: "Go to top/bottom".to_string(),
            },
            HelpContent::KeyBinding {
                key: "Ctrl+U/D".to_string(),
                description: "Page up/down".to_string(),
            },
        ]),
        HelpSection::new("commands", "Slash Commands").with_content(vec![
            HelpContent::Title("General Commands".to_string()),
            HelpContent::Command {
                name: "/help".to_string(),
                description: "Show help".to_string(),
                usage: "/help [topic]".to_string(),
            },
            HelpContent::Command {
                name: "/quit".to_string(),
                description: "Quit application".to_string(),
                usage: "/quit".to_string(),
            },
            HelpContent::Command {
                name: "/settings".to_string(),
                description: "Open settings".to_string(),
                usage: "/settings".to_string(),
            },
            HelpContent::Separator,
            HelpContent::Title("Session Commands".to_string()),
            HelpContent::Command {
                name: "/new".to_string(),
                description: "New session".to_string(),
                usage: "/new".to_string(),
            },
            HelpContent::Command {
                name: "/clear".to_string(),
                description: "Clear messages".to_string(),
                usage: "/clear".to_string(),
            },
            HelpContent::Command {
                name: "/resume".to_string(),
                description: "Resume session".to_string(),
                usage: "/resume [id]".to_string(),
            },
            HelpContent::Command {
                name: "/fork".to_string(),
                description: "Fork session".to_string(),
                usage: "/fork [name]".to_string(),
            },
            HelpContent::Separator,
            HelpContent::Title("Model Commands".to_string()),
            HelpContent::Command {
                name: "/model".to_string(),
                description: "Switch model".to_string(),
                usage: "/model <name>".to_string(),
            },
            HelpContent::Command {
                name: "/models".to_string(),
                description: "List models".to_string(),
                usage: "/models".to_string(),
            },
        ]),
        HelpSection::new("models", "Models").with_content(vec![
            HelpContent::Title("Available Models".to_string()),
            HelpContent::Paragraph(
                "Use /models to see available models or /model <name> to switch.".to_string(),
            ),
            HelpContent::Separator,
            HelpContent::Title("Anthropic".to_string()),
            HelpContent::List(vec![
                "claude-sonnet-4-20250514 (default)".to_string(),
                "claude-3-5-sonnet".to_string(),
                "claude-3-opus".to_string(),
            ]),
            HelpContent::Separator,
            HelpContent::Title("OpenAI".to_string()),
            HelpContent::List(vec![
                "gpt-4o".to_string(),
                "gpt-4-turbo".to_string(),
                "gpt-3.5-turbo".to_string(),
            ]),
            HelpContent::Separator,
            HelpContent::Title("Configuration".to_string()),
            HelpContent::Paragraph(
                "Models can be configured in your cortex.toml config file or \
                via environment variables."
                    .to_string(),
            ),
        ]),
        HelpSection::new("tools", "Built-in Tools").with_content(vec![
            HelpContent::Title("File Operations".to_string()),
            HelpContent::List(vec![
                "read - Read file contents".to_string(),
                "write - Create or overwrite files".to_string(),
                "edit - Make targeted edits".to_string(),
                "glob - Find files by pattern".to_string(),
                "grep - Search file contents".to_string(),
            ]),
            HelpContent::Separator,
            HelpContent::Title("System Operations".to_string()),
            HelpContent::List(vec![
                "bash - Execute shell commands".to_string(),
                "task - Spawn subagent tasks".to_string(),
            ]),
            HelpContent::Separator,
            HelpContent::Title("Tool Usage".to_string()),
            HelpContent::Paragraph(
                "Tools are automatically invoked by the AI based on your requests. \
                You can see tool activity in the status bar and inline tool displays."
                    .to_string(),
            ),
        ]),
        HelpSection::new("mcp", "MCP Servers").with_content(vec![
            HelpContent::Title("Model Context Protocol".to_string()),
            HelpContent::Paragraph(
                "MCP allows extending Cortex with external tools and context providers. \
                Servers can provide additional tools, resources, and prompts."
                    .to_string(),
            ),
            HelpContent::Separator,
            HelpContent::Title("MCP Commands".to_string()),
            HelpContent::Command {
                name: "/mcp".to_string(),
                description: "Open interactive MCP management panel".to_string(),
                usage: "/mcp".to_string(),
            },
            HelpContent::Paragraph(
                "The /mcp command opens an interactive panel where you can:\n\
                • Add new servers (stdio, HTTP, or from registry)\n\
                • View all available tools\n\
                • Start, stop, or restart servers\n\
                • Configure authentication\n\
                • View server logs"
                    .to_string(),
            ),
            HelpContent::Separator,
            HelpContent::Title("Configuration".to_string()),
            HelpContent::Paragraph(
                "MCP servers are configured in your cortex.toml file under the \
                [mcp] section."
                    .to_string(),
            ),
        ]),
        HelpSection::new("faq", "FAQ").with_content(vec![
            HelpContent::Title("Frequently Asked Questions".to_string()),
            HelpContent::Separator,
            HelpContent::Title("How do I switch models?".to_string()),
            HelpContent::Paragraph(
                "Use /model <name> or press Ctrl+M to open the model picker.".to_string(),
            ),
            HelpContent::Separator,
            HelpContent::Title("How do I save my session?".to_string()),
            HelpContent::Paragraph(
                "Sessions are automatically saved. Use /export to export to a file.".to_string(),
            ),
            HelpContent::Separator,
            HelpContent::Title("How do I undo changes?".to_string()),
            HelpContent::Paragraph(
                "Press Ctrl+Z to undo the last action. Cortex maintains a history \
                of edits that can be undone."
                    .to_string(),
            ),
            HelpContent::Separator,
            HelpContent::Title("Where are settings stored?".to_string()),
            HelpContent::Paragraph(
                "Settings are stored in ~/.config/cortex/cortex.toml on Linux/macOS \
                or %APPDATA%\\cortex\\cortex.toml on Windows."
                    .to_string(),
            ),
        ]),
    ]
}
