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
    assert_eq!(store.put_topic_version(&t).await.unwrap(), 1);
    let mut resigned = t.clone();
    resigned.statement.push_str(" (v2)");
    assert_eq!(store.put_topic_version(&resigned).await.unwrap(), 2);
    let (v, latest) = store.latest_topic(&t.id).await.unwrap().unwrap();
    assert_eq!(v, 2);
    assert!(latest.statement.ends_with("(v2)"));

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
