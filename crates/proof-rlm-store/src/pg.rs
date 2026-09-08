//! Postgres [`RlmStore`] over `crates/db` migration `0020_proof_rlm.sql`.
//!
//! Plain runtime `sqlx::query` (no compile-time database). Append-only
//! tables are inserted, never updated; "current" is always the newest row.

use async_trait::async_trait;
use db::PgPool;
use proof_rlm::{Checklist, CustomRunReport, Lifecycle, RlmEvent, RlmState, RuleSet, RuleSource};
use proof_task::{ChecklistRule, TopicDocument};
use serde_json::Value;

use crate::{
    check_artefact, check_promotion, check_rules, replay, ArtefactRow, BaselineRow, ChecklistRow,
    PromotionRow, RlmStore, StoreError, TransitionRow,
};

/// Postgres-backed store.
#[derive(Clone)]
pub struct PgRlmStore {
    pool: PgPool,
}

impl PgRlmStore {
    /// Store over an already-migrated pool.
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// The pool.
    #[must_use]
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }
}

fn malformed<E: std::fmt::Display>(e: E) -> StoreError {
    StoreError::Malformed(e.to_string())
}

fn to_u32(v: i32) -> Result<u32, StoreError> {
    u32::try_from(v).map_err(malformed)
}

fn to_i32(v: u32) -> Result<i32, StoreError> {
    i32::try_from(v).map_err(malformed)
}

fn source_str(s: RuleSource) -> &'static str {
    match s {
        RuleSource::TopicDocument => "topic_document",
        RuleSource::Rlm => "rlm",
        RuleSource::Operator => "operator",
    }
}

fn parse_source(s: &str) -> Result<RuleSource, StoreError> {
    match s {
        "topic_document" => Ok(RuleSource::TopicDocument),
        "rlm" => Ok(RuleSource::Rlm),
        "operator" => Ok(RuleSource::Operator),
        other => Err(StoreError::Malformed(format!("rule source {other:?}"))),
    }
}

fn parse_state(s: &str) -> Result<RlmState, StoreError> {
    RlmState::parse(s).ok_or_else(|| StoreError::Malformed(format!("state {s:?}")))
}

fn parse_event(s: &str) -> Result<RlmEvent, StoreError> {
    serde_json::from_value(Value::String(s.to_owned())).map_err(malformed)
}

#[derive(sqlx::FromRow)]
struct RuleRow {
    topic_id: String,
    version: i32,
    source: String,
    rules: Value,
}

impl RuleRow {
    fn into_set(self) -> Result<RuleSet, StoreError> {
        let rules: Vec<ChecklistRule> = serde_json::from_value(self.rules).map_err(malformed)?;
        Ok(RuleSet {
            topic_id: self.topic_id,
            version: to_u32(self.version)?,
            source: parse_source(&self.source)?,
            rules,
        })
    }
}

#[derive(sqlx::FromRow)]
struct ChecklistDbRow {
    topic_id: String,
    submission_digest: String,
    rules_version: i32,
    green: bool,
    failed_ids: Value,
    document: Value,
}

#[derive(sqlx::FromRow)]
struct TransitionDbRow {
    topic_id: String,
    from_state: String,
    event: String,
    to_state: String,
    note: String,
}

#[derive(sqlx::FromRow)]
struct BaselineDbRow {
    topic_id: String,
    rules_version: i32,
    primary_value: f64,
    report: Value,
}

#[derive(sqlx::FromRow)]
struct ArtefactDbRow {
    topic_id: String,
    submission_id: String,
    submission_digest: String,
    path: String,
    sha256: String,
    bytes: i64,
    primary_value: Option<f64>,
    checklist_green: bool,
    promoted: bool,
}

#[derive(sqlx::FromRow)]
struct PromotionDbRow {
    topic_id: String,
    submission_id: String,
    submission_digest: String,
    primary_value: f64,
    bar: Option<f64>,
    previous_best: Option<String>,
}

impl From<PromotionDbRow> for PromotionRow {
    fn from(r: PromotionDbRow) -> Self {
        Self {
            topic_id: r.topic_id,
            submission_id: r.submission_id,
            submission_digest: r.submission_digest,
            primary_value: r.primary_value,
            bar: r.bar,
            previous_best: r.previous_best,
        }
    }
}

#[async_trait]
impl RlmStore for PgRlmStore {
    async fn put_topic_version(&self, doc: &TopicDocument) -> Result<u32, StoreError> {
        let document = serde_json::to_value(doc).map_err(malformed)?;
        let status = serde_json::to_value(doc.status)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default();
        let version: i32 = sqlx::query_scalar(
            "INSERT INTO proof_topic_version (topic_id, version, status, document, signature) \
             VALUES ($1, (SELECT COALESCE(MAX(version), 0) + 1 FROM proof_topic_version WHERE topic_id = $1), $2, $3, $4) \
             RETURNING version",
        )
        .bind(&doc.id)
        .bind(status)
        .bind(document)
        .bind(&doc.signature)
        .fetch_one(&self.pool)
        .await?;
        to_u32(version)
    }

    async fn latest_topic(
        &self,
        topic_id: &str,
    ) -> Result<Option<(u32, TopicDocument)>, StoreError> {
        let row: Option<(i32, Value)> = sqlx::query_as(
            "SELECT version, document FROM proof_topic_version WHERE topic_id = $1 \
             ORDER BY version DESC LIMIT 1",
        )
        .bind(topic_id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|(v, doc)| Ok((to_u32(v)?, serde_json::from_value(doc).map_err(malformed)?)))
            .transpose()
    }

    async fn put_rules(&self, rules: &RuleSet) -> Result<(), StoreError> {
        let current = self
            .current_rules(&rules.topic_id)
            .await?
            .map(|r| r.version);
        check_rules(rules, current)?;
        sqlx::query(
            "INSERT INTO proof_rule_version (topic_id, version, source, rules, digest) \
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(&rules.topic_id)
        .bind(to_i32(rules.version)?)
        .bind(source_str(rules.source))
        .bind(serde_json::to_value(&rules.rules).map_err(malformed)?)
        .bind(rules.digest())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn current_rules(&self, topic_id: &str) -> Result<Option<RuleSet>, StoreError> {
        let row: Option<RuleRow> = sqlx::query_as(
            "SELECT topic_id, version, source, rules FROM proof_rule_version \
             WHERE topic_id = $1 ORDER BY version DESC LIMIT 1",
        )
        .bind(topic_id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(RuleRow::into_set).transpose()
    }

    async fn rules_at(&self, topic_id: &str, version: u32) -> Result<Option<RuleSet>, StoreError> {
        let row: Option<RuleRow> = sqlx::query_as(
            "SELECT topic_id, version, source, rules FROM proof_rule_version \
             WHERE topic_id = $1 AND version = $2",
        )
        .bind(topic_id)
        .bind(to_i32(version)?)
        .fetch_optional(&self.pool)
        .await?;
        row.map(RuleRow::into_set).transpose()
    }

    async fn put_checklist(&self, row: &ChecklistRow) -> Result<(), StoreError> {
        sqlx::query(
            "INSERT INTO proof_checklist (submission_digest, topic_id, rules_version, green, failed_ids, document) \
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(&row.submission_digest)
        .bind(&row.topic_id)
        .bind(to_i32(row.rules_version)?)
        .bind(row.green)
        .bind(serde_json::to_value(&row.failed_ids).map_err(malformed)?)
        .bind(serde_json::to_value(&row.document).map_err(malformed)?)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn checklist(&self, submission_digest: &str) -> Result<Option<ChecklistRow>, StoreError> {
        let row: Option<ChecklistDbRow> = sqlx::query_as(
            "SELECT topic_id, submission_digest, rules_version, green, failed_ids, document \
             FROM proof_checklist WHERE submission_digest = $1",
        )
        .bind(submission_digest)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|r| {
            let document: Checklist = serde_json::from_value(r.document).map_err(malformed)?;
            Ok(ChecklistRow {
                topic_id: r.topic_id,
                submission_digest: r.submission_digest,
                rules_version: to_u32(r.rules_version)?,
                green: r.green,
                failed_ids: serde_json::from_value(r.failed_ids).map_err(malformed)?,
                document,
            })
        })
        .transpose()
    }

    async fn record_transition(&self, row: &TransitionRow) -> Result<(), StoreError> {
        sqlx::query(
            "INSERT INTO proof_lifecycle_event (topic_id, from_state, event, to_state, note) \
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(&row.topic_id)
        .bind(row.from.as_str())
        .bind(row.event.as_str())
        .bind(row.to.as_str())
        .bind(&row.note)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn lifecycle(&self, topic_id: &str) -> Result<Option<Lifecycle>, StoreError> {
        let rows: Vec<TransitionDbRow> = sqlx::query_as(
            "SELECT topic_id, from_state, event, to_state, note FROM proof_lifecycle_event \
             WHERE topic_id = $1 ORDER BY id",
        )
        .bind(topic_id)
        .fetch_all(&self.pool)
        .await?;
        let mut out = Vec::with_capacity(rows.len());
        for r in rows {
            out.push(TransitionRow {
                topic_id: r.topic_id,
                from: parse_state(&r.from_state)?,
                event: parse_event(&r.event)?,
                to: parse_state(&r.to_state)?,
                note: r.note,
            });
        }
        Ok(replay(topic_id, &out))
    }

    async fn put_baseline(&self, row: &BaselineRow) -> Result<(), StoreError> {
        if !row.primary_value.is_finite() {
            return Err(StoreError::Malformed("baseline primary".into()));
        }
        sqlx::query(
            "INSERT INTO proof_baseline_measurement (topic_id, rules_version, primary_value, report) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(&row.topic_id)
        .bind(to_i32(row.rules_version)?)
        .bind(row.primary_value)
        .bind(serde_json::to_value(&row.report).map_err(malformed)?)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn baseline(&self, topic_id: &str) -> Result<Option<BaselineRow>, StoreError> {
        let row: Option<BaselineDbRow> = sqlx::query_as(
            "SELECT topic_id, rules_version, primary_value, report FROM proof_baseline_measurement \
             WHERE topic_id = $1 ORDER BY rules_version DESC LIMIT 1",
        )
        .bind(topic_id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|r| {
            let report: CustomRunReport = serde_json::from_value(r.report).map_err(malformed)?;
            Ok(BaselineRow {
                topic_id: r.topic_id,
                rules_version: to_u32(r.rules_version)?,
                primary_value: r.primary_value,
                report,
            })
        })
        .transpose()
    }

    async fn put_artefact(&self, row: &ArtefactRow) -> Result<(), StoreError> {
        check_artefact(row)?;
        sqlx::query(
            "INSERT INTO proof_artefact (topic_id, submission_id, submission_digest, path, sha256, bytes, \
             primary_value, checklist_green, promoted) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(&row.topic_id)
        .bind(&row.submission_id)
        .bind(&row.submission_digest)
        .bind(&row.path)
        .bind(&row.sha256)
        .bind(i64::try_from(row.bytes).map_err(malformed)?)
        .bind(row.primary_value)
        .bind(row.checklist_green)
        .bind(row.promoted)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn artefacts(&self, topic_id: &str) -> Result<Vec<ArtefactRow>, StoreError> {
        let rows: Vec<ArtefactDbRow> = sqlx::query_as(
            "SELECT topic_id, submission_id, submission_digest, path, sha256, bytes, primary_value, \
             checklist_green, promoted FROM proof_artefact WHERE topic_id = $1 ORDER BY created_at, submission_id",
        )
        .bind(topic_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|r| {
                Ok(ArtefactRow {
                    topic_id: r.topic_id,
                    submission_id: r.submission_id,
                    submission_digest: r.submission_digest,
                    path: r.path,
                    sha256: r.sha256,
                    bytes: u64::try_from(r.bytes).map_err(malformed)?,
                    primary_value: r.primary_value,
                    checklist_green: r.checklist_green,
                    promoted: r.promoted,
                })
            })
            .collect()
    }

    async fn record_promotion(&self, row: &PromotionRow) -> Result<(), StoreError> {
        check_promotion(row)?;
        sqlx::query(
            "INSERT INTO proof_promotion_event (topic_id, submission_id, submission_digest, primary_value, bar, previous_best) \
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(&row.topic_id)
        .bind(&row.submission_id)
        .bind(&row.submission_digest)
        .bind(row.primary_value)
        .bind(row.bar)
        .bind(&row.previous_best)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn best(&self, topic_id: &str) -> Result<Option<PromotionRow>, StoreError> {
        let row: Option<PromotionDbRow> = sqlx::query_as(
            "SELECT topic_id, submission_id, submission_digest, primary_value, bar, previous_best \
             FROM proof_promotion_event WHERE topic_id = $1 ORDER BY id DESC LIMIT 1",
        )
        .bind(topic_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(PromotionRow::from))
    }

    async fn promotions(&self, topic_id: &str) -> Result<Vec<PromotionRow>, StoreError> {
        let rows: Vec<PromotionDbRow> = sqlx::query_as(
            "SELECT topic_id, submission_id, submission_digest, primary_value, bar, previous_best \
             FROM proof_promotion_event WHERE topic_id = $1 ORDER BY id",
        )
        .bind(topic_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(PromotionRow::from).collect())
    }
}
