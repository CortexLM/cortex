//! Bounty Challenge: pair a hotkey, then file reports.
//!
//! Signing happens locally. Nothing here asks for a mnemonic, and there is no
//! token for a miner to export: `ctx bounty pair` binds the hotkey through the
//! gateway and stores the returned session claim.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use bounty_challenge_task::{
    hotkey_ss58, pairing_code, parse_hotkey, parse_signature, public_from_mini_secret,
    sign_pair_challenge, validate_account_id, verify_pair_signature, PairChallenge,
    DEFAULT_PAIR_TTL_SECS, MAX_PENDING_REPORTS_PER_HOTKEY, MIN_REPORT_BODY_CHARS,
    MIN_REPORT_INTERVAL_SECS, MIN_REPRO_CHARS, TERMS_TEXT,
};
use keystore::{load_hotkey, mini_secret_from_key_file, BittensorWallet};
use serde_json::{json, Value};

use crate::api::{challenge_path, Client};
use crate::catalog::compact;

/// Challenge id for the bounty routes.
const BOUNTY: &str = "bounty";

/// Where the session claim from a successful pair is cached.
const SESSION_FILE: &str = "cortex/bounty-session.json";

/// How a miner proves the hotkey is theirs.
#[derive(Debug, Default)]
pub struct Signer {
    /// 32-byte mini-secret file (never a mnemonic).
    pub secret_file: Option<PathBuf>,
    /// Bittensor wallets directory.
    pub wallet_dir: Option<PathBuf>,
    /// Wallet name under the wallets directory.
    pub wallet_name: Option<String>,
    /// Hotkey file name inside that wallet.
    pub wallet_hotkey: String,
    /// 128-hex signature produced offline.
    pub signature: Option<String>,
}

/// Pair a hotkey to a dedicated Cortex Chat mining account.
pub async fn pair(
    client: &Client,
    hotkey_arg: &str,
    account_id: &str,
    signer: &Signer,
    accept_terms: bool,
    json_out: bool,
) -> Result<(), String> {
    validate_account_id(account_id).map_err(|e| format!("account-id: {e}"))?;
    let hotkey = parse_hotkey(hotkey_arg).map_err(|e| format!("hotkey: {e}"))?;
    let ss58 = hotkey_ss58(&hotkey);

    if !accept_terms {
        println!("Bounty pairing terms (blocking):");
        println!();
        println!("{TERMS_TEXT}");
        println!();
        println!("Pair a dedicated mining account, never a private personal account.");
        println!("Re-run the same command with --accept-terms to pair.");
        return Err("terms not accepted".into());
    }

    let now = unix_now();
    let challenge = PairChallenge {
        account_id: account_id.to_owned(),
        nonce: random_nonce()?,
        exp: now.saturating_add(DEFAULT_PAIR_TTL_SECS),
    };
    let encoded = challenge.encode().map_err(|e| e.to_string())?;

    let Some(signature) = sign(signer, &hotkey, &encoded)? else {
        println!("Sign this string with that hotkey (sr25519, substrate context):");
        println!();
        println!("{encoded}");
        println!();
        println!("Then re-run the same command with --accept-terms and");
        println!("--signature followed by the 128-hex signature.");
        return Ok(());
    };
    verify_pair_signature(&hotkey, &encoded, &signature)
        .map_err(|e| format!("local signature check failed: {e}"))?;

    let body = json!({
        "account_id": challenge.account_id,
        "hotkey": ss58,
        "nonce": challenge.nonce,
        "exp": challenge.exp,
        "signature": hex::encode(signature),
        "terms_accepted": true,
    });
    let reply = client
        .post(&challenge_path(BOUNTY, "/v1/pair"), &body)
        .await?;
    if json_out {
        println!("{}", reply.body);
    }
    if !reply.ok() {
        return Err(explain(reply.status, &reply.message()));
    }
    let session = reply
        .body
        .get("session")
        .and_then(Value::as_str)
        .ok_or("pair reply carried no session claim")?
        .to_owned();
    let stored = store_session(client.gateway(), account_id, &ss58, &reply.body)?;

    if json_out {
        return Ok(());
    }
    println!("Paired hotkey {ss58} to Chat account {account_id}.");
    if let Some(id) = reply.body.get("session_id") {
        println!("session_id: {}", compact(id));
    }
    println!("Session claim cached in {} (mode 0600).", stored.display());
    println!();
    if let Some(activation) = reply.body.get("chat_activation").map(compact) {
        println!("Cortex Chat activation: {activation}");
    } else {
        println!("Cortex Chat activation: open the dedicated mining account in Cortex");
        println!("Chat and paste the pairing code below. Chat confirms the binding and");
        println!("marks the session as bounty-miner. There is nothing for you to export.");
        println!();
        println!("pairing code:");
        println!("{}", pairing_code(&encoded, &hex::encode(signature), &ss58));
    }
    println!();
    println!("File a report with:");
    println!("  ctx bounty report --title \"...\" --body-file report.md --repro-file repro.md");
    println!();
    println!("The session claim expires. Re-run pair when a report answers 401.");
    let _ = session;
    Ok(())
}

/// File a bug report against the paired hotkey.
pub async fn report(
    client: &Client,
    title: &str,
    body_text: &str,
    repro: &str,
    session: Option<&str>,
    json_out: bool,
) -> Result<(), String> {
    let session = match session {
        Some(s) => s.to_owned(),
        None => load_session(client.gateway())?,
    };
    check_report_shape(title, body_text, repro)?;
    let payload = json!({
        "session": session,
        "title": title.trim(),
        "body": body_text.trim(),
        "repro_steps": repro.trim(),
    });
    let reply = client
        .post(&challenge_path(BOUNTY, "/v1/reports"), &payload)
        .await?;
    if json_out {
        println!("{}", reply.body);
    }
    if !reply.ok() {
        return Err(explain(reply.status, &reply.message()));
    }
    if json_out {
        return Ok(());
    }
    println!("Report filed.");
    for field in ["id", "state", "miner_hotkey", "fingerprint"] {
        if let Some(v) = reply.body.get(field) {
            println!("  {field}: {}", compact(v));
        }
    }
    println!();
    println!("Pay is precision times severity: an operator adjudication with a severity");
    println!("is what earns weight, and duplicates or re-files of already-fixed issues");
    println!("count against a ratio that never shows up in your visible score.");
    Ok(())
}

/// Report bodies stay on the operator host. POST already returned id/state.
pub fn show(id: &str, json_out: bool) -> Result<(), String> {
    let msg = report_read_operator_local(id);
    if json_out {
        println!(
            "{}",
            json!({
                "error": "report_reads_operator_local",
                "id": id,
                "message": msg,
            })
        );
    }
    Err(msg)
}

fn report_read_operator_local(id: &str) -> String {
    format!(
        "report {id} is operator-local: GET /v1/reports is bearer-gated on the \
         challenge host and blocked on the public gateway (POST submit stays open). \
         The file reply already carried id, state, and fingerprint. Public scoring \
         is the CortexLM/backend feed, not this ingest list."
    )
}

/// Print the live bounty quotas and whether this host can score.
pub async fn status(client: &Client, json_out: bool) -> Result<(), String> {
    let reply = client.get(&challenge_path(BOUNTY, "/v1/status")).await?;
    if json_out {
        println!("{}", reply.body);
        return Ok(());
    }
    if !reply.ok() {
        return Err(explain(reply.status, &reply.message()));
    }
    let can_score = reply
        .body
        .get("can_score")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    println!("gateway: {}", client.gateway());
    println!("can_score: {can_score}");
    if let Some(v) = reply.body.get("scoring_backend") {
        println!("scoring_backend: {}", compact(v));
    }
    if let Some(v) = reply.body.get("champion_hotkey") {
        println!("champion_hotkey: {}", compact(v));
    }
    if let Some(q) = reply.body.get("quotas") {
        println!("quotas: {q}");
    } else {
        println!(
            "quotas (client defaults): {MAX_PENDING_REPORTS_PER_HOTKEY} pending, \
             {MIN_REPORT_INTERVAL_SECS}s between reports"
        );
    }
    if !can_score {
        println!();
        println!("The adjudication feed is the only scorer. While it is unreadable this");
        println!("host answers 503 on every report and pays nobody for that epoch — the");
        println!("2000 bps burns instead of being paid on numbers nobody adjudicated.");
    }
    Ok(())
}

fn sign(signer: &Signer, hotkey: &[u8; 32], encoded: &str) -> Result<Option<[u8; 64]>, String> {
    if let Some(hex_sig) = &signer.signature {
        return parse_signature(hex_sig)
            .map(Some)
            .map_err(|e| e.to_string());
    }
    if let Some(path) = &signer.secret_file {
        let sk = mini_secret_from_key_file(path).map_err(|e| e.to_string())?;
        let pk = public_from_mini_secret(&sk).map_err(|e| e.to_string())?;
        if &pk != hotkey {
            return Err("secret file holds a different hotkey than --hotkey".into());
        }
        return sign_pair_challenge(&sk, encoded)
            .map(Some)
            .map_err(|e| e.to_string());
    }
    if let Some(name) = &signer.wallet_name {
        let dir = signer
            .wallet_dir
            .clone()
            .unwrap_or_else(keystore::default_wallets_dir);
        let hotkey_name = if signer.wallet_hotkey.is_empty() {
            "default"
        } else {
            &signer.wallet_hotkey
        };
        let wallet = BittensorWallet::new(name, hotkey_name);
        let kp = load_hotkey(&dir, wallet.wallet_name(), wallet.hotkey_name())
            .map_err(|e| e.to_string())?;
        if kp.public_key() != hotkey {
            return Err(format!(
                "wallet hotkey {} is not the hotkey you passed; switch with --wallet-hotkey",
                kp.ss58_address()
            ));
        }
        return sign_pair_challenge(kp.expose_mini_secret(), encoded)
            .map(Some)
            .map_err(|e| e.to_string());
    }
    Ok(None)
}

/// Refuse locally what the service refuses anyway, so a thin report does not
/// burn a rate-limit window.
fn check_report_shape(title: &str, body: &str, repro: &str) -> Result<(), String> {
    if title.trim().is_empty() {
        return Err("--title is required".into());
    }
    if body.trim().chars().count() < MIN_REPORT_BODY_CHARS {
        return Err(format!(
            "report body must be at least {MIN_REPORT_BODY_CHARS} characters: say what broke"
        ));
    }
    if repro.trim().chars().count() < MIN_REPRO_CHARS {
        return Err(format!(
            "repro steps must be at least {MIN_REPRO_CHARS} characters: say how to reproduce it"
        ));
    }
    if title.trim() == body.trim() {
        return Err("title and body must differ".into());
    }
    Ok(())
}

fn session_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join(SESSION_FILE)
}

fn store_session(
    gateway: &str,
    account_id: &str,
    ss58: &str,
    reply: &Value,
) -> Result<PathBuf, String> {
    let path = session_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
    }
    let record = json!({
        "gateway": gateway,
        "account_id": account_id,
        "miner_hotkey": ss58,
        "session": reply.get("session"),
        "session_id": reply.get("session_id"),
    });
    std::fs::write(&path, format!("{record}\n")).map_err(|e| format!("write session: {e}"))?;
    restrict(&path)?;
    Ok(path)
}

fn load_session(gateway: &str) -> Result<String, String> {
    let path = session_path();
    let mut raw = String::new();
    std::fs::File::open(&path)
        .and_then(|mut f| f.read_to_string(&mut raw))
        .map_err(|e| {
            format!(
                "no cached pairing session ({}: {e}). Run 'ctx bounty pair' first, or pass \
                 --session.",
                path.display()
            )
        })?;
    let record: Value = serde_json::from_str(&raw)
        .map_err(|e| format!("{} is not valid JSON: {e}", path.display()))?;
    if record.get("gateway").and_then(Value::as_str) != Some(gateway) {
        return Err(format!(
            "cached session was paired against {}, not {gateway}. Pair again for this gateway.",
            record
                .get("gateway")
                .map_or_else(|| "an unknown gateway".to_owned(), compact)
        ));
    }
    record
        .get("session")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| format!("{} carries no session claim", path.display()))
}

#[cfg(unix)]
fn restrict(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| format!("chmod 0600 {}: {e}", path.display()))
}

#[cfg(not(unix))]
fn restrict(_path: &Path) -> Result<(), String> {
    Ok(())
}

fn explain(status: u16, message: &str) -> String {
    let hint = match status {
        400 => "the report was too thin or malformed. A real title, an 80-character body, and repro steps clear this.",
        401 => "the session claim is missing, wrong, or expired. Run 'ctx bounty pair --accept-terms' again.",
        403 => "pairing terms were not accepted. Pairing is blocking: pass --accept-terms.",
        404 => "no such report id.",
        429 => "quota: too many reports awaiting adjudication, or too soon after the last one. Nothing is recorded against you, but nothing is filed either.",
        503 => "the adjudication feed is unreadable, so this host cannot turn a report into weight and refuses rather than banking unpaid work. Nothing was stored. Check 'ctx bounty status'.",
        _ => "unexpected reply from the bounty service.",
    };
    format!("HTTP {status}: {message}\n  {hint}")
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn random_nonce() -> Result<String, String> {
    let mut buf = [0u8; 16];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut buf))
        .map_err(|e| format!("read randomness: {e}"))?;
    Ok(hex::encode(buf))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_thin_report_is_refused_before_it_burns_a_quota_window() {
        let body = "x".repeat(MIN_REPORT_BODY_CHARS);
        let repro = "y".repeat(MIN_REPRO_CHARS);
        check_report_shape("title", &body, &repro).expect("shape ok");
        assert!(check_report_shape("", &body, &repro).is_err());
        assert!(check_report_shape("title", "too short", &repro).is_err());
        assert!(check_report_shape("title", &body, "short").is_err());
    }

    #[test]
    fn title_pasted_as_body_is_refused() {
        let long = "z".repeat(MIN_REPORT_BODY_CHARS);
        let repro = "y".repeat(MIN_REPRO_CHARS);
        assert!(check_report_shape(&long, &long, &repro).is_err());
    }

    #[test]
    fn nonce_is_hex_and_long_enough() {
        let n = random_nonce().expect("nonce");
        assert_eq!(n.len(), 32);
        assert!(n.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn session_path_lands_under_a_config_dir() {
        let p = session_path();
        assert!(p.ends_with("cortex/bounty-session.json"), "{p:?}");
    }

    #[test]
    fn explanations_name_the_action_not_an_env_var() {
        let e401 = explain(401, "invalid_session");
        assert!(e401.contains("ctx bounty pair"), "{e401}");
        let e503 = explain(503, "scoring unconfigured");
        assert!(e503.contains("Nothing was stored"), "{e503}");
        assert!(!e503.contains("BOUNTY_"), "{e503}");
    }

    #[test]
    fn show_does_not_read_reports_over_the_public_gateway() {
        let err = show("by_1", false).expect_err("operator-local");
        assert!(err.contains("operator-local"), "{err}");
        assert!(err.contains("by_1"), "{err}");
        assert!(err.contains("POST submit stays open"), "{err}");
        assert!(!err.contains("BOUNTY_"), "{err}");
    }

    #[test]
    fn an_unsigned_pair_prints_the_challenge_instead_of_signing() {
        let signer = Signer::default();
        let out = sign(&signer, &[7u8; 32], "cortex-bounty-v1|a|0123456789abcdef|9").expect("sign");
        assert!(out.is_none());
    }
}
