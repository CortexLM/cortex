//! Mock content for state-related scenarios (streaming, permissions, errors).

use crate::screenshot_generator::ScreenshotScenario;

/// Trait for state mock content generation.
pub trait StateMocks {
    /// Create mock content for a state scenario.
    fn create_state_content(&self, scenario: &ScreenshotScenario) -> Option<String> {
        match scenario.id.as_str() {
            // Streaming
            "streaming_started" => Some(mock_streaming_started()),
            "streaming_in_progress" => Some(mock_streaming_progress()),
            "streaming_with_spinner" => Some(mock_streaming_spinner()),
            "streaming_completed" => Some(mock_streaming_completed()),
            // Permissions
            "permission_high" => Some(mock_permission_high()),
            "permission_medium" => Some(mock_permission_medium()),
            "permission_low" => Some(mock_permission_low()),
            "permission_yolo" => Some(mock_permission_yolo()),
            // Errors
            "error_toast" => Some(mock_error_toast()),
            "error_streaming" => Some(mock_error_streaming()),
            "error_connection" => Some(mock_error_connection()),
            "warning_toast" => Some(mock_warning_toast()),
            "info_toast" => Some(mock_info_toast()),
            _ => None,
        }
    }
}

fn mock_streaming_started() -> String {
    r#"
╭─────────────────────────────────────────────────────────────────────────────╮
│ > Analyze the project structure and suggest improvements                    │
│                                                                             │
│ ✽ Thinking...                                                               │
│                                                                             │
│                                                                             │
├─────────────────────────────────────────────────────────────────────────────┤
│ > _                                                                         │
├─────────────────────────────────────────────────────────────────────────────┤
│ Esc to interrupt · 0s elapsed                                  [STREAMING] │
╰─────────────────────────────────────────────────────────────────────────────╯
"#
    .to_string()
}

fn mock_streaming_progress() -> String {
    r#"
╭─────────────────────────────────────────────────────────────────────────────╮
│ > Analyze the project structure                                             │
│                                                                             │
│ Based on my analysis, I can see several areas for improvement:              │
│                                                                             │
│ 1. **Code Organization**: The codebase follows a modular structure          │
│    but could benefit from clearer separation of concerns...█                │
│                                                                             │
│                                                                             │
├─────────────────────────────────────────────────────────────────────────────┤
│ > _                                                                         │
├─────────────────────────────────────────────────────────────────────────────┤
│ Esc to interrupt · 3s elapsed                                  [STREAMING] │
╰─────────────────────────────────────────────────────────────────────────────╯
"#
    .to_string()
}

fn mock_streaming_spinner() -> String {
    r#"
╭─────────────────────────────────────────────────────────────────────────────╮
│ > Run the tests and show me the results                                     │
│                                                                             │
│ ✽ Working... (12s)                                                          │
│                                                                             │
│   ▸ cargo test --workspace                                                  │
│                                                                             │
├─────────────────────────────────────────────────────────────────────────────┤
│ > _                                                                         │
├─────────────────────────────────────────────────────────────────────────────┤
│ Esc to interrupt                                               [STREAMING] │
╰─────────────────────────────────────────────────────────────────────────────╯
"#
    .to_string()
}

fn mock_streaming_completed() -> String {
    r#"
╭─────────────────────────────────────────────────────────────────────────────╮
│ > Analyze the project structure                                             │
│                                                                             │
│ Based on my analysis, here are the key findings:                            │
│                                                                             │
│ 1. **Architecture**: Well-structured monorepo with clear crate boundaries   │
│ 2. **Testing**: Good coverage in core modules, could improve TUI tests      │
│ 3. **Documentation**: Comprehensive doc comments, AGENTS.md is helpful      │
│                                                                             │
│ Would you like me to elaborate on any of these points?                      │
│                                                                             │
├─────────────────────────────────────────────────────────────────────────────┤
│ > _                                                                         │
├─────────────────────────────────────────────────────────────────────────────┤
│ Enter submit · Ctrl+K palette · ? help                          [ASK MODE] │
╰─────────────────────────────────────────────────────────────────────────────╯
"#
    .to_string()
}

fn mock_permission_high() -> String {
    r#"
╭─────────────────────────────────────────────────────────────────────────────╮
│ cortex                                                   [HIGH SECURITY]   │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                             │
│ Permission Mode: HIGH (Ask)                                                 │
│                                                                             │
│ ┌─ Active Rules ────────────────────────────────────────────────────────┐   │
│ │ • All tool executions require approval                                │   │
│ │ • File writes always prompt                                           │   │
│ │ • Command execution always prompts                                    │   │
│ │ • Network requests always prompt                                      │   │
│ └───────────────────────────────────────────────────────────────────────┘   │
│                                                                             │
│ Press Tab to cycle: HIGH → MEDIUM → LOW → YOLO → HIGH                       │
│                                                                             │
├─────────────────────────────────────────────────────────────────────────────┤
│ > _                                                                         │
╰─────────────────────────────────────────────────────────────────────────────╯
"#
    .to_string()
}

fn mock_permission_medium() -> String {
    r#"
╭─────────────────────────────────────────────────────────────────────────────╮
│ cortex                                                        [⚡ MEDIUM]  │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                             │
│ Permission Mode: MEDIUM                                                     │
│                                                                             │
│ ┌─ Active Rules ────────────────────────────────────────────────────────┐   │
│ │ • Read operations auto-approved                                       │   │
│ │ • File writes require approval                                        │   │
│ │ • Safe commands auto-approved                                         │   │
│ │ • Dangerous commands require approval                                 │   │
│ └───────────────────────────────────────────────────────────────────────┘   │
│                                                                             │
├─────────────────────────────────────────────────────────────────────────────┤
│ > _                                                                         │
╰─────────────────────────────────────────────────────────────────────────────╯
"#
    .to_string()
}

fn mock_permission_low() -> String {
    r#"
╭─────────────────────────────────────────────────────────────────────────────╮
│ cortex                                                            [✓ LOW]  │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                             │
│ Permission Mode: LOW (Auto)                                                 │
│                                                                             │
│ ┌─ Active Rules ────────────────────────────────────────────────────────┐   │
│ │ • Most operations auto-approved                                       │   │
│ │ • Only dangerous operations prompt                                    │   │
│ │ • System modifications prompt                                         │   │
│ └───────────────────────────────────────────────────────────────────────┘   │
│                                                                             │
├─────────────────────────────────────────────────────────────────────────────┤
│ > _                                                                         │
╰─────────────────────────────────────────────────────────────────────────────╯
"#
    .to_string()
}

fn mock_permission_yolo() -> String {
    r#"
╭─────────────────────────────────────────────────────────────────────────────╮
│ cortex                                                          [⚡ YOLO]  │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                             │
│ Permission Mode: YOLO (All Auto-Approved)                                   │
│                                                                             │
│ [WARN] WARNING: All tool executions will be automatically approved!         │
│                                                                             │
│ ┌─ Active Rules ────────────────────────────────────────────────────────┐   │
│ │ • ALL operations auto-approved                                        │   │
│ │ • No prompts for any tool execution                                   │   │
│ │ • USE WITH CAUTION                                                    │   │
│ └───────────────────────────────────────────────────────────────────────┘   │
│                                                                             │
├─────────────────────────────────────────────────────────────────────────────┤
│ > _                                                                         │
╰─────────────────────────────────────────────────────────────────────────────╯
"#
    .to_string()
}

fn mock_error_toast() -> String {
    r#"
╭─────────────────────────────────────────────────────────────────────────────╮
│                                                                             │
│  ┌─ [ERROR] ─────────────────────────────────────────────────────────────┐  │
│  │ Failed to connect to API: Connection refused                          │  │
│  │                                                                       │  │
│  │ Check your internet connection and try again.                         │  │
│  └───────────────────────────────────────────────────────────────────────┘  │
│                                                                             │
├─────────────────────────────────────────────────────────────────────────────┤
│ > _                                                                         │
╰─────────────────────────────────────────────────────────────────────────────╯
"#
    .to_string()
}

fn mock_error_streaming() -> String {
    r#"
╭─────────────────────────────────────────────────────────────────────────────╮
│ > Tell me about Rust                                                        │
│                                                                             │
│ Rust is a systems programming language that...                              │
│                                                                             │
│ [ERROR] Streaming Error: Connection reset by peer                           │
│                                                                             │
│    The response was interrupted. You can:                                   │
│    • Press Enter to retry                                                   │
│    • Type a new message                                                     │
│                                                                             │
├─────────────────────────────────────────────────────────────────────────────┤
│ > _                                                                         │
╰─────────────────────────────────────────────────────────────────────────────╯
"#
    .to_string()
}

fn mock_error_connection() -> String {
    r#"
╭─────────────────────────────────────────────────────────────────────────────╮
│                                                                             │
│                         [ERROR] Connection Error                            │
│                                                                             │
│              Unable to reach the Cortex API servers.                        │
│                                                                             │
│              Please check:                                                  │
│              • Your internet connection                                     │
│              • Firewall settings                                            │
│              • API endpoint configuration                                   │
│                                                                             │
│              Error: DNS resolution failed for api.cortex.ai                 │
│                                                                             │
│                        [Retry]    [Settings]                                │
│                                                                             │
╰─────────────────────────────────────────────────────────────────────────────╯
"#
    .to_string()
}

fn mock_warning_toast() -> String {
    r#"
╭─────────────────────────────────────────────────────────────────────────────╮
│                                                                             │
│  ┌─ [WARN] Warning ──────────────────────────────────────────────────────┐  │
│  │ Rate limit approaching: 85% of quota used                             │  │
│  │                                                                       │  │
│  │ Consider upgrading your plan for higher limits.                       │  │
│  └───────────────────────────────────────────────────────────────────────┘  │
│                                                                             │
├─────────────────────────────────────────────────────────────────────────────┤
│ > _                                                                         │
╰─────────────────────────────────────────────────────────────────────────────╯
"#
    .to_string()
}

fn mock_info_toast() -> String {
    r#"
╭─────────────────────────────────────────────────────────────────────────────╮
│                                                                             │
│  ┌─ [INFO] ──────────────────────────────────────────────────────────────┐  │
│  │ Session exported to ~/exports/session_2024-01-15.md                   │  │
│  └───────────────────────────────────────────────────────────────────────┘  │
│                                                                             │
├─────────────────────────────────────────────────────────────────────────────┤
│ > _                                                                         │
╰─────────────────────────────────────────────────────────────────────────────╯
"#
    .to_string()
}
