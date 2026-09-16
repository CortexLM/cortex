//! Proof RLM persistence.
//!
//! Everything a topic's RLM produces that must outlive a process — the signed
//! topic versions, the **rule versions the RLM writes**, every submission's
//! checklist, every lifecycle transition, artefact metadata, the baseline
//! measurement, and the promotion continuum (best pointer + history) — goes
//! through [`RlmStore`]. [`PgRlmStore`] is the production implementation
//! over `crates/db` migration `0020_proof_rlm.sql`; [`MemoryRlmStore`] is the
//! CI / local implementation with the same contract. Rules land here, not
//! only in logs. Nothing here knows a challenge.

#![forbid(unsafe_code)]
#![allow(
    clippy::missing_errors_doc,
    clippy::doc_markdown,
    clippy::module_name_repetitions,
    clippy::must_use_candidate
)]

mod memory;
mod pg;

use async_trait::async_trait;
use proof_rlm::{Checklist, CustomRunReport, Lifecycle, RlmEvent, RlmState, RuleSet};
use proof_task::TopicDocument;
use serde::{Deserialize, Serialize};

pub use memory::MemoryRlmStore;
pub use pg::PgRlmStore;

/// Store failures.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// Lock poisoned (memory store).
    #[error("rlm store lock poisoned")]
    Poison,
    /// A version did not advance by exactly one (or a first version was not 1).
    #[error("rlm store: {0} version must advance by one")]
    VersionGap(&'static str),
    /// A row is malformed (bad id / digest shape, serialisation).
    #[error("rlm store: {0}")]
    Malformed(String),
    /// Database failure.
    #[error("rlm store: db: {0}")]
    Db(String),
}

impl From<sqlx::Error> for StoreError {
    fn from(e: sqlx::Error) -> Self {
        Self::Db(e.to_string())
    }
}

/// One persisted checklist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChecklistRow {
    /// Topic id.
    pub topic_id: String,
    /// Frozen submission digest (primary key).
    pub submission_digest: String,
    /// Rule version the items tick.
    pub rules_version: u32,
    /// Complete and every rule passed.
    pub green: bool,
    /// Red rule ids (empty when green).
    pub failed_ids: Vec<String>,
    /// The document itself.
    pub document: Checklist,
}

impl ChecklistRow {
    /// Row for `checklist` verified against `rules`.
    #[must_use]
    pub fn from_checklist(checklist: &Checklist, rules: &RuleSet) -> Self {
        let complete = checklist.verify_complete(rules).is_ok();
        let failed = checklist.failed_ids();
        Self {
            topic_id: checklist.topic_id.clone(),
            submission_digest: checklist.submission_digest.clone(),
            rules_version: checklist.rules_version,
            green: complete && failed.is_empty(),
            failed_ids: failed,
            document: checklist.clone(),
        }
    }
}

/// One temporary compatibility alias for a topic slug.
///
/// An alias is a **lookup convenience**: a row carries the mapping and nothing
/// else — no name, no pins, no status — so it cannot drift from the topic it
/// names. Retiring the alias is deleting the row. There is no owner default:
/// which aliases exist is what an operator's installs declared.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopicAliasRow {
    /// The alias slug that resolves to `topic_id`.
    pub alias: String,
    /// The canonical topic slug the alias names.
    pub topic_id: String,
}

/// One lifecycle move for one topic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransitionRow {
    /// Topic id.
    pub topic_id: String,
    /// State before.
    pub from: RlmState,
    /// Event applied.
    pub event: RlmEvent,
    /// State after.
    pub to: RlmState,
    /// Operator-readable note (never a secret).
    pub note: String,
}

/// One persisted topic version, as the registry view reads it.
///
/// This is a **view** over `proof_topic_version`, not a second topic table:
/// every field except `version` lives inside the signed document, which stays
/// the one source of truth. The status is `document.status`, and the
/// signature is `document.signature`; neither is duplicated here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TopicVersionRow {
    /// Topic slug (the registry key).
    pub topic_id: String,
    /// Newest persisted version for that slug.
    pub version: u32,
    /// The signed document, verbatim.
    pub document: TopicDocument,
}

/// What the RLM measured before any submission (learning continuum start).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BaselineRow {
    /// Topic id.
    pub topic_id: String,
    /// Rule version in force.
    pub rules_version: u32,
    /// Baseline primary.
    pub primary_value: f64,
    /// Run report verbatim.
    pub report: CustomRunReport,
}

/// Metadata of one artefact zip.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArtefactRow {
    /// Topic id.
    pub topic_id: String,
    /// Store row id (`pf_` + 16 hex).
    pub submission_id: String,
    /// Frozen submission digest.
    pub submission_digest: String,
    /// Where the zip lives.
    pub path: String,
    /// SHA-256 hex of the zip bytes.
    pub sha256: String,
    /// Zip size.
    pub bytes: u64,
    /// Primary value, when a report exists.
    pub primary_value: Option<f64>,
    /// Whether the checklist was green.
    pub checklist_green: bool,
    /// Whether the run was promoted.
    pub promoted: bool,
}

/// One promotion: the continuum's "new best" event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromotionRow {
    /// Topic id.
    pub topic_id: String,
    /// Winning row id.
    pub submission_id: String,
    /// Winning frozen digest.
    pub submission_digest: String,
    /// Winning primary.
    pub primary_value: f64,
    /// Bar it cleared (sealed baseline or previous best).
    pub bar: Option<f64>,
    /// Displaced best, if any.
    pub previous_best: Option<String>,
}

/// Replay lifecycle rows into a [`Lifecycle`].
#[must_use]
pub fn replay(topic_id: &str, rows: &[TransitionRow]) -> Option<Lifecycle> {
    let first = rows.first()?;
    let mut lc = Lifecycle::at(topic_id, first.from);
    for r in rows {
        lc.history.push(proof_rlm::Transition {
            from: r.from,
            event: r.event,
            to: r.to,
            note: r.note.clone(),
        });
        lc.state = r.to;
    }
    Some(lc)
}

/// Persistence contract shared by the Postgres and in-memory stores.
#[async_trait]
pub trait RlmStore: Send + Sync {
    /// Persist a signed topic document as the next version; returns that version.
    async fn put_topic_version(&self, doc: &TopicDocument) -> Result<u32, StoreError>;
    /// Newest persisted version of a topic.
    async fn latest_topic(
        &self,
        topic_id: &str,
    ) -> Result<Option<(u32, TopicDocument)>, StoreError>;

    /// Newest persisted version of **every** topic, ordered by `topic_id`.
    ///
    /// A read-only registry view over the same `proof_topic_version` rows
    /// [`Self::latest_topic`] reads. It exists so the operator CLI can list
    /// what is installed without a second table that could disagree with the
    /// signed documents. An empty result is an empty vector, not an error:
    /// nothing is installed yet is a normal state.
    async fn latest_topics(&self) -> Result<Vec<TopicVersionRow>, StoreError>;

    /// Record (or replace) a temporary alias for a topic slug.
    ///
    /// Fail-closed: the topic must already have a published version, because
    /// an alias pointing at nothing would resolve to no document and look
    /// like an unknown topic to a miner. `alias == topic_id` is refused — that
    /// is the topic's own key, not an alias.
    async fn put_alias(&self, alias: &str, topic_id: &str) -> Result<(), StoreError>;
    /// The topic slug `alias` resolves to, or `None`.
    ///
    /// `None` covers both "no such alias" and "the aliased topic has no
    /// published version", so a stale row can never resolve to an empty
    /// document.
    async fn resolve_alias(&self, alias: &str) -> Result<Option<String>, StoreError>;
    /// Every alias of one topic, ordered by alias.
    async fn aliases_for(&self, topic_id: &str) -> Result<Vec<String>, StoreError>;
    /// Remove a temporary alias. Returns whether a row was deleted.
    async fn delete_alias(&self, alias: &str) -> Result<bool, StoreError>;

    /// Persist a rule version. Must be `current + 1` (or 1 for the first).
    async fn put_rules(&self, rules: &RuleSet) -> Result<(), StoreError>;
    /// Newest rule version.
    async fn current_rules(&self, topic_id: &str) -> Result<Option<RuleSet>, StoreError>;
    /// One rule version.
    async fn rules_at(&self, topic_id: &str, version: u32) -> Result<Option<RuleSet>, StoreError>;

    /// Whether the topic's **newest** rule version was authored by its RLM
    /// ([`proof_rlm::RuleSource::Rlm`]).
    ///
    /// This is the provenance read, not a rules read: it answers "did the
    /// topic's RLM author the vector in force", which is what separates a
    /// topic that set itself up from one whose behavior is still the
    /// operator's signed document. `Ok(false)` covers three states that are
    /// deliberately not distinguished here — no rule row at all, a version
    /// seeded from the signed document (`topic_document`), and an operator
    /// edit (`operator`) — because every one of them means the RLM has not
    /// authored the current vector, and the caller's answer is the same.
    ///
    /// **Fail-closed at the call site:** an `Err` is an unreadable store, not
    /// a `false` that a caller could mistake for "not RLM-authored" (or, if
    /// inverted, for "RLM-authored").
    ///
    /// # Errors
    ///
    /// [`StoreError`] when the store cannot be read.
    async fn rlm_authored_rules(&self, topic_id: &str) -> Result<bool, StoreError>;

    /// The newest rule version's source, or `None` when the topic has no rule
    /// row at all.
    ///
    /// The operator-facing half of [`Self::rlm_authored_rules`]: a caller that
    /// has to explain *why* a topic is not RLM-authored needs the actual
    /// provenance, not a boolean. Kept as its own read rather than a richer
    /// return type so the fail-closed boolean stays trivial to audit.
    ///
    /// # Errors
    ///
    /// [`StoreError`] when the store cannot be read.
    async fn current_rules_source(
        &self,
        topic_id: &str,
    ) -> Result<Option<proof_rlm::RuleSource>, StoreError>;

    /// Persist a submission's checklist.
    ///
    /// A row that already exists for `submission_digest` is replaced
    /// (topic_id / rules_version / green / failed_ids / document) so a miner
    /// resubmit of the same artefact after a false anti-cheat reject can
    /// store the latest inspection. Postgres keeps the original `created_at`.
    async fn put_checklist(&self, row: &ChecklistRow) -> Result<(), StoreError>;
    /// A submission's checklist.
    async fn checklist(&self, submission_digest: &str) -> Result<Option<ChecklistRow>, StoreError>;

    /// Append a lifecycle move.
    async fn record_transition(&self, row: &TransitionRow) -> Result<(), StoreError>;
    /// Replay a topic's lifecycle.
    async fn lifecycle(&self, topic_id: &str) -> Result<Option<Lifecycle>, StoreError>;

    /// Persist the baseline measurement for a rule version.
    async fn put_baseline(&self, row: &BaselineRow) -> Result<(), StoreError>;
    /// Newest baseline measurement.
    async fn baseline(&self, topic_id: &str) -> Result<Option<BaselineRow>, StoreError>;

    /// Persist artefact metadata.
    ///
    /// A row that already exists for `(topic_id, submission_id)` is replaced
    /// (path / sha256 / bytes / digest / primary / checklist / promoted) so a
    /// rare allocator collision still stores the zip that now sits on disk.
    async fn put_artefact(&self, row: &ArtefactRow) -> Result<(), StoreError>;
    /// Every artefact of a topic, oldest first.
    async fn artefacts(&self, topic_id: &str) -> Result<Vec<ArtefactRow>, StoreError>;
    /// Highest numeric `pf_` id among persisted artefact rows, if any.
    async fn max_artefact_numeric_id(&self) -> Result<Option<u64>, StoreError>;

    /// Append a promotion event.
    async fn record_promotion(&self, row: &PromotionRow) -> Result<(), StoreError>;
    /// The current best (newest promotion), if any.
    async fn best(&self, topic_id: &str) -> Result<Option<PromotionRow>, StoreError>;
    /// Promotion history, oldest first.
    async fn promotions(&self, topic_id: &str) -> Result<Vec<PromotionRow>, StoreError>;
}

/// Numeric id from `pf_` + 16 hex. `None` if the string is not a store row id.
#[must_use]
pub fn parse_row_id(id: &str) -> Option<u64> {
    id.strip_prefix("pf_")
        .filter(|h| h.len() == 16 && h.bytes().all(|b| b.is_ascii_hexdigit()))
        .and_then(|h| u64::from_str_radix(h, 16).ok())
}

fn is_row_id(id: &str) -> bool {
    parse_row_id(id).is_some()
}

fn check_artefact(row: &ArtefactRow) -> Result<(), StoreError> {
    if !is_row_id(&row.submission_id) || !proof_canon_hex64(&row.sha256) {
        return Err(StoreError::Malformed(format!(
            "artefact row {} / {}",
            row.submission_id, row.sha256
        )));
    }
    Ok(())
}

fn check_promotion(row: &PromotionRow) -> Result<(), StoreError> {
    if !is_row_id(&row.submission_id) || !row.primary_value.is_finite() {
        return Err(StoreError::Malformed(format!(
            "promotion row {}",
            row.submission_id
        )));
    }
    Ok(())
}

fn proof_canon_hex64(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn check_rules(rules: &RuleSet, current: Option<u32>) -> Result<(), StoreError> {
    rules
        .validate()
        .map_err(|e| StoreError::Malformed(e.to_string()))?;
    let want = current.map_or(1, |c| c.saturating_add(1));
    if rules.version != want {
        return Err(StoreError::VersionGap("rules"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replay_rebuilds_state_from_rows() {
        assert!(replay("t", &[]).is_none());
        let rows = [
            TransitionRow {
                topic_id: "t".into(),
                from: RlmState::Open,
                event: RlmEvent::SubmissionReceived,
                to: RlmState::Evaluating,
                note: "a".into(),
            },
            TransitionRow {
                topic_id: "t".into(),
                from: RlmState::Evaluating,
                event: RlmEvent::VerdictRecorded,
                to: RlmState::Open,
                note: "b".into(),
            },
        ];
        let lc = replay("t", &rows).expect("lifecycle");
        assert_eq!(lc.state, RlmState::Open);
        assert_eq!(lc.history.len(), 2);
        assert_eq!(lc.history[0].note, "a");
    }

    #[test]
    fn row_shapes_are_checked_before_any_write() {
        let good = ArtefactRow {
            topic_id: "t".into(),
            submission_id: "pf_0000000000000001".into(),
            submission_digest: "d".into(),
            path: "/x".into(),
            sha256: "ab".repeat(32),
            bytes: 1,
            primary_value: None,
            checklist_green: false,
            promoted: false,
        };
        check_artefact(&good).expect("good");
        let mut bad = good.clone();
        bad.submission_id = "nope".into();
        assert!(check_artefact(&bad).is_err());
        bad = good;
        bad.sha256 = "zz".into();
        assert!(check_artefact(&bad).is_err());
        let promo = PromotionRow {
            topic_id: "t".into(),
            submission_id: "pf_0000000000000001".into(),
            submission_digest: "d".into(),
            primary_value: f64::NAN,
            bar: None,
            previous_best: None,
        };
        assert!(check_promotion(&promo).is_err());
    }

    #[test]
    fn parse_row_id_reads_zero_padded_hex() {
        assert_eq!(parse_row_id("pf_0000000000000000"), Some(0));
        assert_eq!(parse_row_id("pf_00000000000000ff"), Some(0xff));
        assert_eq!(parse_row_id("pf_0000000000000001"), Some(1));
        assert!(parse_row_id("pf_1").is_none());
        assert!(parse_row_id("nope").is_none());
    }
}
