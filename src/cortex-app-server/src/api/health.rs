//! Health check and metrics endpoints.

use std::sync::Arc;

use axum::{Json, extract::State};

use crate::state::AppState;

use super::types::HealthResponse;

/// Health check endpoint.
pub async fn health_check(State(state): State<Arc<AppState>>) -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "healthy".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        uptime_seconds: state.uptime().as_secs(),
    })
}

/// Get metrics.
pub async fn get_metrics(
    State(state): State<Arc<AppState>>,
) -> Json<crate::state::MetricsSnapshot> {
    Json(state.get_metrics().await)
}
