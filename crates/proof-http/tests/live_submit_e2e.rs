//! Optional live probe of a running Proof host (`PROOF_E2E_BASE`).
//!
//! Set `PROOF_E2E_BASE` to the challenge origin (no trailing slash), e.g.
//! `http://127.0.0.1:28100` or `http://staging.api.joinbase.ai/challenge/proof`.
//!
//! Never POSTs when the host is live Lium and `can_score` (would rent).
//! Never talks to production. Staging sim (`eval_backend=sim`) is the intended
//! target.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use serde_json::Value;

const STAGING_TOPICS: [&str; 2] = ["dt-no-ib-v0", "muon-vs-adamw-10m-v0"];

fn base_url() -> Option<String> {
    std::env::var("PROOF_E2E_BASE")
        .ok()
        .map(|s| s.trim().trim_end_matches('/').to_owned())
        .filter(|s| !s.is_empty())
}

fn is_prod_host(base: &str) -> bool {
    base.contains("network.cortex.foundation") || base.contains("chain.joinbase.ai")
}

fn hex64(label: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(label.as_bytes());
    hex::encode(h.finalize())
}

async fn get(client: &reqwest::Client, url: &str) -> (u16, Value) {
    let resp = client.get(url).send().await.expect("GET");
    let status = resp.status().as_u16();
    let body = resp.json::<Value>().await.unwrap_or(Value::Null);
    (status, body)
}

async fn post(client: &reqwest::Client, url: &str, body: &Value) -> (u16, Value) {
    let resp = client.post(url).json(body).send().await.expect("POST");
    let status = resp.status().as_u16();
    let body = resp.json::<Value>().await.unwrap_or(Value::Null);
    (status, body)
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn live_host_submit_scores_or_fails_closed() {
    let Some(base) = base_url() else {
        eprintln!("skip live_submit_e2e: set PROOF_E2E_BASE to probe a running host");
        return;
    };
    assert!(
        !is_prod_host(&base),
        "refusing production host {base} (staging/local only)"
    );

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .expect("client");

    let (st, health) = get(&client, &format!("{base}/health")).await;
    assert_eq!(st, 200, "{health}");
    assert_eq!(health["challenge_id"], "proof");

    let (st, status) = get(&client, &format!("{base}/v1/status")).await;
    assert_eq!(st, 200, "{status}");
    assert_eq!(status["challenge_id"], "proof");
    assert!(status["can_score"].is_boolean(), "{status}");
    assert!(status["eval_backend"].is_string(), "{status}");
    assert!(
        status.get("error").is_none(),
        "status must not be an error object: {status}"
    );
    let dump = status.to_string();
    assert!(!dump.contains("api_key"), "{dump}");
    assert!(!dump.contains("content_sha256"), "{dump}");

    let (st, topics) = get(&client, &format!("{base}/v1/proof/topics")).await;
    assert_eq!(st, 200, "{topics}");
    let items = topics["items"].as_array().cloned().unwrap_or_default();
    let listed: Vec<String> = items
        .iter()
        .filter_map(|t| t.get("id").and_then(Value::as_str).map(ToOwned::to_owned))
        .collect();
    assert!(
        !topics.to_string().contains("content_sha256"),
        "holdout leak: {topics}"
    );

    let (st, missing) = post(
        &client,
        &format!("{base}/v1/submissions"),
        &serde_json::json!({
            "miner_hotkey": hex64("e2e-hotkey"),
            "artifact_digest": hex64("e2e-artifact"),
            "claim": "e2e probe",
            "declared_flops": 1,
            "topic_id": "",
            "manifest": { "train_dataset_ids": ["e2e-mix-v0"] }
        }),
    )
    .await;
    assert_eq!(st, 400, "empty topic_id must 400, got {st} {missing}");
    assert!(
        missing["error"].as_str().is_some_and(|e| !e.is_empty()),
        "silent empty 400: {missing}"
    );

    let can_score = status["can_score"].as_bool().unwrap_or(false);
    let backend = status["eval_backend"].as_str().unwrap_or_default();
    if can_score && backend == "lium" {
        eprintln!("skip live POST: host is Lium + can_score (would rent)");
        return;
    }

    let mut topic_ids: Vec<&str> = STAGING_TOPICS
        .iter()
        .copied()
        .filter(|id| listed.iter().any(|got| got == *id))
        .collect();
    if topic_ids.is_empty() {
        topic_ids.push(listed.first().map_or("dt-no-ib-v0", String::as_str));
    }

    for topic_id in topic_ids {
        let (st, created) = post(
            &client,
            &format!("{base}/v1/submissions"),
            &serde_json::json!({
                "miner_hotkey": hex64("e2e-hotkey"),
                "artifact_digest": hex64(&format!("e2e-artifact-{topic_id}")),
                "claim": "e2e sim submit against an open topic",
                "declared_flops": 1,
                "topic_id": topic_id,
                "manifest": { "train_dataset_ids": ["e2e-mix-v0"] }
            }),
        )
        .await;
        assert!(
            st == 201 || st == 400 || st == 503,
            "unexpected {st} {created}"
        );
        if st == 201 {
            assert!(
                created["id"]
                    .as_str()
                    .is_some_and(|id| id.starts_with("pf_")),
                "silent empty create: {created}"
            );
            assert_eq!(created["topic_id"], topic_id);
            assert!(created["eval_backend"].is_string(), "{created}");
            let id = created["id"].as_str().expect("id");
            let (gst, row) = get(&client, &format!("{base}/v1/submissions/{id}")).await;
            assert_eq!(gst, 200, "{row}");
            assert!(
                row["verdict"].is_object() || row["state"] == "rejected",
                "{row}"
            );
        } else {
            assert!(
                created["error"].as_str().is_some_and(|e| !e.is_empty()),
                "silent empty fail-closed: HTTP {st} {created}"
            );
        }
    }
}
