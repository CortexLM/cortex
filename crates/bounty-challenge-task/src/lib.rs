//! Bounty challenge identity and miner hotkey pairing.
//!
//! ```text
//! challenge_id     = "bounty"
//! scoring_version  = 1
//! pair prefix      = cortex-bounty-v1|{account_id}|{nonce}|{exp}
//! ```
//!
//! Pairing is signed with the miner's Bittensor hotkey (sr25519, Substrate
//! context). Chat never asks for a mnemonic. The unguessable Chat inject
//! command is env-only (`BOUNTY_CHAT_COMMAND`).

#![forbid(unsafe_code)]
#![allow(clippy::doc_markdown, clippy::missing_errors_doc)]

use keystore::{ss58_decode, ss58_encode, BITTENSOR_SS58_PREFIX, KEY_LEN};
use schnorrkel::{signing_context, ExpansionMode, MiniSecretKey, PublicKey, Signature};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Normative challenge id (trust-root / leaf `challenge_id` string).
pub const CHALLENGE_ID: &str = "bounty";

/// UTF-8 bytes of [`CHALLENGE_ID`].
pub const CHALLENGE_ID_BYTES: &[u8] = b"bounty";

/// Live `challenge_scoring_version` (precision displacement vs champion).
pub const SCORING_VERSION: u16 = 1;

/// Integer score lattice max (same scale as other challenges).
pub const SCORE_MAX: u64 = 1_000_000;

/// Domain tag for pairing session claims (control-plane HMAC, not miner sigs).
pub const SESSION_DOMAIN: &[u8] = b"base-bounty-session-v1";

/// Domain tag for report fingerprints.
pub const REPORT_DOMAIN: &[u8] = b"base-bounty-report-v1";

/// Pairing challenge prefix. Wire format: `{PREFIX}|{account_id}|{nonce}|{exp}`.
pub const PAIR_PREFIX: &str = "cortex-bounty-v1";

/// Default pairing expiry, seconds from now.
pub const DEFAULT_PAIR_TTL_SECS: u64 = 15 * 60;

/// Terms miners must accept before pairing (blocking).
pub const TERMS_TEXT: &str = "By pairing a Bittensor hotkey to a Cortex Chat account \
for Bounty Challenge, you accept that this dedicated mining account, its logs, \
and its conversations may be used for research, to fix product and backend bugs, \
and to remunerate (or penalize) the bound miner hotkey. Do not pair a private \
personal account.";

/// Pairing / signature errors. Never embed secrets or mnemonics.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum PairError {
    /// Account id empty or illegal.
    #[error("invalid account_id")]
    InvalidAccount,
    /// Nonce empty or illegal.
    #[error("invalid nonce")]
    InvalidNonce,
    /// Expiry not a unix-seconds integer, or already elapsed.
    #[error("invalid or expired pairing window")]
    InvalidExp,
    /// Challenge string does not match the canonical form.
    #[error("malformed pairing challenge")]
    MalformedChallenge,
    /// Hotkey is not 64-hex or a valid SS58 address.
    #[error("invalid hotkey")]
    InvalidHotkey,
    /// Signature bytes were malformed.
    #[error("invalid signature")]
    InvalidSignature,
    /// Signature does not match the hotkey over the challenge string.
    #[error("signature verification failed")]
    VerificationFailed,
    /// Mini-secret rejected by schnorrkel.
    #[error("invalid secret")]
    InvalidSecret,
}

/// Canonical pairing challenge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairChallenge {
    /// Cortex Chat account id (dedicated mining account).
    pub account_id: String,
    /// One-time hex nonce.
    pub nonce: String,
    /// Unix-seconds expiry.
    pub exp: u64,
}

impl PairChallenge {
    /// Build `cortex-bounty-v1|{account_id}|{nonce}|{exp}`.
    pub fn encode(&self) -> Result<String, PairError> {
        validate_account_id(&self.account_id)?;
        validate_nonce(&self.nonce)?;
        if self.exp == 0 {
            return Err(PairError::InvalidExp);
        }
        Ok(format!(
            "{PAIR_PREFIX}|{}|{}|{}",
            self.account_id, self.nonce, self.exp
        ))
    }

    /// Parse a canonical challenge string.
    pub fn parse(raw: &str) -> Result<Self, PairError> {
        let mut parts = raw.splitn(4, '|');
        let prefix = parts.next().ok_or(PairError::MalformedChallenge)?;
        let account_id = parts.next().ok_or(PairError::MalformedChallenge)?;
        let nonce = parts.next().ok_or(PairError::MalformedChallenge)?;
        let exp_s = parts.next().ok_or(PairError::MalformedChallenge)?;
        if prefix != PAIR_PREFIX {
            return Err(PairError::MalformedChallenge);
        }
        validate_account_id(account_id)?;
        validate_nonce(nonce)?;
        let exp: u64 = exp_s.parse().map_err(|_| PairError::InvalidExp)?;
        if exp == 0 {
            return Err(PairError::InvalidExp);
        }
        Ok(Self {
            account_id: account_id.to_owned(),
            nonce: nonce.to_owned(),
            exp,
        })
    }

    /// Reject when `now_unix >= exp`.
    pub fn ensure_fresh(&self, now_unix: u64) -> Result<(), PairError> {
        if now_unix >= self.exp {
            return Err(PairError::InvalidExp);
        }
        Ok(())
    }
}

/// `BOUNTY_CHAT_COMMAND` when set. Empty / missing → `None` (docs print a placeholder).
#[must_use]
pub fn chat_command_from_env() -> Option<String> {
    std::env::var("BOUNTY_CHAT_COMMAND")
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

/// Placeholder shown when the inject command is not configured. Never a live token.
pub const CHAT_COMMAND_PLACEHOLDER: &str = "<BOUNTY_CHAT_COMMAND>";

/// Display the inject command or the public placeholder.
#[must_use]
pub fn chat_command_display() -> String {
    chat_command_from_env().unwrap_or_else(|| CHAT_COMMAND_PLACEHOLDER.to_owned())
}

/// `BOUNTY_BACKEND_PUBLIC_URL` when set. Empty / missing → `None`.
///
/// Cortex **reads** the Chat backend public feed. It does not serve one.
/// Never bake a host into git.
#[must_use]
pub fn backend_public_url() -> Option<String> {
    std::env::var("BOUNTY_BACKEND_PUBLIC_URL")
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

/// Retired opt-in for the offline scorer, kept only so a host that still sets
/// it can be told the knob is dead.
///
/// There is no local scorer any more. Adjudication happens in
/// CortexLM/backend, so a host with no feed has nothing to score, and an
/// offline stand-in would pay miners on numbers no validator can verify.
#[must_use]
pub fn legacy_sim_opt_in_present() -> bool {
    matches!(
        std::env::var("BOUNTY_FORCE_SIM")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "yes"
    )
}

/// Where this host's bounty scores come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScoringBackend {
    /// Adjudications published by CortexLM/backend.
    BackendPublic,
    /// No feed: this host cannot produce weight.
    Unconfigured,
}

/// Resolve the scoring backend for this host.
///
/// The backend public feed is the only scorer. `BOUNTY_FORCE_SIM` cannot
/// change this answer: an unset (or blank) `BOUNTY_BACKEND_PUBLIC_URL` is
/// [`ScoringBackend::Unconfigured`], which 503s ingest and emits no leaf.
#[must_use]
pub fn resolve_scoring_backend() -> ScoringBackend {
    if backend_public_url().is_some() {
        ScoringBackend::BackendPublic
    } else {
        ScoringBackend::Unconfigured
    }
}

/// Most reports one hotkey may leave awaiting adjudication.
///
/// Adjudication is the scarce resource in this challenge: every pending report
/// costs a human or an agent a triage pass. Without a cap one miner can flood
/// the queue and starve everyone else's reports of attention, which is a
/// denial of service on the incentive rather than on the service.
pub const MAX_PENDING_REPORTS_PER_HOTKEY: usize = 5;

/// Shortest interval between two reports from one hotkey.
pub const MIN_REPORT_INTERVAL_SECS: u64 = 60;

/// Shortest body a report may have.
///
/// Not a quality bar — an operator still adjudicates. It exists so a one-word
/// submission cannot occupy a queue slot.
pub const MIN_REPORT_BODY_CHARS: usize = 80;

/// Shortest reproduction section a report may have.
pub const MIN_REPRO_CHARS: usize = 20;

/// Distinct body tokens (length ≥ 3) a report must carry.
///
/// Stops a repeated-character or one-word farm from clearing the character
/// floor. An operator still decides whether the bug is real.
pub const MIN_UNIQUE_BODY_TOKENS: usize = 4;

// Quotas that do not bind are quotas that do not protect triage, and a quota
// wide enough to be harmless is the kind of constant that drifts wider.
const _: () = assert!(MAX_PENDING_REPORTS_PER_HOTKEY > 0 && MAX_PENDING_REPORTS_PER_HOTKEY <= 10);
const _: () = assert!(MIN_REPORT_INTERVAL_SECS >= 30);
const _: () = assert!(MIN_REPORT_BODY_CHARS >= 40 && MIN_REPRO_CHARS >= 10);
const _: () = assert!(MIN_UNIQUE_BODY_TOKENS >= 3);

/// Accept `[A-Za-z0-9._:-]` up to 128 chars.
pub fn validate_account_id(id: &str) -> Result<(), PairError> {
    if id.is_empty() || id.len() > 128 {
        return Err(PairError::InvalidAccount);
    }
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '-'))
    {
        return Err(PairError::InvalidAccount);
    }
    Ok(())
}

/// Hex nonce, 16..=64 chars.
pub fn validate_nonce(nonce: &str) -> Result<(), PairError> {
    if nonce.len() < 16 || nonce.len() > 64 || !nonce.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(PairError::InvalidNonce);
    }
    Ok(())
}

/// Parse SS58 or 64-hex into a 32-byte hotkey.
pub fn parse_hotkey(raw: &str) -> Result<[u8; KEY_LEN], PairError> {
    let t = raw.trim();
    if t.is_empty() {
        return Err(PairError::InvalidHotkey);
    }
    if let Ok((bytes, _)) = ss58_decode(t) {
        return Ok(bytes);
    }
    let hex_s = t.trim_start_matches("0x");
    if hex_s.len() != 64 || !hex_s.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(PairError::InvalidHotkey);
    }
    let bytes = hex::decode(hex_s).map_err(|_| PairError::InvalidHotkey)?;
    <[u8; KEY_LEN]>::try_from(bytes).map_err(|_| PairError::InvalidHotkey)
}

/// Encode a hotkey as Bittensor SS58 (prefix 42).
#[must_use]
pub fn hotkey_ss58(hotkey: &[u8; KEY_LEN]) -> String {
    ss58_encode(hotkey, BITTENSOR_SS58_PREFIX)
}

/// Encode a hotkey as lowercase 64-hex.
#[must_use]
pub fn hotkey_hex(hotkey: &[u8; KEY_LEN]) -> String {
    hex::encode(hotkey)
}

/// Derive the 32-byte public key from a mini-secret.
pub fn public_from_mini_secret(secret: &[u8; KEY_LEN]) -> Result<[u8; KEY_LEN], PairError> {
    let mini = MiniSecretKey::from_bytes(secret).map_err(|_| PairError::InvalidSecret)?;
    Ok(mini.expand(ExpansionMode::Ed25519).to_public().to_bytes())
}

/// Sign the challenge string with a 32-byte mini-secret (Substrate sr25519).
pub fn sign_pair_challenge(secret: &[u8; KEY_LEN], challenge: &str) -> Result<[u8; 64], PairError> {
    let mini = MiniSecretKey::from_bytes(secret).map_err(|_| PairError::InvalidSecret)?;
    let keypair = mini.expand(ExpansionMode::Ed25519).to_keypair();
    let ctx = signing_context(b"substrate");
    Ok(keypair.sign(ctx.bytes(challenge.as_bytes())).to_bytes())
}

/// Verify a Substrate-context sr25519 signature over the challenge string.
pub fn verify_pair_signature(
    public: &[u8; KEY_LEN],
    challenge: &str,
    signature: &[u8],
) -> Result<(), PairError> {
    if signature.len() != 64 {
        return Err(PairError::InvalidSignature);
    }
    let pk = PublicKey::from_bytes(public).map_err(|_| PairError::InvalidHotkey)?;
    let sig = Signature::from_bytes(signature).map_err(|_| PairError::InvalidSignature)?;
    let ctx = signing_context(b"substrate");
    pk.verify(ctx.bytes(challenge.as_bytes()), &sig)
        .map_err(|_| PairError::VerificationFailed)
}

/// Decode a 128-hex or raw-64 signature.
pub fn parse_signature(raw: &str) -> Result<[u8; 64], PairError> {
    let t = raw.trim().trim_start_matches("0x");
    if t.len() != 128 || !t.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(PairError::InvalidSignature);
    }
    let bytes = hex::decode(t).map_err(|_| PairError::InvalidSignature)?;
    <[u8; 64]>::try_from(bytes).map_err(|_| PairError::InvalidSignature)
}

/// One-time pairing code pasted into Cortex Chat (not a mnemonic).
#[must_use]
pub fn pairing_code(challenge: &str, signature_hex: &str, hotkey: &str) -> String {
    format!("{challenge}|{signature_hex}|{hotkey}")
}

/// Parse [`pairing_code`] back into challenge + signature + hotkey.
pub fn parse_pairing_code(
    code: &str,
) -> Result<(PairChallenge, [u8; 64], [u8; KEY_LEN]), PairError> {
    let mut parts = code.rsplitn(3, '|');
    let hotkey_s = parts.next().ok_or(PairError::MalformedChallenge)?;
    let sig_s = parts.next().ok_or(PairError::MalformedChallenge)?;
    let challenge_s = parts.next().ok_or(PairError::MalformedChallenge)?;
    let challenge = PairChallenge::parse(challenge_s)?;
    let signature = parse_signature(sig_s)?;
    let hotkey = parse_hotkey(hotkey_s)?;
    Ok((challenge, signature, hotkey))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_secret() -> [u8; KEY_LEN] {
        let mut s = [0x11u8; KEY_LEN];
        s[0] = 0x42;
        s
    }

    fn dummy_public() -> [u8; KEY_LEN] {
        let mini = MiniSecretKey::from_bytes(&dummy_secret()).expect("mini");
        mini.expand(ExpansionMode::Ed25519).to_public().to_bytes()
    }

    #[test]
    fn challenge_id_is_bounty() {
        assert_eq!(CHALLENGE_ID, "bounty");
        assert_eq!(CHALLENGE_ID_BYTES, b"bounty");
        assert_ne!(CHALLENGE_ID, "relearn");
    }

    #[test]
    fn pair_round_trip() {
        let c = PairChallenge {
            account_id: "acct-miner-1".into(),
            nonce: "aabbccddeeff0011".into(),
            exp: 1_900_000_000,
        };
        let s = c.encode().expect("encode");
        assert!(s.starts_with("cortex-bounty-v1|"));
        assert_eq!(PairChallenge::parse(&s).expect("parse"), c);
    }

    #[test]
    fn dummy_sr25519_pair_verifies() {
        let pk = dummy_public();
        let c = PairChallenge {
            account_id: "acct-test".into(),
            nonce: "0123456789abcdef".into(),
            exp: 2_000_000_000,
        };
        let challenge = c.encode().expect("encode");
        let sig = sign_pair_challenge(&dummy_secret(), &challenge).expect("sign");
        verify_pair_signature(&pk, &challenge, &sig).expect("verify");
        assert!(
            verify_pair_signature(&pk, "cortex-bounty-v1|other|0123456789abcdef|1", &sig).is_err()
        );
    }

    #[test]
    fn pairing_code_round_trip() {
        let pk = dummy_public();
        let ss58 = hotkey_ss58(&pk);
        let c = PairChallenge {
            account_id: "acct-2".into(),
            nonce: "deadbeefdeadbeef".into(),
            exp: 2_100_000_000,
        };
        let challenge = c.encode().expect("encode");
        let sig = sign_pair_challenge(&dummy_secret(), &challenge).expect("sign");
        let code = pairing_code(&challenge, &hex::encode(sig), &ss58);
        let (parsed, sig2, hk) = parse_pairing_code(&code).expect("parse code");
        assert_eq!(parsed, c);
        assert_eq!(sig2, sig);
        assert_eq!(hk, pk);
        verify_pair_signature(&hk, &challenge, &sig2).expect("verify");
    }

    #[test]
    fn expired_challenge_rejected() {
        let c = PairChallenge {
            account_id: "acct".into(),
            nonce: "0123456789abcdef".into(),
            exp: 10,
        };
        assert_eq!(c.ensure_fresh(10), Err(PairError::InvalidExp));
        assert!(c.ensure_fresh(9).is_ok());
    }

    #[test]
    fn chat_command_env_only() {
        assert!(chat_command_from_env().is_none());
        assert_eq!(chat_command_display(), CHAT_COMMAND_PLACEHOLDER);
        assert!(!CHAT_COMMAND_PLACEHOLDER.contains("/miner"));
    }

    #[test]
    fn backend_public_url_env_only() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(backend_public_url().is_none());
    }

    /// With no feed this host cannot turn a report into weight, and must say
    /// so rather than quietly collecting unpaid work.
    #[test]
    fn scoring_is_unconfigured_until_a_feed_is_configured() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(!legacy_sim_opt_in_present());
        assert_eq!(resolve_scoring_backend(), ScoringBackend::Unconfigured);
    }

    /// The old opt-in was an escape hatch: it let a host with no feed accept
    /// bug reports and mint weight nobody adjudicated. Setting it now changes
    /// nothing about what this host can score.
    #[test]
    fn the_retired_sim_opt_in_cannot_turn_scoring_back_on() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for value in ["1", "true", "yes"] {
            std::env::set_var("BOUNTY_FORCE_SIM", value);
            assert!(legacy_sim_opt_in_present());
            assert_eq!(
                resolve_scoring_backend(),
                ScoringBackend::Unconfigured,
                "BOUNTY_FORCE_SIM={value} must not resolve a scorer"
            );
        }
        std::env::remove_var("BOUNTY_FORCE_SIM");
    }

    #[test]
    fn a_configured_feed_is_the_only_scorer() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        std::env::set_var("BOUNTY_FORCE_SIM", "1");
        std::env::set_var("BOUNTY_BACKEND_PUBLIC_URL", "http://127.0.0.1:9");
        assert_eq!(resolve_scoring_backend(), ScoringBackend::BackendPublic);
        std::env::remove_var("BOUNTY_BACKEND_PUBLIC_URL");
        std::env::remove_var("BOUNTY_FORCE_SIM");
    }

    /// Process env is shared across threads in one test binary.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn terms_require_dedicated_account() {
        assert!(TERMS_TEXT.contains("dedicated mining account"));
        assert!(TERMS_TEXT.contains("remunerate"));
    }
}
