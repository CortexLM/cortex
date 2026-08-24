use crate::AppState;
use axum::{
    extract::{Multipart, Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Serialize, Deserialize)]
pub struct ApprovalRequest {
    pub approved: bool,
    pub admin_id: String,
}

pub async fn submit_video(
    State(state): State<Arc<AppState>>,
    mut multipart: Multipart,
) -> impl IntoResponse {
    let mut miner_uid = None;
    let mut video_data = None;

    while let Some(field) = multipart.next_field().await.unwrap() {
        let name = field.name().unwrap().to_string();
        if name == "miner_uid" {
            miner_uid = Some(field.text().await.unwrap());
        } else if name == "video" {
            video_data = Some(field.bytes().await.unwrap());
        }
    }

    let uid = match miner_uid {
        Some(u) => u,
        None => return (StatusCode::BAD_REQUEST, "Missing miner_uid").into_response(),
    };
    let data = match video_data {
        Some(d) => d,
        None => return (StatusCode::BAD_REQUEST, "Missing video file").into_response(),
    };

    let compressed = compress_video(data).await;
    let is_similar = check_similarity(&compressed).await;

    if is_similar {
        let recent = sqlx::query!(
            "SELECT id FROM submissions WHERE miner_uid = $1 AND status = 'REJECTED' AND created_at > NOW() - INTERVAL '24 hours'",
            uid
        )
        .fetch_optional(&state.db)
        .await
        .unwrap();

        if recent.is_some() {
            return (StatusCode::CONFLICT, "Similarity reject within 24h").into_response();
        }
        
        sqlx::query!(
            "INSERT INTO submissions (id, miner_uid, status) VALUES ($1, $2, 'REJECTED')",
            Uuid::new_v4(),
            uid
        )
        .execute(&state.db)
        .await
        .unwrap();
        
        return (StatusCode::CONFLICT, "Similarity reject").into_response();
    }

    let sub_id = Uuid::new_v4();
    sqlx::query!(
        "INSERT INTO submissions (id, miner_uid, video_data, status) VALUES ($1, $2, $3, 'PENDING')",
        sub_id,
        uid,
        compressed
    )
    .execute(&state.db)
    .await
    .unwrap();

    (StatusCode::OK, Json(serde_json::json!({"submission_id": sub_id}))).into_response()
}

pub async fn approve_submission(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(payload): Json<ApprovalRequest>,
) -> impl IntoResponse {
    let auth = headers.get("X-Admin-Token").and_then(|h| h.to_str().ok());
    if auth != Some("SUPER_SECRET_ADMIN_TOKEN") {
        return (StatusCode::UNAUTHORIZED, "Unauthorized admin").into_response();
    }

    let status = if payload.approved { "APPROVED" } else { "REJECTED" };
    
    let res = sqlx::query!(
        "UPDATE submissions SET status = $1, approved_by = $2, approved_at = NOW() WHERE id = $3",
        status,
        payload.admin_id,
        id
    )
    .execute(&state.db)
    .await
    .unwrap();

    if res.rows_affected() == 0 {
        return (StatusCode::NOT_FOUND, "Submission not found").into_response();
    }

    if payload.approved {
        let _ = emit_rewards(&state.db, id).await;
    }

    (StatusCode::OK, "Updated").into_response()
}

async fn compress_video(data: bytes::Bytes) -> Vec<u8> {
    data.to_vec()
}

async fn check_similarity(_data: &[u8]) -> bool {
    false
}

async fn emit_rewards(_db: &PgPool, _id: Uuid) -> Result<(), sqlx::Error> {
    Ok(())
}
