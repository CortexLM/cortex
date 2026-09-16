//! The install engine, end to end against Postgres.
//!
//! These tests are DB-gated: they run when `DATABASE_URL` names a Postgres
//! instance the test role can create a schema in, and skip silently
//! otherwise. That is the same convention the store's own contract tests use,
//! so `cargo test --workspace` stays green on a laptop with no database while
//! CI (and a staging host) exercises the real thing.
//!
//! What they cover is the part unit tests cannot: that a **permitted**
//! migration actually applies, that its objects land in the topic's
//! namespace, that a denied one leaves nothing behind, that a re-run resumes
//! from the journal instead of re-applying, and that the routes and rules an
//! install records are the ones the scoring path will read.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use proof_rlm_store::{MemoryRlmStore, PgRlmStore, RlmStore};
use proof_task::{
    default_adamw, holdout_commitment, synthetic_holdout, MetricDirection, MetricFamily,
    MetricSpec, PayoutMode, TopicDocument, TopicStatus, FLOPS_BUDGET_MAX, STRATUM_SIZE,
};
use proof_topic_install::install::{InstallRequest, InstallState, Installer, SetupSummary};
use proof_topic_install::{
    latest_install, topic_routes, InstallError, PgTopicRoutes, Resolved, TopicRouteMux,
    VMS_PER_SUBMISSION,
};
use sqlx::PgPool;
use std::sync::Arc;

/// The test database, or `None` when the suite should skip.
async fn test_pool() -> Option<(db::TestPool, PgPool)> {
    let url = std::env::var("DATABASE_URL")
        .ok()
        .map(|u| u.trim().to_owned())
        .filter(|u| !u.is_empty())?;
    let tp = match db::test_pool_with_url(&url).await {
        Ok(tp) => tp,
        Err(e) => panic!("test_pool: {e}"),
    };
    let pool = tp.pool().clone();
    Some((tp, pool))
}

/// A signed custom topic selecting an in-guest runner, the shape a real
/// bundle carries.
fn topic(id: &str) -> TopicDocument {
    let mut doc = TopicDocument {
        id: id.into(),
        statement: "Score the pinned pack with the pinned runner.".into(),
        payout_mode: PayoutMode::Discovery,
        metric: MetricSpec {
            family: MetricFamily::Custom,
            primary: "primary_value".into(),
            direction: MetricDirection::Max,
            unit: "rate".into(),
            epsilon_rel: 0.05,
            custom_id: format!("{id}-metric"),
            ..MetricSpec::default()
        },
        baseline: default_adamw(FLOPS_BUDGET_MAX),
        holdout_commitment: holdout_commitment(&synthetic_holdout(STRATUM_SIZE, 1)),
        status: TopicStatus::Draft,
        ..TopicDocument::default()
    };
    doc.constraints.params.insert(
        proof_experiment::PARAM_RUNNER.into(),
        "operator_adaptor_v0".into(),
    );
    doc.constraints
        .params
        .insert(proof_experiment::PARAM_PACK_DIGEST.into(), digest());
    doc.signature = "ab".repeat(64);
    doc
}

fn digest() -> String {
    format!("sha256:{}", "cd".repeat(32))
}

/// An install request over `doc` with the given RLM section.
fn request<'a>(doc: &'a TopicDocument, rlm: &'a str) -> InstallRequest<'a> {
    InstallRequest {
        topic: doc,
        bundle_digest: digest(),
        environment: "staging".into(),
        rlm_raw: rlm,
        registered_custom: vec![format!("{}-metric", doc.id)],
        skip_baseline: false,
        authored: None,
    }
}

/// The RLM section a real bundle carries, in the shape the section reader
/// reads.
fn section(id: &str) -> String {
    format!(
        r#"{{
            "rules": [
                {{"id": "no_short_circuit", "text": "the harness must run the task"}},
                {{"id": "no_holdout_leak", "text": "the artefact must not carry holdout records"}}
            ],
            "migrations": [
                {{"name": "0001_scratch", "sql": "CREATE TABLE {id}_scratch (id TEXT, note TEXT)"}},
                {{"name": "0002_index", "sql": "CREATE INDEX {id}_scratch_idx ON {id}_scratch (id)"}}
            ],
            "apis": [
                {{"path": "status", "method": "GET", "summary": "topic status"}},
                {{"path": "runs/{id}", "method": "GET"}}
            ],
            "submission_format": {{"kind": "tar", "max_bytes": 5242880}},
            "scoring": {{"primary": "primary_value", "epsilon_rel": 0.05}},
            "handler": "harbor"
        }}"#
    )
    .replace("{id}", id)
}

/// The cross-topic namespace collision, end to end through the install.
///
/// `-` → `_` is injective, but its prefixes are not prefix-free: `aa` is a
/// prefix of `aa-b`'s mapped form `aa_b`, so the bare name `aa_b_scratch` sits
/// inside **both** namespaces. The per-topic guard accepts it for each topic
/// on its own — it sees one topic — so a migration approved for `aa` could
/// read, modify, or drop a table belonging to `aa-b` in the shared database.
///
/// The install is where the registry is visible, so it refuses there, before
/// the journal opens (nothing is written).
#[tokio::test]
async fn a_migration_that_reaches_a_sibling_topics_namespace_is_refused() {
    let Some((_tp, pool)) = test_pool().await else {
        return;
    };
    let store = PgRlmStore::new(pool.clone());
    // A sibling is registered whose mapped prefix the shorter topic's
    // migration would also match.
    store
        .put_topic_version(&topic("aa-b"))
        .await
        .expect("register the sibling");

    let installer = Installer {
        pool: &pool,
        store: &store,
    };
    let doc = topic("aa");
    // A migration whose object is inside **both** namespaces: `aa` reads it as
    // `aa` + `b_scratch`, `aa-b` reads it as `aa-b` + `scratch`.
    let colliding = r#"{
        "rules": [
            {"id": "no_short_circuit", "text": "the harness must run the task"}
        ],
        "migrations": [
            {"name": "0001_shared", "sql": "CREATE TABLE aa_b_scratch (id TEXT)"}
        ]
    }"#;
    let err = installer
        .install(
            &request(&doc, colliding),
            SetupSummary::Skipped {
                reason: "--skip-baseline".into(),
            },
        )
        .await
        .expect_err("a migration reaching a sibling's namespace must be refused");

    let InstallError::CrossTopicClaim {
        ref object,
        ref this,
        ref other,
        ..
    } = err
    else {
        panic!("want CrossTopicClaim, got {err:?}");
    };
    assert_eq!(object, "aa_b_scratch");
    assert_eq!(this, "aa");
    assert_eq!(other, "aa-b");
    assert!(
        err.to_string().contains("aa_b_scratch"),
        "the refusal names the object: {err}"
    );

    // Nothing was written: the refusal is pre-flight, so there is no journal
    // row and no table.
    let rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM proof_topic_install WHERE topic_id = 'aa'")
            .fetch_one(&pool)
            .await
            .expect("count");
    assert_eq!(rows, 0, "a collision writes nothing at all");
    let exists: Option<String> = sqlx::query_scalar("SELECT to_regclass('aa_b_scratch')::text")
        .fetch_one(&pool)
        .await
        .expect("probe");
    assert_eq!(exists, None, "the colliding table was not created");
}

/// A **hyphenated** topic id installs its migrations end to end.
///
/// Every real topic id is a hyphen slug (`[a-z0-9][a-z0-9-]{1,62}`), and a
/// bare SQL identifier cannot contain a hyphen. The defect this pins: the
/// migration guard required a literal `{topic_id}_` prefix, so its requirement
/// was unsatisfiable — `CREATE TABLE real-topic_scratch` is a syntax error at
/// the first `-`, and the underscore spelling was refused as unscoped. No real
/// topic could install a migration, and `topic install --drive-rlm` would only
/// discover it *after* the paid baseline.
///
/// The suite's other tests all use `tb4`, which is why this went unnoticed.
#[tokio::test]
async fn a_hyphenated_topic_id_installs_its_migrations() {
    let Some((tp, pool)) = test_pool().await else {
        return;
    };
    let store = PgRlmStore::new(pool.clone());
    let id = "fixture-topic-v0";
    let doc = topic(id);
    let installer = Installer {
        pool: &pool,
        store: &store,
    };
    // The table the migration creates, spelled with the identifier-safe
    // prefix the guard maps the id to. `section()` appends `_scratch`.
    let table = "fixture_topic_v0_scratch";
    let report = installer
        .install(
            &request(&doc, &section("fixture_topic_v0")),
            SetupSummary::Skipped {
                reason: "--skip-baseline".into(),
            },
        )
        .await
        .expect("a hyphenated topic's bundle installs");

    assert_eq!(report.topic_id, id);
    assert_eq!(
        report.migrations_applied,
        ["0001_scratch", "0002_index"],
        "both migrations apply for a hyphenated id"
    );
    // The migration really ran: its table exists in this test's schema.
    let exists: Option<String> =
        sqlx::query_scalar(&format!("SELECT to_regclass('{table}')::text"))
            .fetch_one(&pool)
            .await
            .expect("probe the table");
    assert_eq!(
        exists.as_deref(),
        Some(table),
        "the hyphenated topic's migration created its table"
    );

    // And a migration reaching a sibling topic is still refused, in both
    // spellings — the fix widened the namespace, it did not remove it.
    for sql in [
        "CREATE TABLE fixture_topic_v1_scratch (id TEXT)",
        "CREATE TABLE topic_scratch (id TEXT)",
        "CREATE TABLE proof_rule_version (id TEXT)",
    ] {
        let section = format!(r#"{{"migrations": [{{"name": "0003_bad", "sql": "{sql}"}}]}}"#);
        let err = installer
            .install(
                &request(&doc, &section),
                SetupSummary::Skipped { reason: "x".into() },
            )
            .await
            .expect_err("a sibling's table is not this topic's");
        assert!(
            matches!(err, proof_topic_install::InstallError::MigrationDenied(_)),
            "{sql:?} must be refused by the guard, got {err:?}"
        );
    }

    tp.drop_schema().await.expect("drop");
}

/// The happy path: every step applies, and the journal records it.
#[tokio::test]
async fn a_permitted_bundle_installs_and_the_journal_records_it() {
    let Some((tp, pool)) = test_pool().await else {
        return;
    };
    let store = PgRlmStore::new(pool.clone());
    let doc = topic("tb4");
    let installer = Installer {
        pool: &pool,
        store: &store,
    };
    let report = installer
        .install(
            &request(&doc, &section("tb4")),
            SetupSummary::Skipped {
                reason: "--skip-baseline".into(),
            },
        )
        .await
        .expect("the bundle installs");

    assert_eq!(report.topic_id, "tb4");
    assert_eq!(
        report.migrations_applied,
        ["0001_scratch", "0002_index"],
        "both migrations apply, in bundle order"
    );
    assert!(report.migrations_skipped.is_empty());
    assert_eq!(report.rules_version, 1);
    assert_eq!(report.rule_ids, ["no_short_circuit", "no_holdout_leak"]);
    assert_eq!(report.binding.handler, "harbor");
    assert_eq!(
        report.binding.runner_id.as_deref(),
        Some("operator_adaptor_v0"),
        "the signed document's runner is what is bound"
    );
    assert_eq!(report.binding.vms_per_submission, VMS_PER_SUBMISSION);
    assert_eq!(report.binding.vms_per_submission, 1);
    assert!(report.binding.submission_format_digest.is_some());
    assert!(report.binding.scoring_digest.is_some());
    assert_eq!(
        report.apis.len(),
        2,
        "both routes are registered: {:?}",
        report.apis
    );

    // The migration really created its objects, in the topic's namespace.
    let exists: Option<String> = sqlx::query_scalar("SELECT to_regclass('tb4_scratch')::text")
        .fetch_one(&pool)
        .await
        .expect("probe the table");
    assert_eq!(exists.as_deref(), Some("tb4_scratch"));

    // The rules the scoring path reads are the ones the install landed.
    let rules = store
        .current_rules("tb4")
        .await
        .expect("rules")
        .expect("some");
    assert_eq!(rules.version, 1);
    assert_eq!(rules.topic_id, "tb4");

    // The routes are readable back through the mux's own query.
    let routes = topic_routes(&pool, "tb4").await.expect("routes");
    assert_eq!(routes.len(), 2);
    assert_eq!(
        routes[0].path, "runs/tb4",
        "the section's `{{id}}` is the topic id: {routes:?}"
    );
    assert_eq!(routes[0].method, "GET");
    assert_eq!(routes[0].summary, "");
    assert_eq!(routes[1].path, "status");
    assert_eq!(routes[1].summary, "topic status");
    assert!(
        routes.iter().all(|r| !r.path.starts_with('/')),
        "stored paths are relative: {routes:?}"
    );

    // The **mux** reads what the install wrote.
    mux_reads_what_the_install_wrote(&pool).await;

    // The journal's newest row is this run, in the `applied` state.
    let row = latest_install(&pool, "tb4")
        .await
        .expect("journal")
        .expect("a row");
    assert_eq!(row.state, "applied");
    assert_eq!(row.environment, "staging");
    assert_eq!(row.rules_version, Some(1));
    assert_eq!(row.migrations, ["0001_scratch", "0002_index"]);
    assert_eq!(row.binding["vms_per_submission"], 1);

    tp.drop_schema().await.expect("drop");
}

/// The **RLM's own set** is what an install applies, and the journal records
/// its authorship per part.
///
/// This is the whole point of the authorship pin, end to end against a real
/// database: when the topic's RLM authored a set, that set — not the
/// operator's bundle section — is what lands. The migrations, routes,
/// submission format, and pin policy come from the RLM's answer, and the
/// journal's `binding.authorship` names `rlm` as the author of every part,
/// with each part's digest.
///
/// The operator's section is deliberately **different** in this test (a
/// different table, a different route, a different submission format), so a
/// run that quietly applied the bundle would be visible in every assertion.
#[tokio::test]
async fn the_rlm_authored_set_is_what_an_install_applies() {
    let Some((tp, pool)) = test_pool().await else {
        return;
    };
    let store = PgRlmStore::new(pool.clone());
    let doc = topic("rlm-topic");
    let installer = Installer {
        pool: &pool,
        store: &store,
    };
    // The operator's bundle: a *different* set, which must not be applied.
    let operator_section = r#"{
            "rules": [{"id": "operator_rule", "text": "the operator's vector"}],
            "migrations": [{"name": "0001_operator", "sql": "CREATE TABLE rlm_topic_operator (id TEXT)"}],
            "apis": [{"path": "operator-route", "method": "GET"}],
            "submission_format": {"kind": "operator-tar", "max_bytes": 1}
        }"#;
    // The RLM's answer: every part present, and every one of them different.
    let authored = rlm_authored_set();
    let request = InstallRequest {
        topic: &doc,
        bundle_digest: digest(),
        environment: "staging".into(),
        rlm_raw: operator_section,
        registered_custom: vec!["rlm-topic-metric".into()],
        skip_baseline: false,
        authored: Some(&authored),
    };
    let report = installer
        .install(
            &request,
            SetupSummary::Baselined {
                rules_version: 1,
                baseline_primary: "0.5".into(),
            },
        )
        .await
        .expect("the RLM's set installs");

    // The RLM's parts landed, not the operator's.
    assert_eq!(report.migrations_applied, ["0001_rlm"]);
    assert_eq!(report.rule_ids, ["rlm_rule"]);
    assert_eq!(report.apis, ["GET /rlm-status"]);
    assert_eq!(
        report.binding.submission_format_digest,
        Some(proof_topic_authoring::digest_of(
            &serde_json::json!({"kind": "rlm-tar", "max_bytes": 4_194_304})
        )),
        "the digest is the RLM's submission format, not the bundle's"
    );
    let rlm_table: Option<String> = sqlx::query_scalar("SELECT to_regclass('rlm_topic_rlm')::text")
        .fetch_one(&pool)
        .await
        .expect("probe");
    assert_eq!(rlm_table.as_deref(), Some("rlm_topic_rlm"));
    let operator_table: Option<String> =
        sqlx::query_scalar("SELECT to_regclass('rlm_topic_operator')::text")
            .fetch_one(&pool)
            .await
            .expect("probe");
    assert_eq!(
        operator_table, None,
        "the operator's migration never ran: the RLM's set is the topic's behavior"
    );
    let routes = topic_routes(&pool, "rlm-topic").await.expect("routes");
    assert_eq!(routes.len(), 1);
    assert_eq!(routes[0].path, "rlm-status");
    assert_authorship_journal_names_every_part(&pool, &authored).await;

    // A second install of the **same** topic with no authored set falls back
    // to the operator's section and says so: the journal's provenance is what
    // distinguishes the two, and the publish gate reads it.
    let fallback = installer
        .install(
            &InstallRequest {
                authored: None,
                ..request
            },
            SetupSummary::NotDriven {
                reason: "no RLM set".into(),
            },
        )
        .await
        .expect("the operator's section still installs, with its own provenance");
    assert_eq!(fallback.migrations_applied, ["0001_operator"]);
    let row = latest_install(&pool, "rlm-topic")
        .await
        .expect("journal")
        .expect("a row");
    assert_eq!(
        row.binding["authorship"]["source"], "topic_document",
        "an install from the bundle says so: {row:?}"
    );
    assert_eq!(
        row.binding["authorship"]["parts"]["rules"]["source"], "topic_document",
        "the operator's vector is not RLM-authored, whatever it contains"
    );

    tp.drop_schema().await.expect("drop");
}

/// The set a topic's RLM authors in this suite: every part present, and every
/// one of them distinguishable from the operator's bundle.
fn rlm_authored_set() -> proof_topic_authoring::TopicAuthoring {
    use proof_topic_authoring::{AuthoredApi, AuthoredMigration, PinPolicy, TopicAuthoring};
    TopicAuthoring {
        schema_version: proof_topic_authoring::AUTHORING_SCHEMA,
        topic_id: "rlm-topic".into(),
        rules: vec![proof_task::ChecklistRule {
            id: "rlm_rule".into(),
            text: "the RLM's own vector".into(),
        }],
        migrations: vec![AuthoredMigration {
            name: "0001_rlm".into(),
            sql: "CREATE TABLE rlm_topic_rlm (id TEXT, note TEXT)".into(),
        }],
        apis: vec![AuthoredApi {
            path: "rlm-status".into(),
            method: "GET".into(),
            summary: "the RLM's own route".into(),
        }],
        submission_format: serde_json::json!({"kind": "rlm-tar", "max_bytes": 4_194_304}),
        pin_policy: PinPolicy {
            epsilon_nll_min: Some(0.05),
            ..PinPolicy::none()
        },
    }
}

/// The journal says **who authored every part**, with a digest each.
///
/// This is what makes "the RLM authored this topic" a fact an audit reads back
/// per part rather than a label on the row as a whole, and it is the entry the
/// publish gate's provenance read is about.
async fn assert_authorship_journal_names_every_part(
    pool: &PgPool,
    authored: &proof_topic_authoring::TopicAuthoring,
) {
    let row = latest_install(pool, "rlm-topic")
        .await
        .expect("journal")
        .expect("a row");
    let authorship = &row.binding["authorship"];
    assert_eq!(authorship["source"], "rlm");
    assert_eq!(authorship["digest"], authored.digest());
    for part in proof_topic_authoring::PARTS {
        assert_eq!(
            authorship["parts"][part]["source"], "rlm",
            "{part} must name the RLM as its author: {authorship}"
        );
        assert!(
            authorship["parts"][part]["digest"]
                .as_str()
                .is_some_and(|d| d.starts_with("sha256:")),
            "{part} must carry its own digest: {authorship}"
        );
    }
    assert_eq!(authorship["parts"]["migrations"]["names"][0], "0001_rlm");
    assert_eq!(authorship["parts"]["apis"]["routes"][0], "GET /rlm-status");
}

/// A migration whose **function body** reaches a sibling's namespace is
/// refused.
///
/// Greptile's second P1, reproduced here before fixing. The collision check
/// read `statement.blanked`, which blanks dollar-quoted bodies — so the object
/// names *inside* a body were never compared to the registry. Topic `aa` could
/// install `aa_delete_sibling()` whose body is `DELETE FROM aa_b_scratch`
/// while sibling `aa-b` was registered, and calling it would delete the
/// sibling's rows.
///
/// The body is what runs, so the body is what the check must read
/// (`statement.bodies`).
#[tokio::test]
async fn a_function_body_that_reaches_a_sibling_is_refused() {
    let Some((tp, pool)) = test_pool().await else {
        return;
    };
    let store = PgRlmStore::new(pool.clone());
    store
        .put_topic_version(&topic("aa-b"))
        .await
        .expect("register the sibling");

    let installer = Installer {
        pool: &pool,
        store: &store,
    };
    let doc = topic("aa");
    let body = r#"{
        "rules": [{"id": "no_short_circuit", "text": "the harness must run the task"}],
        "migrations": [{
            "name": "0001_body",
            "sql": "CREATE FUNCTION aa_delete_sibling() RETURNS void AS $$ DELETE FROM aa_b_scratch $$ LANGUAGE sql"
        }]
    }"#;
    let err = installer
        .install(
            &request(&doc, body),
            SetupSummary::Skipped {
                reason: "--skip-baseline".into(),
            },
        )
        .await
        .expect_err("a function body reaching a sibling must be refused");
    let InstallError::CrossTopicClaim {
        ref object,
        ref this,
        ref other,
        ..
    } = err
    else {
        panic!("want CrossTopicClaim, got {err:?}");
    };
    assert_eq!(object, "aa_b_scratch");
    assert_eq!(this, "aa");
    assert_eq!(other, "aa-b");
    // Nothing was written: the refusal is pre-flight.
    let rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM proof_topic_install WHERE topic_id = 'aa'")
            .fetch_one(&pool)
            .await
            .expect("count");
    assert_eq!(rows, 0, "a collision writes nothing at all");
    let exists: Option<String> = sqlx::query_scalar("SELECT to_regproc('aa_delete_sibling')::text")
        .fetch_one(&pool)
        .await
        .expect("probe");
    assert_eq!(exists, None, "the function was not created");

    tp.drop_schema().await.expect("drop");
}

/// Two **unpublished** installs whose prefixes overlap cannot both claim the
/// same object.
///
/// Greptile's first P1, reproduced here before fixing. The collision check
/// enumerated `proof_topic_version` only, so a topic that had installed but
/// not yet published was invisible to it: `aa` and `aa-b` could both install
/// and both create `aa_b_scratch`. A topic claims a namespace by installing
/// into it, so the check has to read the **journal** too — and it has to do so
/// under the claim's own lock, or two concurrent installs each read a registry
/// that lacks the other.
///
/// This test installs `aa` (unpublished: it has a journal row and no
/// `proof_topic_version` row for `aa` beyond what the install itself writes),
/// then installs `aa-b` and asserts the refusal.
#[tokio::test]
async fn two_unpublished_installs_cannot_claim_the_same_object() {
    let Some((tp, pool)) = test_pool().await else {
        return;
    };
    let store = PgRlmStore::new(pool.clone());
    let installer = Installer {
        pool: &pool,
        store: &store,
    };
    // `aa` installs a table named `aa_b_scratch`, which `aa-b` also claims.
    let first = r#"{
        "rules": [{"id": "no_short_circuit", "text": "the harness must run the task"}],
        "migrations": [{"name": "0001_shared", "sql": "CREATE TABLE aa_b_scratch (id TEXT)"}]
    }"#;
    installer
        .install(
            &request(&topic("aa"), first),
            SetupSummary::Skipped {
                reason: "--skip-baseline".into(),
            },
        )
        .await
        .expect("the first install is alone and therefore permitted");

    // `aa-b` is **not** registered as a published document; the only record
    // that `aa` exists is its install row. The check must still see it.
    let err = installer
        .install(
            &request(&topic("aa-b"), first),
            SetupSummary::Skipped {
                reason: "--skip-baseline".into(),
            },
        )
        .await
        .expect_err("an unpublished sibling's claim must be visible");
    assert!(
        matches!(err, InstallError::CrossTopicClaim { .. }),
        "{err:?}"
    );
    assert!(
        err.to_string().contains("aa_b_scratch"),
        "the refusal names the object: {err}"
    );

    tp.drop_schema().await.expect("drop");
}

/// The claim is atomic: two installs racing for overlapping namespaces cannot
/// both win.
///
/// The read-then-claim is one transaction under an advisory lock
/// (`claim_namespace`), so the second install sees the first's `pending` row.
/// Without the lock both read a registry lacking the other and both commit —
/// the same defect with a race on top of it.
#[tokio::test]
async fn two_concurrent_installs_cannot_both_claim_a_namespace() {
    let Some((tp, pool)) = test_pool().await else {
        return;
    };
    let store = PgRlmStore::new(pool.clone());
    let section = r#"{
        "rules": [{"id": "no_short_circuit", "text": "the harness must run the task"}],
        "migrations": [{"name": "0001_shared", "sql": "CREATE TABLE aa_b_scratch (id TEXT)"}]
    }"#;
    let doc_a = topic("aa");
    let doc_b = topic("aa-b");
    let req_a = request(&doc_a, section);
    let req_b = request(&doc_b, section);
    let installer_a = Installer {
        pool: &pool,
        store: &store,
    };
    let installer_b = Installer {
        pool: &pool,
        store: &store,
    };
    let (a, b) = tokio::join!(
        installer_a.install(
            &req_a,
            SetupSummary::Skipped {
                reason: "--skip-baseline".into(),
            },
        ),
        installer_b.install(
            &req_b,
            SetupSummary::Skipped {
                reason: "--skip-baseline".into(),
            },
        )
    );
    // Exactly one wins; the other is refused by name. Both succeeding would
    // mean both created `aa_b_scratch`.
    let refused = [&a, &b]
        .iter()
        .filter(|r| {
            matches!(
                r,
                Err(InstallError::CrossTopicClaim { .. } | InstallError::MigrationFailed { .. })
            )
        })
        .count();
    assert_eq!(
        refused, 1,
        "exactly one of the two racing installs must be refused: a={a:?} b={b:?}"
    );

    tp.drop_schema().await.expect("drop");
}

/// A re-install **replaces** the route set: a route the new set does not
/// claim stops resolving.
///
/// Greptile's P1: `register_apis` was insert-only (`ON CONFLICT DO NOTHING`)
/// and the table was `SELECT, INSERT` for the application role, so when an
/// RLM-authored install superseded a bundle-authored one, the bundle's routes
/// stayed in the table and the mux — which loads every row for the topic —
/// kept answering them. A miner would reach an endpoint the topic's current
/// install does not declare, while the journal said the newer set was in
/// force.
#[tokio::test]
async fn a_re_install_replaces_the_route_set() {
    use proof_topic_authoring::{AuthoredApi, AuthoredMigration, PinPolicy, TopicAuthoring};

    let Some((tp, pool)) = test_pool().await else {
        return;
    };
    let store = PgRlmStore::new(pool.clone());
    let doc = topic("routes-topic");
    let installer = Installer {
        pool: &pool,
        store: &store,
    };
    // First install: the operator's bundle claims two routes.
    let bundle = r#"{
        "rules": [{"id": "operator_rule", "text": "the operator's vector"}],
        "apis": [
            {"path": "old-get", "method": "GET"},
            {"path": "old-post", "method": "POST"}
        ]
    }"#;
    installer
        .install(
            &request(&doc, bundle),
            SetupSummary::NotDriven { reason: "x".into() },
        )
        .await
        .expect("the bundle installs");
    let mut routes = topic_routes(&pool, "routes-topic").await.expect("routes");
    routes.sort_by(|a, b| a.path.cmp(&b.path));
    assert_eq!(routes.len(), 2, "both bundle routes are registered");

    // Second install: the RLM's set claims **one** route, and not the old two.
    let authored = TopicAuthoring {
        schema_version: proof_topic_authoring::AUTHORING_SCHEMA,
        topic_id: "routes-topic".into(),
        rules: vec![proof_task::ChecklistRule {
            id: "rlm_rule".into(),
            text: "the RLM's own vector".into(),
        }],
        migrations: vec![AuthoredMigration {
            name: "0001_rlm".into(),
            sql: "CREATE TABLE routes_topic_rlm (id TEXT)".into(),
        }],
        apis: vec![AuthoredApi {
            path: "rlm-status".into(),
            method: "GET".into(),
            summary: "the RLM's own route".into(),
        }],
        submission_format: serde_json::json!({"kind": "rlm-tar", "max_bytes": 4_194_304}),
        pin_policy: PinPolicy::none(),
    };
    let report = installer
        .install(
            &InstallRequest {
                topic: &doc,
                bundle_digest: digest(),
                environment: "staging".into(),
                rlm_raw: bundle,
                registered_custom: vec!["routes-topic-metric".into()],
                skip_baseline: false,
                authored: Some(&authored),
            },
            SetupSummary::Baselined {
                rules_version: 1,
                baseline_primary: "0.5".into(),
            },
        )
        .await
        .expect("the RLM's set installs");
    assert_eq!(report.apis, ["GET /rlm-status"]);

    // The route table now holds **only** the RLM's route: the old ones were
    // reconciled away, not left resolving.
    let routes = topic_routes(&pool, "routes-topic").await.expect("routes");
    assert_eq!(
        routes.len(),
        1,
        "a replacement leaves only the new set: {routes:?}"
    );
    assert_eq!(routes[0].path, "rlm-status");

    // …and the mux agrees: the old paths are gone, the new one resolves.
    let mux = TopicRouteMux::new(Arc::new(PgTopicRoutes::new(pool.clone())));
    assert_eq!(
        mux.resolve("routes-topic", "GET", "old-get")
            .await
            .expect("resolve"),
        Resolved::NotRegistered,
        "a route the current set does not claim must not resolve"
    );
    assert_eq!(
        mux.resolve("routes-topic", "POST", "old-post")
            .await
            .expect("resolve"),
        Resolved::NotRegistered
    );
    assert!(
        matches!(
            mux.resolve("routes-topic", "GET", "rlm-status")
                .await
                .expect("resolve"),
            Resolved::Route(_)
        ),
        "the new set's route resolves"
    );

    tp.drop_schema().await.expect("drop");
}

/// A replacement that keeps the **row count** the same still moves the mux's
/// generation.
///
/// The cache's change signal used to be `count(*) FROM proof_topic_api`, which
/// cannot see a replacement: delete one, insert one, and the count is
/// unchanged while the *routes* are not. The signal is now the sum of the
/// per-topic route revisions, bumped in the same transaction as the
/// reconciliation.
#[tokio::test]
async fn a_same_count_replacement_still_moves_the_generation() {
    let Some((tp, pool)) = test_pool().await else {
        return;
    };
    let store = PgRlmStore::new(pool.clone());
    let doc = topic("swap-topic");
    let installer = Installer {
        pool: &pool,
        store: &store,
    };
    let first = r#"{
        "rules": [{"id": "operator_rule", "text": "the operator's vector"}],
        "apis": [{"path": "one", "method": "GET"}]
    }"#;
    installer
        .install(
            &request(&doc, first),
            SetupSummary::NotDriven { reason: "x".into() },
        )
        .await
        .expect("first install");

    let source = PgTopicRoutes::new(pool.clone());
    let before = proof_topic_install::TopicRouteSource::generation(&source)
        .await
        .expect("generation");

    // A second install with **one** route as well: the count is unchanged, so
    // a count-based generation would miss it entirely.
    let second = r#"{
        "rules": [{"id": "operator_rule", "text": "the operator's vector"}],
        "apis": [{"path": "two", "method": "GET"}]
    }"#;
    installer
        .install(
            &request(&doc, second),
            SetupSummary::NotDriven { reason: "x".into() },
        )
        .await
        .expect("second install");
    let after = proof_topic_install::TopicRouteSource::generation(&source)
        .await
        .expect("generation");
    assert!(
        after > before,
        "a same-count replacement must move the generation: {before} -> {after}"
    );
    let routes = topic_routes(&pool, "swap-topic").await.expect("routes");
    assert_eq!(routes.len(), 1);
    assert_eq!(routes[0].path, "two");

    tp.drop_schema().await.expect("drop");
}

/// The dynamic mux **reads** what an install **wrote**: the routes the
/// challenge answers `/challenge/{topic_id}/…` from are the rows this install
/// recorded, and a path nobody registered is not invented.
///
/// The last part is the cross-process half: a *second* install (another
/// process — the operator's `proof-admin`) appends a row, and the next request
/// serves it with no restart and no in-process signal, because the cache is
/// keyed by the table's generation.
async fn mux_reads_what_the_install_wrote(pool: &PgPool) {
    let mux = TopicRouteMux::new(Arc::new(PgTopicRoutes::new(pool.clone())));
    let resolved = mux.resolve("tb4", "GET", "status").await.expect("resolve");
    assert!(
        matches!(&resolved, Resolved::Route(r) if r.summary == "topic status"),
        "{resolved:?}"
    );
    assert_eq!(
        mux.resolve("tb4", "POST", "status").await.expect("resolve"),
        Resolved::MethodNotAllowed,
        "a path registered for GET is not a route for POST"
    );
    assert_eq!(
        mux.resolve("tb4", "GET", "nothing").await.expect("resolve"),
        Resolved::NotRegistered
    );

    // A later install in **another process** writes a route and bumps the
    // topic's revision — the pair the mux's generation probe watches. Writing
    // the row alone would be a fixture that no install produces, and the cache
    // would (correctly) not notice it.
    sqlx::query(
        "INSERT INTO proof_topic_api (topic_id, path, method, summary) \
         VALUES ('tb4', 'v2/runs', 'GET', 'a later install')",
    )
    .execute(pool)
    .await
    .expect("append the second install's route");
    sqlx::query(
        "INSERT INTO proof_topic_route_revision (topic_id, revision) VALUES ('tb4', 1) \
         ON CONFLICT (topic_id) DO UPDATE \
         SET revision = proof_topic_route_revision.revision + 1",
    )
    .execute(pool)
    .await
    .expect("bump the revision the way an install does");
    let later = mux.resolve("tb4", "GET", "v2/runs").await.expect("resolve");
    assert!(
        matches!(&later, Resolved::Route(r) if r.summary == "a later install"),
        "{later:?}"
    );
}

/// A denied migration writes **nothing at all**.
///
/// The deny-list runs before the journal opens, so a bundle it refuses leaves
/// no row, no rule, and no table — not even from the migrations that would
/// have been legal. This is the stronger of the two failure shapes, and it is
/// what makes "a denied bundle is a no-op" a property rather than a hope.
#[tokio::test]
async fn a_denied_migration_writes_nothing_at_all() {
    let Some((tp, pool)) = test_pool().await else {
        return;
    };
    let store = PgRlmStore::new(pool.clone());
    let doc = topic("tb4");
    let installer = Installer {
        pool: &pool,
        store: &store,
    };
    // The first migration is legal; the second reaches into the scoring path.
    // Both are refused before either runs, because the check runs over every
    // statement of every migration first.
    let bad = r#"{
        "migrations": [
            {"name": "0001_ok", "sql": "CREATE TABLE tb4_ok (id TEXT)"},
            {"name": "0002_evil", "sql": "CREATE TABLE tb4_x (id TEXT); DROP TABLE proof_rule_version;"}
        ]
    }"#;
    let err = installer
        .install(
            &request(&doc, bad),
            SetupSummary::NotDriven { reason: "x".into() },
        )
        .await
        .expect_err("the second statement is denied");
    let InstallError::MigrationDenied(denied) = &err else {
        panic!("expected MigrationDenied, got {err:?}");
    };
    assert_eq!(
        denied.ordinal, 2,
        "the refusal names the offending statement"
    );
    assert!(denied.what.contains("proof_"), "{}", denied.what);

    // Nothing from the *first* migration landed either.
    let exists: Option<String> = sqlx::query_scalar("SELECT to_regclass('tb4_ok')::text")
        .fetch_one(&pool)
        .await
        .expect("probe");
    assert_eq!(
        exists, None,
        "a denied bundle must not leave a partial install"
    );

    // No rules, and no journal row: the refusal happened before the journal
    // opened, so there is nothing to roll back.
    assert!(
        store.current_rules("tb4").await.expect("rules").is_none(),
        "a refused install must not leave rules"
    );
    assert!(
        latest_install(&pool, "tb4")
            .await
            .expect("journal")
            .is_none(),
        "a pre-flight refusal writes no journal row: it is a no-op, not a failed attempt"
    );

    tp.drop_schema().await.expect("drop");
}

/// The admission read binds the install to **the rule version it recorded**.
///
/// The defect this pins: the publish gate composed "the newest install is
/// `applied`" and "the newest rule version is `rlm`" as two independent
/// predicates. A topic whose install landed rule version 1 from the signed
/// document (`topic_document`) was therefore admitted as soon as *any* later
/// version happened to be RLM-authored — so it could open with the operator's
/// vector in force, which is exactly the operator-cloned document the gate
/// exists to refuse.
#[tokio::test]
async fn the_admission_read_binds_the_install_to_the_version_it_recorded() {
    use proof_topic_install::{installed_rules, InstallState, InstalledRules};

    let Some((tp, pool)) = test_pool().await else {
        return;
    };
    let store = PgRlmStore::new(pool.clone());
    let doc = topic("tb4");
    let installer = Installer {
        pool: &pool,
        store: &store,
    };
    // The install seeds version 1 from the signed document.
    installer
        .install(
            &request(&doc, &section("tb4")),
            SetupSummary::NotDriven { reason: "x".into() },
        )
        .await
        .expect("install");

    // Nothing to admit yet: the version the install landed is the operator's.
    let before = installed_rules(&pool, "tb4").await.expect("read");
    assert!(
        matches!(
            before,
            InstalledRules::NotRlmAuthored {
                version: Some(1),
                ..
            }
        ),
        "version 1 is topic_document-sourced, so the topic is not admissible: {before:?}"
    );

    // An **unrelated later** version is RLM-authored. The install still
    // recorded version 1, so the topic must stay refused: reading "the newest
    // rule version is rlm" would admit it.
    let rlm_v2 = store
        .current_rules("tb4")
        .await
        .expect("rules")
        .expect("version 1")
        .next(
            proof_rlm::RuleSource::Rlm,
            vec![proof_task::ChecklistRule {
                id: "r-1".into(),
                text: "the RLM's own rule".into(),
            }],
        )
        .expect("v2");
    store.put_rules(&rlm_v2).await.expect("write v2");
    let after = installed_rules(&pool, "tb4").await.expect("read");
    assert!(
        matches!(
            after,
            InstalledRules::NotRlmAuthored {
                version: Some(1),
                ..
            }
        ),
        "the install recorded version 1; a newer RLM version must not admit it: {after:?}"
    );

    // A **new install row** that records version 2 is what admits the topic.
    // This is the operator's real path: re-install after the RLM wrote rules.
    let report = installer
        .install(
            &request(&doc, &section("tb4")),
            SetupSummary::Baselined {
                rules_version: 2,
                baseline_primary: "0.42".into(),
            },
        )
        .await
        .expect("re-install");
    assert_eq!(
        report.rules_version, 2,
        "the install keeps the RLM's version"
    );
    assert_eq!(
        installed_rules(&pool, "tb4").await.expect("read"),
        InstalledRules::RlmAuthored { version: 2 },
        "an applied install recording an rlm-sourced version is the admission"
    );

    // And the states that are not `applied` are refused as their own shape.
    sqlx::query(
        "INSERT INTO proof_topic_install \
         (topic_id, bundle_digest, environment, state, rules_version) \
         VALUES ('tb4', 'sha256:' || repeat('ab', 32), 'staging', $1, 2)",
    )
    .bind(InstallState::Failed.as_str())
    .execute(&pool)
    .await
    .expect("append a failed row");
    assert_eq!(
        installed_rules(&pool, "tb4").await.expect("read"),
        InstalledRules::NotApplied {
            state: Some("failed".into())
        },
        "the newest row being `failed` refuses regardless of rule provenance"
    );

    tp.drop_schema().await.expect("drop");
}

/// An `operator` edit that supersedes the RLM's vector refuses the topic.
///
/// The other direction of the same defect: the install recorded an
/// RLM-authored version, but the vector **in force** is an operator's. A gate
/// that bound only the install's version would admit the topic and serve rules
/// no RLM wrote.
#[tokio::test]
async fn an_operator_edit_in_force_refuses_the_topic() {
    use proof_rlm::RuleSource;
    use proof_task::ChecklistRule;
    use proof_topic_install::{installed_rules, InstalledRules};

    let Some((tp, pool)) = test_pool().await else {
        return;
    };
    let store = PgRlmStore::new(pool.clone());
    let doc = topic("tb4");
    let installer = Installer {
        pool: &pool,
        store: &store,
    };
    installer
        .install(
            &request(&doc, &section("tb4")),
            SetupSummary::NotDriven { reason: "x".into() },
        )
        .await
        .expect("install");

    // The RLM authors version 2, and an install records it.
    let rlm = store
        .current_rules("tb4")
        .await
        .expect("rules")
        .expect("v1")
        .next(
            RuleSource::Rlm,
            vec![ChecklistRule {
                id: "rlm-1".into(),
                text: "the RLM's rule".into(),
            }],
        )
        .expect("v2");
    store.put_rules(&rlm).await.expect("write v2");
    installer
        .install(
            &request(&doc, &section("tb4")),
            SetupSummary::Baselined {
                rules_version: 2,
                baseline_primary: "0.5".into(),
            },
        )
        .await
        .expect("re-install");
    assert_eq!(
        installed_rules(&pool, "tb4").await.expect("read"),
        InstalledRules::RlmAuthored { version: 2 },
        "the install landed the RLM's vector"
    );

    // An operator edit supersedes it. Both halves of the gate have to hold:
    // the installed version is still `rlm`, but it is no longer in force.
    let edited = store
        .current_rules("tb4")
        .await
        .expect("rules")
        .expect("v2")
        .next(
            RuleSource::Operator,
            vec![ChecklistRule {
                id: "hand-1".into(),
                text: "an operator's rule".into(),
            }],
        )
        .expect("v3");
    store.put_rules(&edited).await.expect("write v3");
    assert_eq!(
        installed_rules(&pool, "tb4").await.expect("read"),
        InstalledRules::SupersededByOperator {
            installed: 2,
            in_force: 3,
            provenance: "operator".into(),
        },
        "an operator vector in force is not an admission, even when the install landed an RLM one"
    );

    // An RLM rewrite of its own rules stays admitted: that is the autonomy this
    // gate protects, not something it may refuse.
    let rewritten = store
        .current_rules("tb4")
        .await
        .expect("rules")
        .expect("v3")
        .next(
            RuleSource::Rlm,
            vec![ChecklistRule {
                id: "rlm-2".into(),
                text: "the RLM rewrote its own rule".into(),
            }],
        )
        .expect("v4");
    store.put_rules(&rewritten).await.expect("write v4");
    assert_eq!(
        installed_rules(&pool, "tb4").await.expect("read"),
        InstalledRules::RlmAuthored { version: 2 },
        "an RLM rewrite (rlm -> rlm) stays admitted"
    );

    tp.drop_schema().await.expect("drop");
}

/// A re-run resumes: migrations already in the journal are skipped, the rules
/// version is not bumped, and the routes are not duplicated.
#[tokio::test]
async fn a_re_run_resumes_instead_of_re_applying() {
    let Some((tp, pool)) = test_pool().await else {
        return;
    };
    let store = PgRlmStore::new(pool.clone());
    let doc = topic("tb4");
    let installer = Installer {
        pool: &pool,
        store: &store,
    };
    let first = installer
        .install(
            &request(&doc, &section("tb4")),
            SetupSummary::NotDriven { reason: "x".into() },
        )
        .await
        .expect("first install");
    assert_eq!(first.migrations_applied.len(), 2);

    // A second run of the same bundle: nothing to apply, nothing to bump.
    let second = installer
        .install(
            &request(&doc, &section("tb4")),
            SetupSummary::NotDriven { reason: "x".into() },
        )
        .await
        .expect("second install");
    assert!(
        second.migrations_applied.is_empty(),
        "already-applied migrations are skipped: {:?}",
        second.migrations_applied
    );
    assert_eq!(
        second.migrations_skipped,
        ["0001_scratch", "0002_index"],
        "and the journal says which"
    );
    assert_eq!(
        second.rules_version, 1,
        "the rules version is not bumped by a re-run"
    );
    let rules = store
        .current_rules("tb4")
        .await
        .expect("rules")
        .expect("some");
    assert_eq!(rules.version, 1, "exactly one rule version exists");

    // Routes are keyed `(topic_id, method, path)`, so a re-run does not
    // duplicate them.
    let routes = topic_routes(&pool, "tb4").await.expect("routes");
    assert_eq!(routes.len(), 2, "{routes:?}");

    // A third run with a *new* migration applies only the new one.
    let extended = section("tb4").replace(
        r#"{"name": "0002_index", "sql": "CREATE INDEX tb4_scratch_idx ON tb4_scratch (id)"}"#,
        r#"{"name": "0002_index", "sql": "CREATE INDEX tb4_scratch_idx ON tb4_scratch (id)"},
           {"name": "0003_more", "sql": "CREATE TABLE tb4_more (id TEXT)"}"#,
    );
    let third = installer
        .install(
            &request(&doc, &extended),
            SetupSummary::NotDriven { reason: "x".into() },
        )
        .await
        .expect("third install");
    assert_eq!(
        third.migrations_applied,
        ["0003_more"],
        "only the new migration applies"
    );
    assert_eq!(third.migrations_skipped.len(), 2);

    tp.drop_schema().await.expect("drop");
}

/// A migration that is permitted but fails in the database **does** journal
/// the failure, because it got past the pre-flight checks.
///
/// This is the other failure shape, and the contrast with the test above is
/// the point: a pre-flight refusal is a no-op, a step failure is recorded so
/// the operator can see how far the run got.
#[tokio::test]
async fn a_failing_migration_rolls_back_its_own_statements_and_journals() {
    let Some((tp, pool)) = test_pool().await else {
        return;
    };
    let store = PgRlmStore::new(pool.clone());
    let doc = topic("tb4");
    let installer = Installer {
        pool: &pool,
        store: &store,
    };
    // The first statement is fine; the second references a column that does
    // not exist, so the database refuses it. Both are in one migration, so
    // the transaction takes the first one down with it.
    let bad = r#"{
        "migrations": [
            {"name": "0001_partial", "sql": "CREATE TABLE tb4_partial (id TEXT); INSERT INTO tb4_partial (nope) VALUES ('x');"}
        ]
    }"#;
    let err = installer
        .install(
            &request(&doc, bad),
            SetupSummary::NotDriven { reason: "x".into() },
        )
        .await
        .expect_err("the insert fails");
    let InstallError::MigrationFailed { name, ordinal, .. } = &err else {
        panic!("expected MigrationFailed, got {err:?}");
    };
    assert_eq!(name, "0001_partial");
    assert_eq!(*ordinal, 2, "the failure names the statement");

    let exists: Option<String> = sqlx::query_scalar("SELECT to_regclass('tb4_partial')::text")
        .fetch_one(&pool)
        .await
        .expect("probe");
    assert_eq!(
        exists, None,
        "one migration is one transaction: a later failure takes the earlier statement with it"
    );

    // This one *is* journaled: it got past the pre-flight checks, so the
    // operator needs to see how far it got.
    let row = latest_install(&pool, "tb4")
        .await
        .expect("journal")
        .expect("a row");
    assert_eq!(row.state, "failed");
    assert!(
        row.detail.contains("0001_partial"),
        "the journal must name the migration: {}",
        row.detail
    );

    tp.drop_schema().await.expect("drop");
}

/// The open-custom-id gate: an open custom topic whose id this host does not
/// register is refused before anything is written, because it could not
/// score.
#[tokio::test]
async fn an_open_custom_topic_with_an_unregistered_id_is_refused() {
    let Some((tp, pool)) = test_pool().await else {
        return;
    };
    let store = PgRlmStore::new(pool.clone());
    let mut doc = topic("tb4");
    doc.status = TopicStatus::Open;
    let installer = Installer {
        pool: &pool,
        store: &store,
    };
    let rlm = section("tb4");
    let mut req = request(&doc, &rlm);
    req.registered_custom = vec!["some_other_metric".into()];
    let err = installer
        .install(&req, SetupSummary::NotDriven { reason: "x".into() })
        .await
        .expect_err("unregistered open custom id");
    let InstallError::CustomIdNotRegistered { custom_id, .. } = &err else {
        panic!("expected CustomIdNotRegistered, got {err:?}");
    };
    assert_eq!(custom_id, "tb4-metric");
    assert!(err.to_string().contains("503"), "{err}");

    // Nothing was written, including the journal: the gate runs before the
    // first row.
    assert!(
        latest_install(&pool, "tb4")
            .await
            .expect("journal")
            .is_none(),
        "a refused binding must not leave a journal row"
    );

    // Registering the id makes it install.
    let mut req = request(&doc, &rlm);
    req.registered_custom = vec!["tb4-metric".into()];
    installer
        .install(&req, SetupSummary::NotDriven { reason: "x".into() })
        .await
        .expect("a registered id installs");

    tp.drop_schema().await.expect("drop");
}

/// The handler allow-list is enforced through the engine, not only in the
/// reader: a section naming an arbitrary binary cannot reach a write.
#[tokio::test]
async fn an_arbitrary_handler_never_reaches_a_write() {
    let Some((tp, pool)) = test_pool().await else {
        return;
    };
    let store = PgRlmStore::new(pool.clone());
    let doc = topic("tb4");
    let installer = Installer {
        pool: &pool,
        store: &store,
    };
    for bad in ["/bin/sh", "arbitrary_binary", "sh -c 'x'"] {
        let section = format!(r#"{{"handler": "{bad}"}}"#);
        let req = request(&doc, &section);
        let err = installer
            .install(&req, SetupSummary::NotDriven { reason: "x".into() })
            .await
            .expect_err(bad);
        // The refusal names the part and why: a path, a URL, or a command
        // line is *not an identifier*; a well-formed id is *not on the list*.
        let InstallError::Section(section) = &err else {
            panic!("{bad}: expected a Section refusal, got {err:?}");
        };
        assert_eq!(section.part, "handler", "{bad}");
        assert!(
            section.why.contains("vm_backed") || section.why.contains("not an identifier"),
            "{bad}: {err}"
        );
        assert!(
            latest_install(&pool, "tb4")
                .await
                .expect("journal")
                .is_none(),
            "{bad}: nothing must be written"
        );
    }
    tp.drop_schema().await.expect("drop");
}

/// An empty section installs the topic's own signed rule vector: a bundle
/// that carries no rules still lands a version the gate can read.
#[tokio::test]
async fn an_empty_section_installs_the_documents_own_rules() {
    let Some((tp, pool)) = test_pool().await else {
        return;
    };
    let store = PgRlmStore::new(pool.clone());
    let mut doc = topic("tb4");
    doc.checklist = vec![proof_task::ChecklistRule {
        id: "signed_rule".into(),
        text: "the rule the operator signed".into(),
    }];
    let installer = Installer {
        pool: &pool,
        store: &store,
    };
    let report = installer
        .install(
            &request(&doc, "{}"),
            SetupSummary::NotDriven { reason: "x".into() },
        )
        .await
        .expect("an empty section is a legal bundle");
    assert_eq!(report.rule_ids, ["signed_rule"]);
    assert!(report.migrations_applied.is_empty());
    assert!(report.apis.is_empty());
    assert_eq!(
        report.binding.handler, "vm_backed",
        "the fail-closed default handler"
    );
    assert_eq!(report.binding.vms_per_submission, 1);

    tp.drop_schema().await.expect("drop");
}

/// A migration may only touch its own namespace, and the refusal names the
/// object — checked through the engine, against a real database.
#[tokio::test]
async fn a_migration_reaching_another_namespace_is_refused() {
    let Some((tp, pool)) = test_pool().await else {
        return;
    };
    let store = PgRlmStore::new(pool.clone());
    let doc = topic("tb4");
    let installer = Installer {
        pool: &pool,
        store: &store,
    };
    for (sql, needle) in [
        (
            "CREATE TABLE other_topic_scores (id TEXT)",
            "other_topic_scores",
        ),
        ("SELECT * FROM proof_topic_version", "proof_"),
        ("DROP DATABASE base", "DROP DATABASE"),
        ("GRANT ALL ON tb4_x TO base_app", "GRANT"),
    ] {
        let section = format!(r#"{{"migrations": [{{"name": "0001_probe", "sql": "{sql}"}}]}}"#);
        let req = request(&doc, &section);
        let err = installer
            .install(&req, SetupSummary::NotDriven { reason: "x".into() })
            .await
            .expect_err(sql);
        let InstallError::MigrationDenied(denied) = &err else {
            panic!("{sql}: expected MigrationDenied, got {err:?}");
        };
        assert!(
            denied.what.to_lowercase().contains(&needle.to_lowercase()),
            "{sql}: refusal must name {needle:?}, said {:?}",
            denied.what
        );
    }
    tp.drop_schema().await.expect("drop");
}

/// A crash between two migrations cannot lose a committed migration.
///
/// The failure this pins: if a migration's effects commit but the record of it
/// does not, a resume re-applies it — and ordinary non-idempotent DDL
/// (`CREATE TABLE`) fails on a duplicate relation, leaving the install
/// unresumable. The engine writes the journal row **in the same transaction**
/// as the migration, so those two facts cannot disagree.
///
/// Simulated by applying the first migration and then failing the second, so
/// the run dies exactly where a crash would, with the first migration's
/// effects and its journal row already durable.
#[tokio::test]
async fn a_crash_between_migrations_does_not_lose_a_committed_one() {
    let Some((tp, pool)) = test_pool().await else {
        return;
    };
    let store = PgRlmStore::new(pool.clone());
    let doc = topic("tb4");
    let installer = Installer {
        pool: &pool,
        store: &store,
    };
    // Migration 1 commits; migration 2 fails in the database. Migration 1's
    // `CREATE TABLE` is not idempotent, so re-applying it would error.
    let interrupted = r#"{
        "rules": [{"id": "no_short_circuit", "text": "run the task"}],
        "migrations": [
            {"name": "0001_first", "sql": "CREATE TABLE tb4_first (id TEXT)"},
            {"name": "0002_boom", "sql": "INSERT INTO tb4_missing (nope) VALUES ('x')"}
        ]
    }"#;
    installer
        .install(
            &request(&doc, interrupted),
            SetupSummary::NotDriven { reason: "x".into() },
        )
        .await
        .expect_err("the second migration fails");

    // The first migration really landed, and the journal durably names it —
    // written in the same transaction, so these two facts cannot disagree.
    let exists: Option<String> = sqlx::query_scalar("SELECT to_regclass('tb4_first')::text")
        .fetch_one(&pool)
        .await
        .expect("probe");
    assert_eq!(exists.as_deref(), Some("tb4_first"));
    let recorded: Vec<serde_json::Value> =
        sqlx::query_scalar("SELECT migrations FROM proof_topic_install WHERE topic_id = 'tb4'")
            .fetch_all(&pool)
            .await
            .expect("journal");
    assert!(
        recorded.iter().any(|v| v
            .as_array()
            .is_some_and(|a| a.iter().any(|n| n.as_str() == Some("0001_first")))),
        "the committed migration must be durably recorded: {recorded:?}"
    );

    // Resume: the bundle now carries the same first migration (non-idempotent,
    // so re-applying it would fail) plus a fixed second one. It must skip the
    // first and apply only the second.
    let resumed = r#"{
        "rules": [{"id": "no_short_circuit", "text": "run the task"}],
        "migrations": [
            {"name": "0001_first", "sql": "CREATE TABLE tb4_first (id TEXT)"},
            {"name": "0002_fixed", "sql": "CREATE TABLE tb4_second (id TEXT)"}
        ]
    }"#;
    let report = installer
        .install(
            &request(&doc, resumed),
            SetupSummary::NotDriven { reason: "x".into() },
        )
        .await
        .expect("the resume succeeds: the committed migration is skipped");
    assert_eq!(
        report.migrations_applied,
        ["0002_fixed"],
        "only the unapplied migration runs"
    );
    assert_eq!(report.migrations_skipped, ["0001_first"]);
    let second: Option<String> = sqlx::query_scalar("SELECT to_regclass('tb4_second')::text")
        .fetch_one(&pool)
        .await
        .expect("probe");
    assert_eq!(second.as_deref(), Some("tb4_second"));

    tp.drop_schema().await.expect("drop");
}

/// The journal is append-only and its states are exactly the three the
/// migration allows: a run that succeeds writes `pending` then `applied`.
#[tokio::test]
async fn the_journal_appends_pending_then_applied() {
    let Some((tp, pool)) = test_pool().await else {
        return;
    };
    let store = PgRlmStore::new(pool.clone());
    let doc = topic("tb4");
    let installer = Installer {
        pool: &pool,
        store: &store,
    };
    installer
        .install(
            &request(&doc, &section("tb4")),
            SetupSummary::NotDriven { reason: "x".into() },
        )
        .await
        .expect("install");

    let states: Vec<String> = sqlx::query_scalar(
        "SELECT state FROM proof_topic_install WHERE topic_id = 'tb4' ORDER BY id",
    )
    .fetch_all(&pool)
    .await
    .expect("states");
    // The journal is a **progress log**, not a single row: one `pending` row
    // opens the run, then each migration appends its own `pending` row in the
    // migration's own transaction (which is what makes a resume correct), and
    // the run closes with `applied`.
    assert_eq!(
        states,
        ["pending", "pending", "pending", "applied"],
        "two migrations → three pending rows (open + one each) then applied: {states:?}"
    );
    assert_eq!(
        *states.last().expect("non-empty"),
        "applied",
        "the run closes with applied"
    );
    for state in &states {
        assert!(
            matches!(state.as_str(), "pending" | "applied" | "failed"),
            "{state}"
        );
    }
    // Every row is one of the three states the migration's CHECK allows, and
    // the last one is terminal for a successful run.
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM proof_topic_install WHERE topic_id = 'tb4'")
            .fetch_one(&pool)
            .await
            .expect("count");
    assert_eq!(count, 4, "one row per durable step plus the closing row");

    tp.drop_schema().await.expect("drop");
}

/// The install does not need a real rule store for its SQL half: the memory
/// store is enough to prove the engine's ordering, and this test doubles as
/// the check that a topic with no database-backed rules still installs.
#[tokio::test]
async fn the_engine_drives_the_store_it_is_given() {
    let Some((tp, pool)) = test_pool().await else {
        return;
    };
    let store = MemoryRlmStore::new();
    let doc = topic("tb4");
    let installer = Installer {
        pool: &pool,
        store: &store,
    };
    let report = installer
        .install(
            &request(&doc, &section("tb4")),
            SetupSummary::Baselined {
                rules_version: 1,
                baseline_primary: "0.42".into(),
            },
        )
        .await
        .expect("install");
    assert!(matches!(report.setup, SetupSummary::Baselined { .. }));
    let rules = store
        .current_rules("tb4")
        .await
        .expect("rules")
        .expect("some");
    assert_eq!(rules.rules.len(), 2);
    // And the journal's state is the same whichever store produced the rules.
    let row = latest_install(&pool, "tb4")
        .await
        .expect("journal")
        .expect("row");
    assert_eq!(row.state, InstallState::Applied.as_str());
    tp.drop_schema().await.expect("drop");
}

/// The operator gate, end to end against Postgres: a topic with no row is
/// enabled, a `disable` row is what the submit path refuses on, and an
/// `enable` row clears it **without deleting the history** — the point of the
/// append-only shape is that an incident review can still see who turned it
/// off, when, and why.
#[tokio::test]
async fn the_operator_gate_is_a_journal_and_the_newest_row_wins() {
    let Some((tp, pool)) = test_pool().await else {
        return;
    };
    // Never thrown: no row, not disabled.
    assert!(
        !proof_topic_install::disabled(&pool, "tb4")
            .await
            .expect("read"),
        "a topic with no gate row is enabled"
    );
    assert!(proof_topic_install::gate(&pool, "tb4")
        .await
        .expect("read")
        .is_none());
    assert!(proof_topic_install::disabled_topics(&pool)
        .await
        .expect("list")
        .is_empty());

    // Disabled, with a reason: the read the submit path makes.
    let disabled = proof_topic_install::disable(&pool, "tb4", "incident 42", "ops")
        .await
        .expect("disable");
    assert!(disabled.is_disabled());
    assert!(proof_topic_install::disabled(&pool, "tb4")
        .await
        .expect("read"));
    assert_eq!(
        proof_topic_install::disabled_topics(&pool)
            .await
            .expect("list")
            .get("tb4")
            .map(String::as_str),
        Some("incident 42")
    );

    // Enabled again: the newest row wins, and the disable row is still there.
    proof_topic_install::enable(&pool, "tb4", "fixed", "ops")
        .await
        .expect("enable");
    assert!(!proof_topic_install::disabled(&pool, "tb4")
        .await
        .expect("read"));
    assert!(proof_topic_install::disabled_topics(&pool)
        .await
        .expect("list")
        .is_empty());
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT state, reason FROM proof_topic_gate WHERE topic_id = 'tb4' ORDER BY id",
    )
    .fetch_all(&pool)
    .await
    .expect("history");
    assert_eq!(
        rows,
        vec![
            ("disabled".to_owned(), "incident 42".to_owned()),
            ("enabled".to_owned(), "fixed".to_owned()),
        ],
        "the journal keeps both rows"
    );

    // The table is topic-scoped: another topic is unaffected by this one.
    proof_topic_install::disable(&pool, "tb9", "other incident", "ops")
        .await
        .expect("disable");
    assert!(proof_topic_install::disabled(&pool, "tb4")
        .await
        .expect("read")
        .eq(&false));
    assert_eq!(
        proof_topic_install::disabled_topics(&pool)
            .await
            .expect("list")
            .keys()
            .cloned()
            .collect::<Vec<_>>(),
        vec!["tb9".to_owned()]
    );

    tp.drop_schema().await.expect("drop");
}
