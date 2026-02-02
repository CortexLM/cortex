//! Tests for session storage functionality.

use tempfile::tempdir;

use crate::paths::CortexPaths;
use crate::sessions::{SessionQuery, SessionStorage, SessionSummary, StoredMessage, StoredSession};
use chrono::Utc;
use std::time::Duration;

#[tokio::test]
async fn test_session_crud() {
    let dir = tempdir().unwrap();
    let paths = CortexPaths::from_root(dir.path().to_path_buf());
    let storage = SessionStorage::with_paths(paths);
    storage.init().await.unwrap();

    // Create and save
    let session = StoredSession::new("gpt-4o", "/test/path");
    let id = session.id.clone();
    storage.save_session(&session).await.unwrap();

    // Get
    let retrieved = storage.get_session(&id).await.unwrap();
    assert_eq!(retrieved.id, id);
    assert_eq!(retrieved.model, "gpt-4o");

    // List
    let sessions = storage.list_sessions().await.unwrap();
    assert_eq!(sessions.len(), 1);

    // Delete
    storage.delete_session(&id).await.unwrap();
    let sessions = storage.list_sessions().await.unwrap();
    assert_eq!(sessions.len(), 0);
}

#[tokio::test]
async fn test_message_history() {
    let dir = tempdir().unwrap();
    let paths = CortexPaths::from_root(dir.path().to_path_buf());
    let storage = SessionStorage::with_paths(paths);
    storage.init().await.unwrap();

    let session_id = "test-session";

    // Append messages
    let msg1 = StoredMessage::user("Hello");
    let msg2 = StoredMessage::assistant("Hi there!");

    storage.append_message(session_id, &msg1).await.unwrap();
    storage.append_message(session_id, &msg2).await.unwrap();

    // Read history
    let history = storage.get_history(session_id).await.unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].role, "user");
    assert_eq!(history[1].role, "assistant");
}

#[tokio::test]
async fn test_favorites() {
    let dir = tempdir().unwrap();
    let paths = CortexPaths::from_root(dir.path().to_path_buf());
    let storage = SessionStorage::with_paths(paths);
    storage.init().await.unwrap();

    // Create and save a session
    let session = StoredSession::new("gpt-4o", "/test/path");
    let id = session.id.clone();
    storage.save_session(&session).await.unwrap();

    // Initial state should not be favorite
    let retrieved = storage.get_session(&id).await.unwrap();
    assert!(!retrieved.is_favorite);

    // Toggle favorite on
    let is_fav = storage.toggle_favorite(&id).await.unwrap();
    assert!(is_fav);

    let retrieved = storage.get_session(&id).await.unwrap();
    assert!(retrieved.is_favorite);

    // Toggle favorite off
    let is_fav = storage.toggle_favorite(&id).await.unwrap();
    assert!(!is_fav);

    let retrieved = storage.get_session(&id).await.unwrap();
    assert!(!retrieved.is_favorite);
}

#[tokio::test]
async fn test_tags() {
    let dir = tempdir().unwrap();
    let paths = CortexPaths::from_root(dir.path().to_path_buf());
    let storage = SessionStorage::with_paths(paths);
    storage.init().await.unwrap();

    let mut session = StoredSession::new("gpt-4o", "/test/path");
    let id = session.id.clone();

    // Add tags
    session.add_tag("project-a");
    session.add_tag("important");
    assert_eq!(session.tags.len(), 2);
    assert!(session.has_tag("project-a"));
    assert!(session.has_tag("important"));

    // Try to add duplicate tag
    session.add_tag("project-a");
    assert_eq!(session.tags.len(), 2);

    // Remove tag
    assert!(session.remove_tag("important"));
    assert_eq!(session.tags.len(), 1);
    assert!(!session.has_tag("important"));

    // Remove non-existent tag
    assert!(!session.remove_tag("nonexistent"));

    storage.save_session(&session).await.unwrap();
    let retrieved = storage.get_session(&id).await.unwrap();
    assert_eq!(retrieved.tags, vec!["project-a"]);
}

#[tokio::test]
async fn test_sharing() {
    let dir = tempdir().unwrap();
    let paths = CortexPaths::from_root(dir.path().to_path_buf());
    let storage = SessionStorage::with_paths(paths);
    storage.init().await.unwrap();

    let session = StoredSession::new("gpt-4o", "/test/path");
    let id = session.id.clone();
    storage.save_session(&session).await.unwrap();

    // Create share with 1 hour expiration
    let share_info = storage
        .share_session(&id, Some(Duration::from_secs(3600)))
        .await
        .unwrap();

    assert!(!share_info.token.is_empty());
    assert!(share_info.url.contains(&share_info.token));
    assert!(share_info.is_valid());

    // Verify the session has the share info
    let retrieved = storage.get_session(&id).await.unwrap();
    assert!(retrieved.has_valid_share());
    assert!(retrieved.share_url().is_some());

    // Unshare
    storage.unshare_session(&id).await.unwrap();
    let retrieved = storage.get_session(&id).await.unwrap();
    assert!(!retrieved.has_valid_share());
    assert!(retrieved.share_url().is_none());
}

#[test]
fn test_session_query_favorites_filter() {
    let query = SessionQuery::new().favorites();

    let fav = SessionSummary {
        id: "1".to_string(),
        title: None,
        model: "gpt-4o".to_string(),
        cwd: "/test".to_string(),
        created_at: 0,
        updated_at: 0,
        is_favorite: true,
        tags: vec![],
        is_shared: false,
    };

    let not_fav = SessionSummary {
        is_favorite: false,
        ..fav.clone()
    };

    assert!(query.matches(&fav));
    assert!(!query.matches(&not_fav));
}

#[test]
fn test_session_query_tags_filter() {
    let query = SessionQuery::new().with_tag("important");

    let with_tag = SessionSummary {
        id: "1".to_string(),
        title: None,
        model: "gpt-4o".to_string(),
        cwd: "/test".to_string(),
        created_at: 0,
        updated_at: 0,
        is_favorite: false,
        tags: vec!["important".to_string(), "other".to_string()],
        is_shared: false,
    };

    let without_tag = SessionSummary {
        tags: vec!["other".to_string()],
        ..with_tag.clone()
    };

    assert!(query.matches(&with_tag));
    assert!(!query.matches(&without_tag));
}

#[test]
fn test_session_query_date_filter() {
    let now = Utc::now().timestamp();
    let week_ago = now - 7 * 24 * 60 * 60;
    let month_ago = now - 30 * 24 * 60 * 60;

    let query = SessionQuery::new().from(week_ago);

    let recent = SessionSummary {
        id: "1".to_string(),
        title: None,
        model: "gpt-4o".to_string(),
        cwd: "/test".to_string(),
        created_at: now,
        updated_at: now - 3 * 24 * 60 * 60, // 3 days ago
        is_favorite: false,
        tags: vec![],
        is_shared: false,
    };

    let old = SessionSummary {
        updated_at: month_ago - 1000, // More than a month ago
        ..recent.clone()
    };

    assert!(query.matches(&recent));
    assert!(!query.matches(&old));
}

#[test]
fn test_session_query_search() {
    let query = SessionQuery::new().search("test");

    let matching_title = SessionSummary {
        id: "abc123".to_string(),
        title: Some("Test Session".to_string()),
        model: "gpt-4o".to_string(),
        cwd: "/project".to_string(),
        created_at: 0,
        updated_at: 0,
        is_favorite: false,
        tags: vec![],
        is_shared: false,
    };

    let matching_id = SessionSummary {
        id: "test-session-1".to_string(),
        title: Some("Other".to_string()),
        ..matching_title.clone()
    };

    let not_matching = SessionSummary {
        id: "abc123".to_string(),
        title: Some("Other".to_string()),
        ..matching_title.clone()
    };

    assert!(query.matches(&matching_title));
    assert!(query.matches(&matching_id));
    assert!(!query.matches(&not_matching));
}

#[tokio::test]
async fn test_query_sessions() {
    let dir = tempdir().unwrap();
    let paths = CortexPaths::from_root(dir.path().to_path_buf());
    let storage = SessionStorage::with_paths(paths);
    storage.init().await.unwrap();

    // Create sessions
    let mut session1 = StoredSession::new("gpt-4o", "/test/path");
    session1.title = Some("Favorite Session".to_string());
    session1.is_favorite = true;
    storage.save_session(&session1).await.unwrap();

    let mut session2 = StoredSession::new("claude", "/other/path");
    session2.title = Some("Normal Session".to_string());
    storage.save_session(&session2).await.unwrap();

    // Query all
    let all = storage.query_sessions(&SessionQuery::new()).await.unwrap();
    assert_eq!(all.len(), 2);

    // Query favorites only
    let favorites = storage
        .query_sessions(&SessionQuery::new().favorites())
        .await
        .unwrap();
    assert_eq!(favorites.len(), 1);
    assert_eq!(favorites[0].title, Some("Favorite Session".to_string()));

    // Query with limit
    let limited = storage
        .query_sessions(&SessionQuery::new().limit(1))
        .await
        .unwrap();
    assert_eq!(limited.len(), 1);
}
