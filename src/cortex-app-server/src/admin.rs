//! Admin API for cortex-app-server.
//!
//! Provides administrative endpoints for managing sessions, viewing statistics,
//! and performing bulk operations.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Query, State},
    routing::{get, post},
};
use chrono::Timelike;
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// Create admin routes.
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        // Statistics
        .route("/admin/stats", get(get_stats))
        .route("/admin/stats/sessions", get(get_session_stats))
        .route("/admin/stats/usage", get(get_usage_stats))
        // Sessions management
        .route("/admin/sessions", get(list_all_sessions))
        .route("/admin/sessions/bulk", post(bulk_action))
        .route("/admin/sessions/export", get(export_sessions_csv))
        // Shares management
        .route("/admin/shares", get(list_all_shares))
        .route("/admin/shares/cleanup", post(cleanup_expired_shares))
}

// ============================================================================
// Statistics Types
// ============================================================================

/// Overall server statistics.
#[derive(Debug, Serialize)]
pub struct ServerStats {
    /// Server uptime in seconds.
    pub uptime_seconds: u64,
    /// Total number of stored sessions.
    pub total_sessions: usize,
    /// Number of active WebSocket connections.
    pub active_connections: usize,
    /// Number of active shares.
    pub active_shares: usize,
    /// Total number of messages across all sessions.
    pub total_messages: usize,
    /// Sessions created today.
    pub sessions_today: usize,
    /// Average messages per session.
    pub avg_messages_per_session: f64,
}

/// Session statistics.
#[derive(Debug, Serialize)]
pub struct SessionStats {
    /// Total sessions.
    pub total: usize,
    /// Sessions by status (if applicable).
    pub by_status: serde_json::Value,
    /// Sessions created in the last 24 hours.
    pub last_24h: usize,
    /// Sessions created in the last 7 days.
    pub last_7d: usize,
    /// Sessions created in the last 30 days.
    pub last_30d: usize,
    /// Top models used.
    pub top_models: Vec<ModelUsage>,
}

/// Model usage statistics.
#[derive(Debug, Serialize)]
pub struct ModelUsage {
    /// Model name.
    pub model: String,
    /// Number of sessions using this model.
    pub session_count: usize,
    /// Percentage of total sessions.
    pub percentage: f64,
}

/// Usage statistics over time.
#[derive(Debug, Serialize)]
pub struct UsageStats {
    /// Daily session counts for the last 30 days.
    pub daily_sessions: Vec<DailyCount>,
    /// Daily message counts for the last 30 days.
    pub daily_messages: Vec<DailyCount>,
    /// Peak usage hour (0-23).
    pub peak_hour: u8,
    /// Average sessions per day.
    pub avg_sessions_per_day: f64,
}

/// Daily count entry.
#[derive(Debug, Serialize)]
pub struct DailyCount {
    /// Date (YYYY-MM-DD).
    pub date: String,
    /// Count for the day.
    pub count: usize,
}

// ============================================================================
// Session Management Types
// ============================================================================

/// Query parameters for listing sessions.
#[derive(Debug, Deserialize)]
pub struct ListSessionsQuery {
    /// Search term for filtering.
    #[serde(default)]
    pub search: Option<String>,
    /// Filter by model.
    #[serde(default)]
    pub model: Option<String>,
    /// Filter by date range start (ISO 8601).
    #[serde(default)]
    pub from: Option<String>,
    /// Filter by date range end (ISO 8601).
    #[serde(default)]
    pub to: Option<String>,
    /// Page number (1-indexed).
    #[serde(default = "default_page")]
    pub page: usize,
    /// Items per page.
    #[serde(default = "default_limit")]
    pub limit: usize,
    /// Sort field.
    #[serde(default = "default_sort")]
    pub sort: String,
    /// Sort order (asc/desc).
    #[serde(default = "default_order")]
    pub order: String,
}

fn default_page() -> usize {
    1
}
fn default_limit() -> usize {
    50
}
fn default_sort() -> String {
    "updated_at".to_string()
}
fn default_order() -> String {
    "desc".to_string()
}

/// Paginated session list response.
#[derive(Debug, Serialize)]
pub struct SessionListResponse {
    /// Sessions in the current page.
    pub sessions: Vec<AdminSessionInfo>,
    /// Total number of sessions matching the query.
    pub total: usize,
    /// Current page.
    pub page: usize,
    /// Total number of pages.
    pub total_pages: usize,
}

/// Session information for admin view.
#[derive(Debug, Serialize)]
pub struct AdminSessionInfo {
    /// Session ID.
    pub id: String,
    /// Session title.
    pub title: Option<String>,
    /// Model used.
    pub model: String,
    /// Working directory.
    pub cwd: String,
    /// Number of messages.
    pub message_count: usize,
    /// Creation timestamp (ISO 8601).
    pub created_at: String,
    /// Last update timestamp (ISO 8601).
    pub updated_at: String,
}

/// Bulk action request.
#[derive(Debug, Deserialize)]
pub struct BulkActionRequest {
    /// Session IDs to operate on.
    pub session_ids: Vec<String>,
    /// Action to perform.
    pub action: BulkAction,
}

/// Bulk action types.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BulkAction {
    /// Delete the sessions.
    Delete,
    /// Export the sessions.
    Export,
}

/// Bulk action response.
#[derive(Debug, Serialize)]
pub struct BulkActionResponse {
    /// Whether the action succeeded.
    pub success: bool,
    /// Number of sessions affected.
    pub affected: usize,
    /// Errors encountered (if any).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
}

// ============================================================================
// Share Management Types
// ============================================================================

/// Query parameters for listing shares.
#[derive(Debug, Deserialize)]
pub struct ListSharesQuery {
    /// Page number.
    #[serde(default = "default_page")]
    pub page: usize,
    /// Items per page.
    #[serde(default = "default_limit")]
    pub limit: usize,
    /// Include expired shares.
    #[serde(default)]
    pub include_expired: bool,
}

/// Share list response.
#[derive(Debug, Serialize)]
pub struct ShareListResponse {
    /// Shares in the current page.
    pub shares: Vec<AdminShareInfo>,
    /// Total number of shares.
    pub total: usize,
    /// Current page.
    pub page: usize,
    /// Total pages.
    pub total_pages: usize,
}

/// Share information for admin view.
#[derive(Debug, Serialize)]
pub struct AdminShareInfo {
    /// Share token.
    pub token: String,
    /// Session ID.
    pub session_id: String,
    /// Session title.
    pub title: Option<String>,
    /// View count.
    pub view_count: u32,
    /// Max views (if set).
    pub max_views: Option<u32>,
    /// Whether the share is expired.
    pub expired: bool,
    /// Creation timestamp.
    pub created_at: String,
    /// Expiration timestamp.
    pub expires_at: String,
}

/// Cleanup response.
#[derive(Debug, Serialize)]
pub struct CleanupResponse {
    /// Number of items cleaned up.
    pub cleaned: usize,
}

// ============================================================================
// Handlers
// ============================================================================

/// Get overall server statistics.
async fn get_stats(State(state): State<Arc<AppState>>) -> AppResult<Json<ServerStats>> {
    let sessions = state
        .cli_sessions
        .storage()
        .list_sessions()
        .unwrap_or_default();

    let total_sessions = sessions.len();
    let active_shares = state.share_manager.count().await;

    // Count messages across all sessions
    let mut total_messages = 0;
    for session in &sessions {
        if let Ok(history) = state.cli_sessions.storage().read_history(&session.id) {
            total_messages += history.len();
        }
    }

    // Count sessions created today
    let today_start = chrono::Utc::now()
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc()
        .timestamp();
    let sessions_today = sessions
        .iter()
        .filter(|s| s.created_at >= today_start)
        .count();

    let avg_messages = if total_sessions > 0 {
        total_messages as f64 / total_sessions as f64
    } else {
        0.0
    };

    Ok(Json(ServerStats {
        uptime_seconds: state.uptime().as_secs(),
        total_sessions,
        active_connections: state.cli_sessions.count().await,
        active_shares,
        total_messages,
        sessions_today,
        avg_messages_per_session: (avg_messages * 100.0).round() / 100.0,
    }))
}

/// Get session statistics.
async fn get_session_stats(State(state): State<Arc<AppState>>) -> AppResult<Json<SessionStats>> {
    let sessions = state
        .cli_sessions
        .storage()
        .list_sessions()
        .unwrap_or_default();

    let total = sessions.len();
    let now = chrono::Utc::now().timestamp();
    let day_secs = 24 * 60 * 60;

    let last_24h = sessions
        .iter()
        .filter(|s| now - s.created_at < day_secs)
        .count();
    let last_7d = sessions
        .iter()
        .filter(|s| now - s.created_at < 7 * day_secs)
        .count();
    let last_30d = sessions
        .iter()
        .filter(|s| now - s.created_at < 30 * day_secs)
        .count();

    // Count model usage
    let mut model_counts: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    for session in &sessions {
        *model_counts.entry(session.model.clone()).or_insert(0) += 1;
    }

    let mut top_models: Vec<ModelUsage> = model_counts
        .into_iter()
        .map(|(model, count)| ModelUsage {
            model,
            session_count: count,
            percentage: if total > 0 {
                (count as f64 / total as f64 * 100.0 * 10.0).round() / 10.0
            } else {
                0.0
            },
        })
        .collect();
    top_models.sort_by(|a, b| b.session_count.cmp(&a.session_count));
    top_models.truncate(10);

    Ok(Json(SessionStats {
        total,
        by_status: serde_json::json!({
            "active": total, // All stored sessions are considered "active"
        }),
        last_24h,
        last_7d,
        last_30d,
        top_models,
    }))
}

/// Get usage statistics over time.
async fn get_usage_stats(State(state): State<Arc<AppState>>) -> AppResult<Json<UsageStats>> {
    let sessions = state
        .cli_sessions
        .storage()
        .list_sessions()
        .unwrap_or_default();

    // Build daily counts for the last 30 days
    let mut daily_sessions: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    let mut daily_messages: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    let mut hour_counts: [usize; 24] = [0; 24];

    let now = chrono::Utc::now();
    let thirty_days_ago = (now - chrono::Duration::days(30)).timestamp();

    for session in &sessions {
        if session.created_at >= thirty_days_ago {
            // Format date
            if let Some(dt) = chrono::DateTime::from_timestamp(session.created_at, 0) {
                let date = dt.format("%Y-%m-%d").to_string();
                *daily_sessions.entry(date.clone()).or_insert(0) += 1;

                // Count hour for peak detection
                let hour = dt.hour() as usize;
                hour_counts[hour] += 1;

                // Count messages
                if let Ok(history) = state.cli_sessions.storage().read_history(&session.id) {
                    *daily_messages.entry(date).or_insert(0) += history.len();
                }
            }
        }
    }

    // Convert to sorted vectors
    let mut daily_sessions_vec: Vec<DailyCount> = daily_sessions
        .into_iter()
        .map(|(date, count)| DailyCount { date, count })
        .collect();
    daily_sessions_vec.sort_by(|a, b| a.date.cmp(&b.date));

    let mut daily_messages_vec: Vec<DailyCount> = daily_messages
        .into_iter()
        .map(|(date, count)| DailyCount { date, count })
        .collect();
    daily_messages_vec.sort_by(|a, b| a.date.cmp(&b.date));

    // Find peak hour
    let peak_hour = hour_counts
        .iter()
        .enumerate()
        .max_by_key(|(_, count)| *count)
        .map(|(hour, _)| hour as u8)
        .unwrap_or(0);

    // Average sessions per day
    let total_days = daily_sessions_vec.len().max(1);
    let total_session_count: usize = daily_sessions_vec.iter().map(|d| d.count).sum();
    let avg_sessions_per_day =
        (total_session_count as f64 / total_days as f64 * 10.0).round() / 10.0;

    Ok(Json(UsageStats {
        daily_sessions: daily_sessions_vec,
        daily_messages: daily_messages_vec,
        peak_hour,
        avg_sessions_per_day,
    }))
}

/// List all sessions with filtering and pagination.
async fn list_all_sessions(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ListSessionsQuery>,
) -> AppResult<Json<SessionListResponse>> {
    let mut sessions = state
        .cli_sessions
        .storage()
        .list_sessions()
        .unwrap_or_default();

    // Apply filters
    if let Some(search) = &query.search {
        let search_lower = search.to_lowercase();
        sessions.retain(|s| {
            s.id.to_lowercase().contains(&search_lower)
                || s.title
                    .as_ref()
                    .map(|t| t.to_lowercase().contains(&search_lower))
                    .unwrap_or(false)
        });
    }

    if let Some(model) = &query.model {
        sessions.retain(|s| &s.model == model);
    }

    if let Some(from) = &query.from
        && let Ok(from_dt) = chrono::DateTime::parse_from_rfc3339(from)
    {
        let from_ts = from_dt.timestamp();
        sessions.retain(|s| s.created_at >= from_ts);
    }

    if let Some(to) = &query.to
        && let Ok(to_dt) = chrono::DateTime::parse_from_rfc3339(to)
    {
        let to_ts = to_dt.timestamp();
        sessions.retain(|s| s.created_at <= to_ts);
    }

    // Sort
    match (query.sort.as_str(), query.order.as_str()) {
        ("created_at", "asc") => sessions.sort_by_key(|s| s.created_at),
        ("created_at", "desc") => sessions.sort_by_key(|s| std::cmp::Reverse(s.created_at)),
        ("updated_at", "asc") => sessions.sort_by_key(|s| s.updated_at),
        (_, _) => sessions.sort_by_key(|s| std::cmp::Reverse(s.updated_at)),
    }

    let total = sessions.len();
    let total_pages = total.div_ceil(query.limit);

    // Paginate
    let start = (query.page - 1) * query.limit;
    let sessions: Vec<AdminSessionInfo> = sessions
        .into_iter()
        .skip(start)
        .take(query.limit)
        .map(|s| {
            let message_count = state
                .cli_sessions
                .storage()
                .read_history(&s.id)
                .map(|h| h.len())
                .unwrap_or(0);

            AdminSessionInfo {
                id: s.id,
                title: s.title,
                model: s.model,
                cwd: s.cwd,
                message_count,
                created_at: chrono::DateTime::from_timestamp(s.created_at, 0)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
                updated_at: chrono::DateTime::from_timestamp(s.updated_at, 0)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default(),
            }
        })
        .collect();

    Ok(Json(SessionListResponse {
        sessions,
        total,
        page: query.page,
        total_pages,
    }))
}

/// Perform bulk actions on sessions.
async fn bulk_action(
    State(state): State<Arc<AppState>>,
    Json(req): Json<BulkActionRequest>,
) -> AppResult<Json<BulkActionResponse>> {
    let mut affected = 0;
    let mut errors = Vec::new();

    match req.action {
        BulkAction::Delete => {
            for session_id in &req.session_ids {
                match state.cli_sessions.storage().delete_session(session_id) {
                    Ok(_) => affected += 1,
                    Err(e) => errors.push(format!("{}: {}", session_id, e)),
                }
            }
        }
        BulkAction::Export => {
            // Export is handled by the export endpoint
            return Err(AppError::BadRequest(
                "Use /admin/sessions/export endpoint for exporting".to_string(),
            ));
        }
    }

    Ok(Json(BulkActionResponse {
        success: errors.is_empty(),
        affected,
        errors,
    }))
}

/// Export sessions as CSV.
async fn export_sessions_csv(
    State(state): State<Arc<AppState>>,
) -> AppResult<axum::response::Response> {
    use axum::http::header;
    use axum::response::IntoResponse;

    let sessions = state
        .cli_sessions
        .storage()
        .list_sessions()
        .unwrap_or_default();

    let mut csv = String::new();
    csv.push_str("id,title,model,cwd,message_count,created_at,updated_at\n");

    for session in sessions {
        let message_count = state
            .cli_sessions
            .storage()
            .read_history(&session.id)
            .map(|h| h.len())
            .unwrap_or(0);

        let title = session
            .title
            .as_ref()
            .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
            .unwrap_or_default();

        let created_at = chrono::DateTime::from_timestamp(session.created_at, 0)
            .map(|dt| dt.to_rfc3339())
            .unwrap_or_default();
        let updated_at = chrono::DateTime::from_timestamp(session.updated_at, 0)
            .map(|dt| dt.to_rfc3339())
            .unwrap_or_default();

        csv.push_str(&format!(
            "{},{},{},\"{}\",{},{},{}\n",
            session.id,
            title,
            session.model,
            session.cwd.replace('"', "\"\""),
            message_count,
            created_at,
            updated_at,
        ));
    }

    Ok((
        [
            (header::CONTENT_TYPE, "text/csv"),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=sessions.csv",
            ),
        ],
        csv,
    )
        .into_response())
}

/// List all shares.
async fn list_all_shares(
    State(_state): State<Arc<AppState>>,
    Query(query): Query<ListSharesQuery>,
) -> AppResult<Json<ShareListResponse>> {
    // Get all shares from the manager (assuming it's a user, we get their shares)
    // For admin, we'd need a method to get all shares regardless of user
    // For now, return what's available

    let _now = chrono::Utc::now().timestamp();

    // Note: In a real implementation, ShareManager would have a list_all method
    // For now, we use what's available
    let shares: Vec<AdminShareInfo> = Vec::new(); // Placeholder

    let total = shares.len();
    let total_pages = (total + query.limit - 1).max(1) / query.limit.max(1);

    Ok(Json(ShareListResponse {
        shares,
        total,
        page: query.page,
        total_pages,
    }))
}

/// Cleanup expired shares.
async fn cleanup_expired_shares(
    State(state): State<Arc<AppState>>,
) -> AppResult<Json<CleanupResponse>> {
    let cleaned = state.share_manager.cleanup_expired().await;
    Ok(Json(CleanupResponse { cleaned }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bulk_action_deserialization() {
        let json = r#"{"session_ids": ["a", "b"], "action": "delete"}"#;
        let req: BulkActionRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.session_ids.len(), 2);
        assert!(matches!(req.action, BulkAction::Delete));
    }

    #[test]
    fn test_list_sessions_query_defaults() {
        let query: ListSessionsQuery = serde_json::from_str("{}").unwrap();
        assert_eq!(query.page, 1);
        assert_eq!(query.limit, 50);
        assert_eq!(query.sort, "updated_at");
        assert_eq!(query.order, "desc");
    }
}
