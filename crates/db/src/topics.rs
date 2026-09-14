//! Typed persistence for the Proof topic install registry
//! (`0024_proof_topics.sql`).
//!
//! One row per topic, keyed by `topic_id`. This is install state — which
//! runner, which image and pack pins, how much concurrency, whether the topic
//! is live — not the scoring contract, which stays the operator-signed topic
//! document in `proof_topic_version`.
//!
//! Runtime `sqlx::query` (no compile-time database), matching
//! `proof-rlm-store`: the table's shape lives in the migration and in the
//! `CHECK`s there, and these queries are checked against it in
//! `tests/topics.rs`.
//!
//! Nothing here enables a topic. [`upsert_topic`] writes `enabled = FALSE` on
//! insert and leaves the column untouched on conflict, because installing a
//! topic and opening it are different operator actions; the enable path is a
//! later slice (P1+) and is a fail-closed stub in the CLI today.

use serde_json::Value;
use sqlx::{PgPool, Row};

use crate::DbError;

/// One `proof_topic` row.
#[derive(Debug, Clone, PartialEq)]
pub struct TopicRow {
    /// Topic slug (primary key).
    pub topic_id: String,
    /// Human label.
    pub display_name: String,
    /// Install version.
    pub version: i32,
    /// Install target (`staging` | `metal`).
    pub environment: String,
    /// In-guest runner id, empty when the topic selects none.
    pub runner_id: String,
    /// Extra slugs the topic answers to.
    pub aliases: Vec<String>,
    /// Whether the topic is live. Always `false` in P0.
    pub enabled: bool,
    /// Opaque per-topic operator config.
    pub config: Value,
    /// RLM VM image pin (`sha256:<hex>`), empty when unpinned.
    pub pin_rlm: String,
    /// Experiment guest image pin, empty when unpinned.
    pub pin_experiment: String,
    /// Experiment pack digest, empty when absent.
    pub pack_digest: String,
    /// Concurrency bound.
    pub n_concurrent: i32,
    /// Sealed baseline primary, `None` until measured.
    pub sealed_custom_value: Option<f64>,
    /// Install bundle schema version.
    pub schema_version: i32,
    /// The validated bundle, verbatim.
    pub bundle: Value,
    /// `sha256:<hex>` over the canonical bundle.
    pub bundle_digest: String,
    /// Row creation instant (RFC 3339, UTC).
    pub created_at: String,
    /// Last write instant (RFC 3339, UTC).
    pub updated_at: String,
}

/// An install to write. Borrowed so a caller can hand over a parsed plan
/// without cloning it field by field.
#[derive(Debug, Clone)]
pub struct NewTopic<'a> {
    /// Topic slug (primary key).
    pub topic_id: &'a str,
    /// Human label.
    pub display_name: &'a str,
    /// Install version (`>= 1`).
    pub version: i32,
    /// Install target (`staging` | `metal`).
    pub environment: &'a str,
    /// In-guest runner id, empty when none.
    pub runner_id: &'a str,
    /// Extra slugs.
    pub aliases: &'a [String],
    /// Opaque per-topic config (must be a JSON object).
    pub config: &'a Value,
    /// RLM image pin, empty when unpinned.
    pub pin_rlm: &'a str,
    /// Experiment guest image pin, empty when unpinned.
    pub pin_experiment: &'a str,
    /// Experiment pack digest, empty when absent.
    pub pack_digest: &'a str,
    /// Concurrency bound (`>= 1`).
    pub n_concurrent: i32,
    /// Sealed baseline primary, `None` until measured.
    pub sealed_custom_value: Option<f64>,
    /// Install bundle schema version.
    pub schema_version: i32,
    /// The validated bundle, verbatim.
    pub bundle: &'a Value,
    /// `sha256:<hex>` over the canonical bundle.
    pub bundle_digest: &'a str,
}

/// Timestamps as RFC 3339 UTC text, so no consumer needs a time crate to
/// print a row and the two columns stay comparable as strings.
const ROW_COLUMNS: &str = "\
    topic_id, display_name, version, environment, runner_id, aliases, enabled, \
    config, pin_rlm, pin_experiment, pack_digest, n_concurrent, sealed_custom_value, \
    schema_version, bundle, bundle_digest, \
    to_char(created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"') AS created_at, \
    to_char(updated_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"') AS updated_at";

fn row_to_topic(row: &sqlx::postgres::PgRow) -> Result<TopicRow, DbError> {
    Ok(TopicRow {
        topic_id: row.try_get("topic_id")?,
        display_name: row.try_get("display_name")?,
        version: row.try_get("version")?,
        environment: row.try_get("environment")?,
        runner_id: row.try_get("runner_id")?,
        aliases: row.try_get("aliases")?,
        enabled: row.try_get("enabled")?,
        config: row.try_get("config")?,
        pin_rlm: row.try_get("pin_rlm")?,
        pin_experiment: row.try_get("pin_experiment")?,
        pack_digest: row.try_get("pack_digest")?,
        n_concurrent: row.try_get("n_concurrent")?,
        sealed_custom_value: row.try_get("sealed_custom_value")?,
        schema_version: row.try_get("schema_version")?,
        bundle: row.try_get("bundle")?,
        bundle_digest: row.try_get("bundle_digest")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

/// Write one topic install and return the row as persisted.
///
/// A re-install of the same `topic_id` replaces the install fields and bumps
/// `updated_at`; `created_at` keeps the first install's instant. `enabled` is
/// set `FALSE` on insert and deliberately **not** touched on conflict: an
/// install is not an enable, and a re-install of a live topic must not
/// silently drop it out of scoring either. The enable/disable path is a later
/// slice.
///
/// The write and the reported state are **one statement**: `RETURNING` gives
/// the caller the row it just wrote, including the `enabled` it did not set.
/// A separate read afterwards could fail after the commit and leave a caller
/// believing a successful install failed — which is how an automation
/// retries and overwrites a newer concurrent install.
///
/// # Errors
///
/// Propagates sqlx errors, including the row's `CHECK` violations (slug,
/// digest shape, non-finite baseline, empty config, ...).
pub async fn upsert_topic(pool: &PgPool, topic: &NewTopic<'_>) -> Result<TopicRow, DbError> {
    let sql = format!(
        "INSERT INTO proof_topic (
             topic_id, display_name, version, environment, runner_id, aliases,
             config, pin_rlm, pin_experiment, pack_digest, n_concurrent,
             sealed_custom_value, schema_version, bundle, bundle_digest
         ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)
         ON CONFLICT (topic_id) DO UPDATE SET
             display_name = EXCLUDED.display_name,
             version = EXCLUDED.version,
             environment = EXCLUDED.environment,
             runner_id = EXCLUDED.runner_id,
             aliases = EXCLUDED.aliases,
             config = EXCLUDED.config,
             pin_rlm = EXCLUDED.pin_rlm,
             pin_experiment = EXCLUDED.pin_experiment,
             pack_digest = EXCLUDED.pack_digest,
             n_concurrent = EXCLUDED.n_concurrent,
             sealed_custom_value = EXCLUDED.sealed_custom_value,
             schema_version = EXCLUDED.schema_version,
             bundle = EXCLUDED.bundle,
             bundle_digest = EXCLUDED.bundle_digest,
             updated_at = now()
         RETURNING {ROW_COLUMNS}"
    );
    let row = sqlx::query(&sql)
        .bind(topic.topic_id)
        .bind(topic.display_name)
        .bind(topic.version)
        .bind(topic.environment)
        .bind(topic.runner_id)
        .bind(topic.aliases)
        .bind(topic.config)
        .bind(topic.pin_rlm)
        .bind(topic.pin_experiment)
        .bind(topic.pack_digest)
        .bind(topic.n_concurrent)
        .bind(topic.sealed_custom_value)
        .bind(topic.schema_version)
        .bind(topic.bundle)
        .bind(topic.bundle_digest)
        .fetch_one(pool)
        .await?;
    row_to_topic(&row)
}

/// Every installed topic, ordered by `topic_id`.
///
/// An empty table is an empty vector, not an error: P0 ships before any topic
/// is installed, and `topic list` has to say so rather than fail.
///
/// # Errors
///
/// Propagates sqlx query and decode errors.
pub async fn list_topics(pool: &PgPool) -> Result<Vec<TopicRow>, DbError> {
    let sql = format!("SELECT {ROW_COLUMNS} FROM proof_topic ORDER BY topic_id");
    let rows = sqlx::query(&sql).fetch_all(pool).await?;
    rows.iter().map(row_to_topic).collect()
}

/// One installed topic by slug, or `None`.
///
/// Looks up `topic_id` only. Aliases are stored for a later slice and are not
/// resolved here, so `show tbench` on a row whose id is `tb4` is a miss — the
/// CLI says so instead of guessing which row was meant.
///
/// # Errors
///
/// Propagates sqlx query and decode errors.
pub async fn get_topic(pool: &PgPool, topic_id: &str) -> Result<Option<TopicRow>, DbError> {
    let sql = format!("SELECT {ROW_COLUMNS} FROM proof_topic WHERE topic_id = $1");
    let row = sqlx::query(&sql)
        .bind(topic_id)
        .fetch_optional(pool)
        .await?;
    row.as_ref().map(row_to_topic).transpose()
}

#[cfg(test)]
mod unit_tests {
    use super::*;

    const MIGRATION: &str = include_str!("../migrations/0024_proof_topics.sql");

    /// The selected columns are what [`row_to_topic`] reads: a column added to
    /// one side only is a decode error at runtime, so both lists are pinned.
    /// The upsert's `RETURNING` reuses the same list, so the write and the
    /// read cannot drift apart either.
    #[test]
    fn the_column_list_covers_every_decoded_field() {
        for column in [
            "topic_id",
            "display_name",
            "version",
            "environment",
            "runner_id",
            "aliases",
            "enabled",
            "config",
            "pin_rlm",
            "pin_experiment",
            "pack_digest",
            "n_concurrent",
            "sealed_custom_value",
            "schema_version",
            "bundle",
            "bundle_digest",
            "created_at",
            "updated_at",
        ] {
            assert!(ROW_COLUMNS.contains(column), "missing column {column}");
        }
    }

    #[test]
    fn timestamps_are_formatted_as_utc_rfc3339_text() {
        assert!(ROW_COLUMNS.contains("AT TIME ZONE 'UTC'"), "{ROW_COLUMNS}");
        assert!(!ROW_COLUMNS.contains("now()"), "reads never write");
    }

    /// A topic is disabled, never dropped: the app role may write and update
    /// the install, and must not be able to delete the row a bundle digest
    /// pins. The integration test proves the runtime refusal; this pins the
    /// migration's grant without needing a database.
    #[test]
    fn the_app_role_may_write_and_update_but_never_delete() {
        assert!(
            MIGRATION.contains("GRANT SELECT, INSERT, UPDATE ON TABLE proof_topic TO base_app;"),
            "the install is mutable (enable/disable, re-install, seal)"
        );
        for forbidden in [
            "GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE proof_topic",
            "GRANT DELETE ON TABLE proof_topic",
            "GRANT ALL ON TABLE proof_topic",
        ] {
            assert!(
                !MIGRATION.contains(forbidden),
                "the app role must never delete a topic: {forbidden}"
            );
        }
    }

    /// Nothing here may enable a topic: the column exists, and the upsert
    /// deliberately leaves it alone on conflict.
    #[test]
    fn installing_never_enables() {
        assert!(!ROW_COLUMNS.contains("enabled = TRUE"), "reads never write");
        assert!(
            MIGRATION.contains("enabled             BOOLEAN NOT NULL DEFAULT FALSE"),
            "a fresh install starts disabled"
        );
    }
}
