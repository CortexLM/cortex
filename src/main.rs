use axum::{
    routing::{get, post},
    Router,
};
use sqlx::postgres::PgPoolOptions;
use std::net::SocketAddr;
use tracing_subscriber;

mod routes;
mod services;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let database_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| "postgres://postgres:postgres@localhost/bounty".into());
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&database_url)
        .await?;

    let app = Router::new()
        .route("/health", get(|| async { "OK" }))
        .route("/v1/bounty/submit", post(routes::bounty::submit_video))
        .route("/v1/bounty/approve", post(routes::bounty::approve_submission))
        .route("/v1/bounty/similarity", post(routes::bounty::check_similarity))
        .with_state(pool);

    let addr = SocketAddr::from(([0, 0, 0, 0], 8095));
    tracing::info!("Bounty service listening on {}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
