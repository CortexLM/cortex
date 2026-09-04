//! Off challenges: `relearn`, `relearn-image`, `relearn-agent`.
//!
//! Commands still talk to a local stack (`--gateway`) so those services stay
//! exercisable. They are not live: no trust-root row, no emission.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use clap::{Args, Subcommand};
use serde_json::{json, Value};

use crate::api::{challenge_path, Client};
use crate::catalog::{compact, find_off, Challenge};

const TERMINAL_STATES: [&str; 3] = ["awaiting_admin", "rejected", "champion"];
const POLL_SECS: u64 = 20;
const IMAGE_BASE: &str = "nvidia/Cosmos3-Super-Text2Image";
const IMAGE_BASE_LICENSE: &str = "OpenMDW-1.1";

/// Shared submit / show / status group for Relearn and Relearn Agent.
#[derive(Debug, Subcommand)]
pub enum OffCmd {
    /// Submit an artifact digest plus the training manifest.
    Submit(Box<SubmitArgs>),
    /// Show one submission.
    Show {
        /// Submission id returned by submit.
        id: String,
        /// Keep polling until the submission stops moving.
        #[arg(long)]
        wait: bool,
    },
    /// Show this (off) challenge's status.
    Status,
}

/// Relearn Image: submit / show / status / public prompts.
#[derive(Debug, Subcommand)]
pub enum ImageCmd {
    /// Submit an artifact digest plus the training manifest.
    Submit(Box<SubmitArgs>),
    /// Show one submission.
    Show {
        /// Submission id returned by submit.
        id: String,
        /// Keep polling until the submission stops moving.
        #[arg(long)]
        wait: bool,
    },
    /// Show this (off) challenge's status.
    Status,
    /// Show the frozen public prompt split and its seeds.
    Prompts,
}

/// Artifact and manifest arguments for a Relearn-family submit.
#[derive(Debug, Args)]
pub struct SubmitArgs {
    /// 64-hex miner hotkey.
    #[arg(long, value_name = "HEX64")]
    pub hotkey: String,
    /// SHA-256 hex of the artifact you are submitting.
    #[arg(long, value_name = "SHA256")]
    pub artifact_digest: String,
    /// Optional locator for the artifact.
    #[arg(long, value_name = "URL")]
    pub artifact_uri: Option<String>,
    /// Full manifest JSON file, used verbatim.
    #[arg(long, value_name = "PATH")]
    pub manifest_file: Option<PathBuf>,
    /// Public item, prompt, or episode id. Repeatable.
    #[arg(long = "train-id", value_name = "ID")]
    pub train_ids: Vec<u32>,
    /// Image or observation hash. Repeatable.
    #[arg(long = "train-hash", value_name = "SHA256")]
    pub train_hashes: Vec<String>,
    /// Dataset, corpus, or environment id. Repeatable.
    #[arg(long = "train-dataset", value_name = "ID")]
    pub train_datasets: Vec<String>,
    /// Declared base checkpoint. Relearn Image only.
    #[arg(long, value_name = "MODEL")]
    pub base: Option<String>,
    /// Declared base license. Relearn Image only.
    #[arg(long, value_name = "LICENSE")]
    pub base_license: Option<String>,
    /// Seed-replay claim as cell=sha256. Relearn Image only.
    #[arg(long = "claimed-output", value_name = "CELL=SHA256")]
    pub claimed_outputs: Vec<String>,
    /// Poll the submission until it stops moving.
    #[arg(long)]
    pub wait: bool,
}

/// Run `ctx relearn|agent …`.
pub async fn run(client: &Client, id: &str, cmd: OffCmd, json: bool) -> Result<(), String> {
    let challenge = off(id)?;
    warn_off(challenge);
    match cmd {
        OffCmd::Submit(args) => submit(client, challenge, &args, json).await,
        OffCmd::Show { id, wait } => show(client, challenge, &id, wait, json).await,
        OffCmd::Status => print_one_status(client, challenge, json).await,
    }
}

/// Run `ctx image …`.
pub async fn run_image(client: &Client, cmd: ImageCmd, json: bool) -> Result<(), String> {
    let challenge = off("relearn-image")?;
    warn_off(challenge);
    match cmd {
        ImageCmd::Submit(args) => submit(client, challenge, &args, json).await,
        ImageCmd::Show { id, wait } => show(client, challenge, &id, wait, json).await,
        ImageCmd::Status => print_one_status(client, challenge, json).await,
        ImageCmd::Prompts => print_prompts(client, json).await,
    }
}

fn off(id: &str) -> Result<&'static Challenge, String> {
    find_off(id).ok_or_else(|| format!("{id} is not an off Relearn challenge"))
}

fn warn_off(challenge: &Challenge) {
    eprintln!(
        "ctx: {} is off — no trust-root row, no emission. Live work is bounty and proof.",
        challenge.id
    );
}

async fn submit(
    client: &Client,
    challenge: &Challenge,
    input: &SubmitArgs,
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
        println!(
            "{} submission accepted (off: earns nothing)",
            challenge.label
        );
        print_fields(&reply.body);
        println!();
        println!("Track it: ctx {} show {id}", challenge.command);
    }
    if input.wait && !id.is_empty() {
        println!();
        poll(client, challenge, &id, true, json_out).await?;
    }
    Ok(())
}

async fn show(
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
        let reply = client
            .get(&challenge_path(
                challenge.id,
                &format!("/v1/submissions/{id}"),
            ))
            .await?;
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
        }
        if !wait || TERMINAL_STATES.contains(&state.as_str()) {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_secs(POLL_SECS)).await;
    }
}

async fn print_one_status(
    client: &Client,
    challenge: &Challenge,
    json: bool,
) -> Result<(), String> {
    let body = crate::catalog::fetch_status(client, challenge.id).await?;
    if json {
        println!("{body}");
    } else {
        println!("{} ({}) — OFF", challenge.label, challenge.id);
        if let Some(map) = body.as_object() {
            for (k, v) in map {
                println!("  {k}: {}", compact(v));
            }
        }
    }
    Ok(())
}

async fn print_prompts(client: &Client, json: bool) -> Result<(), String> {
    let reply = client.get("/challenge/relearn-image/v1/prompts").await?;
    if json {
        println!("{}", reply.body);
        return Ok(());
    }
    if !reply.ok() {
        return Err(format!("HTTP {}: {}", reply.status, reply.message()));
    }
    let cells = reply
        .body
        .get("public")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    println!("public cells: {cells}");
    if let Some(v) = reply.body.get("dataset") {
        println!("dataset: {}", compact(v));
    }
    if let Some(v) = reply.body.get("holdout") {
        println!("holdout commitment: {v}");
    }
    println!("Full cells: ctx image prompts --json");
    Ok(())
}

fn print_fields(body: &Value) {
    for field in [
        "id",
        "state",
        "submission_digest",
        "eval_backend",
        "judge_backend",
        "eligible",
        "error",
    ] {
        if let Some(v) = body.get(field) {
            println!("  {field}: {}", compact(v));
        }
    }
}

fn build_manifest(challenge_id: &str, input: &SubmitArgs) -> Result<Value, String> {
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
    match challenge_id {
        "relearn" => Ok(json!({
            "train_item_ids": input.train_ids,
            "train_image_hashes": input.train_hashes,
            "train_dataset_ids": input.train_datasets,
        })),
        "relearn-agent" => Ok(json!({
            "train_episode_ids": input.train_ids,
            "train_observation_hashes": input.train_hashes,
            "train_environment_ids": input.train_datasets,
        })),
        "relearn-image" => Ok(json!({
            "base": input.base.clone().unwrap_or_else(|| IMAGE_BASE.to_owned()),
            "base_license": input
                .base_license
                .clone()
                .unwrap_or_else(|| IMAGE_BASE_LICENSE.to_owned()),
            "train_prompt_ids": input.train_ids,
            "train_dataset_ids": input.train_datasets,
            "claimed_output_hashes": parse_claimed(&input.claimed_outputs)?,
        })),
        other => Err(format!("{other} does not take a Relearn manifest")),
    }
}

fn parse_claimed(pairs: &[String]) -> Result<BTreeMap<String, String>, String> {
    let mut out = BTreeMap::new();
    for pair in pairs {
        let Some((cell, digest)) = pair.split_once('=') else {
            return Err(format!(
                "--claimed-output wants cell=sha256, got {pair:?} (cells look like p1#v0)"
            ));
        };
        out.insert(
            cell.trim().to_owned(),
            normalize_hex64(digest, "claimed output hash")?,
        );
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
        "contamination_evidence_missing: declare what you trained on ({flags}, or --manifest-file). \
         An empty manifest is not a clean check."
    )
}

fn explain_failure(challenge: &Challenge, status: u16, message: &str) -> String {
    let hint = match status {
        400 => "rejected before scoring. Check 64-hex hotkey/digest and a declared manifest.",
        503 => {
            "the host cannot score. Nothing was stored and nothing was spent. This challenge is off."
        }
        _ => "unexpected reply from the challenge service.",
    };
    format!(
        "{} HTTP {status}: {message}\n  {hint}\n  guide: {}",
        challenge.label, challenge.guide
    )
}

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

    fn with_dataset() -> SubmitArgs {
        SubmitArgs {
            hotkey: String::new(),
            artifact_digest: String::new(),
            artifact_uri: None,
            manifest_file: None,
            train_ids: Vec::new(),
            train_hashes: Vec::new(),
            train_datasets: vec!["my-sft-mix-v3".into()],
            base: None,
            base_license: None,
            claimed_outputs: Vec::new(),
            wait: false,
        }
    }

    #[test]
    fn relearn_manifest_uses_item_ids() {
        let mut input = with_dataset();
        input.train_ids = vec![1, 2];
        let m = build_manifest("relearn", &input).expect("manifest");
        assert_eq!(m["train_item_ids"], json!([1, 2]));
    }

    #[test]
    fn agent_manifest_uses_episode_ids() {
        let m = build_manifest("relearn-agent", &with_dataset()).expect("manifest");
        assert!(m.get("train_episode_ids").is_some());
        assert!(m.get("train_item_ids").is_none());
    }

    #[test]
    fn image_manifest_defaults_to_the_pinned_base() {
        let m = build_manifest("relearn-image", &with_dataset()).expect("manifest");
        assert_eq!(m["base"], json!(IMAGE_BASE));
        assert_eq!(m["base_license"], json!(IMAGE_BASE_LICENSE));
    }

    #[test]
    fn empty_manifest_is_refused_locally() {
        let err = build_manifest(
            "relearn",
            &SubmitArgs {
                hotkey: String::new(),
                artifact_digest: String::new(),
                artifact_uri: None,
                manifest_file: None,
                train_ids: Vec::new(),
                train_hashes: Vec::new(),
                train_datasets: Vec::new(),
                base: None,
                base_license: None,
                claimed_outputs: Vec::new(),
                wait: false,
            },
        )
        .expect_err("must refuse");
        assert!(err.contains("contamination_evidence_missing"), "{err}");
    }

    #[test]
    fn claimed_outputs_need_cell_and_digest() {
        assert!(parse_claimed(&["p1#v0".into()]).is_err());
        let ok = parse_claimed(&["p1#v0=".to_owned() + &"ab".repeat(32)]).expect("pairs");
        assert_eq!(ok.len(), 1);
    }
}
