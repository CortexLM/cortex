//! Tests for the sessions modal.

use std::path::PathBuf;

use chrono::{Duration, Utc};

use super::modal_impl::SessionsModal;
use super::session_info::SessionInfo;
use crate::modal::Modal;

fn create_test_sessions() -> Vec<SessionInfo> {
    let now = Utc::now();
    vec![
        SessionInfo::new(
            PathBuf::from("/sessions/session1"),
            "First Session",
            "anthropic/claude-sonnet-4-20250514",
            now - Duration::hours(2),
            5,
        ),
        SessionInfo::new(
            PathBuf::from("/sessions/session2"),
            "Second Session",
            "openai/gpt-4",
            now - Duration::days(1),
            10,
        ),
        SessionInfo::new(
            PathBuf::from("/sessions/session3"),
            "Third Session",
            "anthropic/claude-opus-4-20250514",
            now - Duration::minutes(30),
            3,
        ),
    ]
}

#[test]
fn test_sessions_modal_creation() {
    let sessions = create_test_sessions();
    let modal = SessionsModal::new(sessions);

    assert_eq!(modal.title(), "Sessions");
}

#[test]
fn test_session_info_format_time() {
    let now = Utc::now();

    let recent = SessionInfo::new(
        PathBuf::from("/test"),
        "Test",
        "model",
        now - Duration::seconds(30),
        0,
    );
    assert_eq!(recent.format_time(), "just now");

    let minutes_ago = SessionInfo::new(
        PathBuf::from("/test"),
        "Test",
        "model",
        now - Duration::minutes(5),
        0,
    );
    assert_eq!(minutes_ago.format_time(), "5 minutes ago");

    let hours_ago = SessionInfo::new(
        PathBuf::from("/test"),
        "Test",
        "model",
        now - Duration::hours(3),
        0,
    );
    assert_eq!(hours_ago.format_time(), "3 hours ago");

    let yesterday = SessionInfo::new(
        PathBuf::from("/test"),
        "Test",
        "model",
        now - Duration::days(1),
        0,
    );
    assert_eq!(yesterday.format_time(), "yesterday");
}

#[test]
fn test_session_info_relative_time() {
    let now = Utc::now();

    let recent = SessionInfo::new(
        PathBuf::from("/test"),
        "Test",
        "model",
        now - Duration::seconds(30),
        0,
    );
    assert_eq!(recent.relative_time(), "now");

    let minutes_ago = SessionInfo::new(
        PathBuf::from("/test"),
        "Test",
        "model",
        now - Duration::minutes(5),
        0,
    );
    assert_eq!(minutes_ago.relative_time(), "5m ago");

    let hours_ago = SessionInfo::new(
        PathBuf::from("/test"),
        "Test",
        "model",
        now - Duration::hours(3),
        0,
    );
    assert_eq!(hours_ago.relative_time(), "3h ago");

    let days_ago = SessionInfo::new(
        PathBuf::from("/test"),
        "Test",
        "model",
        now - Duration::days(2),
        0,
    );
    assert_eq!(days_ago.relative_time(), "2d ago");
}

#[test]
fn test_short_model() {
    let session = SessionInfo::new(
        PathBuf::from("/test"),
        "Test",
        "anthropic/claude-sonnet-4-20250514",
        Utc::now(),
        0,
    );
    assert_eq!(session.short_model(), "claude-sonnet-4-20250514");

    let simple = SessionInfo::new(PathBuf::from("/test"), "Test", "gpt-4", Utc::now(), 0);
    assert_eq!(simple.short_model(), "gpt-4");
}

#[test]
fn test_is_searchable() {
    let sessions = create_test_sessions();
    let modal = SessionsModal::new(sessions);
    assert!(modal.is_searchable());
}

#[test]
fn test_desired_height() {
    let sessions = create_test_sessions();
    let modal = SessionsModal::new(sessions);

    let height = modal.desired_height(20, 80);
    assert!(height >= 6);
    assert!(height <= 16);
}

#[test]
fn test_empty_sessions() {
    let modal = SessionsModal::new(vec![]);
    assert_eq!(modal.desired_height(20, 80), 6); // Minimum height
}
