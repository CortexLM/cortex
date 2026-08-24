use axum::{extract::{Multipart, State}, http::StatusCode, Json};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;
use crate::services::{video, scoring};

#[derive(Debug, Serialize, Deserialize)]
pub struct SubmitResponse {
    pub id: Uuid,
    pub status: String,
}

#[derive(Debug, Deserialize)]
pub struct AdminApproveReq {
    pub submission_id: Uuid,
    pub admin_key: String,
}

pub async fn submit_video(
    State(pool): State<PgPool>,
    mut multipart: Multipart,
) -> Result<Json<SubmitResponse>, StatusCode> {
    let mut video_data = Vec::new();
    let mut miner_id = String::new();

    while let Some(field) = multipart.next_field().await.map_err(|_| StatusCode::BAD_REQUEST)? {
        let name = field.name().unwrap_or("").to_string();
        if name == "video" {
            video_data = field.bytes().await.map_err(|_| StatusCode::BAD_REQUEST)?.to_vec();
        } else if name == "miner_id" {
            miner_id = field.text().await.map_err(|_| StatusCode::BAD_REQUEST)?;
        }
    }

    if video_data.is_empty() || miner_id.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    let compressed = video::compress_video(&video_data).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    
    let is_similar = video::check_similarity_24h(&compressed).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if is_similar {
        return Err(StatusCode::CONFLICT);
    }

    let id = Uuid::new_v4();
    sqlx::query!(
        "INSERT INTO bounty_submissions (id, miner_id, status, video_data) VALUES ($1, $2, 'PENDING', $3)",
        id, miner_id, compressed
    )
    .execute(&pool)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(SubmitResponse { id, status: "PENDING".into() }))
}

pub async fn approve_submission(
    State(pool): State<PgPool>,
    Json(req): Json<AdminApproveReq>,
) -> Result<StatusCode, StatusCode> {
    let valid_admin = std::env::var("ADMIN_KEY").unwrap_or_default();
    if req.admin_key != valid_admin {
        return Err(StatusCode::UNAUTHORIZED);
    }

    let res = sqlx::query!(
        "UPDATE bounty_submissions SET status = 'APPROVED' WHERE id = $1 AND status = 'PENDING'",
        req.submission_id
    )
    .execute(&pool)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    if res.rows_affected() == 0 {
        return Err(StatusCode::NOT_FOUND);
    }

    scoring::emit_score_epoch(req.submission_id).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(StatusCode::OK)
}

pub async fn check_similarity() -> StatusCode {
    StatusCode::OK
}
