//! Proof challenge: submit a reproducible experiment against an open topic.

use std::path::PathBuf;
use std::time::Duration;

use serde_json::{json, Value};

use crate::api::{challenge_path, Client};
use crate::catalog::{compact, find};

/// States a submission does not move out of on its own.
const TERMINAL_STATES: [&str; 3] = ["awaiting_admin", "rejected", "champion"];

/// Poll interval for `--wait`.
const POLL_SECS: u64 = 20;

/// Artifact, topic, and manifest arguments for a Proof submit.
#[derive(Debug, Default)]
pub struct SubmitInput {
    /// 64-hex miner hotkey.
    pub hotkey: String,
    /// Open topic id from `GET /v1/proof/topics`.
    pub topic_id: String,
    /// SHA-256 hex of the artifact you are submitting.
    pub artifact_digest: String,
    /// Optional locator for the artifact.
    pub artifact_uri: Option<String>,
    /// Public claim the RLM re-runs (what you say the recipe achieved).
    pub claim: String,
    /// FLOPs you spent. Must be ≤ the topic budget.
    pub declared_flops: u64,
    /// Complete manifest JSON, used verbatim when present.
    pub manifest_file: Option<PathBuf>,
    /// Shard content hashes your training mix touched.
    pub train_hashes: Vec<String>,
    /// Dataset / corpus ids you trained on.
    pub train_datasets: Vec<String>,
    /// Poll until the submission reaches a terminal state.
    pub wait: bool,
}

/// POST a Proof submission and print the reply.
pub async fn submit(client: &Client, input: &SubmitInput, json_out: bool) -> Result<(), String> {
    let challenge = find("proof").ok_or_else(|| "proof is not a live challenge".to_owned())?;
    let hotkey = normalize_hex64(&input.hotkey, "hotkey")?;
    let digest = normalize_hex64(&input.artifact_digest, "artifact-digest")?;
    let topic_id = input.topic_id.trim();
    if topic_id.is_empty() {
        return Err("topic-id is required (ctx proof topics lists currently open ids)".into());
    }
    let claim = input.claim.trim();
    if claim.is_empty() {
        return Err("claim is required (what the recipe achieved)".into());
    }
    let manifest = build_manifest(input)?;
    let mut body = json!({
        "miner_hotkey": hotkey,
        "topic_id": topic_id,
        "artifact_digest": digest,
        "claim": claim,
        "declared_flops": input.declared_flops,
        "manifest": manifest,
    });
    if let Some(uri) = &input.artifact_uri {
        body["artifact_uri"] = Value::String(uri.clone());
    }

    let reply = client
        .post(&challenge_path(challenge.id, "/v1/submissions"), &body)
        .await?;
    if json_out {
        println!("{}", reply.body);
    }
    if !reply.ok() {
        return Err(explain_failure(reply.status, &reply.message()));
    }
    let id = reply
        .body
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    if !json_out {
        println!("Proof submission accepted");
        print_fields(&reply.body);
        println!();
        println!("Track it:");
        println!("  ctx proof show {id}");
    }
    if input.wait && !id.is_empty() {
        println!();
        poll(client, &id, true, json_out).await?;
    }
    Ok(())
}

/// GET one submission, optionally polling to a terminal state.
pub async fn show(client: &Client, id: &str, wait: bool, json_out: bool) -> Result<(), String> {
    poll(client, id, wait, json_out).await
}

/// List currently published topics. Holdout records are never in this payload.
pub async fn topics(client: &Client, json_out: bool) -> Result<(), String> {
    let reply = client
        .get(&challenge_path("proof", "/v1/proof/topics"))
        .await?;
    if json_out {
        println!("{}", reply.body);
    }
    if !reply.ok() {
        return Err(explain_failure(reply.status, &reply.message()));
    }
    if json_out {
        return Ok(());
    }
    let items = reply
        .body
        .as_array()
        .or_else(|| reply.body.get("topics").and_then(Value::as_array));
    match items {
        Some(list) if list.is_empty() => {
            println!("No open topics. Submits answer 503 until an operator publishes one.");
        }
        Some(list) => {
            println!("{} published topic(s):", list.len());
            for t in list {
                let id = compact(
                    t.get("topic_id")
                        .or_else(|| t.get("id"))
                        .unwrap_or(&Value::Null),
                );
                let status = compact(t.get("status").unwrap_or(&Value::Null));
                let mode = compact(t.get("payout_mode").unwrap_or(&Value::Null));
                println!("  {id}  status={status}  payout={mode}");
            }
            println!();
            println!("Holdout records are never listed. WTA: winner takes the topic. Discovery: floor + novelty.");
        }
        None => println!("{}", compact(&reply.body)),
    }
    Ok(())
}

async fn poll(client: &Client, id: &str, wait: bool, json_out: bool) -> Result<(), String> {
    loop {
        let reply = client
            .get(&challenge_path("proof", &format!("/v1/submissions/{id}")))
            .await?;
        if json_out {
            println!("{}", reply.body);
        }
        if !reply.ok() {
            return Err(explain_failure(reply.status, &reply.message()));
        }
        let state = reply
            .body
            .get("state")
            .and_then(Value::as_str)
            .unwrap_or("");
        if !json_out {
            println!("proof {id}  state={state}");
            print_fields(&reply.body);
        }
        if !wait || TERMINAL_STATES.contains(&state) {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_secs(POLL_SECS)).await;
    }
}

fn build_manifest(input: &SubmitInput) -> Result<Value, String> {
    if let Some(path) = &input.manifest_file {
        let text =
            std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
        let value: Value =
            serde_json::from_str(&text).map_err(|e| format!("manifest JSON: {e}"))?;
        ensure_declared(&value)?;
        return Ok(value);
    }
    let hashes: Vec<&str> = input
        .train_hashes
        .iter()
        .map(String::as_str)
        .filter(|s| !s.trim().is_empty())
        .collect();
    let datasets: Vec<&str> = input
        .train_datasets
        .iter()
        .map(String::as_str)
        .filter(|s| !s.trim().is_empty())
        .collect();
    if hashes.is_empty() && datasets.is_empty() {
        return Err(
            "contamination_evidence_missing: declare train-hash or train-dataset \
             (an empty manifest is not a clean check)"
                .into(),
        );
    }
    Ok(json!({
        "train_content_hashes": hashes,
        "train_dataset_ids": datasets,
    }))
}

fn ensure_declared(manifest: &Value) -> Result<(), String> {
    let hashes = manifest
        .get("train_content_hashes")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    let datasets = manifest
        .get("train_dataset_ids")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    if hashes == 0 && datasets == 0 {
        return Err(
            "contamination_evidence_missing: the manifest declared nothing to check".into(),
        );
    }
    Ok(())
}

fn normalize_hex64(s: &str, field: &str) -> Result<String, String> {
    let t = s.trim().trim_start_matches("0x");
    if t.len() != 64 || !t.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("{field} must be 64 hex characters"));
    }
    Ok(t.to_ascii_lowercase())
}

fn print_fields(body: &Value) {
    for field in [
        "claim",
        "declared_flops",
        "id",
        "topic_id",
        "state",
        "eval_backend",
        "eligible",
        "submission_digest",
        "error",
    ] {
        if let Some(v) = body.get(field) {
            println!("  {field}: {}", compact(v));
        }
    }
}

fn explain_failure(status: u16, message: &str) -> String {
    match status {
        400 => format!("refused ({message}). Nothing was stored and nothing was rented."),
        503 => format!(
            "HTTP 503: {message}\n  The host cannot score right now (empty eval digest, \
             missing/closed RLM judge backend, no open topics, or an unsealed baseline). \
             Nothing was stored, nothing was rented."
        ),
        other => format!("HTTP {other}: {message}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_manifest_is_contamination_evidence_missing() {
        let err = build_manifest(&SubmitInput::default()).expect_err("empty");
        assert!(err.contains("contamination_evidence_missing"), "{err}");
    }

    #[test]
    fn claim_and_declared_flops_are_required_on_the_wire_shape() {
        let input = SubmitInput {
            train_datasets: vec!["my-mix-v0".into()],
            claim: "beat baseline".into(),
            declared_flops: 1,
            ..SubmitInput::default()
        };
        let m = build_manifest(&input).expect("declared");
        assert_eq!(m["train_dataset_ids"][0], "my-mix-v0");
        assert_eq!(input.claim, "beat baseline");
        assert_eq!(input.declared_flops, 1);
    }

    #[test]
    fn hotkey_must_be_64_hex() {
        assert!(normalize_hex64("abcd", "hotkey").is_err());
        let ok = "a".repeat(64);
        assert_eq!(normalize_hex64(&ok, "hotkey").unwrap(), ok);
    }
}
