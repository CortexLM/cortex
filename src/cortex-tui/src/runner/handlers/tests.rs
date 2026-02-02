//! Tests for action handlers.

use crate::actions::KeyAction;
use crate::app::{AppState, AppView, FocusTarget};
use crate::bridge::StreamController;

use super::ActionHandler;

use anyhow::Result;

fn create_test_state() -> AppState {
    AppState::new()
}

fn create_test_stream() -> StreamController {
    StreamController::new()
}

/// Helper to run an action and get the result without borrow conflicts.
async fn run_action(
    state: &mut AppState,
    stream: &mut StreamController,
    action: KeyAction,
) -> Result<bool> {
    let mut handler = ActionHandler::new(state, None, stream);
    handler.handle(action).await
}

#[tokio::test]
async fn test_handle_help() {
    let mut state = create_test_state();
    let mut stream = create_test_stream();

    let result = run_action(&mut state, &mut stream, KeyAction::Help).await;
    assert!(result.is_ok());
    assert!(result.unwrap());
    assert_eq!(state.view, AppView::Help);
}

#[tokio::test]
async fn test_handle_focus_next() {
    let mut state = create_test_state();
    let mut stream = create_test_stream();

    assert_eq!(state.focus, FocusTarget::Input);

    let result = run_action(&mut state, &mut stream, KeyAction::FocusNext).await;
    assert!(result.is_ok());
    assert!(result.unwrap());
    assert_eq!(state.focus, FocusTarget::Chat);
}

#[tokio::test]
async fn test_handle_focus_prev() {
    let mut state = create_test_state();
    let mut stream = create_test_stream();

    assert_eq!(state.focus, FocusTarget::Input);

    let result = run_action(&mut state, &mut stream, KeyAction::FocusPrev).await;
    assert!(result.is_ok());
    assert!(result.unwrap());
    assert_eq!(state.focus, FocusTarget::Sidebar);
}

#[tokio::test]
async fn test_handle_focus_specific() {
    let mut state = create_test_state();
    let mut stream = create_test_stream();

    let result = run_action(&mut state, &mut stream, KeyAction::FocusChat).await;
    assert!(result.is_ok());
    assert!(result.unwrap());
    assert_eq!(state.focus, FocusTarget::Chat);
}

#[tokio::test]
async fn test_handle_scroll() {
    let mut state = create_test_state();
    let mut stream = create_test_stream();
    state.set_focus(FocusTarget::Chat);

    let result = run_action(&mut state, &mut stream, KeyAction::ScrollDown).await;
    assert!(result.is_ok());
    assert!(result.unwrap());
    assert_eq!(state.chat_scroll, 1);

    let result = run_action(&mut state, &mut stream, KeyAction::ScrollUp).await;
    assert!(result.is_ok());
    assert_eq!(state.chat_scroll, 0);
}

#[tokio::test]
async fn test_handle_toggle_sidebar() {
    let mut state = create_test_state();
    let mut stream = create_test_stream();

    assert!(state.sidebar_visible);

    let result = run_action(&mut state, &mut stream, KeyAction::ToggleSidebar).await;
    assert!(result.is_ok());
    assert!(result.unwrap());
    assert!(!state.sidebar_visible);
}

#[tokio::test]
async fn test_handle_settings() {
    let mut state = create_test_state();
    let mut stream = create_test_stream();

    let result = run_action(&mut state, &mut stream, KeyAction::ToggleSettings).await;
    assert!(result.is_ok());
    assert!(result.unwrap());
    assert_eq!(state.view, AppView::Settings);
}

#[tokio::test]
async fn test_handle_clear() {
    let mut state = create_test_state();
    let mut stream = create_test_stream();

    // Add some text to input
    state.input.set_text("test input");

    let result = run_action(&mut state, &mut stream, KeyAction::Clear).await;
    assert!(result.is_ok());
    assert!(result.unwrap());
    assert!(state.input.text().is_empty());
}

#[tokio::test]
async fn test_handle_quit() {
    let mut state = create_test_state();
    let mut stream = create_test_stream();

    assert!(state.running);

    let result = run_action(&mut state, &mut stream, KeyAction::Quit).await;
    assert!(result.is_ok());
    assert!(result.unwrap());
    assert!(!state.running);
}

#[tokio::test]
async fn test_handle_none() {
    let mut state = create_test_state();
    let mut stream = create_test_stream();

    let result = run_action(&mut state, &mut stream, KeyAction::None).await;
    assert!(result.is_ok());
    assert!(!result.unwrap()); // None action should not be consumed
}

#[tokio::test]
async fn test_handle_submit_empty() {
    let mut state = create_test_state();
    let mut stream = create_test_stream();

    // Empty submit should return false
    let result = run_action(&mut state, &mut stream, KeyAction::Submit).await;
    assert!(result.is_ok());
    assert!(!result.unwrap());
}

#[tokio::test]
async fn test_handle_new_session() {
    let mut state = create_test_state();
    let mut stream = create_test_stream();

    let result = run_action(&mut state, &mut stream, KeyAction::NewSession).await;
    assert!(result.is_ok());
    assert!(result.unwrap());
    assert!(state.session_id.is_some());
    assert_eq!(state.view, AppView::Session);
}

#[tokio::test]
async fn test_handle_cancel_in_modal() {
    let mut state = create_test_state();
    let mut stream = create_test_stream();
    state.set_view(AppView::Help);

    let result = run_action(&mut state, &mut stream, KeyAction::Cancel).await;
    assert!(result.is_ok());
    assert!(result.unwrap());
    assert_eq!(state.view, AppView::Session);
}

#[tokio::test]
async fn test_handle_scroll_page() {
    let mut state = create_test_state();
    let mut stream = create_test_stream();
    state.set_focus(FocusTarget::Chat);

    // Default terminal size is (80, 24), so page size = 24 - 1 = 23
    let expected_page_size = (state.terminal_size.1 as usize).saturating_sub(1).max(1);

    let result = run_action(&mut state, &mut stream, KeyAction::ScrollPageDown).await;
    assert!(result.is_ok());
    assert!(result.unwrap());
    assert_eq!(state.chat_scroll, expected_page_size);
}
