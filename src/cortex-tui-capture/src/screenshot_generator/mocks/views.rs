//! Mock content for view-related scenarios.

use crate::screenshot_generator::ScreenshotScenario;

/// Trait for view mock content generation.
pub trait ViewMocks {
    /// Create mock content for a view scenario.
    fn create_view_content(&self, scenario: &ScreenshotScenario) -> Option<String> {
        match scenario.id.as_str() {
            "empty_session" => Some(mock_empty_session()),
            "session_with_messages" => Some(mock_session_with_messages()),
            "help_view" => Some(mock_help_view()),
            "settings_view" => Some(mock_settings_view()),
            "message_user" => Some(mock_message_user()),
            "message_assistant" => Some(mock_message_assistant()),
            "message_long" => Some(mock_message_long()),
            "message_code_block" => Some(mock_message_code()),
            "message_markdown" => Some(mock_message_markdown()),
            _ => None,
        }
    }
}

fn mock_empty_session() -> String {
    r#"
╭─────────────────────────────────────────────────────────────────────────────╮
│                                                                             │
│                                                                             │
│          %%#*##*****###@@            Welcome to Cortex                      │
│      @%***+****+++#+**#%#@@@@        ─────────────────                      │
│    @%*++*%#*+*+*#+++##*%%++@@@@                                             │
│ @%***+**+++***+*#++*****%%@@%*%@@    Model: claude-sonnet-4-20250514        │
│ %++****+##++*++*##******%@*#@%#@@@                                          │
│@#*#*++++@#****+++++*##++##%%%*#@@@@  Type a message below to start.         │
│%*++*+++***+*#*+++**++++**%@%#*%@@@@  Press Ctrl+K for command palette.      │
│@**+**+**+#*#+++#*+###*#####@@@@@@@@  Press ? for help.                      │
│ ##@@%@@#+#**###%@@%%%%#####%@@@@@@@                                         │
│   @@@@ @@@@@#%@@@@@@%*+**#*#@@@@                                            │
│           @@@@   @@%%@%%%%%%@@@                                             │
│                     @##@@@@@@                                               │
│                                                                             │
├─────────────────────────────────────────────────────────────────────────────┤
│ > _                                                                         │
├─────────────────────────────────────────────────────────────────────────────┤
│ Enter submit · Ctrl+K palette · Ctrl+M model · ? help           [ASK MODE] │
╰─────────────────────────────────────────────────────────────────────────────╯
"#
    .to_string()
}

fn mock_session_with_messages() -> String {
    r#"
╭─────────────────────────────────────────────────────────────────────────────╮
│ Session: Project Analysis                                        v0.1.0    │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                             │
│ > How do I add a new command to the TUI?                                    │
│                                                                             │
│ To add a new command to the TUI, you need to:                               │
│                                                                             │
│ 1. Register the command in `registry.rs`:                                   │
│    ```rust                                                                  │
│    registry.register(CommandDef::new(                                       │
│        "your-command",                                                      │
│        &["alias"],                                                          │
│        "Description",                                                       │
│        "/your-command [args]",                                              │
│        CommandCategory::General,                                            │
│        true,                                                                │
│    ));                                                                      │
│    ```                                                                      │
│                                                                             │
│ 2. Add the handler in `executor.rs`                                         │
│                                                                             │
│ > Thanks! Can you show me an example?                                       │
│                                                                             │
├─────────────────────────────────────────────────────────────────────────────┤
│ > _                                                                         │
├─────────────────────────────────────────────────────────────────────────────┤
│ Enter submit · Ctrl+K palette · Ctrl+M model · ? help           [ASK MODE] │
╰─────────────────────────────────────────────────────────────────────────────╯
"#
    .to_string()
}

fn mock_help_view() -> String {
    r#"
╭─────────────────────────────────────────────────────────────────────────────╮
│ Help - Keyboard Shortcuts                                                   │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                             │
│ ══════════════════════════════════════════════════════════════════════════  │
│                            GLOBAL                                           │
│ ══════════════════════════════════════════════════════════════════════════  │
│  Ctrl+Q       Quit application                                              │
│  Ctrl+K       Open command palette                                          │
│  Ctrl+M       Switch model                                                  │
│  Ctrl+N       New session                                                   │
│  Ctrl+B       Toggle sidebar                                                │
│  ?            Show this help                                                │
│  Tab          Cycle permission mode                                         │
│                                                                             │
│ ══════════════════════════════════════════════════════════════════════════  │
│                            CHAT                                             │
│ ══════════════════════════════════════════════════════════════════════════  │
│  j/k          Scroll up/down                                                │
│  g/G          Jump to top/bottom                                            │
│  y            Copy selection                                                │
│  e            Toggle tool details                                           │
│                                                                             │
│                              [Press ? or Esc to close]                      │
╰─────────────────────────────────────────────────────────────────────────────╯
"#
    .to_string()
}

fn mock_settings_view() -> String {
    r#"
╭─────────────────────────────────────────────────────────────────────────────╮
│ Settings                                                                    │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                             │
│ ┌─ Display ─────────────────────────────────────────────────────────────┐   │
│ │  [x] Show timestamps on messages                                      │   │
│ │  [x] Show line numbers in code blocks                                 │   │
│ │  [ ] Compact mode                                                     │   │
│ │  [x] Syntax highlighting                                              │   │
│ └───────────────────────────────────────────────────────────────────────┘   │
│                                                                             │
│ ┌─ Model ───────────────────────────────────────────────────────────────┐   │
│ │  Provider:     cortex                                                 │   │
│ │  Model:        gpt-4                                                  │   │
│ │  Temperature:  0.7                                                    │   │
│ │  Max Tokens:   4096                                                   │   │
│ └───────────────────────────────────────────────────────────────────────┘   │
│                                                                             │
│ ┌─ Security ────────────────────────────────────────────────────────────┐   │
│ │  Permission Mode:  [ASK] Ask for each tool                            │   │
│ │  Sandbox:          Disabled                                           │   │
│ └───────────────────────────────────────────────────────────────────────┘   │
│                                                                             │
╰─────────────────────────────────────────────────────────────────────────────╯
"#
    .to_string()
}

fn mock_message_user() -> String {
    "> How do I implement a binary search in Rust?\n".to_string()
}

fn mock_message_assistant() -> String {
    r#"Here's how to implement binary search in Rust:

```rust
fn binary_search<T: Ord>(arr: &[T], target: &T) -> Option<usize> {
    let mut left = 0;
    let mut right = arr.len();
    
    while left < right {
        let mid = left + (right - left) / 2;
        match arr[mid].cmp(target) {
            Ordering::Less => left = mid + 1,
            Ordering::Greater => right = mid,
            Ordering::Equal => return Some(mid),
        }
    }
    None
}
```

This implementation has O(log n) time complexity."#
        .to_string()
}

fn mock_message_long() -> String {
    "Lorem ipsum dolor sit amet, consectetur adipiscing elit. Sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris nisi ut aliquip ex ea commodo consequat. Duis aute irure dolor in reprehenderit in voluptate velit esse cillum dolore eu fugiat nulla pariatur. Excepteur sint occaecat cupidatat non proident, sunt in culpa qui officia deserunt mollit anim id est laborum. Sed ut perspiciatis unde omnis iste natus error sit voluptatem accusantium doloremque laudantium, totam rem aperiam, eaque ipsa quae ab illo inventore veritatis et quasi architecto beatae vitae dicta sunt explicabo.".to_string()
}

fn mock_message_code() -> String {
    r#"Here's a code example:

```rust
fn main() {
    println!("Hello, world!");
}
```

And some Python:

```python
def greet(name):
    return f"Hello, {name}!"
```"#
        .to_string()
}

fn mock_message_markdown() -> String {
    r#"# Heading 1

This is a paragraph with **bold** and *italic* text.

## Lists

- Item 1
- Item 2
  - Nested item

1. Numbered
2. List

> This is a blockquote

`inline code` and [links](https://example.com)"#
        .to_string()
}
