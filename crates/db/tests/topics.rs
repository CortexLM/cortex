//! Integration tests for the Proof topic install registry (migration 0024).
//!
//! Runs against an isolated migrated schema when `DATABASE_URL` is set (the
//! same gating as the other `crates/db/tests`) and is skipped otherwise, so
//! default CI without Postgres stays green.
//!
//! Scenarios:
//! - S1 happy: install a topic, read it back, list it
//! - S2 edge: empty table lists as empty; an unknown id is `None`, not an error
//! - S3 edge: re-install replaces install fields, keeps `created_at`, and does
//!   not change `enabled`
//! - S4 fail-closed: the table's own `CHECK`s refuse a malformed row
//! - S5 role: `base_app` may write and update, never delete

#![cfg(feature = "testing")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

use db::{get_topic, list_topics, upsert_topic, NewTopic, TestPool};
use serde_json::json;

/// Returns `false` when `DATABASE_URL` is unset so default CI (no Postgres) skips.
fn database_url_present() -> bool {
    std::env::var_os("DATABASE_URL").is_some()
}

const HEX: &str = "abababababababababababababababababababababababababababababababab";

fn digest() -> String {
    format!("sha256:{HEX}")
}

/// Owns every borrowed field so a test can mutate one and still hand the row
/// to [`upsert_topic`] without fighting temporary lifetimes.
struct Fixture {
    aliases: Vec<String>,
    config: serde_json::Value,
    pin_rlm: String,
    pin_experiment: String,
    pack_digest: String,
    bundle: serde_json::Value,
    bundle_digest: String,
}

impl Fixture {
    fn new() -> Self {
        let digest = digest();
        Self {
            aliases: vec!["tbench".to_owned()],
            config: json!({}),
            pin_rlm: digest.clone(),
            pin_experiment: digest.clone(),
            pack_digest: digest.clone(),
            bundle: json!({ "schema_version": 1, "topic_id": "tb4" }),
            bundle_digest: digest,
        }
    }

    fn row(&self) -> NewTopic<'_> {
        NewTopic {
            topic_id: "tb4",
            display_name: "Terminal-Bench 4",
            version: 1,
            environment: "metal",
            runner_id: "rlm_fc_in_guest_harbor",
            aliases: &self.aliases,
            config: &self.config,
            pin_rlm: &self.pin_rlm,
            pin_experiment: &self.pin_experiment,
            pack_digest: &self.pack_digest,
            n_concurrent: 2,
            sealed_custom_value: None,
            schema_version: 1,
            bundle: &self.bundle,
            bundle_digest: &self.bundle_digest,
        }
    }
}

/// A runner-less row (the harvest-family shape), for ordering probes.
fn harvest_row<'a>(f: &'a Fixture, topic_id: &'a str) -> NewTopic<'a> {
    NewTopic {
        topic_id,
        runner_id: "",
        pack_digest: "",
        ..f.row()
    }
}

#[tokio::test]
async fn s1_install_read_back_and_list() {
    if !database_url_present() {
        return;
    }
    let tp: TestPool = db::test_pool().await.expect("test_pool");
    let pool = tp.pool();

    assert!(
        list_topics(pool).await.expect("empty list").is_empty(),
        "a fresh schema has no topics; that is empty, not an error"
    );
    assert!(get_topic(pool, "tb4").await.expect("miss").is_none());

    let fixture = Fixture::new();
    upsert_topic(pool, &fixture.row()).await.expect("install");

    let row = get_topic(pool, "tb4").await.expect("get").expect("row");
    assert_eq!(row.topic_id, "tb4");
    assert_eq!(row.display_name, "Terminal-Bench 4");
    assert_eq!(row.version, 1);
    assert_eq!(row.environment, "metal");
    assert_eq!(row.runner_id, "rlm_fc_in_guest_harbor");
    assert_eq!(row.aliases, fixture.aliases);
    assert!(!row.enabled, "an install never enables a topic");
    assert_eq!(row.config, json!({}));
    assert_eq!(row.pin_rlm, fixture.pin_rlm);
    assert_eq!(row.pin_experiment, fixture.pin_experiment);
    assert_eq!(row.pack_digest, fixture.pack_digest);
    assert_eq!(row.n_concurrent, 2);
    assert!(row.sealed_custom_value.is_none(), "unsealed stays NULL");
    assert_eq!(row.schema_version, 1);
    assert_eq!(row.bundle, fixture.bundle);
    assert_eq!(row.bundle_digest, fixture.bundle_digest);
    assert!(row.created_at.ends_with('Z'), "{}", row.created_at);
    assert!(row.updated_at.ends_with('Z'), "{}", row.updated_at);
    assert_eq!(row.created_at, row.updated_at, "one write, one instant");

    let listed = list_topics(pool).await.expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0], row);

    tp.drop_schema().await.expect("drop");
}

#[tokio::test]
async fn s2_list_is_ordered_by_topic_id() {
    if !database_url_present() {
        return;
    }
    let tp = db::test_pool().await.expect("test_pool");
    let pool = tp.pool();

    let fixture = Fixture::new();
    for id in ["zeta", "alpha", "mid"] {
        upsert_topic(pool, &harvest_row(&fixture, id))
            .await
            .expect("install");
    }
    let ids: Vec<String> = list_topics(pool)
        .await
        .expect("list")
        .into_iter()
        .map(|r| r.topic_id)
        .collect();
    assert_eq!(ids, ["alpha", "mid", "zeta"]);

    tp.drop_schema().await.expect("drop");
}

#[tokio::test]
async fn s3_reinstall_replaces_the_install_and_keeps_the_first_created_at() {
    if !database_url_present() {
        return;
    }
    let tp = db::test_pool().await.expect("test_pool");
    let pool = tp.pool();

    let fixture = Fixture::new();
    upsert_topic(pool, &fixture.row())
        .await
        .expect("first install");
    let first = get_topic(pool, "tb4").await.expect("get").expect("row");

    // An operator opens the topic by hand (the enable path is a later slice,
    // so the test writes the column directly to prove the re-install rule).
    sqlx::query("UPDATE proof_topic SET enabled = TRUE WHERE topic_id = 'tb4'")
        .execute(pool)
        .await
        .expect("enable");

    let next_bundle = json!({ "v": 2 });
    let next_digest = format!("sha256:{}", "cd".repeat(32));
    let second = Fixture {
        bundle: next_bundle.clone(),
        bundle_digest: next_digest.clone(),
        ..Fixture::new()
    };
    upsert_topic(
        pool,
        &NewTopic {
            version: 2,
            n_concurrent: 4,
            sealed_custom_value: Some(0.42),
            ..second.row()
        },
    )
    .await
    .expect("re-install");

    let row = get_topic(pool, "tb4").await.expect("get").expect("row");
    assert_eq!(row.version, 2, "the install version advances");
    assert_eq!(row.n_concurrent, 4);
    assert_eq!(row.sealed_custom_value, Some(0.42));
    assert_eq!(row.bundle, next_bundle);
    assert_eq!(row.bundle_digest, next_digest);
    assert_eq!(
        row.created_at, first.created_at,
        "created_at belongs to the first install"
    );
    assert!(
        row.enabled,
        "a re-install must not silently disable a live topic"
    );
    assert_eq!(
        list_topics(pool).await.expect("list").len(),
        1,
        "one row per topic_id"
    );

    tp.drop_schema().await.expect("drop");
}

#[tokio::test]
async fn s4_the_schema_refuses_a_malformed_row() {
    if !database_url_present() {
        return;
    }
    let tp = db::test_pool().await.expect("test_pool");
    let pool = tp.pool();

    let fixture = Fixture::new();

    // Each probe mutates one field into a shape the schema must refuse.
    for (label, mutate) in [
        (
            "uppercase topic id",
            Box::new(|r: &mut NewTopic<'_>| r.topic_id = "TB4") as Box<dyn Fn(&mut NewTopic<'_>)>,
        ),
        (
            "underscore topic id",
            Box::new(|r: &mut NewTopic<'_>| r.topic_id = "tb_4"),
        ),
        (
            "empty display name",
            Box::new(|r: &mut NewTopic<'_>| r.display_name = ""),
        ),
        (
            "zero version",
            Box::new(|r: &mut NewTopic<'_>| r.version = 0),
        ),
        (
            "unknown environment",
            Box::new(|r: &mut NewTopic<'_>| r.environment = "prod"),
        ),
        (
            "zero concurrency",
            Box::new(|r: &mut NewTopic<'_>| r.n_concurrent = 0),
        ),
        (
            "bare-hex pin",
            Box::new(|r: &mut NewTopic<'_>| r.pin_rlm = HEX),
        ),
        (
            "short pack digest",
            Box::new(|r: &mut NewTopic<'_>| r.pack_digest = "sha256:abc"),
        ),
        (
            "bare bundle digest",
            Box::new(|r: &mut NewTopic<'_>| r.bundle_digest = HEX),
        ),
        (
            "non-finite baseline",
            Box::new(|r: &mut NewTopic<'_>| r.sealed_custom_value = Some(f64::NAN)),
        ),
        (
            "zero schema version",
            Box::new(|r: &mut NewTopic<'_>| r.schema_version = 0),
        ),
        (
            "bad runner id",
            Box::new(|r: &mut NewTopic<'_>| r.runner_id = "Runner With Spaces"),
        ),
    ] {
        let mut row = fixture.row();
        mutate(&mut row);
        upsert_topic(pool, &row)
            .await
            .expect_err(&format!("{label} must be refused by the schema"));
    }

    assert!(
        list_topics(pool).await.expect("list").is_empty(),
        "no refused probe may leave a row"
    );

    // The alias array is checked element-wise, including self-aliasing.
    for (label, alias_list) in [
        ("malformed alias", vec!["Bad Alias".to_owned()]),
        ("self alias", vec!["tb4".to_owned()]),
    ] {
        let bad_aliases = Fixture {
            aliases: alias_list,
            ..Fixture::new()
        };
        let err = upsert_topic(pool, &bad_aliases.row())
            .await
            .expect_err(label);
        let msg = err.to_string();
        assert!(
            msg.contains("aliases") || msg.contains("check constraint"),
            "{label}: {msg}"
        );
    }

    // A non-object config is refused by the schema too.
    let list_config = Fixture {
        config: json!([1, 2]),
        ..Fixture::new()
    };
    upsert_topic(pool, &list_config.row())
        .await
        .expect_err("config must be an object");

    tp.drop_schema().await.expect("drop");
}

/// A `NULL` inside the alias array is refused.
///
/// `array_to_string` drops NULL elements, so a joined-string shape check
/// alone would accept `{tbench,NULL}` — and the typed reader decodes every
/// element as a `String`, so that one row would make `topic list` and
/// `topic show` fail for the whole table rather than just its own row.
#[tokio::test]
async fn s4b_a_null_alias_element_is_refused() {
    if !database_url_present() {
        return;
    }
    let tp = db::test_pool().await.expect("test_pool");
    let pool = tp.pool();

    for label in ["NULL first", "NULL last", "NULL only"] {
        let insert = match label {
            "NULL first" => "INSERT INTO proof_topic \
                (topic_id, display_name, version, environment, schema_version, bundle, bundle_digest, aliases) \
                VALUES ('tb4', 'x', 1, 'metal', 1, '{}', 'sha256:' || repeat('a', 64), ARRAY[NULL, 'tbench'])",
            "NULL last" => "INSERT INTO proof_topic \
                (topic_id, display_name, version, environment, schema_version, bundle, bundle_digest, aliases) \
                VALUES ('tb4', 'x', 1, 'metal', 1, '{}', 'sha256:' || repeat('a', 64), ARRAY['tbench', NULL])",
            _ => "INSERT INTO proof_topic \
                (topic_id, display_name, version, environment, schema_version, bundle, bundle_digest, aliases) \
                VALUES ('tb4', 'x', 1, 'metal', 1, '{}', 'sha256:' || repeat('a', 64), ARRAY[NULL])",
        };
        let err = sqlx::query(insert).execute(pool).await.expect_err(label);
        let msg = err.to_string();
        assert!(
            msg.contains("aliases_no_null") || msg.contains("check constraint"),
            "{label}: {msg}"
        );
    }

    // The shape check alone would have accepted the NULL (it is dropped by
    // array_to_string), which is exactly why the separate constraint exists.
    let joined: String =
        sqlx::query_scalar("SELECT array_to_string(ARRAY['tbench', NULL]::text[], ',')")
            .fetch_one(pool)
            .await
            .expect("array_to_string");
    assert_eq!(
        joined, "tbench",
        "the NULL is dropped, not caught, by the join"
    );

    assert!(
        list_topics(pool).await.expect("list").is_empty(),
        "no refused probe may leave a row"
    );
    tp.drop_schema().await.expect("drop");
}

#[tokio::test]
async fn s5_app_role_writes_and_updates_but_never_deletes() {
    if !database_url_present() {
        return;
    }
    let tp = db::test_pool().await.expect("test_pool");

    let fixture = Fixture::new();
    let app = tp.app_pool().await.expect("app_pool");
    upsert_topic(&app, &fixture.row())
        .await
        .expect("app role installs a topic");
    assert!(
        get_topic(&app, "tb4").await.expect("get").is_some(),
        "app role reads its own install"
    );

    sqlx::query("UPDATE proof_topic SET enabled = TRUE WHERE topic_id = 'tb4'")
        .execute(&app)
        .await
        .expect("app role may enable (the enable path is a later slice)");

    // The shared test harness grants the app role DELETE on every table and
    // revokes it only for `APPEND_ONLY_TABLES`. `proof_topic` is mutable but
    // deliberately not append-only, so restore the privilege set the migration
    // alone grants (SELECT, INSERT, UPDATE) before asserting the refusal.
    // That the migration never grants DELETE is pinned without a database in
    // `crates/db/src/topics.rs`.
    sqlx::query("REVOKE DELETE ON TABLE proof_topic FROM base_app")
        .execute(tp.pool())
        .await
        .expect("restore the migration's grants");

    let err = sqlx::query("DELETE FROM proof_topic WHERE topic_id = 'tb4'")
        .execute(&app)
        .await
        .expect_err("a topic is disabled, never dropped");
    let msg = err.to_string();
    assert!(
        msg.contains("permission denied") || msg.contains("42501"),
        "{msg}"
    );

    tp.drop_schema().await.expect("drop");
}
