//! Tests for minimal session view.

#[cfg(test)]
mod tests {
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::widgets::Widget;

    use cortex_core::widgets::MessageRole;

    use crate::app::AppState;
    use crate::views::minimal_session::{ChatMessage, MinimalSessionView};

    fn create_test_buffer(width: u16, height: u16) -> Buffer {
        Buffer::empty(Rect::new(0, 0, width, height))
    }

    fn create_test_app_state() -> AppState {
        AppState::default()
    }

    #[test]
    fn test_chat_message_user() {
        let msg = ChatMessage::user("Hello");
        assert_eq!(msg.role, MessageRole::User);
        assert_eq!(msg.content, "Hello");
        assert!(!msg.is_streaming);
    }

    #[test]
    fn test_chat_message_assistant() {
        let msg = ChatMessage::assistant("Hi there");
        assert_eq!(msg.role, MessageRole::Assistant);
        assert_eq!(msg.content, "Hi there");
    }

    #[test]
    fn test_chat_message_streaming() {
        let msg = ChatMessage::assistant("Working...").streaming();
        assert!(msg.is_streaming);
    }

    #[test]
    fn test_chat_message_tool() {
        let msg = ChatMessage::tool("read_file", "Contents here");
        assert_eq!(msg.role, MessageRole::Tool);
        assert_eq!(msg.tool_name, Some("read_file".to_string()));
    }

    #[test]
    fn test_minimal_session_view_new() {
        let state = create_test_app_state();
        let _view = MinimalSessionView::new(&state);
        // View created successfully
    }

    #[test]
    fn test_minimal_session_view_render() {
        let state = create_test_app_state();
        let view = MinimalSessionView::new(&state);

        let mut buf = create_test_buffer(80, 24);
        let area = Rect::new(0, 0, 80, 24);
        view.render(area, &mut buf);

        // Check that something was rendered somewhere in the buffer
        // This is a basic sanity check that the view renders without panic
        // and produces some output
        let mut has_content = false;
        for y in 0..24 {
            for x in 0..80 {
                let symbol = buf[(x, y)].symbol();
                if !symbol.trim().is_empty() && symbol != " " {
                    has_content = true;
                    break;
                }
            }
            if has_content {
                break;
            }
        }
        // The view should render something
        assert!(has_content, "View should render some content");
    }

    #[test]
    #[ignore = "TUI behavior differs across platforms"]
    fn test_cursor_position() {
        let state = create_test_app_state();
        let view = MinimalSessionView::new(&state);

        let input_area = Rect::new(0, 20, 80, 1);
        let cursor = view.cursor_position(input_area);

        assert!(cursor.is_some());
        let (x, y) = cursor.unwrap();
        // x should be 0 + 2 ("> ") + cursor_pos
        assert_eq!(x, 2); // Empty input, cursor at position 0
        assert_eq!(y, 20);
    }
}
