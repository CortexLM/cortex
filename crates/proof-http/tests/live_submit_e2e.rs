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

fn fixture_sk() -> [u8; 32] {
    let mut s = [0x11u8; 32];
    s[0] = 0x42;
    s
}

fn sign_submit_json(body: &mut Value) {
    proof_submit::attach_to_json(body, &fixture_sk()).expect("sign");
}

fn base_url() -> Option<String> {
    std::env::var("PROOF_E2E_BASE")
        .ok()
        .map(|s| s.trim().trim_end_matches('/').to_owned())
        .filter(|s| !s.is_empty())
}

/// Production hosts the live probe must never touch. Compared as a parsed,
/// lower-cased hostname (DNS is case-insensitive); a mixed-case spelling of
/// the same host is still production.
const PROD_HOSTS: &[&str] = &[
    "gateway.cortex.foundation",
    "network.cortex.foundation",
    "chain.joinbase.ai",
];

/// Host part of `url`, lower-cased, trailing-dot stripped. Bare hosts (no
/// scheme) are parsed as HTTPS so `GATEWAY.CORTEX.FOUNDATION` still matches.
fn url_hostname(url: &str) -> Option<String> {
    let parsed = reqwest::Url::parse(url)
        .ok()
        .or_else(|| reqwest::Url::parse(&format!("https://{url}")).ok())?;
    let host = parsed.host_str()?.trim_end_matches('.');
    if host.is_empty() {
        return None;
    }
    Some(host.to_ascii_lowercase())
}

fn host_is_listed(host: &str, listed: &str) -> bool {
    host == listed
        || (host.len() > listed.len()
            && host.ends_with(listed)
            && host.as_bytes()[host.len() - listed.len() - 1] == b'.')
}

fn is_prod_host(base: &str) -> bool {
    url_hostname(base).is_some_and(|host| PROD_HOSTS.iter().any(|p| host_is_listed(&host, p)))
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
    assert_eq!(status["executor"]["gpu_class"], "1x", "{status}");

    // Public executor contract: always 200, `ready` says whether the live 1x
    // offer can rent, `reason` names the refusal when it cannot.
    let (st, executor) = get(&client, &format!("{base}/v1/proof/executor")).await;
    assert_eq!(st, 200, "{executor}");
    assert!(executor["ready"].is_boolean(), "{executor}");
    assert_eq!(executor["pin"]["gpu_class"], "1x", "{executor}");
    if executor["ready"] == false {
        assert!(
            executor["reason"].as_str().is_some_and(|r| !r.is_empty()),
            "silent not-ready executor: {executor}"
        );
    }
    assert!(!executor.to_string().contains("api_key"), "{executor}");

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
        let mut body = serde_json::json!({
            "artifact_digest": hex64(&format!("e2e-artifact-{topic_id}")),
            "claim": "e2e sim submit against an open topic",
            "declared_flops": 1,
            "topic_id": topic_id,
            "manifest": { "train_dataset_ids": ["e2e-mix-v0"] }
        });
        sign_submit_json(&mut body);
        let (st, created) = post(&client, &format!("{base}/v1/submissions"), &body).await;
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

#[test]
fn mixed_case_and_lowercase_production_hosts_are_refused() {
    for url in [
        "https://gateway.cortex.foundation/challenge/proof",
        "https://GATEWAY.CORTEX.FOUNDATION/challenge/proof",
        "https://Gateway.Cortex.Foundation/challenge/proof",
        "http://user@Chain.JoinBase.AI:8080/challenge/proof",
        "https://NETWORK.CORTEX.FOUNDATION./v1",
        "https://sub.gateway.cortex.foundation:443/challenge/proof",
        "GATEWAY.CORTEX.FOUNDATION/challenge/proof",
    ] {
        assert!(is_prod_host(url), "{url} must be refused as production");
    }
}

#[test]
fn staging_loopback_and_path_lookalikes_are_not_production() {
    for url in [
        "http://127.0.0.1:28100",
        "http://localhost:8100/challenge/proof",
        "http://staging.api.joinbase.ai/challenge/proof",
        "http://159.223.159.205/challenge/proof",
        "https://example.com/gateway.cortex.foundation",
    ] {
        assert!(
            !is_prod_host(url),
            "{url} must not be refused as production"
        );
    }
}

/// The shell `--probe` path must refuse mixed-case production hosts before
/// any HTTP client runs. A fake `curl` first on PATH fails the test if the
/// guard is bypassed.
#[test]
fn proof_submit_e2e_script_refuses_mixed_case_production_hosts() {
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;

    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root");
    let bin =
        std::env::temp_dir().join(format!("proof-submit-e2e-fake-curl-{}", std::process::id()));
    std::fs::create_dir_all(&bin).expect("fake curl dir");
    let curl = bin.join("curl");
    std::fs::write(&curl, "#!/bin/sh\necho UNEXPECTED_CURL >&2\nexit 99\n").expect("fake curl");
    let mut perm = std::fs::metadata(&curl).expect("stat").permissions();
    perm.set_mode(0o755);
    std::fs::set_permissions(&curl, perm).expect("chmod");
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    for url in [
        "https://GATEWAY.CORTEX.FOUNDATION/challenge/proof",
        "https://gateway.cortex.foundation/challenge/proof",
        "http://Chain.JoinBase.AI/challenge/proof",
        "https://NETWORK.CORTEX.FOUNDATION/challenge/proof",
    ] {
        let out = Command::new("bash")
            .arg(root.join("deploy/scripts/proof-submit-e2e.sh"))
            .args(["--probe", url])
            .env("PATH", &path)
            .current_dir(&root)
            .output()
            .expect("run proof-submit-e2e.sh");
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            out.status.code().unwrap_or(-1),
            2,
            "{url} must exit 2:\n{text}"
        );
        assert!(text.contains("refusing production host"), "{url}: {text}");
        assert!(
            !text.contains("UNEXPECTED_CURL"),
            "{url}: curl must not run:\n{text}"
        );
    }

    let staging = Command::new("bash")
        .arg(root.join("deploy/scripts/proof-submit-e2e.sh"))
        .args(["--probe", "http://staging.api.joinbase.ai/challenge/proof"])
        .env("PATH", &path)
        .current_dir(&root)
        .output()
        .expect("run proof-submit-e2e.sh staging");
    let staging_text = format!(
        "{}{}",
        String::from_utf8_lossy(&staging.stdout),
        String::from_utf8_lossy(&staging.stderr)
    );
    assert_ne!(
        staging.status.code().unwrap_or(-1),
        2,
        "staging must not be refused as production:\n{staging_text}"
    );
    assert!(
        !staging_text.contains("refusing production host"),
        "staging must not be refused as production:\n{staging_text}"
    );

    let _ = std::fs::remove_dir_all(bin);
}
