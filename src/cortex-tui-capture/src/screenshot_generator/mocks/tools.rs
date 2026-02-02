//! Mock content for tool execution scenarios.

use crate::screenshot_generator::ScreenshotScenario;

/// Trait for tool mock content generation.
pub trait ToolMocks {
    /// Create mock content for a tool scenario.
    fn create_tool_content(&self, scenario: &ScreenshotScenario) -> Option<String> {
        match scenario.id.as_str() {
            "tool_pending" => Some(mock_tool_pending()),
            "tool_running" => Some(mock_tool_running()),
            "tool_completed" => Some(mock_tool_completed()),
            "tool_failed" => Some(mock_tool_failed()),
            "tool_collapsed" => Some(mock_tool_collapsed()),
            "tool_expanded" => Some(mock_tool_expanded()),
            "tool_multiple" => Some(mock_tool_multiple()),
            // Approvals
            "approval_simple" => Some(mock_approval_simple()),
            "approval_with_diff" => Some(mock_approval_diff()),
            "approval_dangerous" => Some(mock_approval_dangerous()),
            "approval_modes" => Some(mock_approval_modes()),
            _ => None,
        }
    }
}

fn mock_tool_pending() -> String {
    r#"
╭─────────────────────────────────────────────────────────────────────────────╮
│ > Read the main.rs file                                                     │
│                                                                             │
│ ┌─ Tool: read_file ──────────────────────────────────────────── PENDING ┐   │
│ │  path: "src/main.rs"                                                  │   │
│ │                                                                       │   │
│ │  ○ Waiting to execute...                                              │   │
│ └───────────────────────────────────────────────────────────────────────┘   │
│                                                                             │
├─────────────────────────────────────────────────────────────────────────────┤
│ > _                                                                         │
╰─────────────────────────────────────────────────────────────────────────────╯
"#
    .to_string()
}

fn mock_tool_running() -> String {
    r#"
╭─────────────────────────────────────────────────────────────────────────────╮
│ > Run cargo build                                                           │
│                                                                             │
│ ┌─ Tool: execute ──────────────────────────────────────────── RUNNING ──┐   │
│ │ ◐ execute cargo build --release                                       │   │
│ │   │ Compiling cortex-core v0.1.0                                      │   │
│ │   │ Compiling cortex-tui v0.1.0                                       │   │
│ │   │ Building [=====>                    ] 23/89                       │   │
│ └───────────────────────────────────────────────────────────────────────┘   │
│                                                                             │
├─────────────────────────────────────────────────────────────────────────────┤
│ > _                                                                         │
├─────────────────────────────────────────────────────────────────────────────┤
│ Esc to interrupt                                               [STREAMING] │
╰─────────────────────────────────────────────────────────────────────────────╯

Tool spinner: ◐ ◑ ◒ ◓ (half-circle rotation)
Status dots: ○ pending, ◐◑◒◓ running, ● completed/failed
"#
    .to_string()
}

fn mock_tool_completed() -> String {
    r#"
╭─────────────────────────────────────────────────────────────────────────────╮
│ > Read the config file                                                      │
│                                                                             │
│ ● read_file config.toml                                                     │
│                                                                             │
│ The configuration file contains the following settings:                     │
│ - Database connection string                                                │
│ - API keys for external services                                            │
│ - Feature flags                                                             │
│                                                                             │
├─────────────────────────────────────────────────────────────────────────────┤
│ > _                                                                         │
├─────────────────────────────────────────────────────────────────────────────┤
│ Enter submit · Ctrl+K palette · ? help                          [ASK MODE] │
╰─────────────────────────────────────────────────────────────────────────────╯

Status: ● = completed (green), ○ = pending (yellow)
"#
    .to_string()
}

fn mock_tool_failed() -> String {
    r#"
╭─────────────────────────────────────────────────────────────────────────────╮
│ > Try to run the nonexistent script                                         │
│                                                                             │
│ ┌─ Tool: execute ─────────────────────────────────────────────── FAILED ┐   │
│ │  command: "./run-tests.sh"                                        ✗   │   │
│ │                                                                       │   │
│ │  Error: Command failed with exit code 127                             │   │
│ │         sh: ./run-tests.sh: No such file or directory                 │   │
│ │                                                                       │   │
│ │  [e] expand output                                                    │   │
│ └───────────────────────────────────────────────────────────────────────┘   │
│                                                                             │
│ The script doesn't exist. Would you like me to create it?                   │
│                                                                             │
├─────────────────────────────────────────────────────────────────────────────┤
│ > _                                                                         │
╰─────────────────────────────────────────────────────────────────────────────╯
"#
    .to_string()
}

fn mock_tool_collapsed() -> String {
    r#"
╭─────────────────────────────────────────────────────────────────────────────╮
│ > Read multiple files                                                       │
│                                                                             │
│ ▶ read_file("src/main.rs") ✓                                  [e] expand   │
│ ▶ read_file("src/lib.rs") ✓                                   [e] expand   │
│ ▶ read_file("Cargo.toml") ✓                                   [e] expand   │
│                                                                             │
│ I've read all three files. Here's a summary...                              │
│                                                                             │
├─────────────────────────────────────────────────────────────────────────────┤
│ > _                                                                         │
╰─────────────────────────────────────────────────────────────────────────────╯
"#
    .to_string()
}

fn mock_tool_expanded() -> String {
    r#"
╭─────────────────────────────────────────────────────────────────────────────╮
│ > Read the main file                                                        │
│                                                                             │
│ ┌─ Tool: read_file ─────────────────────────────────────────── COMPLETE ┐   │
│ │  path: "src/main.rs"                                              ✓   │   │
│ ├───────────────────────────────────────────────────────────────────────┤   │
│ │  Output:                                                              │   │
│ │  ┌────────────────────────────────────────────────────────────────┐   │   │
│ │  │ 1 │ use cortex_tui::run;                                       │   │   │
│ │  │ 2 │                                                            │   │   │
│ │  │ 3 │ #[tokio::main]                                             │   │   │
│ │  │ 4 │ async fn main() -> anyhow::Result<()> {                    │   │   │
│ │  │ 5 │     run().await                                            │   │   │
│ │  │ 6 │ }                                                          │   │   │
│ │  └────────────────────────────────────────────────────────────────┘   │   │
│ │                                                                       │   │
│ │  [e] collapse                                                         │   │
│ └───────────────────────────────────────────────────────────────────────┘   │
│                                                                             │
├─────────────────────────────────────────────────────────────────────────────┤
│ > _                                                                         │
╰─────────────────────────────────────────────────────────────────────────────╯
"#
    .to_string()
}

fn mock_tool_multiple() -> String {
    r#"
╭─────────────────────────────────────────────────────────────────────────────╮
│ > Analyze the codebase and run tests                                        │
│                                                                             │
│ ● read_file src/lib.rs                                                      │
│ ◐ execute cargo test                                                        │
│   │ Running... (8s)                                                         │
│ ○ write_file ANALYSIS.md                                                    │
│                                                                             │
│ Running tests now. Once complete, I'll write the analysis...                │
│                                                                             │
├─────────────────────────────────────────────────────────────────────────────┤
│ > _                                                                         │
├─────────────────────────────────────────────────────────────────────────────┤
│ Esc to interrupt                                               [STREAMING] │
╰─────────────────────────────────────────────────────────────────────────────╯

Status: ● completed, ◐ running (animated), ○ pending
"#
    .to_string()
}

fn mock_approval_simple() -> String {
    r#"
╭─────────────────────────────────────────────────────────────────────────────╮
│                        ╭─ Approval Required ─────────────╮                  │
│                        │                                 │                  │
│                        │  Tool: execute                  │                  │
│                        │                                 │                  │
│                        │  Arguments:                     │                  │
│                        │    command: "rm -rf ./build"    │                  │
│                        │                                 │                  │
│                        │  ────────────────────────────   │                  │
│                        │                                 │                  │
│                        │  [y] Approve   [n] Reject       │                  │
│                        │  [s] Approve for session        │                  │
│                        │  [a] Always allow this tool     │                  │
│                        │                                 │                  │
│                        ╰─────────────────────────────────╯                  │
╰─────────────────────────────────────────────────────────────────────────────╯
"#
    .to_string()
}

fn mock_approval_diff() -> String {
    r#"
╭─────────────────────────────────────────────────────────────────────────────╮
│                     ╭─ Approval Required ────────────────────╮              │
│                     │                                        │              │
│                     │  Tool: write_file                      │              │
│                     │  Path: src/main.rs                     │              │
│                     │                                        │              │
│                     │  ┌─ Diff Preview ───────────────────┐  │              │
│                     │  │ @@ -1,5 +1,7 @@                  │  │              │
│                     │  │  use cortex_tui::run;            │  │              │
│                     │  │                                  │  │              │
│                     │  │ +use tracing::info;              │  │              │
│                     │  │ +                                │  │              │
│                     │  │  #[tokio::main]                  │  │              │
│                     │  │  async fn main() -> Result<()> { │  │              │
│                     │  │ +    info!("Starting...");       │  │              │
│                     │  │      run().await                 │  │              │
│                     │  │  }                               │  │              │
│                     │  └──────────────────────────────────┘  │              │
│                     │                                        │              │
│                     │  [y] Approve  [n] Reject  [d] Full diff│              │
│                     ╰────────────────────────────────────────╯              │
╰─────────────────────────────────────────────────────────────────────────────╯
"#
    .to_string()
}

fn mock_approval_dangerous() -> String {
    r#"
╭─────────────────────────────────────────────────────────────────────────────╮
│               ╭─ [DANGER] DANGEROUS OPERATION ───────────────╮              │
│               │                                              │              │
│               │  Tool: execute                               │              │
│               │                                              │              │
│               │  Command: sudo rm -rf /var/log/*             │              │
│               │                                              │              │
│               │  [WARN] WARNING: This command will permanently│             │
│               │     delete system log files with elevated    │              │
│               │     privileges. This action cannot be undone.│              │
│               │                                              │              │
│               │  Are you SURE you want to proceed?           │              │
│               │                                              │              │
│               │     [y] Yes, I understand the risks          │              │
│               │     [n] No, cancel this operation            │              │
│               │                                              │              │
│               ╰──────────────────────────────────────────────╯              │
╰─────────────────────────────────────────────────────────────────────────────╯
"#
    .to_string()
}

fn mock_approval_modes() -> String {
    r#"
╭─────────────────────────────────────────────────────────────────────────────╮
│                     ╭─ Approval Required ────────────────────╮              │
│                     │                                        │              │
│                     │  Tool: read_file                       │              │
│                     │  Path: /etc/hosts                      │              │
│                     │                                        │              │
│                     │  Select approval mode:                 │              │
│                     │                                        │              │
│                     │  [y] Approve once                      │              │
│                     │      Execute this specific call        │              │
│                     │                                        │              │
│                     │  [s] Approve for session               │              │
│                     │      Auto-approve read_file this       │              │
│                     │      session only                      │              │
│                     │                                        │              │
│                     │  [a] Always allow                      │              │
│                     │      Add read_file to trusted tools    │              │
│                     │                                        │              │
│                     │  [n] Reject                            │              │
│                     │                                        │              │
│                     ╰────────────────────────────────────────╯              │
╰─────────────────────────────────────────────────────────────────────────────╯
"#
    .to_string()
}
