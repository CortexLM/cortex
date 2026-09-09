//! Miner Proof submit signatures.
//!
//! ```text
//! domain  = b"base-proof-submit-v1"
//! payload = hotkey_hex || 0xff || topic_id || 0xff
//!           || artifact_digest || 0xff || declared_flops_decimal || 0xff || claim
//! ```
//!
//! Distinct from `base-proof-topic-v1` (operator topic documents) and from
//! consensus tags (`base-bundle-v1`, `base-rawweight-v1`, …). A captured
//! signature cannot be replayed with a different claim or FLOP declaration
//! on the same artifact.

#![forbid(unsafe_code)]
#![allow(
    clippy::doc_markdown,
    clippy::missing_errors_doc,
    clippy::must_use_candidate
)]

use thiserror::Error;

/// Signature domain for miner Proof submits (`POST /v1/submissions`).
pub const PROOF_SUBMIT_DOMAIN: crypto::DomainTag = crypto::DomainTag::new(b"base-proof-submit-v1");

/// ASCII label of [`PROOF_SUBMIT_DOMAIN`].
pub const PROOF_SUBMIT_DOMAIN_LABEL: &str = "base-proof-submit-v1";

/// Why a submit signature cannot be built or checked.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SubmitSigError {
    /// `hotkey_signature` was not 128 hex characters.
    #[error("hotkey_signature invalid")]
    InvalidSignature,
    /// `miner_hotkey` was not 64 hex characters.
    #[error("invalid miner hotkey")]
    InvalidHotkey,
    /// JSON body was not an object.
    #[error("submit body is not a JSON object")]
    NotAnObject,
    /// schnorrkel rejected the key or the signature.
    #[error(transparent)]
    Crypto(#[from] crypto::CryptoError),
}

/// Exact bytes signed under [`PROOF_SUBMIT_DOMAIN`].
pub fn submit_signing_payload(
    hotkey_hex: &str,
    topic_id: &str,
    artifact_digest: &str,
    declared_flops: u64,
    claim: &str,
) -> Vec<u8> {
    let flops = declared_flops.to_string();
    let mut p = Vec::with_capacity(
        hotkey_hex.len() + topic_id.len() + artifact_digest.len() + flops.len() + claim.len() + 4,
    );
    p.extend_from_slice(hotkey_hex.as_bytes());
    p.push(0xff);
    p.extend_from_slice(topic_id.as_bytes());
    p.push(0xff);
    p.extend_from_slice(artifact_digest.as_bytes());
    p.push(0xff);
    p.extend_from_slice(flops.as_bytes());
    p.push(0xff);
    p.extend_from_slice(claim.as_bytes());
    p
}

/// 64-hex public key for `secret`.
pub fn hotkey_hex(secret: &[u8; 32]) -> Result<String, SubmitSigError> {
    Ok(hex::encode(crypto::public_key_from_mini_secret(secret)?))
}

/// Sign a Proof submit with a 32-byte mini-secret.
pub fn sign_submit(
    secret: &[u8; 32],
    hotkey_hex: &str,
    topic_id: &str,
    artifact_digest: &str,
    declared_flops: u64,
    claim: &str,
) -> Result<[u8; 64], SubmitSigError> {
    Ok(crypto::sign_raw(
        secret,
        PROOF_SUBMIT_DOMAIN,
        &submit_signing_payload(hotkey_hex, topic_id, artifact_digest, declared_flops, claim),
    )?)
}

/// Verify a Proof submit signature against `hotkey_pk`.
pub fn verify_submit(
    hotkey_pk: &[u8; 32],
    hotkey_hex: &str,
    topic_id: &str,
    artifact_digest: &str,
    declared_flops: u64,
    claim: &str,
    signature: &[u8],
) -> Result<(), SubmitSigError> {
    Ok(crypto::verify_raw(
        hotkey_pk,
        PROOF_SUBMIT_DOMAIN,
        &submit_signing_payload(hotkey_hex, topic_id, artifact_digest, declared_flops, claim),
        signature,
    )?)
}

/// Decode 128-hex (optional `0x`) into a 64-byte sr25519 signature.
pub fn parse_signature_hex(raw: &str) -> Result<[u8; 64], SubmitSigError> {
    let t = raw.trim().trim_start_matches("0x");
    if t.len() != 128 || !t.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(SubmitSigError::InvalidSignature);
    }
    let bytes = hex::decode(t).map_err(|_| SubmitSigError::InvalidSignature)?;
    <[u8; 64]>::try_from(bytes).map_err(|_| SubmitSigError::InvalidSignature)
}

/// Decode 64-hex (optional `0x`) into a 32-byte public key.
pub fn parse_hotkey_hex(raw: &str) -> Result<[u8; 32], SubmitSigError> {
    let t = raw.trim().trim_start_matches("0x");
    if t.len() != 64 || !t.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(SubmitSigError::InvalidHotkey);
    }
    let bytes = hex::decode(t).map_err(|_| SubmitSigError::InvalidHotkey)?;
    <[u8; 32]>::try_from(bytes).map_err(|_| SubmitSigError::InvalidHotkey)
}

/// Set `miner_hotkey` and `hotkey_signature` on a submit JSON object.
pub fn attach_to_json(
    body: &mut serde_json::Value,
    secret: &[u8; 32],
) -> Result<(), SubmitSigError> {
    let obj = body.as_object_mut().ok_or(SubmitSigError::NotAnObject)?;
    let topic_id = obj
        .get("topic_id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_owned();
    let artifact = obj
        .get("artifact_digest")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_owned();
    let declared = obj
        .get("declared_flops")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let claim = obj
        .get("claim")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_owned();
    let hotkey = hotkey_hex(secret)?;
    let sig = sign_submit(secret, &hotkey, &topic_id, &artifact, declared, &claim)?;
    obj.insert("miner_hotkey".into(), serde_json::Value::String(hotkey));
    obj.insert(
        "hotkey_signature".into(),
        serde_json::Value::String(hex::encode(sig)),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sk() -> [u8; 32] {
        let mut s = [0x11u8; 32];
        s[0] = 0x42;
        s
    }

    fn other_sk() -> [u8; 32] {
        let mut s = [0x22u8; 32];
        s[0] = 0x43;
        s
    }

    #[test]
    fn domain_is_proof_prefixed_and_not_a_reuse() {
        assert_eq!(PROOF_SUBMIT_DOMAIN.as_bytes(), b"base-proof-submit-v1");
        assert_eq!(
            PROOF_SUBMIT_DOMAIN_LABEL.as_bytes(),
            PROOF_SUBMIT_DOMAIN.as_bytes()
        );
        for other in [
            b"base-proof-topic-v1".as_slice(),
            b"base-bundle-v1",
            b"base-rawweight-v1",
            b"base-proof-receipt-v1",
            b"base-proof-baseline-v1",
            b"base-proof-task-id-v1",
            b"base-proof-holdout-v1",
        ] {
            assert_ne!(PROOF_SUBMIT_DOMAIN.as_bytes(), other);
        }
    }

    #[test]
    fn payload_is_the_locked_concatenation() {
        let p = submit_signing_payload("aa", "topic", "bb", 12, "claim");
        assert_eq!(p, b"aa\xfftopic\xffbb\xff12\xffclaim");
        assert_ne!(
            submit_signing_payload("aa", "topic", "bb", 12, "claim"),
            submit_signing_payload("aa", "topic", "bb", 13, "claim")
        );
        assert_ne!(
            submit_signing_payload("aa", "topic", "bb", 12, "claim"),
            submit_signing_payload("aa", "topic", "bb", 12, "other")
        );
    }

    #[test]
    fn round_trip_and_wrong_key_fails() {
        let hotkey = hotkey_hex(&sk()).expect("pk");
        let pk = parse_hotkey_hex(&hotkey).expect("pk bytes");
        let sig =
            sign_submit(&sk(), &hotkey, "dt-no-ib-v0", &"ab".repeat(32), 1, "beat").expect("sign");
        verify_submit(
            &pk,
            &hotkey,
            "dt-no-ib-v0",
            &"ab".repeat(32),
            1,
            "beat",
            &sig,
        )
        .expect("verify");
        let other = parse_hotkey_hex(&hotkey_hex(&other_sk()).expect("other")).expect("other pk");
        assert!(verify_submit(
            &other,
            &hotkey,
            "dt-no-ib-v0",
            &"ab".repeat(32),
            1,
            "beat",
            &sig,
        )
        .is_err());
        assert!(verify_submit(
            &pk,
            &hotkey,
            "dt-no-ib-v0",
            &"ab".repeat(32),
            1,
            "other claim",
            &sig,
        )
        .is_err());
    }

    #[test]
    fn attach_to_json_sets_hotkey_and_signature() {
        let mut body = serde_json::json!({
            "topic_id": "dt-no-ib-v0",
            "artifact_digest": "ab".repeat(32),
            "declared_flops": 7u64,
            "claim": "beat baseline",
        });
        attach_to_json(&mut body, &sk()).expect("attach");
        assert_eq!(body["miner_hotkey"], hotkey_hex(&sk()).expect("pk"));
        let sig =
            parse_signature_hex(body["hotkey_signature"].as_str().expect("sig")).expect("hex");
        let pk = parse_hotkey_hex(body["miner_hotkey"].as_str().expect("hk")).expect("pk");
        verify_submit(
            &pk,
            body["miner_hotkey"].as_str().expect("hk"),
            "dt-no-ib-v0",
            &"ab".repeat(32),
            7,
            "beat baseline",
            &sig,
        )
        .expect("verify attached");
    }
}
