//! Mock content for widget-related scenarios (autocomplete, modals, etc.).

use crate::screenshot_generator::ScreenshotScenario;

/// Trait for widget mock content generation.
pub trait WidgetMocks {
    /// Create mock content for a widget scenario.
    fn create_widget_content(&self, scenario: &ScreenshotScenario) -> Option<String> {
        match scenario.id.as_str() {
            // Autocomplete
            "autocomplete_commands" => Some(mock_autocomplete_commands()),
            "autocomplete_commands_filtered" => Some(mock_autocomplete_filtered()),
            "autocomplete_mentions" => Some(mock_autocomplete_mentions()),
            "autocomplete_scroll" => Some(mock_autocomplete_scroll()),
            "autocomplete_selected" => Some(mock_autocomplete_selected()),
            // Modals
            "modal_model_picker" => Some(mock_model_picker()),
            "modal_command_palette" => Some(mock_command_palette()),
            "modal_export" => Some(mock_export_modal()),
            "modal_form" => Some(mock_form_modal()),
            // Sidebar
            "sidebar_visible" => Some(mock_sidebar_visible()),
            "sidebar_hidden" => Some(mock_sidebar_hidden()),
            "sidebar_sessions" => Some(mock_sidebar_sessions()),
            "sidebar_selected" => Some(mock_sidebar_selected()),
            // Questions
            "question_single" => Some(mock_question_single()),
            "question_multiple" => Some(mock_question_multiple()),
            "question_text" => Some(mock_question_text()),
            "question_tabs" => Some(mock_question_tabs()),
            _ => None,
        }
    }
}

fn mock_autocomplete_commands() -> String {
    r#"
╭─────────────────────────────────────────────────────────────────────────────╮
│ ┌─ Commands ──────────────────────────────────────────────────────┐         │
│ │ > /help      - Show help information                            │         │
│ │   /model     - Switch to a model                                │         │
│ │   /models    - List available models                            │         │
│ │   /new       - Start a new session                              │         │
│ │   /clear     - Clear current conversation                       │         │
│ │   /session   - Show current session info                        │         │
│ │   /export    - Export session to file                           │         │
│ │   /quit      - Quit the application                             │         │
│ └─────────────────────────────────────────────────────────────────┘         │
├─────────────────────────────────────────────────────────────────────────────┤
│ > /_                                                                        │
╰─────────────────────────────────────────────────────────────────────────────╯
"#
    .to_string()
}

fn mock_autocomplete_filtered() -> String {
    r#"
╭─────────────────────────────────────────────────────────────────────────────╮
│ ┌─ Commands ──────────────────────────────────────────────────────┐         │
│ │ > /model     - Switch to a model                                │         │
│ │   /models    - List available models                            │         │
│ └─────────────────────────────────────────────────────────────────┘         │
├─────────────────────────────────────────────────────────────────────────────┤
│ > /mod_                                                                     │
╰─────────────────────────────────────────────────────────────────────────────╯

Filtering: "mod" matches 2 commands
"#
    .to_string()
}

fn mock_autocomplete_mentions() -> String {
    r#"
╭─────────────────────────────────────────────────────────────────────────────╮
│ ┌─ Mentions ──────────────────────────────────────────────────────┐         │
│ │ > # file      - Add a file to context                           │         │
│ │   + folder    - Add folder contents                             │         │
│ │   @ url       - Fetch URL content                               │         │
│ │   * git       - Git repository info                             │         │
│ │   > terminal  - Recent terminal output                          │         │
│ │   ! problems  - LSP diagnostics                                 │         │
│ │   | tree      - Directory tree                                  │         │
│ └─────────────────────────────────────────────────────────────────┘         │
├─────────────────────────────────────────────────────────────────────────────┤
│ > @_                                                                        │
╰─────────────────────────────────────────────────────────────────────────────╯
"#
    .to_string()
}

fn mock_autocomplete_scroll() -> String {
    r#"
╭─────────────────────────────────────────────────────────────────────────────╮
│ ┌─ Commands ──────────────────────────────────────────────────────┐         │
│ │ > /help      - Show help information                           │█│       │
│ │   /model     - Switch to a model                               │ │       │
│ │   /models    - List available models                           │ │       │
│ │   /new       - Start a new session                             │ │       │
│ │   /clear     - Clear current conversation                      │ │       │
│ │   /session   - Show current session info                       │ │       │
│ │   /export    - Export session to file                          │ │       │
│ │   /quit      - Quit the application                            │▄│       │
│ └─────────────────────────────────────────────────────────────────┘         │
│   + 42 more commands (scroll to see)                                        │
├─────────────────────────────────────────────────────────────────────────────┤
│ > /_                                                                        │
╰─────────────────────────────────────────────────────────────────────────────╯
"#
    .to_string()
}

fn mock_autocomplete_selected() -> String {
    r#"
╭─────────────────────────────────────────────────────────────────────────────╮
│ ┌─ Commands ──────────────────────────────────────────────────────┐         │
│ │   /help      - Show help information                            │         │
│ │   /model     - Switch to a model                                │         │
│ │ █ /models    - List available models                 [SELECTED] │         │
│ │   /new       - Start a new session                              │         │
│ │   /clear     - Clear current conversation                       │         │
│ └─────────────────────────────────────────────────────────────────┘         │
├─────────────────────────────────────────────────────────────────────────────┤
│ > /mod_                                                                     │
├─────────────────────────────────────────────────────────────────────────────┤
│ ↑↓ navigate · Tab/Enter select · Esc cancel                                 │
╰─────────────────────────────────────────────────────────────────────────────╯
"#
    .to_string()
}

fn mock_model_picker() -> String {
    r#"
╭─────────────────────────────────────────────────────────────────────────────╮
│                     ╭─ Select Model ────────────────────╮                   │
│                     │                                   │                   │
│                     │  Search: _                        │                   │
│                     │                                   │                   │
│                     │  ┌─ OpenAI ─────────────────────┐ │                   │
│                     │  │ > gpt-4                      │ │                   │
│                     │  │   gpt-4-turbo                │ │                   │
│                     │  │   gpt-3.5-turbo              │ │                   │
│                     │  └──────────────────────────────┘ │                   │
│                     │                                   │                   │
│                     │  ┌─ Anthropic ──────────────────┐ │                   │
│                     │  │   claude-3-opus              │ │                   │
│                     │  │   claude-3-sonnet            │ │                   │
│                     │  │   claude-3-haiku             │ │                   │
│                     │  └──────────────────────────────┘ │                   │
│                     │                                   │                   │
│                     │        [Enter to select]          │                   │
│                     ╰───────────────────────────────────╯                   │
╰─────────────────────────────────────────────────────────────────────────────╯
"#
    .to_string()
}

fn mock_command_palette() -> String {
    r#"
╭─────────────────────────────────────────────────────────────────────────────╮
│                  ╭─ Command Palette ─────────────────────╮                  │
│                  │                                       │                  │
│                  │  > _                                  │                  │
│                  │                                       │                  │
│                  │  Recent:                              │                  │
│                  │    /model gpt-4                       │                  │
│                  │    /clear                             │                  │
│                  │    /help commands                     │                  │
│                  │                                       │                  │
│                  │  All Commands:                        │                  │
│                  │    /help       Show help              │                  │
│                  │    /new        New session            │                  │
│                  │    /model      Switch model           │                  │
│                  │    /export     Export session         │                  │
│                  │    ...                                │                  │
│                  │                                       │                  │
│                  │    [Ctrl+K to close]                  │                  │
│                  ╰───────────────────────────────────────╯                  │
╰─────────────────────────────────────────────────────────────────────────────╯
"#
    .to_string()
}

fn mock_export_modal() -> String {
    r#"
╭─────────────────────────────────────────────────────────────────────────────╮
│                    ╭─ Export Session ────────────────────╮                  │
│                    │                                     │                  │
│                    │  Format:                            │                  │
│                    │    (•) Markdown                     │                  │
│                    │    ( ) JSON                         │                  │
│                    │    ( ) Plain Text                   │                  │
│                    │                                     │                  │
│                    │  Filename:                          │                  │
│                    │    [session_2024-01-15.md        ]  │                  │
│                    │                                     │                  │
│                    │  Include:                           │                  │
│                    │    [x] Messages                     │                  │
│                    │    [x] Tool outputs                 │                  │
│                    │    [ ] Timestamps                   │                  │
│                    │                                     │                  │
│                    │    [Cancel]        [Export]         │                  │
│                    ╰─────────────────────────────────────╯                  │
╰─────────────────────────────────────────────────────────────────────────────╯
"#
    .to_string()
}

fn mock_form_modal() -> String {
    r#"
╭─────────────────────────────────────────────────────────────────────────────╮
│                    ╭─ Rename Session ────────────────────╮                  │
│                    │                                     │                  │
│                    │  Session Name:                      │                  │
│                    │  [Project Analysis______________ ]  │                  │
│                    │                                     │                  │
│                    │  Description (optional):            │                  │
│                    │  [_________________________________]│                  │
│                    │  [_________________________________]│                  │
│                    │                                     │                  │
│                    │    [Cancel]         [Save]          │                  │
│                    ╰─────────────────────────────────────╯                  │
╰─────────────────────────────────────────────────────────────────────────────╯
"#
    .to_string()
}

fn mock_sidebar_visible() -> String {
    r#"
╭────────────────────────╮╭───────────────────────────────────────────────────╮
│ Sessions               ││ > Hello!                                          │
│ ───────────────────────││                                                   │
│ > Project Analysis     ││ Hi! How can I help you today?                     │
│   Code Review          ││                                                   │
│   Bug Investigation    ││                                                   │
│   Documentation        ││                                                   │
│                        │├───────────────────────────────────────────────────┤
│                        ││ > _                                               │
│                        │├───────────────────────────────────────────────────┤
│ [Ctrl+N] New Session   ││ Enter submit · Ctrl+B hide sidebar                │
╰────────────────────────╯╰───────────────────────────────────────────────────╯
"#
    .to_string()
}

fn mock_sidebar_hidden() -> String {
    r#"
╭─────────────────────────────────────────────────────────────────────────────╮
│ > Hello!                                                                    │
│                                                                             │
│ Hi! How can I help you today?                                               │
│                                                                             │
│                                                                             │
│                                                                             │
├─────────────────────────────────────────────────────────────────────────────┤
│ > _                                                                         │
├─────────────────────────────────────────────────────────────────────────────┤
│ Enter submit · Ctrl+B show sidebar                                          │
╰─────────────────────────────────────────────────────────────────────────────╯
"#
    .to_string()
}

fn mock_sidebar_sessions() -> String {
    r#"
╭────────────────────────╮╭───────────────────────────────────────────────────╮
│ Sessions           (5) ││                                                   │
│ ───────────────────────││ Select a session or start a new one               │
│ ★ Project Analysis     ││                                                   │
│   Today 14:30          ││                                                   │
│ ───────────────────────││                                                   │
│   Code Review          ││                                                   │
│   Today 10:15          ││                                                   │
│ ───────────────────────││                                                   │
│   Bug Investigation    ││                                                   │
│   Yesterday            ││                                                   │
│ ───────────────────────││                                                   │
│   Documentation        ││                                                   │
│   2 days ago           ││                                                   │
│ ───────────────────────││                                                   │
│   Refactoring Plan     ││                                                   │
│   1 week ago           │├───────────────────────────────────────────────────┤
│                        ││ Enter: load · d: delete · r: rename               │
╰────────────────────────╯╰───────────────────────────────────────────────────╯
"#
    .to_string()
}

fn mock_sidebar_selected() -> String {
    r#"
╭────────────────────────╮╭───────────────────────────────────────────────────╮
│ Sessions           (5) ││                                                   │
│ ───────────────────────││ Session: Code Review                              │
│   Project Analysis     ││ Created: Today 10:15                              │
│   Today 14:30          ││ Messages: 12                                      │
│ ───────────────────────││                                                   │
│ █ Code Review        ◀ ││ Last message:                                     │
│   Today 10:15          ││ "The refactoring looks good..."                   │
│ ───────────────────────││                                                   │
│   Bug Investigation    ││                                                   │
│   Yesterday            ││                                                   │
│ ───────────────────────││                                                   │
│                        │├───────────────────────────────────────────────────┤
│                        ││ Enter: load · d: delete · r: rename               │
╰────────────────────────╯╰───────────────────────────────────────────────────╯
"#
    .to_string()
}

fn mock_question_single() -> String {
    r#"
╭─────────────────────────────────────────────────────────────────────────────╮
│                      ╭─ Question ─────────────────────╮                     │
│                      │                                │                     │
│                      │  How would you like to proceed │                     │
│                      │  with the refactoring?         │                     │
│                      │                                │                     │
│                      │  ○ Apply all changes           │                     │
│                      │  ● Review each change          │                     │
│                      │  ○ Show diff only              │                     │
│                      │  ○ Cancel                      │                     │
│                      │                                │                     │
│                      │    [Enter to confirm]          │                     │
│                      ╰────────────────────────────────╯                     │
╰─────────────────────────────────────────────────────────────────────────────╯
"#
    .to_string()
}

fn mock_question_multiple() -> String {
    r#"
╭─────────────────────────────────────────────────────────────────────────────╮
│                   ╭─ Question ──────────────────────────╮                   │
│                   │                                     │                   │
│                   │  Which files should I modify?       │                   │
│                   │  (Select multiple with Space)       │                   │
│                   │                                     │                   │
│                   │  [x] src/main.rs                    │                   │
│                   │  [x] src/lib.rs                     │                   │
│                   │  [ ] src/config.rs                  │                   │
│                   │  [x] tests/integration.rs           │                   │
│                   │  [ ] Cargo.toml                     │                   │
│                   │                                     │                   │
│                   │    [Space toggle · Enter confirm]   │                   │
│                   ╰─────────────────────────────────────╯                   │
╰─────────────────────────────────────────────────────────────────────────────╯
"#
    .to_string()
}

fn mock_question_text() -> String {
    r#"
╭─────────────────────────────────────────────────────────────────────────────╮
│                   ╭─ Question ──────────────────────────╮                   │
│                   │                                     │                   │
│                   │  What should the new function be    │                   │
│                   │  called?                            │                   │
│                   │                                     │                   │
│                   │  [process_data___________________]  │                   │
│                   │                                     │                   │
│                   │    [Enter to confirm]               │                   │
│                   ╰─────────────────────────────────────╯                   │
╰─────────────────────────────────────────────────────────────────────────────╯
"#
    .to_string()
}

fn mock_question_tabs() -> String {
    r#"
╭─────────────────────────────────────────────────────────────────────────────╮
│               ╭─ Configuration Wizard ─────────────────────╮                │
│               │                                            │                │
│               │  [General]  [Model]  [Security]  [Advanced]│                │
│               │  ─────────────────────────────────────────  │               │
│               │                                            │                │
│               │  General Settings (1/4)                    │                │
│               │                                            │                │
│               │  Theme:                                    │                │
│               │    (•) Dark                                │                │
│               │    ( ) Light                               │                │
│               │    ( ) System                              │                │
│               │                                            │                │
│               │  Show timestamps:  [x]                     │                │
│               │                                            │                │
│               │    [← Back]  [Next →]                      │                │
│               ╰────────────────────────────────────────────╯                │
╰─────────────────────────────────────────────────────────────────────────────╯
"#
    .to_string()
}
