//! Teacher HTTP smoke: `GET /v1/models` when URL + key are set.
//!
//! Skip when `RELEARN_TEACHER_API_URL` or `RELEARN_TEACHER_API_KEY` is unset
//! so CI stays green. Never print the key. Never bake a host.

#![forbid(unsafe_code)]

use prism_lium::EvalJobBackend;
use relearn_challenge_task::{teacher_api_key, teacher_api_url};

#[tokio::test]
async fn teacher_http_models_or_skip() {
    let Some(base) = teacher_api_url() else {
        eprintln!("skip teacher HTTP smoke: RELEARN_TEACHER_API_URL unset");
        return;
    };
    let Some(key) = teacher_api_key() else {
        eprintln!("skip teacher HTTP smoke: RELEARN_TEACHER_API_KEY unset");
        return;
    };
    let url = format!("{}/models", base.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .unwrap_or_else(|e| panic!("teacher HTTP client: {e}"));
    let resp = client
        .get(&url)
        .bearer_auth(&key)
        .send()
        .await
        .unwrap_or_else(|e| panic!("teacher GET /v1/models transport: {e}"));
    let status = resp.status();
    assert!(
        status.is_success(),
        "teacher GET /v1/models HTTP {status} (do not log the key)"
    );
}

#[tokio::test]
async fn lium_list_offers_or_skip() {
    let key = match std::env::var("LIUM_API_KEY") {
        Ok(k) if !k.trim().is_empty() => k,
        _ => {
            eprintln!("skip Lium smoke: LIUM_API_KEY unset");
            return;
        }
    };
    let client = prism_lium::LiumClient::new(key).unwrap_or_else(|e| panic!("lium client: {e}"));
    client
        .list_offers(Some(2.0))
        .await
        .unwrap_or_else(|e| panic!("lium list_offers: {e}"));
}
