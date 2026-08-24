use sqlx::{PgPool, Row};

pub async fn run_migrations(pool: &PgPool) {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS submissions (
            id UUID PRIMARY KEY,
            miner_uid TEXT NOT NULL,
            video_data BYTEA,
            status TEXT NOT NULL,
            approved_by TEXT,
            approved_at TIMESTAMPTZ,
            created_at TIMESTAMPTZ DEFAULT NOW()
        )"
    )
    .execute(pool)
    .await
    .unwrap();
}
