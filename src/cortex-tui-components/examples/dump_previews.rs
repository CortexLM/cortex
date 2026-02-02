//! UI Component ASCII Preview Dumper
//!
//! This example generates ASCII art previews of all cortex-tui-components
//! and saves them to markdown files for documentation purposes.
//!
//! Run with: `cargo run -p cortex-tui-components --example dump_previews`

use cortex_tui_components::prelude::*;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::Widget;
use std::fs::{self, File};
use std::io::Write;
use std::path::Path;

/// Convert a buffer to ASCII art string
fn buffer_to_ascii(buf: &Buffer, area: Rect) -> String {
    let mut result = String::new();

    for y in area.y..area.bottom() {
        for x in area.x..area.right() {
            if let Some(cell) = buf.cell((x, y)) {
                let symbol = cell.symbol();
                if symbol.is_empty() || symbol == " " {
                    result.push(' ');
                } else {
                    result.push_str(symbol);
                }
            } else {
                result.push(' ');
            }
        }
        result.push('\n');
    }

    result
}

/// Render a component to ASCII art using Widget trait
fn render_to_ascii<W: Widget>(widget: W, width: u16, height: u16) -> String {
    let area = Rect::new(0, 0, width, height);
    let mut buf = Buffer::empty(area);
    widget.render(area, &mut buf);
    buffer_to_ascii(&buf, area)
}

/// Render a component that implements Component trait
fn render_component_to_ascii<C: Component>(component: &C, width: u16, height: u16) -> String {
    let area = Rect::new(0, 0, width, height);
    let mut buf = Buffer::empty(area);
    component.render(area, &mut buf);
    buffer_to_ascii(&buf, area)
}

/// Generate preview for Card component
fn preview_card() -> String {
    let card = Card::new()
        .title("Example Card")
        .focused(false)
        .key_hints(vec![("Enter", "Select"), ("Esc", "Close")]);

    let mut ascii = String::new();
    ascii.push_str("### Card Component\n\n");
    ascii.push_str("A bordered container with optional title and key hints.\n\n");
    ascii.push_str("**Normal Card:**\n");
    ascii.push_str("```\n");
    ascii.push_str(&render_to_ascii(card, 40, 8));
    ascii.push_str("```\n\n");

    let focused_card = Card::new()
        .title("Focused Card")
        .focused(true)
        .key_hints(vec![("Enter", "Select")]);

    ascii.push_str("**Focused Card:**\n");
    ascii.push_str("```\n");
    ascii.push_str(&render_to_ascii(focused_card, 40, 8));
    ascii.push_str("```\n\n");

    ascii
}

/// Generate preview for Modal component
fn preview_modal() -> String {
    let modal = Modal::new("Confirm Action")
        .width_percent(100)
        .height(8)
        .key_hints(vec![("Enter", "Confirm"), ("Esc", "Cancel")]);

    let mut ascii = String::new();
    ascii.push_str("### Modal Component\n\n");
    ascii.push_str("Overlay dialog for confirmations and input.\n\n");
    ascii.push_str("```\n");
    ascii.push_str(&render_to_ascii(modal, 50, 10));
    ascii.push_str("```\n\n");

    ascii
}

/// Generate preview for Dropdown component
fn preview_dropdown() -> String {
    let mut state = DropdownState::new();
    state.show(vec![
        DropdownItem::new("opt1", "Option 1", "First option"),
        DropdownItem::new("opt2", "Option 2", "Second option"),
        DropdownItem::new("opt3", "Option 3", "Third option"),
    ]);
    state.selected = 1;

    let dropdown = Dropdown::new(&state)
        .title("Select")
        .position(DropdownPosition::Below);

    let mut ascii = String::new();
    ascii.push_str("### Dropdown Component\n\n");
    ascii.push_str("Compact dropdown menu with selection.\n\n");
    ascii.push_str("```\n");
    ascii.push_str(&render_to_ascii(dropdown, 45, 7));
    ascii.push_str("```\n\n");

    ascii
}

/// Generate preview for TextInput component
fn preview_text_input() -> String {
    let state = InputState::new()
        .with_value("Hello World")
        .with_placeholder("Enter text...");

    let input = TextInput::new(&state).focused(true).label("Name:");

    let mut ascii = String::new();
    ascii.push_str("### TextInput Component\n\n");
    ascii.push_str("Single-line text input with cursor.\n\n");
    ascii.push_str("**Focused with value:**\n");
    ascii.push_str("```\n");
    ascii.push_str(&render_to_ascii(input, 40, 1));
    ascii.push_str("```\n\n");

    let empty_state = InputState::new().with_placeholder("Enter your name...");
    let empty_input = TextInput::new(&empty_state).focused(true);

    ascii.push_str("**Empty with placeholder:**\n");
    ascii.push_str("```\n");
    ascii.push_str(&render_to_ascii(empty_input, 40, 1));
    ascii.push_str("```\n\n");

    let password_state = InputState::new().with_value("secret123").masked();
    let password_input = TextInput::new(&password_state)
        .focused(true)
        .label("Password:");

    ascii.push_str("**Password (masked):**\n");
    ascii.push_str("```\n");
    ascii.push_str(&render_to_ascii(password_input, 40, 1));
    ascii.push_str("```\n\n");

    ascii
}

/// Generate preview for Panel component
fn preview_panel() -> String {
    let panel = Panel::new()
        .title("Side Panel")
        .position(PanelPosition::Left)
        .focused(true)
        .size(25);

    let mut ascii = String::new();
    ascii.push_str("### Panel Component\n\n");
    ascii.push_str("Resizable panel container for layouts.\n\n");
    ascii.push_str("```\n");
    ascii.push_str(&render_to_ascii(panel, 25, 10));
    ascii.push_str("```\n\n");

    ascii
}

/// Generate preview for Selector component
fn preview_selector() -> String {
    let items = vec![
        SelectItem::new("apple", "Apple").with_shortcut('a'),
        SelectItem::new("banana", "Banana")
            .with_shortcut('b')
            .current(),
        SelectItem::new("cherry", "Cherry")
            .with_shortcut('c')
            .default_item(),
        SelectItem::new("date", "Date").disabled(Some("Unavailable")),
        SelectItem::new("elderberry", "Elderberry"),
    ];

    let selector = Selector::new(items)
        .with_title("Select Fruit")
        .with_max_visible(10);

    let mut ascii = String::new();
    ascii.push_str("### Selector Component\n\n");
    ascii.push_str("Selection list with keyboard shortcuts and search.\n\n");
    ascii.push_str("```\n");
    ascii.push_str(&render_component_to_ascii(&selector, 45, 9));
    ascii.push_str("```\n\n");

    ascii
}

/// Generate preview for CheckboxGroup component
fn preview_checkbox() -> String {
    let group = CheckboxGroup::new(vec![
        CheckboxItem::new("opt1", "Enable notifications").checked(),
        CheckboxItem::new("opt2", "Dark mode"),
        CheckboxItem::new("opt3", "Auto-save").checked(),
        CheckboxItem::new("opt4", "Disabled option").disabled(),
    ]);

    let mut ascii = String::new();
    ascii.push_str("### CheckboxGroup Component\n\n");
    ascii.push_str("Multi-selection checkbox list.\n\n");
    ascii.push_str("```\n");
    ascii.push_str(&render_to_ascii(&group, 35, 4));
    ascii.push_str("```\n\n");

    ascii
}

/// Generate preview for RadioGroup component
fn preview_radio() -> String {
    let mut group = RadioGroup::new(vec![
        RadioItem::new("small", "Small"),
        RadioItem::new("medium", "Medium"),
        RadioItem::new("large", "Large"),
    ]);
    group.selected = 1;
    group.focused = 1;

    let mut ascii = String::new();
    ascii.push_str("### RadioGroup Component\n\n");
    ascii.push_str("Single-selection radio button list.\n\n");
    ascii.push_str("```\n");
    ascii.push_str(&render_to_ascii(&group, 25, 3));
    ascii.push_str("```\n\n");

    ascii
}

/// Generate preview for ScrollableList component
fn preview_list() -> String {
    let items: Vec<ListItem> = vec![
        ListItem::new("Item 1").with_secondary("Description"),
        ListItem::new("Item 2").with_secondary("Another desc"),
        ListItem::new("Item 3"),
        ListItem::new("Item 4").with_secondary("More info"),
        ListItem::new("Item 5"),
    ];

    let list = ScrollableList::new(items).with_visible_height(5);

    let mut ascii = String::new();
    ascii.push_str("### ScrollableList Component\n\n");
    ascii.push_str("Scrollable list with selection.\n\n");
    ascii.push_str("```\n");
    ascii.push_str(&render_to_ascii(&list, 40, 5));
    ascii.push_str("```\n\n");

    ascii
}

/// Generate preview for Toast component
fn preview_toast() -> String {
    let info_toast = Toast::info("Operation completed successfully");
    let success_toast = Toast::success("File saved!");
    let warning_toast = Toast::warning("Low disk space");
    let error_toast = Toast::error("Connection failed");

    let mut ascii = String::new();
    ascii.push_str("### Toast Component\n\n");
    ascii.push_str("Notification messages with severity levels.\n\n");

    ascii.push_str("**Info:**\n");
    ascii.push_str("```\n");
    ascii.push_str(&render_to_ascii(&info_toast, 45, 1));
    ascii.push_str("```\n\n");

    ascii.push_str("**Success:**\n");
    ascii.push_str("```\n");
    ascii.push_str(&render_to_ascii(&success_toast, 45, 1));
    ascii.push_str("```\n\n");

    ascii.push_str("**Warning:**\n");
    ascii.push_str("```\n");
    ascii.push_str(&render_to_ascii(&warning_toast, 45, 1));
    ascii.push_str("```\n\n");

    ascii.push_str("**Error:**\n");
    ascii.push_str("```\n");
    ascii.push_str(&render_to_ascii(&error_toast, 45, 1));
    ascii.push_str("```\n\n");

    ascii
}

/// Generate preview for Spinner component
fn preview_spinner() -> String {
    let mut ascii = String::new();
    ascii.push_str("### LoadingSpinner Component\n\n");
    ascii.push_str("Loading indicators with various styles.\n\n");

    let styles = [
        (SpinnerStyle::Breathing, "Breathing (default)"),
        (SpinnerStyle::HalfCircle, "HalfCircle (tools)"),
        (SpinnerStyle::Dots, "Dots"),
        (SpinnerStyle::Line, "Line"),
        (SpinnerStyle::Braille, "Braille"),
        (SpinnerStyle::Blocks, "Blocks"),
    ];

    for (style, name) in styles {
        let spinner = LoadingSpinner::new()
            .with_style(style)
            .with_label(format!("Loading ({})", name));

        ascii.push_str(&format!("**{}:**\n", name));
        ascii.push_str("```\n");
        ascii.push_str(&render_to_ascii(&spinner, 35, 1));
        ascii.push_str("```\n\n");
    }

    ascii
}

/// Generate preview for KeyHintsBar component
fn preview_key_hints() -> String {
    let hints = KeyHintsBar::new()
        .hint("↑↓", "Navigate")
        .hint("Enter", "Select")
        .hint("Esc", "Cancel")
        .hint("/", "Search");

    let mut ascii = String::new();
    ascii.push_str("### KeyHintsBar Component\n\n");
    ascii.push_str("Keyboard shortcut hints display.\n\n");
    ascii.push_str("```\n");
    ascii.push_str(&render_to_ascii(hints, 60, 1));
    ascii.push_str("```\n\n");

    ascii
}

/// Generate preview for Popup component
fn preview_popup() -> String {
    let popup = Popup::new(30, 5)
        .title("Autocomplete")
        .position(PopupPosition::Below);

    let mut ascii = String::new();
    ascii.push_str("### Popup Component\n\n");
    ascii.push_str("Inline popup for autocomplete and tooltips.\n\n");
    ascii.push_str("```\n");
    ascii.push_str(&render_to_ascii(popup, 30, 5));
    ascii.push_str("```\n\n");

    ascii
}

/// Generate preview for Form component
fn preview_form() -> String {
    let form = Form::new(
        "User Settings",
        vec![
            FormField::text("name", "Username")
                .required()
                .with_value("john_doe"),
            FormField::secret("password", "Password").required(),
            FormField::toggle("notifications", "Enable Notifications"),
            FormField::select(
                "theme",
                "Theme",
                vec!["Light".to_string(), "Dark".to_string(), "Auto".to_string()],
            ),
        ],
    );

    let mut ascii = String::new();
    ascii.push_str("### Form Component\n\n");
    ascii.push_str("Multi-field form with validation.\n\n");
    ascii.push_str("```\n");
    ascii.push_str(&render_component_to_ascii(&form, 50, 14));
    ascii.push_str("```\n\n");

    ascii
}

/// Generate preview for Border styles
fn preview_borders() -> String {
    let mut ascii = String::new();
    ascii.push_str("### Border Styles\n\n");
    ascii.push_str("Available border styles for components.\n\n");

    let styles = [
        (BorderStyle::Rounded, "Rounded (default)"),
        (BorderStyle::Single, "Single"),
        (BorderStyle::Double, "Double"),
        (BorderStyle::Ascii, "ASCII"),
    ];

    for (style, name) in styles {
        let border = RoundedBorder::new().title(name).style(style);

        ascii.push_str(&format!("**{}:**\n", name));
        ascii.push_str("```\n");
        ascii.push_str(&render_to_ascii(border, 25, 5));
        ascii.push_str("```\n\n");
    }

    ascii
}

fn main() -> std::io::Result<()> {
    let output_dir = Path::new("ui-previews");

    // Create output directory
    fs::create_dir_all(output_dir)?;

    println!("🎨 Generating UI Component Previews...\n");

    // Generate main preview file
    let mut content = String::new();
    content.push_str("# Cortex TUI Components - ASCII Previews\n\n");
    content.push_str("This document contains ASCII art previews of all UI components in the `cortex-tui-components` crate.\n\n");
    content.push_str("> Generated automatically by `cargo run -p cortex-tui-components --example dump_previews`\n\n");
    content.push_str("---\n\n");
    content.push_str("## Table of Contents\n\n");
    content.push_str("- [Card](#card-component)\n");
    content.push_str("- [Modal](#modal-component)\n");
    content.push_str("- [Dropdown](#dropdown-component)\n");
    content.push_str("- [TextInput](#textinput-component)\n");
    content.push_str("- [Panel](#panel-component)\n");
    content.push_str("- [Selector](#selector-component)\n");
    content.push_str("- [CheckboxGroup](#checkboxgroup-component)\n");
    content.push_str("- [RadioGroup](#radiogroup-component)\n");
    content.push_str("- [ScrollableList](#scrollablelist-component)\n");
    content.push_str("- [Toast](#toast-component)\n");
    content.push_str("- [LoadingSpinner](#loadingspinner-component)\n");
    content.push_str("- [KeyHintsBar](#keyhintsbar-component)\n");
    content.push_str("- [Popup](#popup-component)\n");
    content.push_str("- [Form](#form-component)\n");
    content.push_str("- [Border Styles](#border-styles)\n\n");
    content.push_str("---\n\n");

    // Add all component previews
    content.push_str(&preview_card());
    content.push_str("---\n\n");
    content.push_str(&preview_modal());
    content.push_str("---\n\n");
    content.push_str(&preview_dropdown());
    content.push_str("---\n\n");
    content.push_str(&preview_text_input());
    content.push_str("---\n\n");
    content.push_str(&preview_panel());
    content.push_str("---\n\n");
    content.push_str(&preview_selector());
    content.push_str("---\n\n");
    content.push_str(&preview_checkbox());
    content.push_str("---\n\n");
    content.push_str(&preview_radio());
    content.push_str("---\n\n");
    content.push_str(&preview_list());
    content.push_str("---\n\n");
    content.push_str(&preview_toast());
    content.push_str("---\n\n");
    content.push_str(&preview_spinner());
    content.push_str("---\n\n");
    content.push_str(&preview_key_hints());
    content.push_str("---\n\n");
    content.push_str(&preview_popup());
    content.push_str("---\n\n");
    content.push_str(&preview_form());
    content.push_str("---\n\n");
    content.push_str(&preview_borders());

    // Write main file
    let main_path = output_dir.join("COMPONENTS.md");
    let mut file = File::create(&main_path)?;
    file.write_all(content.as_bytes())?;

    println!("✅ Generated: {}", main_path.display());

    // Generate individual component files
    let components = vec![
        ("card.md", preview_card()),
        ("modal.md", preview_modal()),
        ("dropdown.md", preview_dropdown()),
        ("text_input.md", preview_text_input()),
        ("panel.md", preview_panel()),
        ("selector.md", preview_selector()),
        ("checkbox.md", preview_checkbox()),
        ("radio.md", preview_radio()),
        ("list.md", preview_list()),
        ("toast.md", preview_toast()),
        ("spinner.md", preview_spinner()),
        ("key_hints.md", preview_key_hints()),
        ("popup.md", preview_popup()),
        ("form.md", preview_form()),
        ("borders.md", preview_borders()),
    ];

    for (filename, content) in components {
        let path = output_dir.join(filename);
        let mut file = File::create(&path)?;
        file.write_all(content.as_bytes())?;
        println!("✅ Generated: {}", path.display());
    }

    // Generate README for the previews directory
    let readme = r#"# UI Previews

This directory contains ASCII art previews of all Cortex TUI components.

## Main Documentation

- **[COMPONENTS.md](COMPONENTS.md)** - Complete component gallery with all previews

## Individual Components

| Component | File |
|-----------|------|
| Card | [card.md](card.md) |
| Modal | [modal.md](modal.md) |
| Dropdown | [dropdown.md](dropdown.md) |
| TextInput | [text_input.md](text_input.md) |
| Panel | [panel.md](panel.md) |
| Selector | [selector.md](selector.md) |
| CheckboxGroup | [checkbox.md](checkbox.md) |
| RadioGroup | [radio.md](radio.md) |
| ScrollableList | [list.md](list.md) |
| Toast | [toast.md](toast.md) |
| LoadingSpinner | [spinner.md](spinner.md) |
| KeyHintsBar | [key_hints.md](key_hints.md) |
| Popup | [popup.md](popup.md) |
| Form | [form.md](form.md) |
| Border Styles | [borders.md](borders.md) |

## Regenerating Previews

To regenerate these previews after making changes to components:

```bash
cargo run -p cortex-tui-components --example dump_previews
```

This will update all markdown files in this directory with fresh ASCII renders.
"#;

    let readme_path = output_dir.join("README.md");
    let mut file = File::create(&readme_path)?;
    file.write_all(readme.as_bytes())?;
    println!("✅ Generated: {}", readme_path.display());

    println!("\n🎉 All previews generated successfully!");
    println!("📁 Output directory: {}/", output_dir.display());

    Ok(())
}
