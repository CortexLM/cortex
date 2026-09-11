//! Proof challenge: submit a reproducible experiment against an open topic.

use std::path::PathBuf;
use std::time::Duration;

use keystore::{default_wallets_dir, load_hotkey, mini_secret_from_key_file, BittensorWallet};
use proof_submit::{
    canonical_hex, declared_manifest, fresh_submit_nonce_hex, hotkey_hex, is_lowercase_hex,
    manifest_declares_training, manifest_lists, parse_hotkey_hex, parse_signature_hex,
    parse_submit_nonce_hex, sha256_hex, sign_submit, verify_submit, SubmitFields,
    MAX_ARTEFACT_BYTES, PROOF_SUBMIT_DOMAIN_LABEL,
};
use serde_json::{json, Value};

use crate::catalog::{compact, find};
use ctx_client::{challenge_path, Client};

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
    /// Local uncompressed tar to upload (≤5 MiB). Digest is hashed from this
    /// file when `artifact_digest` is empty.
    pub artifact: Option<PathBuf>,
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
    /// Bring-your-own-key variables, already resolved to `(NAME, value)`.
    /// Posted as the body's `env`; not part of the signature.
    pub env: Vec<(String, String)>,
    /// Poll until the submission reaches a terminal state.
    pub wait: bool,
    /// sr25519 key or offline signature.
    pub key: SubmitKey,
}

/// POST a Proof submission and print the reply.
pub async fn submit(client: &Client, input: &SubmitInput, json_out: bool) -> Result<(), String> {
    let challenge = find("proof").ok_or_else(|| "proof is not a live challenge".to_owned())?;
    let (digest, artifact_bytes) = resolve_artifact(input)?;
    let topic_id = input.topic_id.trim();
    if topic_id.is_empty() {
        return Err("topic-id is required (ctx proof topics lists currently open ids)".into());
    }
    let claim = input.claim.trim();
    if claim.is_empty() {
        return Err("claim is required (what the recipe achieved)".into());
    }
    let Some(require_training) = fetch_topic_training_required(client, topic_id).await? else {
        return Err(format!(
            "unknown topic {topic_id:?} (ctx proof topics lists currently open ids)"
        ));
    };
    let signed = resolve_signed(input, topic_id, &digest, claim, require_training)?;
    let body = submit_wire_body(input, &signed, topic_id, &digest, claim);

    let reply = if let Some(bytes) = artifact_bytes {
        client
            .post_multipart(
                &challenge_path(challenge.id, "/v1/submissions"),
                &body,
                bytes,
            )
            .await?
    } else {
        client
            .post(&challenge_path(challenge.id, "/v1/submissions"), &body)
            .await?
    };
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
pub async fn print_signature(
    client: &Client,
    input: &SubmitInput,
    json_out: bool,
) -> Result<(), String> {
    let digest = normalize_hex64(&input.artifact_digest, "artifact-digest")?;
    let topic_id = input.topic_id.trim();
    if topic_id.is_empty() {
        return Err("topic-id is required".into());
    }
    let claim = input.claim.trim();
    if claim.is_empty() {
        return Err("claim is required".into());
    }
    let require_training = fetch_topic_training_required(client, topic_id)
        .await
        .ok()
        .flatten()
        .unwrap_or(false);
    let signed = resolve_signed(input, topic_id, &digest, claim, require_training)?;
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
    require_training: bool,
) -> Result<SignedSubmit, String> {
    let manifest = build_manifest(input, require_training)?;
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

/// The manifest to sign: `--manifest-file` verbatim when given, else the
/// `--train-hash` / `--train-dataset` lists.
///
/// The empty-manifest rule is checked once on the result, so a file and the
/// flags cannot drift apart. A topic that needs training evidence is refused
/// here, before a nonce is spent; one with no training step signs the empty
/// lists as they are.
fn build_manifest(input: &SubmitInput, require_training: bool) -> Result<Value, String> {
    let manifest = match &input.manifest_file {
        Some(path) => {
            let text = std::fs::read_to_string(path)
                .map_err(|e| format!("read {}: {e}", path.display()))?;
            serde_json::from_str(&text).map_err(|e| format!("manifest JSON: {e}"))?
        }
        None => declared_manifest(&input.train_hashes, &input.train_datasets),
    };
    if require_training && !manifest_declares_training(&manifest) {
        return Err(
            "contamination_evidence_missing: declare train-hash or train-dataset \
             (an empty manifest is not a clean check on this topic)"
                .into(),
        );
    }
    Ok(manifest)
}

/// Wire JSON for `POST /v1/submissions`. `env` is posted beside the
/// signature, never inside it.
fn submit_wire_body(
    input: &SubmitInput,
    signed: &SignedSubmit,
    topic_id: &str,
    digest: &str,
    claim: &str,
) -> Value {
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
    attach_miner_env(&mut body, &input.env);
    body
}

fn resolve_artifact(input: &SubmitInput) -> Result<(String, Option<Vec<u8>>), String> {
    let Some(path) = &input.artifact else {
        let digest = normalize_hex64(&input.artifact_digest, "artifact-digest")?;
        return Ok((digest, None));
    };
    let bytes = std::fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    if bytes.is_empty() {
        return Err("artifact is empty".into());
    }
    if bytes.len() > MAX_ARTEFACT_BYTES {
        return Err("artifact exceeds 5 MiB".into());
    }
    let got = sha256_hex(&bytes);
    let declared = input.artifact_digest.trim();
    if !declared.is_empty() {
        let want = normalize_hex64(declared, "artifact-digest")?;
        if want != got {
            return Err("artifact-digest does not match --artifact (sha256 of the file)".into());
        }
    }
    Ok((got, Some(bytes)))
}

fn attach_miner_env(body: &mut Value, env: &[(String, String)]) {
    if env.is_empty() {
        return;
    }
    body["env"] = Value::Object(
        env.iter()
            .map(|(k, v)| (k.clone(), Value::String(v.clone())))
            .collect(),
    );
}

/// Merge an explicit `--openrouter-api-key` into the submit `env` map as
/// `OPENROUTER_API_KEY`. Empty is ignored. A value already present via
/// `--env` wins. The process environment is not read here — clap no longer
/// binds `OPENROUTER_API_KEY`, so a leftover shell export cannot follow
/// `--gateway` to another host. The value is never written into an error
/// string.
#[must_use]
pub fn merge_openrouter_api_key(
    mut env: Vec<(String, String)>,
    openrouter_api_key: Option<String>,
) -> Vec<(String, String)> {
    let Some(raw) = openrouter_api_key else {
        return env;
    };
    let value = raw.trim();
    if value.is_empty() {
        return env;
    }
    if env.iter().any(|(n, _)| n == "OPENROUTER_API_KEY") {
        return env;
    }
    env.push(("OPENROUTER_API_KEY".to_owned(), value.to_owned()));
    env
}

fn topic_requires_training_evidence(topic: &Value) -> bool {
    let param = topic
        .pointer("/constraints/params/require_training_evidence")
        .and_then(Value::as_str)
        .map(str::trim);
    match param {
        Some(p) if p.eq_ignore_ascii_case("true") => true,
        Some(p) if p.eq_ignore_ascii_case("false") => false,
        _ => topic.pointer("/metric/family").and_then(Value::as_str) != Some("custom"),
    }
}

async fn fetch_topic_training_required(
    client: &Client,
    topic_id: &str,
) -> Result<Option<bool>, String> {
    let reply = client
        .get(&challenge_path(
            "proof",
            &format!("/v1/proof/topics/{topic_id}"),
        ))
        .await?;
    if reply.status == 404 {
        return Ok(None);
    }
    if !reply.ok() {
        return Err(explain_failure(reply.status, &reply.message()));
    }
    Ok(Some(topic_requires_training_evidence(&reply.body)))
}

/// Resolve `--env` arguments into the `(NAME, value)` pairs the submit body
/// carries.
///
/// `NAME=value` takes the value as written; a bare `NAME` reads it from this
/// process's environment, which is how a miner passes a key without putting
/// it in their shell history or in `ps`. A name that is not exported, or an
/// empty value, is an error here rather than a 400 from the host — and the
/// message never repeats the value.
pub fn parse_env_args(args: &[String]) -> Result<Vec<(String, String)>, String> {
    let mut out: Vec<(String, String)> = Vec::new();
    for arg in args {
        let (name, value) = if let Some((n, v)) = arg.split_once('=') {
            (n.trim().to_owned(), v.to_owned())
        } else {
            let name = arg.trim().to_owned();
            let value = std::env::var(&name).map_err(|_| {
                format!("--env {name}: no value given and {name} is not set in this shell")
            })?;
            (name, value)
        };
        if name.is_empty() {
            return Err(
                "--env needs a variable name (NAME=value, or NAME to read your shell)".into(),
            );
        }
        if value.trim().is_empty() {
            return Err(format!("--env {name}: the value is empty"));
        }
        if out.iter().any(|(n, _)| *n == name) {
            return Err(format!("--env {name} was given twice"));
        }
        out.push((name, value.trim().to_owned()));
    }
    Ok(out)
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
        400 if message.contains("env") => format!(
            "refused ({message}). Nothing was stored and nothing was rented. \
             A topic that sets constraints.params.miner_byok wants your own key in the \
             submit body: pass `--env <NAME>=<value>` (or bare `--env <NAME>` to read your \
             shell, or `--openrouter-api-key` for OPENROUTER_API_KEY). Your signed \
             submit_nonce is untouched, so you can re-post it."
        ),
        400 => format!("refused ({message}). Nothing was stored and nothing was rented."),
        401 => format!(
            "unauthorized ({message}). Proof submit requires a hotkey_signature over \
             {PROOF_SUBMIT_DOMAIN_LABEL} covering topic, artifact, declared_flops, claim, manifest \
             and a single-use submit_nonce (X-Lium-Api-Key is not identity)."
        ),
        503 => {
            let mut out = format!(
                "HTTP 503: {message}\n  The host cannot score right now (empty eval digest, \
                 missing/closed RLM judge backend, no open topics, or an unsealed baseline). \
                 Nothing was stored, nothing was rented."
            );
            if message.starts_with("upstream ") {
                out.push_str(
                    " The gateway may have hidden the challenge error body; \
                     upload with --artifact (≤5 MiB), or artifact_uri must be https:// \
                     to a tar whose sha256 is artifact_digest.",
                );
            }
            out
        }
        other => format!("HTTP {other}: {message}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_manifest_depends_on_whether_the_topic_requires_training() {
        let err = build_manifest(&SubmitInput::default(), true).expect_err("empty");
        assert!(err.contains("contamination_evidence_missing"), "{err}");
        let empty = build_manifest(&SubmitInput::default(), false).expect("agent topic");
        assert_eq!(empty["train_content_hashes"], json!([]));
        assert_eq!(empty["train_dataset_ids"], json!([]));
        let mut sk = [0x11u8; 32];
        sk[0] = 0x42;
        let dir = std::env::temp_dir().join(format!(
            "ctx-proof-empty-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("sk");
        std::fs::write(&path, hex::encode(sk)).expect("write sk");
        let input = SubmitInput {
            topic_id: "tbench".into(),
            artifact_digest: "ab".repeat(32),
            claim: "beat".into(),
            key: SubmitKey {
                secret_file: Some(path),
                wallet_hotkey: "default".into(),
                ..SubmitKey::default()
            },
            ..SubmitInput::default()
        };
        let signed = resolve_signed(&input, "tbench", &"ab".repeat(32), "beat", false)
            .expect("empty custom manifest signs");
        assert_eq!(signed.manifest["train_dataset_ids"], json!([]));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn topic_json_training_evidence_follows_family_and_param() {
        let harvest = json!({"metric": {"family": "throughput"}});
        assert!(topic_requires_training_evidence(&harvest));
        let nll = json!({"metric": {"family": "nll"}});
        assert!(topic_requires_training_evidence(&nll));
        let custom = json!({"metric": {"family": "custom"}});
        assert!(!topic_requires_training_evidence(&custom));
        let tight = json!({
            "metric": {"family": "custom"},
            "constraints": {"params": {"require_training_evidence": "true"}}
        });
        assert!(topic_requires_training_evidence(&tight));
        let skip = json!({
            "metric": {"family": "nll"},
            "constraints": {"params": {"require_training_evidence": "false"}}
        });
        assert!(!topic_requires_training_evidence(&skip));
    }

    #[test]
    fn claim_is_required_declared_flops_is_optional() {
        let input = SubmitInput {
            train_datasets: vec!["my-mix-v0".into()],
            claim: "beat baseline".into(),
            declared_flops: 0,
            ..SubmitInput::default()
        };
        let m = build_manifest(&input, true).expect("manifest");
        assert_eq!(m["train_dataset_ids"][0], "my-mix-v0");
        assert_eq!(input.claim, "beat baseline");
        assert_eq!(input.declared_flops, 0);
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
        let signed = resolve_signed(&input, "dt-no-ib-v0", &digest, "beat", true).expect("sign");
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
        let again = resolve_signed(&input, "dt-no-ib-v0", &digest, "beat", true).expect("sign");
        assert_ne!(again.nonce, signed.nonce);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_signer_is_rejected() {
        let input = signed_input(SubmitKey::default());
        let err = resolve_signed(&input, "dt-no-ib-v0", &"ab".repeat(32), "beat", true)
            .expect_err("unsigned");
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
        let err = resolve_signed(&input, "dt-no-ib-v0", &"ab".repeat(32), "beat", true)
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
        let err = resolve_signed(&input, "dt-no-ib-v0", &"ab".repeat(32), "beat", true)
            .expect_err("no nonce");
        assert!(err.contains("submit-nonce"), "{err}");
        let mut with_bad_nonce = signed_input(SubmitKey {
            hotkey: Some("ab".repeat(32)),
            signature: Some("cd".repeat(64)),
            submit_nonce: Some("zz".repeat(32)),
            ..SubmitKey::default()
        });
        let err = resolve_signed(
            &with_bad_nonce,
            "dt-no-ib-v0",
            &"ab".repeat(32),
            "beat",
            true,
        )
        .expect_err("bad nonce");
        assert!(err.contains("64 hex"), "{err}");
        with_bad_nonce.key.submit_nonce = Some("ab".repeat(32));
        let err = resolve_signed(
            &with_bad_nonce,
            "dt-no-ib-v0",
            &"ab".repeat(32),
            "beat",
            true,
        )
        .expect_err("garbage signature");
        assert!(err.contains("hotkey_signature invalid"), "{err}");
    }

    /// `--env` resolves a value from the flag or from the miner's shell, and
    /// nothing it refuses ever repeats the value back at them.
    #[test]
    fn env_args_take_a_value_or_read_the_shell() {
        assert!(parse_env_args(&[]).expect("none").is_empty());
        assert_eq!(
            parse_env_args(&["MINER_PROVIDED_API_KEY=sk-value".into()]).expect("inline"),
            vec![("MINER_PROVIDED_API_KEY".to_owned(), "sk-value".to_owned())]
        );
        // A value with '=' in it stays whole; only the first '=' splits.
        assert_eq!(
            parse_env_args(&["A=b=c".into()]).expect("first split"),
            vec![("A".to_owned(), "b=c".to_owned())]
        );
        let name = format!("CTX_ENV_TEST_{}", std::process::id());
        let err = parse_env_args(std::slice::from_ref(&name)).expect_err("not exported");
        assert!(
            err.contains(&name) && err.contains("not set in this shell"),
            "{err}"
        );
        // SAFETY-free in this crate: a single-threaded unit test setting its
        // own process env to prove the bare form reads it.
        std::env::set_var(&name, " sk-from-shell ");
        assert_eq!(
            parse_env_args(std::slice::from_ref(&name)).expect("from shell"),
            vec![(name.clone(), "sk-from-shell".to_owned())],
            "trimmed, so a trailing newline from a here-doc is not the key"
        );
        let err = parse_env_args(&[name.clone(), format!("{name}=other")]).expect_err("twice");
        assert!(err.contains("twice"), "{err}");
        std::env::remove_var(&name);
        assert!(parse_env_args(&["=value".into()])
            .expect_err("no name")
            .contains("needs a variable name"));
        for bad in ["A=", "A=   "] {
            let err = parse_env_args(&[bad.into()]).expect_err(bad);
            assert!(err.contains("the value is empty"), "{err}");
        }
        // A refusal names the variable and never repeats what was passed.
        let err = parse_env_args(&["A=  ".into()]).expect_err("blank");
        assert!(!err.contains("sk-"), "{err}");
    }

    /// The BYOK value goes in `env` on the wire and nowhere near the
    /// signature: the same key signs the same bytes with or without it.
    #[test]
    fn env_is_posted_but_never_signed() {
        let mut sk = [0x11u8; 32];
        sk[0] = 0x42;
        let input = SubmitInput {
            env: vec![("MINER_PROVIDED_API_KEY".into(), "sk-value".into())],
            ..signed_input(SubmitKey {
                hotkey: None,
                submit_nonce: Some("ab".repeat(32)),
                ..SubmitKey::default()
            })
        };
        let dir = std::env::temp_dir().join(format!("ctx-proof-env-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("sk");
        std::fs::write(&path, hex::encode(sk)).expect("write sk");
        let with_key = SubmitInput {
            key: SubmitKey {
                secret_file: Some(path),
                submit_nonce: Some("ab".repeat(32)),
                wallet_hotkey: "default".into(),
                ..SubmitKey::default()
            },
            ..input
        };
        let digest = "ab".repeat(32);
        let signed = resolve_signed(&with_key, "dt-no-ib-v0", &digest, "beat", true).expect("sign");
        // The signature verifies against the v1 payload, which has no room
        // for `env`: the BYOK value is posted beside the signature, not in it.
        let pk = parse_hotkey_hex(&signed.hotkey).expect("pk");
        let raw = parse_signature_hex(&signed.signature).expect("sig");
        let datasets = ["my-mix-v0".to_owned()];
        verify_submit(
            &pk,
            &SubmitFields {
                hotkey_hex: &signed.hotkey,
                topic_id: "dt-no-ib-v0",
                artifact_digest: &digest,
                declared_flops: 1,
                claim: "beat",
                train_content_hashes: &[],
                train_dataset_ids: &datasets,
                submit_nonce_hex: &signed.nonce,
            },
            &raw,
        )
        .expect("the v1 payload verifies with a BYOK value on the body");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A miner who forgot their key gets told how to pass it, and that their
    /// signed nonce survived.
    #[test]
    fn a_missing_byok_explains_the_env_flag() {
        let msg = explain_failure(400, "env.MINER_PROVIDED_API_KEY is required by this topic");
        assert!(msg.contains("--env"), "{msg}");
        assert!(msg.contains("--openrouter-api-key"), "{msg}");
        assert!(msg.contains("miner_byok"), "{msg}");
        assert!(msg.contains("submit_nonce is untouched"), "{msg}");
        let plain = explain_failure(400, "unknown topic");
        assert!(!plain.contains("--env"), "{plain}");
    }

    #[test]
    fn submit_body_omits_env_when_absent_and_includes_openrouter_when_set() {
        let signed = SignedSubmit {
            hotkey: "ab".repeat(32),
            signature: "cd".repeat(64),
            nonce: "ef".repeat(32),
            manifest: json!({"train_dataset_ids": ["my-mix-v0"]}),
        };
        let digest = "ab".repeat(32);
        let base = SubmitInput {
            topic_id: "tbench".into(),
            artifact_digest: digest.clone(),
            artifact_uri: Some("https://example.org/recipe.tar".into()),
            claim: "beat".into(),
            declared_flops: 1,
            ..SubmitInput::default()
        };
        let absent = submit_wire_body(&base, &signed, "tbench", &digest, "beat");
        assert!(absent.get("env").is_none(), "{absent}");
        assert_eq!(absent["artifact_uri"], "https://example.org/recipe.tar");
        assert_eq!(absent["topic_id"], "tbench");
        let present = submit_wire_body(
            &SubmitInput {
                env: vec![("OPENROUTER_API_KEY".into(), "sk-or-test".into())],
                ..base
            },
            &signed,
            "tbench",
            &digest,
            "beat",
        );
        assert_eq!(present["env"]["OPENROUTER_API_KEY"], "sk-or-test");
        assert!(present.get("hotkey_signature").is_some());
    }

    #[test]
    fn openrouter_flag_merges_into_env_without_echoing_the_key() {
        let merged = merge_openrouter_api_key(Vec::new(), Some("sk-or-test".into()));
        assert_eq!(
            merged,
            vec![("OPENROUTER_API_KEY".to_owned(), "sk-or-test".to_owned())]
        );
        assert!(merge_openrouter_api_key(Vec::new(), None).is_empty());
        // A leftover process export is not an implicit opt-in.
        std::env::set_var("OPENROUTER_API_KEY", "sk-or-must-not-attach");
        assert!(
            merge_openrouter_api_key(Vec::new(), None).is_empty(),
            "process OPENROUTER_API_KEY must not be forwarded without the flag"
        );
        std::env::remove_var("OPENROUTER_API_KEY");
        assert!(merge_openrouter_api_key(Vec::new(), Some("   ".into())).is_empty());
        let already = merge_openrouter_api_key(
            vec![("OPENROUTER_API_KEY".into(), "from-env-flag".into())],
            Some("from-dedicated".into()),
        );
        assert_eq!(already[0].1, "from-env-flag");
        let trimmed = merge_openrouter_api_key(Vec::new(), Some(" sk-or-pad ".into()));
        assert_eq!(trimmed[0].1, "sk-or-pad");
    }

    #[test]
    fn opaque_gateway_503_hints_stripped_body_and_https_artifact() {
        let msg = explain_failure(503, "upstream 503 Service Unavailable");
        assert!(msg.contains("hidden the challenge error body"), "{msg}");
        assert!(msg.contains("--artifact"), "{msg}");
        assert!(msg.contains("artifact_uri must be https://"), "{msg}");
        let real = explain_failure(
            503,
            "backend: runner backend: artifact_uri must be https://",
        );
        assert!(!real.contains("hidden the challenge error body"), "{real}");
        assert!(real.contains("artifact_uri must be https://"), "{real}");
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

    #[test]
    fn resolve_artifact_hashes_the_file_and_refuses_empty_or_mismatch() {
        let dir = std::env::temp_dir().join(format!("ctx-art-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("recipe.tar");
        std::fs::write(&path, b"recipe-tar-not-empty").expect("write");
        let got = resolve_artifact(&SubmitInput {
            artifact: Some(path.clone()),
            ..SubmitInput::default()
        })
        .expect("hash");
        assert_eq!(got.0, sha256_hex(b"recipe-tar-not-empty"));
        assert_eq!(got.1.as_deref(), Some(b"recipe-tar-not-empty".as_slice()));
        let err = resolve_artifact(&SubmitInput {
            artifact: Some(path),
            artifact_digest: "aa".repeat(32),
            ..SubmitInput::default()
        })
        .expect_err("mismatch");
        assert!(err.contains("does not match"), "{err}");
        let empty = dir.join("empty.tar");
        std::fs::write(&empty, b"").expect("empty");
        let err = resolve_artifact(&SubmitInput {
            artifact: Some(empty),
            ..SubmitInput::default()
        })
        .expect_err("empty");
        assert!(err.contains("empty"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
