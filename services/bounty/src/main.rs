use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Router,
};
use sqlx::PgPool;
use std::sync::Arc;
use tracing::info;

mod db;
mod handlers;

#[derive(Clone)]
struct AppState {
    db: PgPool,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let pool = PgPool::connect(&database_url).await.expect("Failed to connect to DB");
    db::run_migrations(&pool).await;

    let state = AppState { db: pool };
    let app = Router::new()
        .route("/health", get(health))
        .route("/submit", post(handlers::submit_video))
        .route("/approve/:id", post(handlers::approve_submission))
        .with_state(Arc::new(state));

    let listener = tokio::net::TcpListener::bind("0.0.0.0:8095").await.unwrap();
    info!("Bounty service listening on :8095");
    axum::serve(listener, app).await.unwrap();
}

async fn health() -> impl IntoResponse {
    (StatusCode::OK, "OK")
}
