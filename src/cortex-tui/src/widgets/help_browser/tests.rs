//! Tests for help browser module.

#[cfg(test)]
mod tests {
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::widgets::Widget;

    use crate::widgets::help_browser::content::{HelpContent, HelpSection, get_help_sections};
    use crate::widgets::help_browser::render::HelpBrowser;
    use crate::widgets::help_browser::state::{HelpBrowserState, HelpFocus};
    use crate::widgets::help_browser::utils::wrap_text;

    fn create_test_buffer(width: u16, height: u16) -> Buffer {
        Buffer::empty(Rect::new(0, 0, width, height))
    }

    #[test]
    fn test_help_section_new() {
        let section = HelpSection::new("test-id", "Test Title");
        assert_eq!(section.id, "test-id");
        assert_eq!(section.title, "Test Title");
        assert!(section.content.is_empty());
    }

    #[test]
    fn test_help_section_with_content() {
        let section = HelpSection::new("test", "Test")
            .with_content(vec![HelpContent::Title("Hello".to_string())]);
        assert_eq!(section.content.len(), 1);
    }

    #[test]
    fn test_help_browser_state_new() {
        let state = HelpBrowserState::new();
        assert!(!state.sections.is_empty());
        assert_eq!(state.selected_section, 0);
        assert_eq!(state.content_scroll, 0);
        assert!(!state.search_mode);
        assert_eq!(state.focus, HelpFocus::Sidebar);
    }

    #[test]
    fn test_help_browser_state_with_topic() {
        let state = HelpBrowserState::new().with_topic(Some("keyboard"));
        assert_eq!(state.current_section().id, "keyboard");
    }

    #[test]
    fn test_help_browser_state_with_invalid_topic() {
        let state = HelpBrowserState::new().with_topic(Some("nonexistent"));
        assert_eq!(state.selected_section, 0);
    }

    #[test]
    fn test_select_navigation() {
        let mut state = HelpBrowserState::new();
        let initial = state.selected_section;
        let total_sections = state.sections.len();

        state.select_next();
        assert_eq!(state.selected_section, initial + 1);

        state.select_prev();
        assert_eq!(state.selected_section, initial);

        // Wrap around to last item when at 0
        state.select_prev();
        assert_eq!(state.selected_section, total_sections - 1);
    }

    #[test]
    fn test_scroll() {
        let mut state = HelpBrowserState::new();

        state.scroll_down();
        assert_eq!(state.content_scroll, 1);

        state.scroll_down();
        state.scroll_down();
        assert_eq!(state.content_scroll, 3);

        state.scroll_up();
        assert_eq!(state.content_scroll, 2);

        // Can't go below 0
        state.content_scroll = 0;
        state.scroll_up();
        assert_eq!(state.content_scroll, 0);
    }

    #[test]
    fn test_page_scroll() {
        let mut state = HelpBrowserState::new();

        state.page_down(10);
        assert_eq!(state.content_scroll, 10);

        state.page_up(5);
        assert_eq!(state.content_scroll, 5);

        state.page_up(10);
        assert_eq!(state.content_scroll, 0);
    }

    #[test]
    fn test_toggle_focus() {
        let mut state = HelpBrowserState::new();
        assert_eq!(state.focus, HelpFocus::Sidebar);

        state.toggle_focus();
        assert_eq!(state.focus, HelpFocus::Content);

        state.toggle_focus();
        assert_eq!(state.focus, HelpFocus::Sidebar);
    }

    #[test]
    fn test_toggle_search() {
        let mut state = HelpBrowserState::new();
        assert!(!state.search_mode);

        state.toggle_search();
        assert!(state.search_mode);
        assert_eq!(state.focus, HelpFocus::Search);

        state.toggle_search();
        assert!(!state.search_mode);
        assert_eq!(state.focus, HelpFocus::Sidebar);
    }

    #[test]
    fn test_search_input() {
        let mut state = HelpBrowserState::new();
        state.toggle_search();

        state.search_input('h');
        state.search_input('e');
        state.search_input('l');
        state.search_input('p');
        assert_eq!(state.search_query, "help");

        state.search_backspace();
        assert_eq!(state.search_query, "hel");
    }

    #[test]
    fn test_wrap_text() {
        let text = "hello world foo bar";
        let lines = wrap_text(text, 15);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "hello world foo");
        assert_eq!(lines[1], "bar");
    }

    #[test]
    fn test_wrap_text_long_word() {
        let text = "superlongwordthatexceedswidth";
        let lines = wrap_text(text, 10);
        // Word is split at width boundaries
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], "superlongw");
        assert_eq!(lines[1], "ordthatexc");
        assert_eq!(lines[2], "eedswidth");
    }

    #[test]
    fn test_wrap_text_empty() {
        let lines = wrap_text("", 10);
        assert!(lines.is_empty());

        let lines = wrap_text("hello", 0);
        assert!(lines.is_empty());
    }

    #[test]
    fn test_get_help_sections() {
        let sections = get_help_sections();
        assert!(!sections.is_empty());

        // Check that expected sections exist
        let ids: Vec<&str> = sections.iter().map(|s| s.id).collect();
        assert!(ids.contains(&"getting-started"));
        assert!(ids.contains(&"keyboard"));
        assert!(ids.contains(&"commands"));
        assert!(ids.contains(&"models"));
        assert!(ids.contains(&"tools"));
        assert!(ids.contains(&"mcp"));
        assert!(ids.contains(&"faq"));
    }

    #[test]
    fn test_help_browser_render() {
        let state = HelpBrowserState::new();
        let widget = HelpBrowser::new(&state);
        let mut buf = create_test_buffer(80, 40);
        let area = Rect::new(0, 0, 80, 40);

        // Should not panic
        widget.render(area, &mut buf);
    }

    #[test]
    fn test_help_browser_render_small_area() {
        let state = HelpBrowserState::new();
        let widget = HelpBrowser::new(&state);
        let mut buf = create_test_buffer(10, 5);
        let area = Rect::new(0, 0, 10, 5);

        // Should not panic, should just return early
        widget.render(area, &mut buf);
    }

    #[test]
    fn test_help_focus_default() {
        let focus = HelpFocus::default();
        assert_eq!(focus, HelpFocus::Sidebar);
    }

    #[test]
    fn test_current_section() {
        let mut state = HelpBrowserState::new();
        assert_eq!(state.current_section().id, "getting-started");

        state.select_next();
        assert_eq!(state.current_section().id, "keyboard");
    }

    #[test]
    fn test_help_browser_state_default() {
        let state = HelpBrowserState::default();
        assert!(!state.sections.is_empty());
        assert_eq!(state.selected_section, 0);
    }
}
