//! One contract, two stores. The memory store always runs; the Postgres store
//! runs against an isolated migrated schema when `DATABASE_URL` is set (same
//! gating as `crates/db/tests`) and is skipped otherwise.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

use proof_rlm::fixtures::{green, report_for, request, rules, topic};
use proof_rlm::{RlmEvent, RlmState, RuleSource};
use proof_rlm_store::{
    ArtefactRow, BaselineRow, ChecklistRow, MemoryRlmStore, PgRlmStore, PromotionRow, RlmStore,
    StoreError, TransitionRow,
};
use proof_task::ChecklistRule;

async fn contract(store: &dyn RlmStore) {
    let t = topic();

    // Topic versions advance per persisted document.
    assert!(store.latest_topic(&t.id).await.unwrap().is_none());
    // The registry view is empty before anything is installed, and empty is
    // an empty vector rather than an error.
    assert!(store.latest_topics().await.unwrap().is_empty());
    assert_eq!(store.put_topic_version(&t).await.unwrap(), 1);
    let mut resigned = t.clone();
    resigned.statement.push_str(" (v2)");
    assert_eq!(store.put_topic_version(&resigned).await.unwrap(), 2);
    let (v, latest) = store.latest_topic(&t.id).await.unwrap().unwrap();
    assert_eq!(v, 2);
    assert!(latest.statement.ends_with("(v2)"));

    // The registry view reads the newest version of every topic, ordered by
    // id, and carries the signed document verbatim — the one source of truth.
    let listed = store.latest_topics().await.unwrap();
    assert_eq!(listed.len(), 1, "{listed:?}");
    assert_eq!(listed[0].topic_id, t.id);
    assert_eq!(listed[0].version, 2);
    assert_eq!(listed[0].document, resigned);
    let mut other = t.clone();
    other.id = "aaa-other-v0".into();
    store.put_topic_version(&other).await.unwrap();
    let listed = store.latest_topics().await.unwrap();
    assert_eq!(
        listed
            .iter()
            .map(|r| r.topic_id.as_str())
            .collect::<Vec<_>>(),
        ["aaa-other-v0", t.id.as_str()],
        "ordered by topic_id"
    );

    // Aliases: the Owner default is slug `tb4` with temporary alias `tbench`.
    // An alias resolves to the canonical slug, an unknown one to nothing, and
    // an alias for a topic with no published version is refused outright.
    assert!(store.resolve_alias("tbench").await.unwrap().is_none());
    store.put_alias("tbench", &t.id).await.unwrap();
    assert_eq!(
        store.resolve_alias("tbench").await.unwrap().as_deref(),
        Some(t.id.as_str())
    );
    assert_eq!(store.aliases_for(&t.id).await.unwrap(), ["tbench"]);
    assert!(store
        .resolve_alias("no-such-alias")
        .await
        .unwrap()
        .is_none());
    assert!(
        store.put_alias("orphan", "never-published").await.is_err(),
        "an alias must name a topic that has a published version"
    );
    assert!(
        store.resolve_alias("orphan").await.unwrap().is_none(),
        "the refused alias must not have been written"
    );
    // A canonical slug is never shadowed. An alias that equals another
    // *published* topic's id would make that slug resolve to a different
    // topic's document, so it is refused at write time and never resolved.
    store
        .put_alias("shadow-attempt", "aaa-other-v0")
        .await
        .unwrap();
    assert!(
        store.put_alias("aaa-other-v0", &t.id).await.is_err(),
        "an alias may not take a published topic's canonical slug"
    );
    assert_eq!(
        store
            .resolve_alias("aaa-other-v0")
            .await
            .unwrap()
            .as_deref(),
        None,
        "the canonical slug must not resolve to another topic"
    );
    store.delete_alias("shadow-attempt").await.unwrap();

    // A second alias on the same topic, then retire one.
    store.put_alias("tb4-legacy", &t.id).await.unwrap();
    assert_eq!(
        store.aliases_for(&t.id).await.unwrap(),
        ["tb4-legacy", "tbench"],
        "aliases are ordered by alias"
    );
    assert!(store.delete_alias("tb4-legacy").await.unwrap());
    assert!(
        !store.delete_alias("tb4-legacy").await.unwrap(),
        "already gone"
    );
    assert_eq!(store.aliases_for(&t.id).await.unwrap(), ["tbench"]);
    // Re-pointing an existing alias replaces it rather than conflicting.
    let other = store.latest_topic("aaa-other-v0").await.unwrap();
    assert!(other.is_some(), "the second topic was published above");
    store.put_alias("tbench", "aaa-other-v0").await.unwrap();
    assert_eq!(
        store.resolve_alias("tbench").await.unwrap().as_deref(),
        Some("aaa-other-v0")
    );
    assert!(store.aliases_for(&t.id).await.unwrap().is_empty());

    // Rules: v1 from the document, v2 from the RLM, gaps refused.
    let v1 = rules();
    assert!(store.current_rules(&t.id).await.unwrap().is_none());
    let v2 = v1
        .next(
            RuleSource::Rlm,
            vec![ChecklistRule {
                id: "rlm_rule".into(),
                text: "rewritten by the rlm".into(),
            }],
        )
        .unwrap();
    assert!(matches!(
        store.put_rules(&v2).await,
        Err(StoreError::VersionGap("rules"))
    ));
    store.put_rules(&v1).await.unwrap();
    assert!(matches!(
        store.put_rules(&v1).await,
        Err(StoreError::VersionGap("rules"))
    ));
    store.put_rules(&v2).await.unwrap();
    assert_eq!(store.current_rules(&t.id).await.unwrap().unwrap(), v2);
    assert_eq!(store.rules_at(&t.id, 1).await.unwrap().unwrap(), v1);
    assert!(store.rules_at(&t.id, 3).await.unwrap().is_none());

    // Checklists are keyed by the frozen digest.
    let digest = "ab".repeat(32);
    let mut c = green(&v1, &digest);
    c.items[0].pass = false;
    let row = ChecklistRow::from_checklist(&c, &v1);
    assert!(!row.green);
    assert_eq!(row.failed_ids, vec![v1.rules[0].id.clone()]);
    store.put_checklist(&row).await.unwrap();
    assert_eq!(store.checklist(&digest).await.unwrap().unwrap(), row);
    assert!(store.checklist(&"cd".repeat(32)).await.unwrap().is_none());

    // Same digest may be re-inspected (miner resubmit after a false
    // anti-cheat reject). Upsert overwrites green / failed_ids / document.
    let green_row = ChecklistRow::from_checklist(&green(&v1, &digest), &v1);
    assert!(green_row.green);
    assert!(green_row.failed_ids.is_empty());
    store.put_checklist(&green_row).await.unwrap();
    assert_eq!(store.checklist(&digest).await.unwrap().unwrap(), green_row);

    // Lifecycle replays from appended rows.
    assert!(store.lifecycle(&t.id).await.unwrap().is_none());
    for (from, event, to) in [
        (
            RlmState::Open,
            RlmEvent::SubmissionReceived,
            RlmState::Evaluating,
        ),
        (
            RlmState::Evaluating,
            RlmEvent::PromotionCandidate,
            RlmState::Promoting,
        ),
        (RlmState::Promoting, RlmEvent::Promoted, RlmState::Open),
    ] {
        store
            .record_transition(&TransitionRow {
                topic_id: t.id.clone(),
                from,
                event,
                to,
                note: format!("{event:?}"),
            })
            .await
            .unwrap();
    }
    let lc = store.lifecycle(&t.id).await.unwrap().unwrap();
    assert_eq!(lc.state, RlmState::Open);
    assert_eq!(lc.history.len(), 3);
    assert_eq!(lc.history[1].event, RlmEvent::PromotionCandidate);

    // Baseline per rule version; newest wins.
    let req = request();
    assert!(store.baseline(&t.id).await.unwrap().is_none());
    store
        .put_baseline(&BaselineRow {
            topic_id: t.id.clone(),
            rules_version: 1,
            primary_value: 0.4,
            report: report_for(&req, 0.4),
        })
        .await
        .unwrap();
    store
        .put_baseline(&BaselineRow {
            topic_id: t.id.clone(),
            rules_version: 2,
            primary_value: 0.45,
            report: report_for(&req, 0.45),
        })
        .await
        .unwrap();
    let b = store.baseline(&t.id).await.unwrap().unwrap();
    assert_eq!(b.rules_version, 2);
    assert!((b.primary_value - 0.45).abs() < 1e-12);
    assert!(matches!(
        store
            .put_baseline(&BaselineRow {
                topic_id: t.id.clone(),
                rules_version: 3,
                primary_value: f64::NAN,
                report: report_for(&req, 0.0),
            })
            .await,
        Err(StoreError::Malformed(_))
    ));

    // Artefact metadata and the promotion continuum.
    assert!(store.max_artefact_numeric_id().await.unwrap().is_none());
    let art = ArtefactRow {
        topic_id: t.id.clone(),
        submission_id: "pf_0000000000000001".into(),
        submission_digest: digest.clone(),
        path: "/artefacts/topic-a/pf_0000000000000001.zip".into(),
        sha256: "ef".repeat(32),
        bytes: 1_024,
        primary_value: Some(0.7),
        checklist_green: true,
        promoted: true,
    };
    store.put_artefact(&art).await.unwrap();
    assert!(matches!(
        store
            .put_artefact(&ArtefactRow {
                submission_id: "nope".into(),
                ..art.clone()
            })
            .await,
        Err(StoreError::Malformed(_))
    ));
    assert_eq!(store.artefacts(&t.id).await.unwrap(), vec![art.clone()]);
    assert_eq!(store.max_artefact_numeric_id().await.unwrap(), Some(1));

    let mut replaced = art.clone();
    replaced.submission_digest = "cd".repeat(32);
    replaced.sha256 = "cd".repeat(32);
    replaced.bytes = 2_048;
    replaced.primary_value = Some(0.9);
    replaced.checklist_green = false;
    replaced.promoted = false;
    store.put_artefact(&replaced).await.unwrap();
    let got = store.artefacts(&t.id).await.unwrap();
    assert_eq!(got.len(), 1, "PK stays one row");
    assert_eq!(got[0].sha256, "cd".repeat(32));
    assert_eq!(got[0].submission_digest, "cd".repeat(32));
    assert_eq!(got[0].bytes, 2_048);
    assert_eq!(got[0].primary_value, Some(0.9));
    assert!(!got[0].checklist_green);
    assert!(!got[0].promoted);

    assert!(store.best(&t.id).await.unwrap().is_none());
    let first = PromotionRow {
        topic_id: t.id.clone(),
        submission_id: "pf_0000000000000001".into(),
        submission_digest: digest.clone(),
        primary_value: 0.7,
        bar: Some(0.45),
        previous_best: None,
    };
    store.record_promotion(&first).await.unwrap();
    let second = PromotionRow {
        submission_id: "pf_0000000000000002".into(),
        primary_value: 0.8,
        bar: Some(0.7),
        previous_best: Some("pf_0000000000000001".into()),
        ..first.clone()
    };
    store.record_promotion(&second).await.unwrap();
    assert_eq!(store.best(&t.id).await.unwrap().unwrap(), second);
    assert_eq!(
        store.promotions(&t.id).await.unwrap(),
        vec![first, second.clone()]
    );
    assert!(matches!(
        store
            .record_promotion(&PromotionRow {
                primary_value: f64::INFINITY,
                ..second
            })
            .await,
        Err(StoreError::Malformed(_))
    ));
    // Another topic sees none of it.
    assert!(store.best("topic-b").await.unwrap().is_none());
    assert!(store.artefacts("topic-b").await.unwrap().is_empty());
}

#[tokio::test]
async fn memory_store_honours_the_contract() {
    contract(&MemoryRlmStore::new()).await;
}

/// An alias claim and a topic publish must not both claim one slug.
///
/// This is the **cross-table** race the advisory lock exists for. The alias
/// insert and the topic publish each check the other table, and under READ
/// COMMITTED neither sees the other's uncommitted row. Two things must hold,
/// and the test checks both because each covers a different layer:
///
/// 1. **Blocking.** With the shared transaction-scoped lock, the publish waits
///    for the in-flight alias claim. Without the lock it sees no committed
///    alias and succeeds immediately. (Pass condition: still blocked.)
/// 2. **Rejection.** Once the alias claim *commits* and releases the lock, the
///    waiting publish must be **refused**, not admitted — that is the
///    publisher-side collision check. A test that only asserts the block
///    stays green if that check is deleted, which is exactly the regression
///    this covers.
///
/// Postgres-only: the memory store has one mutex and no such race.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_concurrent_alias_and_topic_publish_cannot_both_claim_a_slug() {
    let Ok(url) = std::env::var("DATABASE_URL") else {
        return;
    };
    if url.trim().is_empty() {
        return;
    }
    let tp = db::test_pool_with_url(&url).await.expect("isolated schema");
    let pool = tp.pool();
    let store = PgRlmStore::new(pool.clone());

    // A published topic for the alias to point at.
    let doc = topic();
    store.put_topic_version(&doc).await.unwrap();
    let contested = "contested-slug-v0";

    // A: claim `contested` as an alias and hold the transaction open.
    let mut holder = pool.begin().await.expect("begin holder");
    sqlx::query("INSERT INTO proof_topic_alias (alias, topic_id) VALUES ($1, $2)")
        .bind(contested)
        .bind(&doc.id)
        .execute(&mut *holder)
        .await
        .expect("alias insert inside the open transaction");

    // B: publish a topic whose id is `contested`, on another connection.
    let publisher_pool = pool.clone();
    let mut publisher = tokio::spawn(async move {
        sqlx::query(
            "INSERT INTO proof_topic_version (topic_id, version, status, document, signature) \
             VALUES ($1, 1, 'draft', '{}'::jsonb, 'sig')",
        )
        .bind(contested)
        .execute(&publisher_pool)
        .await
    });

    // 1. It must block while A is open.
    let blocked = tokio::time::timeout(std::time::Duration::from_millis(750), &mut publisher).await;
    assert!(
        blocked.is_err(),
        "the publish did not block on the slug claim, so an alias and a topic can both \
         claim {contested}: {blocked:?}"
    );

    // A commits: the alias claim is now visible and the lock is released.
    holder.commit().await.expect("commit the alias claim");

    // 2. The waiting publish must be **refused**, not admitted.
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(5), &mut publisher)
        .await
        .expect("the blocked publish must finish once the claim commits")
        .expect("the publisher task must not panic");
    let err = outcome.expect_err(
        "the publish was admitted after the alias claim committed, so both claimed the slug",
    );
    assert!(
        err.to_string().contains("already claimed as an alias"),
        "the publish must be refused by the collision check, got: {err}"
    );

    // And the invariant, read back: exactly one claim exists, and the slug
    // never resolves through an alias to a different topic's document.
    assert!(
        store.latest_topic(contested).await.unwrap().is_none(),
        "the refused publish must not have written a topic row"
    );
    assert_eq!(
        store.resolve_alias(contested).await.unwrap().as_deref(),
        Some(doc.id.as_str()),
        "the alias claim is the one that won"
    );

    tp.drop_schema().await.expect("drop");
}

#[tokio::test]
async fn postgres_store_honours_the_contract_when_a_database_is_present() {
    if std::env::var_os("DATABASE_URL").is_none() {
        eprintln!("DATABASE_URL unset; skipping the Postgres contract");
        return;
    }
    let pool = db::test_pool().await.expect("isolated migrated schema");
    contract(&PgRlmStore::new(pool.pool().clone())).await;
    pool.drop_schema().await.expect("drop schema");
}

#[tokio::test]
async fn postgres_app_role_replaces_artefact_metadata_on_conflict() {
    if std::env::var_os("DATABASE_URL").is_none() {
        eprintln!("DATABASE_URL unset; skipping the Postgres conflict upsert");
        return;
    }
    let pool = db::test_pool().await.expect("isolated migrated schema");
    let store = PgRlmStore::new(pool.app_pool().await.expect("app"));
    let art = ArtefactRow {
        topic_id: "topic-a".into(),
        submission_id: "pf_0000000000000000".into(),
        submission_digest: "ab".repeat(32),
        path: "/artefacts/topic-a/pf_0000000000000000.zip".into(),
        sha256: "aa".repeat(32),
        bytes: 10,
        primary_value: Some(0.1),
        checklist_green: true,
        promoted: false,
    };
    store.put_artefact(&art).await.unwrap();
    let mut again = art.clone();
    again.sha256 = "bb".repeat(32);
    again.submission_digest = "cd".repeat(32);
    store.put_artefact(&again).await.unwrap();
    let got = store.artefacts("topic-a").await.unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].sha256, "bb".repeat(32));
    assert_eq!(got[0].submission_digest, "cd".repeat(32));
    assert_eq!(store.max_artefact_numeric_id().await.unwrap(), Some(0));
    pool.drop_schema().await.expect("drop schema");
}

#[tokio::test]
async fn postgres_app_role_replaces_checklist_on_conflict() {
    if std::env::var_os("DATABASE_URL").is_none() {
        eprintln!("DATABASE_URL unset; skipping the Postgres checklist upsert");
        return;
    }
    let pool = db::test_pool().await.expect("isolated migrated schema");
    let store = PgRlmStore::new(pool.app_pool().await.expect("app"));
    let digest = "ab".repeat(32);
    let rules = rules();
    let mut red = green(&rules, &digest);
    red.items[0].pass = false;
    let red_row = ChecklistRow::from_checklist(&red, &rules);
    assert!(!red_row.green);
    store.put_checklist(&red_row).await.unwrap();
    let green_row = ChecklistRow::from_checklist(&green(&rules, &digest), &rules);
    assert!(green_row.green);
    store.put_checklist(&green_row).await.unwrap();
    let got = store.checklist(&digest).await.unwrap().unwrap();
    assert_eq!(got, green_row);
    assert!(got.green);
    assert!(got.failed_ids.is_empty());
    pool.drop_schema().await.expect("drop schema");
}
