//! Proof challenge: submit a reproducible experiment against an open topic.

use std::path::PathBuf;
use std::time::Duration;

use keystore::{default_wallets_dir, load_hotkey, mini_secret_from_key_file, BittensorWallet};
use proof_submit::{
    canonical_hex, fresh_submit_nonce_hex, hotkey_hex, is_lowercase_hex, manifest_lists,
    parse_hotkey_hex, parse_signature_hex, parse_submit_nonce_hex, sign_submit, verify_submit,
    SubmitFields, PROOF_SUBMIT_DOMAIN_LABEL,
};
use serde_json::{json, Value};

use crate::api::{challenge_path, Client};
use crate::catalog::{compact, find};

/// States a submission does not move out of on its own. `queued` is the one
/// non-terminal state: the topic defers scoring and the operator drains the
/// queue later, so `--wait` keeps polling through it.
const TERMINAL_STATES: [&str; 3] = ["awaiting_admin", "rejected", "champion"];

/// What a `queued` row means for the miner.
const QUEUED_HINT: &str =
    "Scoring is deferred on this topic: the row is queued (no eval, no rent yet) \
     and is scored in order once the operator lifts the flag and drains the queue.";

/// Poll interval for `--wait`.
const POLL_SECS: u64 = 20;

/// Miner key for a Proof submit signature.
#[derive(Debug, Default)]
pub struct SubmitKey {
    /// 64-hex miner hotkey. Derived from the secret when omitted.
    pub hotkey: Option<String>,
    /// 32-byte hotkey mini-secret file (never a mnemonic).
    pub secret_file: Option<PathBuf>,
    /// Bittensor wallets directory.
    pub wallet_dir: Option<PathBuf>,
    /// Wallet name under the wallets directory.
    pub wallet_name: Option<String>,
    /// Hotkey file name inside that wallet.
    pub wallet_hotkey: String,
    /// 128-hex signature produced offline.
    pub signature: Option<String>,
    /// 64-hex single-use nonce. Fresh random when omitted; required with
    /// `signature` (it is part of the signed bytes).
    pub submit_nonce: Option<String>,
}

/// What goes on the wire and into the signature, resolved once so both
/// agree byte for byte.
#[derive(Debug)]
struct SignedSubmit {
    hotkey: String,
    signature: String,
    nonce: String,
    manifest: Value,
}

/// Artifact, topic, and manifest arguments for a Proof submit.
#[derive(Debug, Default)]
pub struct SubmitInput {
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
    /// sr25519 key or offline signature.
    pub key: SubmitKey,
}

/// POST a Proof submission and print the reply.
pub async fn submit(client: &Client, input: &SubmitInput, json_out: bool) -> Result<(), String> {
    let challenge = find("proof").ok_or_else(|| "proof is not a live challenge".to_owned())?;
    let digest = normalize_hex64(&input.artifact_digest, "artifact-digest")?;
    let topic_id = input.topic_id.trim();
    if topic_id.is_empty() {
        return Err("topic-id is required (ctx proof topics lists currently open ids)".into());
    }
    let claim = input.claim.trim();
    if claim.is_empty() {
        return Err("claim is required (what the recipe achieved)".into());
    }
    let signed = resolve_signed(input, topic_id, &digest, claim)?;
    let mut body = json!({
        "miner_hotkey": signed.hotkey,
        "hotkey_signature": signed.signature,
        "submit_nonce": signed.nonce,
        "topic_id": topic_id,
        "artifact_digest": digest,
        "claim": claim,
        "declared_flops": input.declared_flops,
        "manifest": signed.manifest,
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
        if is_queued(&reply.body) {
            println!("  {QUEUED_HINT}");
        }
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

/// Print `miner_hotkey`, `hotkey_signature`, `submit_nonce`, and the exact
/// `manifest` to post, without posting.
pub fn print_signature(input: &SubmitInput, json_out: bool) -> Result<(), String> {
    let digest = normalize_hex64(&input.artifact_digest, "artifact-digest")?;
    let topic_id = input.topic_id.trim();
    if topic_id.is_empty() {
        return Err("topic-id is required".into());
    }
    let claim = input.claim.trim();
    if claim.is_empty() {
        return Err("claim is required".into());
    }
    let signed = resolve_signed(input, topic_id, &digest, claim)?;
    if json_out {
        println!(
            "{}",
            json!({
                "miner_hotkey": signed.hotkey,
                "hotkey_signature": signed.signature,
                "submit_nonce": signed.nonce,
                "manifest": signed.manifest,
                "domain": PROOF_SUBMIT_DOMAIN_LABEL,
            })
        );
        return Ok(());
    }
    println!("miner_hotkey={}", signed.hotkey);
    println!("hotkey_signature={}", signed.signature);
    println!("submit_nonce={}", signed.nonce);
    println!("manifest={}", signed.manifest);
    println!("domain={PROOF_SUBMIT_DOMAIN_LABEL}");
    Ok(())
}

fn resolve_signed(
    input: &SubmitInput,
    topic_id: &str,
    digest: &str,
    claim: &str,
) -> Result<SignedSubmit, String> {
    let manifest = build_manifest(input)?;
    let (hashes, datasets) = manifest_lists(&json!({ "manifest": manifest }))
        .map_err(|_| "manifest lists must be arrays of strings".to_owned())?;
    let nonce = match &input.key.submit_nonce {
        Some(raw) => {
            let n = canonical_hex(raw);
            parse_submit_nonce_hex(&n).map_err(|_| "submit-nonce must be 64 hex characters")?;
            n
        }
        None if input.key.signature.is_some() => {
            return Err("submit-nonce is required with --signature (it is signed)".into());
        }
        None => fresh_submit_nonce_hex(),
    };
    let (hotkey, signature) = resolve_hotkey_and_sig(
        &input.key,
        SubmitFields {
            hotkey_hex: "",
            topic_id,
            artifact_digest: digest,
            declared_flops: input.declared_flops,
            claim,
            train_content_hashes: &hashes,
            train_dataset_ids: &datasets,
            submit_nonce_hex: &nonce,
        },
    )?;
    Ok(SignedSubmit {
        hotkey,
        signature,
        nonce,
        manifest,
    })
}

/// Exactly one signer. The CLI already refuses combinations; this guards
/// library callers so a wallet is never silently shadowed by another key.
fn single_signer(key: &SubmitKey) -> Result<(), String> {
    let sources = usize::from(key.signature.is_some())
        + usize::from(key.secret_file.is_some())
        + usize::from(key.wallet_name.is_some());
    if sources > 1 {
        return Err(
            "pass one signer: --signature, --secret-file, or --wallet-name (not several)".into(),
        );
    }
    Ok(())
}

/// `fields.hotkey_hex` is filled in here from `--hotkey` / the loaded secret.
fn resolve_hotkey_and_sig(
    key: &SubmitKey,
    fields: SubmitFields<'_>,
) -> Result<(String, String), String> {
    single_signer(key)?;
    if let Some(hex_sig) = &key.signature {
        let hotkey = normalize_hex64(
            key.hotkey
                .as_deref()
                .ok_or("hotkey is required with --signature")?,
            "hotkey",
        )?;
        let pk = parse_hotkey_hex(&hotkey).map_err(|e| e.to_string())?;
        let sig_hex = canonical_hex(hex_sig);
        let sig =
            parse_signature_hex(&sig_hex).map_err(|_| "hotkey_signature invalid".to_owned())?;
        let fields = SubmitFields {
            hotkey_hex: &hotkey,
            ..fields
        };
        verify_submit(&pk, &fields, &sig).map_err(|_| "hotkey_signature invalid".to_owned())?;
        return Ok((hotkey, sig_hex));
    }
    let sk = load_mini_secret(key)?;
    let derived = hotkey_hex(&sk).map_err(|e| e.to_string())?;
    let hotkey = if let Some(raw) = &key.hotkey {
        let want = normalize_hex64(raw, "hotkey")?;
        if want != derived {
            return Err(
                "hotkey_mismatch: --hotkey is not the public key of the loaded secret".into(),
            );
        }
        want
    } else {
        derived
    };
    let fields = SubmitFields {
        hotkey_hex: &hotkey,
        ..fields
    };
    let sig = sign_submit(&sk, &fields).map_err(|e| e.to_string())?;
    Ok((hotkey, hex::encode(sig)))
}

fn load_mini_secret(key: &SubmitKey) -> Result<[u8; 32], String> {
    if let Some(path) = &key.secret_file {
        return mini_secret_from_key_file(path).map_err(|e| e.to_string());
    }
    if let Some(name) = &key.wallet_name {
        let dir = key.wallet_dir.clone().unwrap_or_else(default_wallets_dir);
        let hotkey_name = if key.wallet_hotkey.is_empty() {
            "default"
        } else {
            &key.wallet_hotkey
        };
        let wallet = BittensorWallet::new(name, hotkey_name);
        let kp = load_hotkey(&dir, wallet.wallet_name(), wallet.hotkey_name())
            .map_err(|e| e.to_string())?;
        if let Some(raw) = &key.hotkey {
            let want = normalize_hex64(raw, "hotkey")?;
            let got = hex::encode(kp.public_key());
            if want != got {
                return Err(format!(
                    "hotkey_mismatch: wallet hotkey {} is not --hotkey",
                    kp.ss58_address()
                ));
            }
        }
        return Ok(*kp.expose_mini_secret());
    }
    Err(
        "Proof submit requires an sr25519 signature over base-proof-submit-v1: \
         pass --secret-file, --wallet-name, or --signature"
            .into(),
    )
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
    let items = topic_list_items(&reply.body);
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
            if is_queued(&reply.body) {
                println!("  {QUEUED_HINT}");
            }
        }
        if !wait || TERMINAL_STATES.contains(&state) {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_secs(POLL_SECS)).await;
    }
}

fn is_queued(body: &Value) -> bool {
    body.get("state").and_then(Value::as_str) == Some("queued")
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

/// The one wire form of a 64-hex field, produced by the same `proof-submit`
/// canonicaliser the host's parser accepts — `ctx` never spells hex itself.
fn normalize_hex64(s: &str, field: &str) -> Result<String, String> {
    let canonical = canonical_hex(s);
    if !is_lowercase_hex(&canonical, 64) {
        return Err(format!("{field} must be 64 hex characters"));
    }
    Ok(canonical)
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
        "detail",
        "error",
    ] {
        if let Some(v) = body.get(field) {
            println!("  {field}: {}", compact(v));
        }
    }
}

/// `GET /v1/proof/topics` returns `{ "items": [...] }`. Older shapes used
/// a bare array or `{ "topics": [...] }`.
fn topic_list_items(body: &Value) -> Option<&Vec<Value>> {
    body.as_array()
        .or_else(|| body.get("items").and_then(Value::as_array))
        .or_else(|| body.get("topics").and_then(Value::as_array))
}

fn explain_failure(status: u16, message: &str) -> String {
    match status {
        400 => format!("refused ({message}). Nothing was stored and nothing was rented."),
        401 => format!(
            "unauthorized ({message}). Proof submit requires a hotkey_signature over \
             {PROOF_SUBMIT_DOMAIN_LABEL} covering topic, artifact, FLOPs, claim, manifest \
             and a single-use submit_nonce (X-Lium-Api-Key is not identity)."
        ),
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

    fn signed_input(key: SubmitKey) -> SubmitInput {
        SubmitInput {
            topic_id: "dt-no-ib-v0".into(),
            artifact_digest: "ab".repeat(32),
            claim: "beat".into(),
            declared_flops: 1,
            train_datasets: vec!["my-mix-v0".into()],
            key,
            ..SubmitInput::default()
        }
    }

    #[test]
    fn secret_file_signs_the_locked_payload_with_manifest_and_nonce() {
        let mut sk = [0x11u8; 32];
        sk[0] = 0x42;
        let dir = std::env::temp_dir().join(format!(
            "ctx-proof-sig-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("sk");
        std::fs::write(&path, hex::encode(sk)).expect("write sk");
        let input = signed_input(SubmitKey {
            secret_file: Some(path),
            wallet_hotkey: "default".into(),
            ..SubmitKey::default()
        });
        let digest = "ab".repeat(32);
        let signed = resolve_signed(&input, "dt-no-ib-v0", &digest, "beat").expect("sign");
        assert_eq!(signed.hotkey, hotkey_hex(&sk).expect("pk"));
        parse_submit_nonce_hex(&signed.nonce).expect("fresh 64-hex nonce");
        assert_eq!(signed.manifest["train_dataset_ids"][0], "my-mix-v0");
        let pk = parse_hotkey_hex(&signed.hotkey).expect("pk");
        let raw = parse_signature_hex(&signed.signature).expect("sig");
        let datasets = ["my-mix-v0".to_owned()];
        let fields = SubmitFields {
            hotkey_hex: &signed.hotkey,
            topic_id: "dt-no-ib-v0",
            artifact_digest: &digest,
            declared_flops: 1,
            claim: "beat",
            train_content_hashes: &[],
            train_dataset_ids: &datasets,
            submit_nonce_hex: &signed.nonce,
        };
        verify_submit(&pk, &fields, &raw).expect("verify");
        let other = ["other-mix".to_owned()];
        let tampered = SubmitFields {
            train_dataset_ids: &other,
            ..fields
        };
        assert!(verify_submit(&pk, &tampered, &raw).is_err());

        // A second run never reuses the nonce.
        let again = resolve_signed(&input, "dt-no-ib-v0", &digest, "beat").expect("sign");
        assert_ne!(again.nonce, signed.nonce);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_signer_is_rejected() {
        let input = signed_input(SubmitKey::default());
        let err =
            resolve_signed(&input, "dt-no-ib-v0", &"ab".repeat(32), "beat").expect_err("unsigned");
        assert!(err.contains("base-proof-submit-v1"), "{err}");
    }

    #[test]
    fn queued_is_the_only_non_terminal_state() {
        assert!(!TERMINAL_STATES.contains(&"queued"));
        assert!(is_queued(&serde_json::json!({ "state": "queued" })));
        assert!(!is_queued(
            &serde_json::json!({ "state": "awaiting_admin" })
        ));
        assert!(!is_queued(&serde_json::json!({})));
    }

    #[test]
    fn two_signers_are_refused_before_any_key_is_read() {
        let input = signed_input(SubmitKey {
            secret_file: Some("/nonexistent/hotkey.sk".into()),
            wallet_name: Some("miner".into()),
            ..SubmitKey::default()
        });
        let err = resolve_signed(&input, "dt-no-ib-v0", &"ab".repeat(32), "beat")
            .expect_err("two signers");
        assert!(err.contains("one signer"), "{err}");
    }

    #[test]
    fn offline_signature_needs_the_signed_nonce() {
        let input = signed_input(SubmitKey {
            hotkey: Some("ab".repeat(32)),
            signature: Some("cd".repeat(64)),
            ..SubmitKey::default()
        });
        let err =
            resolve_signed(&input, "dt-no-ib-v0", &"ab".repeat(32), "beat").expect_err("no nonce");
        assert!(err.contains("submit-nonce"), "{err}");
        let mut with_bad_nonce = signed_input(SubmitKey {
            hotkey: Some("ab".repeat(32)),
            signature: Some("cd".repeat(64)),
            submit_nonce: Some("zz".repeat(32)),
            ..SubmitKey::default()
        });
        let err = resolve_signed(&with_bad_nonce, "dt-no-ib-v0", &"ab".repeat(32), "beat")
            .expect_err("bad nonce");
        assert!(err.contains("64 hex"), "{err}");
        with_bad_nonce.key.submit_nonce = Some("ab".repeat(32));
        let err = resolve_signed(&with_bad_nonce, "dt-no-ib-v0", &"ab".repeat(32), "beat")
            .expect_err("garbage signature");
        assert!(err.contains("hotkey_signature invalid"), "{err}");
    }

    #[test]
    fn topic_list_reads_the_items_wrapper() {
        let body = serde_json::json!({
            "items": [
                { "id": "dt-no-ib-v0", "status": "open", "payout_mode": "wta" },
                { "id": "muon-vs-adamw-10m-v0", "status": "open", "payout_mode": "wta" }
            ]
        });
        let items = topic_list_items(&body).expect("items");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["id"], "dt-no-ib-v0");
        assert_eq!(items[1]["id"], "muon-vs-adamw-10m-v0");
    }
}
