//! Submit and poll the three Relearn challenges.
//!
//! All three take the same envelope — hotkey, artifact digest, and a training
//! manifest — and all three reject a manifest that declares nothing, because a
//! contamination gate with no evidence fails closed instead of passing.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::{json, Value};

use crate::api::{challenge_path, Client};
use crate::catalog::{compact, Challenge};

/// States a submission does not move out of on its own.
const TERMINAL_STATES: [&str; 3] = ["awaiting_admin", "rejected", "champion"];

/// Poll interval for `--wait`.
const POLL_SECS: u64 = 20;

/// Manifest and artifact arguments shared by the three Relearn challenges.
#[derive(Debug, Default)]
pub struct SubmitInput {
    /// 64-hex miner hotkey.
    pub hotkey: String,
    /// SHA-256 hex of the artifact you are submitting.
    pub artifact_digest: String,
    /// Optional locator for the artifact.
    pub artifact_uri: Option<String>,
    /// Complete manifest JSON, used verbatim when present.
    pub manifest_file: Option<PathBuf>,
    /// Public item / prompt / episode ids your training mix touched.
    pub train_ids: Vec<u32>,
    /// Image or observation hashes your training mix touched.
    pub train_hashes: Vec<String>,
    /// Dataset / environment / corpus ids you trained on.
    pub train_datasets: Vec<String>,
    /// Declared base checkpoint (Relearn Image).
    pub base: Option<String>,
    /// Declared base license (Relearn Image).
    pub base_license: Option<String>,
    /// `cell=sha256` pairs for the seed-replay gate (Relearn Image).
    pub claimed_outputs: Vec<String>,
    /// Poll until the submission reaches a terminal state.
    pub wait: bool,
}

/// Pinned Relearn Image generator seed. Anything else is a `400`, not a low score.
const IMAGE_BASE: &str = "nvidia/Cosmos3-Super-Text2Image";

/// License inherited from that checkpoint.
const IMAGE_BASE_LICENSE: &str = "OpenMDW-1.1";

/// POST a submission and print the reply.
pub async fn submit(
    client: &Client,
    challenge: &Challenge,
    input: &SubmitInput,
    json_out: bool,
) -> Result<(), String> {
    let hotkey = normalize_hex64(&input.hotkey, "hotkey")?;
    let digest = normalize_hex64(&input.artifact_digest, "artifact-digest")?;
    let manifest = build_manifest(challenge.id, input)?;
    let mut body = json!({
        "miner_hotkey": hotkey,
        "artifact_digest": digest,
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
        return Err(explain_failure(challenge, reply.status, &reply.message()));
    }
    let id = reply
        .body
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    if !json_out {
        println!("{} submission accepted", challenge.label);
        print_fields(&reply.body);
        println!();
        println!("Track it:");
        println!("  ctx {} show {id}", challenge.command);
        println!("An eligible run stops at awaiting_admin. Operators promote; you do not.");
    }
    if input.wait && !id.is_empty() {
        println!();
        poll(client, challenge, &id, true, json_out).await?;
    }
    Ok(())
}

/// GET one submission, optionally polling to a terminal state.
pub async fn show(
    client: &Client,
    challenge: &Challenge,
    id: &str,
    wait: bool,
    json_out: bool,
) -> Result<(), String> {
    poll(client, challenge, id, wait, json_out).await
}

async fn poll(
    client: &Client,
    challenge: &Challenge,
    id: &str,
    wait: bool,
    json_out: bool,
) -> Result<(), String> {
    loop {
        let path = challenge_path(challenge.id, &format!("/v1/submissions/{id}"));
        let reply = client.get(&path).await?;
        if !reply.ok() {
            return Err(explain_failure(challenge, reply.status, &reply.message()));
        }
        let state = reply
            .body
            .get("state")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_owned();
        if json_out {
            println!("{}", reply.body);
        } else {
            println!("{} {id}: {state}", challenge.label);
            print_fields(&reply.body);
            if let Some(reason) = reply.body.get("reject_reason") {
                println!("  reject_reason: {}", compact(reason));
            }
        }
        if !wait || TERMINAL_STATES.contains(&state.as_str()) {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_secs(POLL_SECS)).await;
    }
}

/// Print the fields a miner acts on, skipping the receipt blob.
fn print_fields(body: &Value) {
    for field in [
        "id",
        "state",
        "submission_digest",
        "eval_backend",
        "judge_backend",
        "eligible",
        "holdout_unsealed",
        "holdout_cells",
        "episodes_scored",
    ] {
        if let Some(v) = body.get(field) {
            println!("  {field}: {}", compact(v));
        }
    }
}

fn build_manifest(challenge_id: &str, input: &SubmitInput) -> Result<Value, String> {
    if let Some(path) = &input.manifest_file {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| format!("read manifest {}: {e}", path.display()))?;
        return serde_json::from_str::<Value>(&raw)
            .map_err(|e| format!("manifest {} is not valid JSON: {e}", path.display()));
    }
    let declared = input.train_ids.len() + input.train_hashes.len() + input.train_datasets.len();
    if declared == 0 {
        return Err(missing_evidence_help(challenge_id));
    }
    let manifest = match challenge_id {
        "relearn" => json!({
            "train_item_ids": input.train_ids,
            "train_image_hashes": input.train_hashes,
            "train_dataset_ids": input.train_datasets,
        }),
        "relearn-agent" => json!({
            "train_episode_ids": input.train_ids,
            "train_observation_hashes": input.train_hashes,
            "train_environment_ids": input.train_datasets,
        }),
        "relearn-image" => json!({
            "base": input.base.clone().unwrap_or_else(|| IMAGE_BASE.to_owned()),
            "base_license": input
                .base_license
                .clone()
                .unwrap_or_else(|| IMAGE_BASE_LICENSE.to_owned()),
            "train_prompt_ids": input.train_ids,
            "train_dataset_ids": input.train_datasets,
            "claimed_output_hashes": parse_claimed(&input.claimed_outputs)?,
        }),
        other => return Err(format!("{other} does not take a Relearn manifest")),
    };
    Ok(manifest)
}

fn parse_claimed(pairs: &[String]) -> Result<BTreeMap<String, String>, String> {
    let mut out = BTreeMap::new();
    for pair in pairs {
        let Some((cell, digest)) = pair.split_once('=') else {
            return Err(format!(
                "--claimed-output wants cell=sha256, got {pair:?} (cells look like p1#v0)"
            ));
        };
        let digest = normalize_hex64(digest, "claimed output hash")?;
        out.insert(cell.trim().to_owned(), digest);
    }
    Ok(out)
}

fn missing_evidence_help(challenge_id: &str) -> String {
    let flags = match challenge_id {
        "relearn" => "--train-id, --train-hash, or --train-dataset",
        "relearn-agent" => {
            "--train-id (episode), --train-hash (observation), or --train-dataset (environment)"
        }
        _ => "--train-id (bench prompt) or --train-dataset",
    };
    format!(
        "declare what you trained on: pass at least one of {flags}, or a full manifest with \
         --manifest-file.\n  An empty manifest does not skip the contamination gate; it fails it \
         (contamination_evidence_missing)."
    )
}

fn explain_failure(challenge: &Challenge, status: u16, message: &str) -> String {
    let hint = match status {
        400 => "the service rejected the request before scoring anything. Check the hotkey and artifact digest are 64 hex chars, and that the manifest declares what you trained on.",
        404 => "no such submission id on this challenge.",
        429 => "you are inside a quota window. Wait and retry; nothing was recorded against you.",
        503 => "the host cannot score right now, so it refuses rather than banking work it could never pay for. Nothing was stored and nothing was spent. Check 'ctx status' for can_score and retry later.",
        _ => "unexpected reply from the challenge service.",
    };
    format!(
        "{} HTTP {status}: {message}\n  {hint}\n  guide: {}",
        challenge.label, challenge.guide
    )
}

/// Accept `0x`-prefixed or bare 64-hex, returning lowercase hex.
fn normalize_hex64(raw: &str, field: &str) -> Result<String, String> {
    let t = raw.trim().trim_start_matches("0x");
    if t.len() != 64 || !t.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("{field} must be 64 hex characters"));
    }
    Ok(t.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input_with_dataset() -> SubmitInput {
        SubmitInput {
            train_datasets: vec!["my-sft-mix-v3".into()],
            ..SubmitInput::default()
        }
    }

    #[test]
    fn relearn_manifest_uses_item_ids() {
        let mut input = input_with_dataset();
        input.train_ids = vec![1, 2];
        let m = build_manifest("relearn", &input).expect("manifest");
        assert_eq!(m["train_item_ids"], json!([1, 2]));
        assert_eq!(m["train_dataset_ids"], json!(["my-sft-mix-v3"]));
    }

    #[test]
    fn agent_manifest_uses_episode_ids() {
        let m = build_manifest("relearn-agent", &input_with_dataset()).expect("manifest");
        assert!(m.get("train_episode_ids").is_some());
        assert!(m.get("train_environment_ids").is_some());
        assert!(m.get("train_item_ids").is_none());
    }

    #[test]
    fn image_manifest_defaults_to_the_pinned_base_and_license() {
        let m = build_manifest("relearn-image", &input_with_dataset()).expect("manifest");
        assert_eq!(m["base"], json!("nvidia/Cosmos3-Super-Text2Image"));
        assert_eq!(m["base_license"], json!("OpenMDW-1.1"));
    }

    /// An empty manifest is the failure mode this CLI exists to prevent: it
    /// reads like a clean contamination check and is rejected as evidence
    /// missing after the miner has already paid for a run.
    #[test]
    fn an_empty_manifest_is_refused_locally() {
        let err = build_manifest("relearn", &SubmitInput::default()).expect_err("must refuse");
        assert!(err.contains("contamination_evidence_missing"), "{err}");
    }

    #[test]
    fn claimed_outputs_need_cell_and_digest() {
        let ok = parse_claimed(&["p1#v0=".to_owned() + &"ab".repeat(32)]).expect("pairs");
        assert_eq!(ok.len(), 1);
        assert!(parse_claimed(&["p1#v0".to_owned()]).is_err());
        assert!(parse_claimed(&["p1#v0=nothex".to_owned()]).is_err());
    }

    #[test]
    fn hex64_normalizes_and_rejects() {
        let hk = "AB".repeat(32);
        assert_eq!(
            normalize_hex64(&hk, "hotkey").expect("hex"),
            "ab".repeat(32)
        );
        assert_eq!(
            normalize_hex64(&format!("0x{hk}"), "hotkey").expect("hex"),
            "ab".repeat(32)
        );
        assert!(normalize_hex64("abc", "hotkey").is_err());
    }

    #[test]
    fn a_503_is_explained_as_fail_closed() {
        let c = &crate::catalog::LIVE[0];
        let msg = explain_failure(c, 503, "scoring unconfigured");
        assert!(msg.contains("Nothing was stored"), "{msg}");
        assert!(msg.contains("can_score"), "{msg}");
    }

    #[test]
    fn terminal_states_stop_the_poll_loop() {
        assert!(TERMINAL_STATES.contains(&"awaiting_admin"));
        assert!(TERMINAL_STATES.contains(&"rejected"));
        assert!(!TERMINAL_STATES.contains(&"evaluating"));
    }
}
