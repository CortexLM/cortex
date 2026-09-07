//! Real database coverage of both terminal submission HTTP branches.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]
use super::*;
use http_body_util::BodyExt;
use tower::ServiceExt;

async fn request(
    app: Router,
    method: &str,
    uri: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let response = app
        .oneshot(
            axum::http::Request::builder()
                .method(method)
                .uri(uri)
                .header("content-type", "application/json")
                .body(axum::body::Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
#[ignore = "requires disposable DATABASE_URL"]
async fn committed_terminal_paths_are_visible_across_instances_and_fail_closed() {
    let database = db::test_pool().await.unwrap();
    let pool = database.app_pool().await.unwrap();
    let writer = MemoryStore::new()
        .with_journal(proof_store::durable::DurableJournal::new(pool.clone()))
        .await
        .unwrap();
    let reader = MemoryStore::new()
        .with_journal(proof_store::durable::DurableJournal::new(
            database.app_pool().await.unwrap(),
        ))
        .await
        .unwrap();
    let state = |store| AppState {
        store,
        pin: ProofPin::default(),
        backend: EvalBackend::Sim,
        live_scorer: None,
        offer: None,
        judge_api_key: None,
        admin_hashes: Arc::new(Vec::new()),
        epoch: 0,
    };
    let st = state(writer.clone());
    let body = || SubmitBody {
        topic_id: "topic".into(),
        miner_hotkey: "a".repeat(64),
        artifact_digest: "b".repeat(64),
        artifact_uri: None,
        claim: "claim".into(),
        declared_flops: 1,
        architecture: String::new(),
        manifest: ArtifactManifest::default(),
    };
    let topic = TopicDocument {
        id: "topic".into(),
        ..TopicDocument::default()
    };
    writer.put_topic(topic.clone()).unwrap();
    let (_, rejected) = persist_pre_eval_reject(
        &st,
        body(),
        &topic,
        "a".repeat(64),
        "b".repeat(64),
        "nonce".into(),
        "digest".into(),
        &[GateFail::Contamination],
    )
    .await
    .unwrap();
    let rejected_row = reader.get_durable(&rejected.id).await.unwrap();
    assert_eq!(rejected_row.state, SubmissionState::Rejected);
    let verdict = rejected_row.verdict.unwrap();
    let (_, scored) = persist_scored(
        &st,
        body(),
        "a".repeat(64),
        "b".repeat(64),
        "nonce2".into(),
        "digest2".into(),
        verdict,
        "receipt".into(),
        EvalBackend::Sim,
    )
    .await
    .unwrap();
    assert!(reader
        .get_durable(&scored.id)
        .await
        .unwrap()
        .receipt_json
        .is_some());
    assert!(
        !reader
            .snapshot_durable()
            .await
            .unwrap()
            .miner_runs(&"a".repeat(64))
            .unwrap()["topic"]
            .pass
    );
    let app = proof_router(state(reader));
    let (status, rows) =
        request(app.clone(), "GET", "/v1/submissions", serde_json::json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(rows["items"].as_array().unwrap().len(), 2);
    assert_eq!(
        request(
            app.clone(),
            "GET",
            &format!("/v1/submissions/{}", scored.id),
            serde_json::json!({})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        request(app, "GET", "/v1/submissions/missing", serde_json::json!({}))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    pool.close().await;
    let app = proof_router(state(writer));
    for uri in [
        "/v1/submissions".to_owned(),
        format!("/v1/submissions/{}", scored.id),
    ] {
        assert_eq!(
            request(app.clone(), "GET", &uri, serde_json::json!({}))
                .await
                .0,
            StatusCode::SERVICE_UNAVAILABLE
        );
    }
    database.drop_schema().await.unwrap();
}
