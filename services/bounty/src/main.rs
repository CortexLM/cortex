use axum::{
    extract::{Multipart, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::sync::Arc;
use tracing::info;

mod compression;
mod db;
mod openrouter;

#[derive(Clone)]
struct AppState {
    db: PgPool,
}

#[derive(Serialize)]
struct SubmissionResponse {
    id: uuid::Uuid,
    status: String,
}

async fn submit_video(
    State(state): State<Arc<AppState>>,
    mut multipart: Multipart,
) -> impl IntoResponse {
    let mut video_data: Option<Vec<u8>> = None;
    let mut miner_uid: Option<String> = None;

    while let Some(field) = multipart.next_field().await.unwrap() {
        let name = field.name().unwrap().to_string();
        if name == "video" {
            video_data = Some(field.bytes().await.unwrap().to_vec());
        } else if name == "miner_uid" {
            miner_uid = Some(field.text().await.unwrap());
        }
    }

    let video_data = match video_data {
        Some(data) => data,
        None => return (StatusCode::BAD_REQUEST, "Missing video file").into_response(),
    };
    let miner_uid = miner_uid.unwrap_or_default();

    let compressed = match compression::compress_video(&video_data).await {
        Ok(data) => data,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "Compression failed").into_response(),
    };

    match openrouter::check_similarity(&compressed).await {
        Ok(true) => {
            return (StatusCode::CONFLICT, "Similar submission within 24h").into_response();
        }
        Ok(false) => {}
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "Similarity check failed").into_response(),
    }

    let id = db::save_submission(&state.db, &miner_uid, &compressed).await.unwrap();

    (StatusCode::OK, Json(SubmissionResponse { id, status: "pending".to_string() })).into_response()
}

async fn approve_submission(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(payload): Json<ApprovePayload>,
) -> impl IntoResponse {
    if headers.get("X-Admin-Token").and_then(|h| h.to_str().ok()) != Some("super_secret_admin_token") {
        return (StatusCode::UNAUTHORIZED, "Unauthorized").into_response();
    }

    db::approve_and_emit(&state.db, payload.id).await.unwrap();
    (StatusCode::OK, "Approved").into_response()
}

#[derive(Deserialize)]
struct ApprovePayload {
    id: uuid::Uuid,
}

async fn health() -> &'static str {
    "OK"
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    let db_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let db = PgPool::connect(&db_url).await.expect("Failed to connect to DB");

    let state = Arc::new(AppState { db });

    let app = Router::new()
        .route("/health", get(health))
        .route("/submit", post(submit_video))
        .route("/approve", post(approve_submission))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:8095").await.unwrap();
    info!("Bounty service listening on port 8095");
    axum::serve(listener, app).await.unwrap();
}
