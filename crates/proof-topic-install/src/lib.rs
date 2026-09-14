//! Proof **topic install executor**: applying a bundle's RLM section.
//!
//! `proof-admin topic install` is the trigger; this crate is the work. A
//! bundle carries the signed [`TopicDocument`](proof_task::TopicDocument)
//! plus an **RLM section** that owns everything topic-specific — its SQL
//! migrations, the APIs it exposes, its anti-cheat rules, its submission
//! format, its scoring, and the run handler it wants bound.
//!
//! [`proof_topic_bundle`] carries that section **verbatim and opaque**: it
//! checks the shape and hands the bytes over, so no topic behavior is
//! compiled into a binary. This crate is the other half of that boundary — it
//! is the *consumer* that applies the parts an install knows how to apply,
//! and it does so under two closed gates:
//!
//! | Gate | What it refuses |
//! |------|-----------------|
//! | [`proof_topic_sql_guard`] | a migration that names a `proof_*` object, a role, the sqlx bookkeeping table, or any object outside the topic's own namespace; `DROP DATABASE` / `SCHEMA` / `ROLE`; privilege changes; server-side file access; `SECURITY DEFINER` |
//! | [`handler`] | a handler that is not an allow-listed run backend — never a path, a URL, or a command line |
//!
//! Both gates run **before** anything is applied, and both refuse on doubt.
//!
//! # Where the pieces live
//!
//! - [`section`] reads the RLM section's parts strictly, and carries the rest.
//! - [`install`] is the engine: migrations, routes, rules, binding, journal.
//! - [`routes`] is the **read** side of the routes an install recorded: the
//!   dynamic mux the challenge answers `/challenge/{topic_id}/…` from, behind
//!   a cache an install invalidates.
//! - [`proof_topic_sql_guard`] is the migration deny-list (its own crate: it
//!   is pure text analysis, and keeping it separate means it can be reasoned
//!   about — and tested — without a database).
//! - [`handler`] is the run-backend allow-list.
//!
//! # What this crate does not do
//!
//! It does not publish a document (the operator's bearer does that), does not
//! seal a baseline (the operator does, from the RLM's measurement), does not
//! move a topic's status, and does not decide what a rule, a metric, a task,
//! or a scoring function *means*. It records the executor binding so an audit
//! can see what a topic was installed with; the signed document stays the one
//! source of truth the scoring path reads.

#![forbid(unsafe_code)]
#![allow(
    clippy::missing_errors_doc,
    clippy::module_name_repetitions,
    clippy::must_use_candidate,
    clippy::doc_markdown
)]

pub mod handler;
pub mod install;
pub mod routes;
pub mod section;

pub use handler::{bound_runner, check_handler, resolve_handler, Handler, HandlerError};
pub use install::{
    install_history, latest_install, topic_routes, ExecutorBinding, InstallReport, InstallRequest,
    InstallRow, InstallState, Installer, SetupSummary, VMS_PER_SUBMISSION,
};
pub use proof_topic_sql_guard::{
    blank_statements, check_migration, check_statement, is_topic_scoped, split_statements,
    MigrationDenied, Statement, DENIED_DROP_KINDS, DENIED_FUNCTIONS, DENIED_OBJECTS, DENIED_VERBS,
    OWNED_TABLES, OWNED_TABLE_PREFIX,
};
pub use routes::{is_topic_id, PgTopicRoutes, Resolved, TopicRouteMux, TopicRouteSource};
pub use section::{
    is_api_method, is_relative_api_path, read_section, ApiRoute, Migration, SectionPlan, MAX_APIS,
    MAX_MIGRATIONS, MAX_MIGRATION_SQL_BYTES, READ_KEYS,
};

/// Why an install refused or failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InstallError {
    /// A migration statement reached outside the topic's namespace.
    #[error("{0}")]
    MigrationDenied(#[from] MigrationDenied),
    /// A migration was checked and allowed, then the database refused it.
    #[error("migration {name:?} failed at statement {ordinal}: {detail}")]
    MigrationFailed {
        /// Migration name from the bundle.
        name: String,
        /// Statement ordinal, or 0 when the transaction itself failed.
        ordinal: usize,
        /// What the database said.
        detail: String,
    },
    /// The bundle carries more migrations than an install applies.
    #[error("bundle carries {count} migrations, at most {MAX_MIGRATIONS} are applied")]
    TooManyMigrations {
        /// How many it carried.
        count: usize,
    },
    /// A handler outside the allow-list.
    #[error("handler refused: {0}")]
    HandlerNotAllowed(String),
    /// The signed document's own binding is malformed.
    #[error("topic binding: {0}")]
    Binding(String),
    /// An open custom topic whose id this host does not register.
    #[error(
        "the signed document is an open custom topic whose metric.custom_id {custom_id:?} is not \
         registered on this host (registered: {registered:?}); an unregistered id answers 503, so \
         the install refuses rather than publishing a topic that cannot score"
    )]
    CustomIdNotRegistered {
        /// The id the document names.
        custom_id: String,
        /// The ids this host registers.
        registered: Vec<String>,
    },
    /// A part of the RLM section is malformed or carries an unknown key.
    #[error("rlm.{part}: {why}")]
    Section {
        /// Which part (`migrations[0]`, `apis`, `rules`, …).
        part: String,
        /// What is wrong.
        why: String,
    },
    /// The rule vector was refused by the shared shape check.
    #[error("rules: {0}")]
    Rules(String),
    /// The rule store refused.
    #[error("store: {0}")]
    Store(String),
    /// The database refused.
    #[error("db: {0}")]
    Db(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pins this crate exists to hold, asserted where a future edit
    /// would have to see them.
    #[test]
    fn the_vms_per_submission_pin_is_one() {
        assert_eq!(VMS_PER_SUBMISSION, 1);
        let binding = ExecutorBinding {
            handler: Handler::VmBacked.as_str().to_owned(),
            runner_id: None,
            custom_id: "c".into(),
            pack_digest: None,
            vms_per_submission: VMS_PER_SUBMISSION,
            submission_format_digest: None,
            scoring_digest: None,
        };
        let json = serde_json::to_value(&binding).expect("json");
        assert_eq!(json["vms_per_submission"], 1);
    }

    #[test]
    fn every_owned_proof_table_is_denied_by_name() {
        // The deny-list is enforced by prefix, so a new proof_* table is
        // protected automatically; this asserts the readable list keeps up
        // with the migrations, so a refusal can name the object.
        for table in OWNED_TABLES {
            assert!(table.starts_with(OWNED_TABLE_PREFIX), "{table}");
        }
        assert!(OWNED_TABLES.contains(&"proof_topic_version"));
        assert!(OWNED_TABLES.contains(&"proof_rule_version"));
        assert!(OWNED_TABLES.contains(&"proof_topic_install"));
    }

    #[test]
    fn error_messages_name_the_step_and_stay_actionable() {
        let denied = InstallError::MigrationDenied(MigrationDenied {
            ordinal: 2,
            statement: "DROP TABLE proof_rule_version".into(),
            what: "proof_rule_version".into(),
            why: "owned".into(),
        });
        let text = denied.to_string();
        assert!(text.contains("statement 2"), "{text}");
        assert!(text.contains("proof_rule_version"), "{text}");

        let custom = InstallError::CustomIdNotRegistered {
            custom_id: "metric".into(),
            registered: vec!["other".into()],
        };
        assert!(custom.to_string().contains("503"), "{custom}");
    }
}
