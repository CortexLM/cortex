//! Mock content for input, scroll, and animation scenarios.

use crate::screenshot_generator::ScreenshotScenario;

/// Trait for input mock content generation.
pub trait InputMocks {
    /// Create mock content for an input scenario.
    fn create_input_content(&self, scenario: &ScreenshotScenario) -> Option<String> {
        match scenario.id.as_str() {
            // Input
            "input_empty" => Some(mock_input_empty()),
            "input_with_text" => Some(mock_input_text()),
            "input_multiline" => Some(mock_input_multiline()),
            "input_with_cursor" => Some(mock_input_cursor()),
            "input_command" => Some(mock_input_command()),
            // Scroll
            "scroll_top" => Some(mock_scroll_top()),
            "scroll_bottom" => Some(mock_scroll_bottom()),
            "scroll_middle" => Some(mock_scroll_middle()),
            "scrollbar_visible" => Some(mock_scrollbar_visible()),
            // Animations
            "spinner_frame_1" => Some(mock_spinner_frame(1)),
            "spinner_frame_2" => Some(mock_spinner_frame(2)),
            "spinner_frame_3" => Some(mock_spinner_frame(3)),
            "brain_pulse" => Some(mock_brain_pulse()),
            "typewriter_effect" => Some(mock_typewriter()),
            _ => None,
        }
    }
}

fn mock_input_empty() -> String {
    r#"
├─────────────────────────────────────────────────────────────────────────────┤
│ > _                                                                         │
├─────────────────────────────────────────────────────────────────────────────┤
│ Type a message or /command                                                  │
╰─────────────────────────────────────────────────────────────────────────────╯
"#
    .to_string()
}

fn mock_input_text() -> String {
    r#"
├─────────────────────────────────────────────────────────────────────────────┤
│ > How do I implement a binary search?_                                      │
├─────────────────────────────────────────────────────────────────────────────┤
│ Enter to submit · Shift+Enter for newline                                   │
╰─────────────────────────────────────────────────────────────────────────────╯
"#
    .to_string()
}

fn mock_input_multiline() -> String {
    r#"
├─────────────────────────────────────────────────────────────────────────────┤
│ > Here is my code:                                                          │
│   ```rust                                                                   │
│   fn main() {                                                               │
│       println!("Hello");                                                    │
│   }                                                                         │
│   ```                                                                       │
│   Can you review it?_                                                       │
├─────────────────────────────────────────────────────────────────────────────┤
│ Enter to submit · Shift+Enter for newline                                   │
╰─────────────────────────────────────────────────────────────────────────────╯
"#
    .to_string()
}

fn mock_input_cursor() -> String {
    r#"
├─────────────────────────────────────────────────────────────────────────────┤
│ > How do I impl█ment this?                                                  │
│                 ↑                                                           │
│              cursor                                                         │
├─────────────────────────────────────────────────────────────────────────────┤
│ ←→ move cursor · Ctrl+A start · Ctrl+E end                                  │
╰─────────────────────────────────────────────────────────────────────────────╯
"#
    .to_string()
}

fn mock_input_command() -> String {
    r#"
├─────────────────────────────────────────────────────────────────────────────┤
│ > /mod_                                                                     │
│                                                                             │
│   ┌─ Commands ──────────────────────────────────────────────────────────┐   │
│   │ > /model     - Switch to a model                                    │   │
│   │   /models    - List available models                                │   │
│   └─────────────────────────────────────────────────────────────────────┘   │
├─────────────────────────────────────────────────────────────────────────────┤
│ Tab to complete · Enter to select                                           │
╰─────────────────────────────────────────────────────────────────────────────╯
"#
    .to_string()
}

fn mock_scroll_top() -> String {
    r#"
╭─────────────────────────────────────────────────────────────────────────────╮
│ ▲ TOP OF CONVERSATION                                                    ▲  │
│ ─────────────────────────────────────────────────────────────────────────── │
│                                                                             │
│ > Hello, I need help with Rust                                      [10:00] │
│                                                                             │
│ Hi! I'd be happy to help with Rust. What would you like to know?    [10:00] │
│                                                                             │
│ > How do I handle errors?                                           [10:01] │
│                                                                             │
│ In Rust, error handling is done using the Result type...            [10:01] │
│                                                                             │
│                                                    ↓ Scroll for more (g/G) │
├─────────────────────────────────────────────────────────────────────────────┤
│ > _                                                                         │
╰─────────────────────────────────────────────────────────────────────────────╯
"#
    .to_string()
}

fn mock_scroll_bottom() -> String {
    r#"
╭─────────────────────────────────────────────────────────────────────────────╮
│                                                    ↑ Scroll for more (g/G) │
│                                                                             │
│ > What about async/await?                                           [10:15] │
│                                                                             │
│ Rust's async/await syntax allows writing asynchronous code that     [10:15] │
│ looks synchronous. Here's how it works...                                   │
│                                                                             │
│ > Thanks, that helps!                                               [10:16] │
│                                                                             │
│ You're welcome! Let me know if you have more questions.             [10:16] │
│                                                                             │
│ ─────────────────────────────────────────────────────────────────────────── │
│ ▼ END OF CONVERSATION                                                    ▼  │
├─────────────────────────────────────────────────────────────────────────────┤
│ > _                                                                         │
╰─────────────────────────────────────────────────────────────────────────────╯
"#
    .to_string()
}

fn mock_scroll_middle() -> String {
    r#"
╭─────────────────────────────────────────────────────────────────────────────╮
│                                                    ↑ Scroll for more (g/G) │
│                                                                             │
│ > How do I handle errors?                                           [10:01] │
│                                                                             │
│ In Rust, error handling is done using the Result type. The Result   [10:01] │
│ enum has two variants: Ok(value) for success and Err(error) for            │
│ failures...                                                                 │
│                                                                             │
│ > Can you show me an example?                                       [10:05] │
│                                                                             │
│ Sure! Here's a simple example...                                    [10:05] │
│                                                                             │
│                                                    ↓ Scroll for more (g/G) │
├─────────────────────────────────────────────────────────────────────────────┤
│ > _                                                                         │
╰─────────────────────────────────────────────────────────────────────────────╯
"#
    .to_string()
}

fn mock_scrollbar_visible() -> String {
    r#"
╭─────────────────────────────────────────────────────────────────────────────┤
│ > Hello!                                                                   █│
│                                                                            █│
│ Hi! How can I help you today?                                              ▓│
│                                                                             │
│ > Can you explain Rust's ownership?                                         │
│                                                                             │
│ Of course! Rust's ownership system is one of its most unique               │
│ features. It consists of three main rules:                                  │
│                                                                             │
│ 1. Each value has an owner                                                  │
│ 2. There can only be one owner at a time                                   │
│ 3. When the owner goes out of scope, the value is dropped                  │
│                                                                            ░│
├─────────────────────────────────────────────────────────────────────────────┤
│ > _                                                                         │
╰─────────────────────────────────────────────────────────────────────────────╯

Scrollbar: █ = thumb position, ▓ = partial, ░ = track
"#
    .to_string()
}

fn mock_spinner_frame(frame: u8) -> String {
    // Cortex TUI uses "breathing" spinner: · ✢ ✻ ✽ ✻ ✢ (ping-pong)
    let spinners = ["·", "✢", "✻", "✽", "✻", "✢"];
    let spinner = spinners[((frame as usize - 1) * 2) % spinners.len()];
    format!(
        r#"
╭─────────────────────────────────────────────────────────────────────────────╮
│ > Analyze the codebase                                                      │
│                                                                             │
│ {} Working... (frame {})                                                    │
│                                                                             │
│                                                                             │
├─────────────────────────────────────────────────────────────────────────────┤
│ > _                                                                         │
╰─────────────────────────────────────────────────────────────────────────────╯

Breathing spinner frames: · ✢ ✻ ✽ ✻ ✢ (ping-pong pattern)
Current: {} (frame {}/6)
"#,
        spinner, frame, spinner, frame
    )
}

fn mock_brain_pulse() -> String {
    r#"
╭─────────────────────────────────────────────────────────────────────────────╮
│                                                                             │
│          %%#*##*****###@@            ✽ Thinking...                          │
│      @%***+****+++#+**#%#@@@@                                               │
│    @%*++*%#*+*+*#+++##*%%++@@@@      The brain ASCII art pulses with a      │
│ @%***+**+++***+*#++*****%%@@%*%@@    radial wave animation.                 │
│ %++****+##++*++*##******%@*#@%#@@@                                          │
│@#*#*++++@#****+++++*##++##%%%*#@@@@  Characters cycle through:              │
│%*++*+++***+*#*+++**++++**%@%#*%@@@@    . : - = + * #                        │
│@**+**+**+#*#+++#*+###*#####@@@@@@@@                                         │
│ ##@@%@@#+#**###%@@%%%%#####%@@@@@@@  Wave expands from center creating      │
│   @@@@ @@@@@#%@@@@@@%*+**#*#@@@@     a "neural activity" effect.            │
│           @@@@   @@%%@%%%%%%@@@                                             │
│                     @##@@@@@@                                               │
│                                                                             │
├─────────────────────────────────────────────────────────────────────────────┤
│ > _                                                                         │
╰─────────────────────────────────────────────────────────────────────────────╯
"#
    .to_string()
}

fn mock_typewriter() -> String {
    r#"
╭─────────────────────────────────────────────────────────────────────────────╮
│ > Tell me about Rust                                                        │
│                                                                             │
│ Rust is a systems prog█                                                     │
│                        ↑                                                    │
│                   typewriter cursor                                         │
│                                                                             │
│ Text appears character by character with a blinking cursor,                 │
│ simulating typing. Speed adapts to streaming rate.                          │
│                                                                             │
│ Fast stream: characters appear quickly                                      │
│ Slow stream: more visible typing effect                                     │
│                                                                             │
├─────────────────────────────────────────────────────────────────────────────┤
│ > _                                                                         │
╰─────────────────────────────────────────────────────────────────────────────╯
"#
    .to_string()
}
