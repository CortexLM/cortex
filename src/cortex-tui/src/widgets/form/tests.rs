//! Tests for form widgets.

#[cfg(test)]
mod tests {
    use crate::widgets::form::{
        field::FormField, field_kind::FieldKind, modal::FormModal, state::FormState,
        utils::grapheme_count,
    };
    use ratatui::prelude::Rect;

    #[test]
    fn test_field_builders() {
        let text_field = FormField::text("username", "Username")
            .required()
            .with_placeholder("Enter username");
        assert_eq!(text_field.key, "username");
        assert_eq!(text_field.label, "Username");
        assert!(text_field.required);
        assert_eq!(text_field.placeholder, Some("Enter username".to_string()));

        let secret_field = FormField::secret("password", "Password");
        assert!(matches!(secret_field.kind, FieldKind::Secret));

        let number_field = FormField::number("age", "Age").with_value("25");
        assert!(matches!(number_field.kind, FieldKind::Number));
        assert_eq!(number_field.value, "25");

        let toggle_field = FormField::toggle("enabled", "Enabled");
        assert!(matches!(toggle_field.kind, FieldKind::Toggle));

        let select_field = FormField::select(
            "color",
            "Color",
            vec!["Red".to_string(), "Green".to_string(), "Blue".to_string()],
        );
        assert!(matches!(select_field.kind, FieldKind::Select(_)));
    }

    #[test]
    fn test_form_state_navigation() {
        let fields = vec![
            FormField::text("field1", "Field 1"),
            FormField::text("field2", "Field 2"),
            FormField::text("field3", "Field 3"),
        ];
        let mut state = FormState::new("Test Form", "test_command", fields);

        assert_eq!(state.focus_index, 0);

        state.focus_next();
        assert_eq!(state.focus_index, 1);

        state.focus_next();
        assert_eq!(state.focus_index, 2);

        state.focus_next();
        assert_eq!(state.focus_index, 3); // Submit button

        state.focus_next();
        assert_eq!(state.focus_index, 0); // Wrap around

        state.focus_prev();
        assert_eq!(state.focus_index, 3); // Submit button

        state.focus_prev();
        assert_eq!(state.focus_index, 2);
    }

    #[test]
    fn test_form_state_text_input() {
        let fields = vec![FormField::text("name", "Name")];
        let mut state = FormState::new("Test", "cmd", fields);

        state.handle_char('H');
        state.handle_char('i');
        assert_eq!(state.fields[0].value, "Hi");
        assert_eq!(state.fields[0].cursor_pos, 2);

        state.handle_backspace();
        assert_eq!(state.fields[0].value, "H");
        assert_eq!(state.fields[0].cursor_pos, 1);

        state.handle_left();
        assert_eq!(state.fields[0].cursor_pos, 0);

        state.handle_right();
        assert_eq!(state.fields[0].cursor_pos, 1);
    }

    #[test]
    fn test_form_state_toggle() {
        let fields = vec![FormField::toggle("enabled", "Enabled")];
        let mut state = FormState::new("Test", "cmd", fields);

        assert!(!state.fields[0].toggle_state);

        state.toggle_current();
        assert!(state.fields[0].toggle_state);

        state.toggle_current();
        assert!(!state.fields[0].toggle_state);
    }

    #[test]
    fn test_form_state_select() {
        let options = vec!["A".to_string(), "B".to_string(), "C".to_string()];
        let fields = vec![FormField::select("opt", "Option", options)];
        let mut state = FormState::new("Test", "cmd", fields);

        assert_eq!(state.fields[0].select_index, 0);

        state.handle_right();
        assert_eq!(state.fields[0].select_index, 1);

        state.handle_right();
        assert_eq!(state.fields[0].select_index, 2);

        state.handle_right();
        assert_eq!(state.fields[0].select_index, 0); // Wrap around

        state.handle_left();
        assert_eq!(state.fields[0].select_index, 2);
    }

    #[test]
    fn test_number_field_validation() {
        let fields = vec![FormField::number("count", "Count")];
        let mut state = FormState::new("Test", "cmd", fields);

        state.handle_char('1');
        state.handle_char('2');
        state.handle_char('3');
        assert_eq!(state.fields[0].value, "123");

        // Letters should be ignored
        state.handle_char('a');
        assert_eq!(state.fields[0].value, "123");

        // Decimal point should work
        state.handle_char('.');
        state.handle_char('5');
        assert_eq!(state.fields[0].value, "123.5");
    }

    #[test]
    fn test_centered_rect() {
        let area = Rect::new(0, 0, 100, 50);
        let centered = FormModal::centered_rect(area, 50, 50);

        assert_eq!(centered.width, 50);
        assert_eq!(centered.height, 25);
        assert_eq!(centered.x, 25);
        assert_eq!(centered.y, 12);
    }

    #[test]
    fn test_handle_paste_text_field() {
        let fields = vec![FormField::text("api_key", "API Key")];
        let mut state = FormState::new("Test", "cmd", fields);

        state.handle_paste("sk-abc123");
        assert_eq!(state.fields[0].value, "sk-abc123");
        assert_eq!(state.fields[0].cursor_pos, 9);

        // Paste more at cursor position
        state.handle_paste("-xyz");
        assert_eq!(state.fields[0].value, "sk-abc123-xyz");
        assert_eq!(state.fields[0].cursor_pos, 13);
    }

    #[test]
    fn test_handle_paste_secret_field() {
        let fields = vec![FormField::secret("password", "Password")];
        let mut state = FormState::new("Test", "cmd", fields);

        state.handle_paste("secret123");
        assert_eq!(state.fields[0].value, "secret123");
        assert_eq!(state.fields[0].cursor_pos, 9);
    }

    #[test]
    fn test_handle_paste_number_field_filters_invalid() {
        let fields = vec![FormField::number("port", "Port")];
        let mut state = FormState::new("Test", "cmd", fields);

        // Paste mixed content - only digits should be kept
        state.handle_paste("abc123def");
        assert_eq!(state.fields[0].value, "123");
        assert_eq!(state.fields[0].cursor_pos, 3);
    }

    #[test]
    fn test_handle_paste_does_nothing_for_toggle() {
        let fields = vec![FormField::toggle("enabled", "Enabled")];
        let mut state = FormState::new("Test", "cmd", fields);

        state.handle_paste("some text");
        assert_eq!(state.fields[0].value, "");
        assert!(!state.fields[0].toggle_state);
    }

    #[test]
    fn test_can_submit_with_no_required_fields() {
        let fields = vec![
            FormField::text("name", "Name"),
            FormField::text("email", "Email"),
        ];
        let state = FormState::new("Test", "cmd", fields);

        // Should be submittable even with empty optional fields
        assert!(state.can_submit());
    }

    #[test]
    fn test_can_submit_with_required_field_empty() {
        let fields = vec![
            FormField::text("name", "Name").required(),
            FormField::text("email", "Email"),
        ];
        let state = FormState::new("Test", "cmd", fields);

        // Required field is empty, should NOT be submittable
        assert!(!state.can_submit());
    }

    #[test]
    fn test_can_submit_with_required_field_filled() {
        let fields = vec![
            FormField::text("name", "Name")
                .required()
                .with_value("John"),
            FormField::text("email", "Email"),
        ];
        let state = FormState::new("Test", "cmd", fields);

        // Required field is filled, should be submittable
        assert!(state.can_submit());
    }

    #[test]
    fn test_can_submit_with_whitespace_only_required_field() {
        let fields = vec![FormField::text("name", "Name").required().with_value("   ")];
        let state = FormState::new("Test", "cmd", fields);

        // Whitespace-only values should NOT count as filled
        assert!(!state.can_submit());
    }

    #[test]
    fn test_can_submit_with_required_toggle_field() {
        let fields = vec![FormField::toggle("enabled", "Enabled").required()];
        let state = FormState::new("Test", "cmd", fields);

        // Toggle fields always have a valid state (ON or OFF)
        assert!(state.can_submit());
    }

    #[test]
    fn test_can_submit_with_required_select_field() {
        let fields =
            vec![FormField::select("color", "Color", vec!["red".into(), "blue".into()]).required()];
        let state = FormState::new("Test", "cmd", fields);

        // Select fields always have a valid selection (first option by default)
        assert!(state.can_submit());
    }

    #[test]
    fn test_can_submit_goto_form() {
        // Simulates the /goto command form with an empty required number field
        let fields = vec![
            FormField::number("message_number", "Message Number")
                .required()
                .with_placeholder("Enter message number..."),
        ];
        let state = FormState::new("Go To Message", "goto", fields);

        // Empty required field - should NOT be submittable
        assert!(!state.can_submit());
    }

    #[test]
    fn test_can_submit_goto_form_with_value() {
        // Simulates the /goto command form with a filled required number field
        let fields = vec![
            FormField::number("message_number", "Message Number")
                .required()
                .with_value("5"),
        ];
        let state = FormState::new("Go To Message", "goto", fields);

        // Required field is filled - should be submittable
        assert!(state.can_submit());
    }

    #[test]
    fn test_emoji_input_and_backspace() {
        // Test that emoji characters (multi-byte graphemes) are handled correctly
        let fields = vec![FormField::text("message", "Message")];
        let mut state = FormState::new("Test", "cmd", fields);

        // Type some text with emoji
        state.handle_char('H');
        state.handle_char('i');
        state.handle_char(' ');
        // Insert emoji character by character (as typed)
        for c in "🎉".chars() {
            state.handle_char(c);
        }
        state.handle_char('!');

        assert_eq!(state.fields[0].value, "Hi 🎉!");
        // Cursor should be at grapheme position 5 (H, i, space, 🎉, !)
        assert_eq!(state.fields[0].cursor_pos, 5);

        // Backspace should delete the '!' (single character)
        state.handle_backspace();
        assert_eq!(state.fields[0].value, "Hi 🎉");
        assert_eq!(state.fields[0].cursor_pos, 4);

        // Backspace should delete the entire emoji (one grapheme)
        state.handle_backspace();
        assert_eq!(state.fields[0].value, "Hi ");
        assert_eq!(state.fields[0].cursor_pos, 3);
    }

    #[test]
    fn test_compound_emoji_deletion() {
        // Test compound emoji like flag or skin tone emoji
        let fields = vec![FormField::text("message", "Message")];
        let mut state = FormState::new("Test", "cmd", fields);

        // Paste a compound emoji (family emoji - multiple code points)
        state.handle_paste("Hello 👨‍👩‍👧!");

        // The family emoji is one grapheme cluster
        let expected_graphemes = grapheme_count("Hello 👨‍👩‍👧!");
        assert_eq!(state.fields[0].cursor_pos, expected_graphemes);

        // Backspace should delete '!'
        state.handle_backspace();
        assert_eq!(state.fields[0].value, "Hello 👨‍👩‍👧");

        // Backspace should delete the entire family emoji (one grapheme)
        state.handle_backspace();
        assert_eq!(state.fields[0].value, "Hello ");
    }

    #[test]
    fn test_cursor_movement_with_emoji() {
        let fields = vec![FormField::text("message", "Message").with_value("A🎉B")];
        let mut state = FormState::new("Test", "cmd", fields);

        // Cursor starts at end (position 3: A, 🎉, B)
        assert_eq!(state.fields[0].cursor_pos, 3);

        // Move left once (to after 🎉)
        state.handle_left();
        assert_eq!(state.fields[0].cursor_pos, 2);

        // Move left again (to after A)
        state.handle_left();
        assert_eq!(state.fields[0].cursor_pos, 1);

        // Move left again (to beginning)
        state.handle_left();
        assert_eq!(state.fields[0].cursor_pos, 0);

        // Move right (to after A)
        state.handle_right();
        assert_eq!(state.fields[0].cursor_pos, 1);

        // Insert at this position
        state.handle_char('X');
        assert_eq!(state.fields[0].value, "AX🎉B");
        assert_eq!(state.fields[0].cursor_pos, 2);
    }
}
