use sqlx::PgPool;
use uuid::Uuid;

pub async fn save_submission(pool: &PgPool, miner_uid: &str, video_data: &[u8]) -> Result<Uuid, sqlx::Error> {
    let id = Uuid::new_v4();
    sqlx::query!(
        "INSERT INTO bounty_submissions (id, miner_uid, video_data, status, created_at) VALUES ($1, $2, $3, 'pending', NOW())",
        id,
        miner_uid,
        video_data
    )
    .execute(pool)
    .await?;
    Ok(id)
}

pub async fn approve_and_emit(pool: &PgPool, id: Uuid) -> Result<(), sqlx::Error> {
    // Triggers score_epoch TARGET=50 emission with uid0 burn sink in production
    sqlx::query!(
        "UPDATE bounty_submissions SET status = 'approved', approved_at = NOW() WHERE id = $1",
        id
    )
    .execute(pool)
    .await?;
    Ok(())
}
