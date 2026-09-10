//! Miner Proof submit signatures.
//!
//! ```text
//! domain  = b"base-proof-submit-v1"
//! payload = hotkey_hex || 0xff || topic_id || 0xff || artifact_digest || 0xff
//!           || declared_flops_decimal || 0xff || claim || 0xff
//!           || manifest_canonical || 0xff || submit_nonce_hex
//!
//! manifest_canonical =
//!           count(hashes)_decimal   || (0xff || hash_i)*      hashes sorted bytewise
//!           || 0xff ||
//!           count(datasets)_decimal || (0xff || dataset_j)*   datasets sorted bytewise
//! ```
//!
//! `hashes` / `datasets` are `manifest.train_content_hashes` /
//! `manifest.train_dataset_ids` as exact UTF-8 strings (duplicates kept, no
//! trimming; a missing list is `0`). UTF-8 never contains `0xff`, so the
//! encoding is injective. `submit_nonce_hex` is a client-chosen 32-byte value
//! as 64 lowercase hex; the host accepts each `(hotkey, nonce)` once.
//!
//! Distinct from `base-proof-topic-v1` (operator topic documents) and from
//! consensus tags (`base-bundle-v1`, `base-rawweight-v1`, …). A captured
//! signature cannot be replayed (nonce), nor reused with a different claim,
//! FLOP declaration, or contamination manifest on the same artifact.

#![forbid(unsafe_code)]
#![allow(
    clippy::doc_markdown,
    clippy::missing_errors_doc,
    clippy::must_use_candidate
)]

use rand_core::{OsRng, RngCore};
use thiserror::Error;

/// Signature domain for miner Proof submits (`POST /v1/submissions`).
pub const PROOF_SUBMIT_DOMAIN: crypto::DomainTag = crypto::DomainTag::new(b"base-proof-submit-v1");

/// ASCII label of [`PROOF_SUBMIT_DOMAIN`].
pub const PROOF_SUBMIT_DOMAIN_LABEL: &str = "base-proof-submit-v1";

/// Hex length of a `submit_nonce` (32 bytes).
pub const SUBMIT_NONCE_HEX_LEN: usize = 64;

/// Hex length of a `hotkey_signature` (64 bytes).
pub const SIGNATURE_HEX_LEN: usize = 128;

/// Why a submit signature cannot be built or checked.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SubmitSigError {
    /// `hotkey_signature` was not exactly 128 lowercase hex characters.
    #[error("hotkey_signature invalid")]
    InvalidSignature,
    /// `miner_hotkey` was not 64 hex characters.
    #[error("invalid miner hotkey")]
    InvalidHotkey,
    /// `submit_nonce` was not exactly 64 lowercase hex characters.
    #[error("submit_nonce invalid")]
    InvalidNonce,
    /// JSON body was not an object, or its manifest lists were not strings.
    #[error("submit body is not a JSON object with string manifest lists")]
    NotAnObject,
    /// schnorrkel rejected the key or the signature.
    #[error(transparent)]
    Crypto(#[from] crypto::CryptoError),
}

/// Every field a miner binds into `hotkey_signature`.
///
/// Nothing else on the wire is authenticated: `artifact_uri` and
/// `architecture` are locators / labels, not gate inputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubmitFields<'a> {
    /// 64 lowercase hex sr25519 public key (the verifying key).
    pub hotkey_hex: &'a str,
    /// Open topic id, as sent.
    pub topic_id: &'a str,
    /// 64 lowercase hex sha256 of the artefact.
    pub artifact_digest: &'a str,
    /// Declared FLOPs (decimal in the payload).
    pub declared_flops: u64,
    /// Claim text, as sent.
    pub claim: &'a str,
    /// `manifest.train_content_hashes`, as sent (any order).
    pub train_content_hashes: &'a [String],
    /// `manifest.train_dataset_ids`, as sent (any order).
    pub train_dataset_ids: &'a [String],
    /// 64 lowercase hex client nonce (single use per hotkey).
    pub submit_nonce_hex: &'a str,
}

impl SubmitFields<'_> {
    /// Exact bytes signed under [`PROOF_SUBMIT_DOMAIN`].
    pub fn signing_payload(&self) -> Vec<u8> {
        let flops = self.declared_flops.to_string();
        let manifest = manifest_canonical(self.train_content_hashes, self.train_dataset_ids);
        let mut p = Vec::with_capacity(
            self.hotkey_hex.len()
                + self.topic_id.len()
                + self.artifact_digest.len()
                + flops.len()
                + self.claim.len()
                + manifest.len()
                + self.submit_nonce_hex.len()
                + 7,
        );
        p.extend_from_slice(self.hotkey_hex.as_bytes());
        p.push(0xff);
        p.extend_from_slice(self.topic_id.as_bytes());
        p.push(0xff);
        p.extend_from_slice(self.artifact_digest.as_bytes());
        p.push(0xff);
        p.extend_from_slice(flops.as_bytes());
        p.push(0xff);
        p.extend_from_slice(self.claim.as_bytes());
        p.push(0xff);
        p.extend_from_slice(&manifest);
        p.push(0xff);
        p.extend_from_slice(self.submit_nonce_hex.as_bytes());
        p
    }
}

/// Canonical bytes of the manifest fields the contamination gate reads.
///
/// `count_decimal || (0xff || entry)*` per list, lists joined by `0xff`,
/// entries sorted bytewise, duplicates kept, nothing trimmed.
pub fn manifest_canonical(
    train_content_hashes: &[String],
    train_dataset_ids: &[String],
) -> Vec<u8> {
    let mut out = Vec::new();
    push_sorted_list(&mut out, train_content_hashes);
    out.push(0xff);
    push_sorted_list(&mut out, train_dataset_ids);
    out
}

fn push_sorted_list(out: &mut Vec<u8>, entries: &[String]) {
    let mut sorted: Vec<&str> = entries.iter().map(String::as_str).collect();
    sorted.sort_unstable();
    out.extend_from_slice(sorted.len().to_string().as_bytes());
    for entry in sorted {
        out.push(0xff);
        out.extend_from_slice(entry.as_bytes());
    }
}

/// 64-hex public key for `secret`.
pub fn hotkey_hex(secret: &[u8; 32]) -> Result<String, SubmitSigError> {
    Ok(hex::encode(crypto::public_key_from_mini_secret(secret)?))
}

/// Fresh random `submit_nonce` (64 lowercase hex).
pub fn fresh_submit_nonce_hex() -> String {
    let mut nonce = [0u8; 32];
    OsRng.fill_bytes(&mut nonce);
    hex::encode(nonce)
}

/// Sign a Proof submit with a 32-byte mini-secret.
pub fn sign_submit(
    secret: &[u8; 32],
    fields: &SubmitFields<'_>,
) -> Result<[u8; 64], SubmitSigError> {
    Ok(crypto::sign_raw(
        secret,
        PROOF_SUBMIT_DOMAIN,
        &fields.signing_payload(),
    )?)
}

/// Verify a Proof submit signature against `hotkey_pk`.
pub fn verify_submit(
    hotkey_pk: &[u8; 32],
    fields: &SubmitFields<'_>,
    signature: &[u8],
) -> Result<(), SubmitSigError> {
    Ok(crypto::verify_raw(
        hotkey_pk,
        PROOF_SUBMIT_DOMAIN,
        &fields.signing_payload(),
        signature,
    )?)
}

fn is_lower_hex(s: &str, len: usize) -> bool {
    s.len() == len
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Decode exactly 128 lowercase hex (no `0x`) into a 64-byte sr25519 signature.
pub fn parse_signature_hex(raw: &str) -> Result<[u8; 64], SubmitSigError> {
    if !is_lower_hex(raw, SIGNATURE_HEX_LEN) {
        return Err(SubmitSigError::InvalidSignature);
    }
    let bytes = hex::decode(raw).map_err(|_| SubmitSigError::InvalidSignature)?;
    <[u8; 64]>::try_from(bytes).map_err(|_| SubmitSigError::InvalidSignature)
}

/// Decode exactly 64 lowercase hex (no `0x`) into a 32-byte `submit_nonce`.
pub fn parse_submit_nonce_hex(raw: &str) -> Result<[u8; 32], SubmitSigError> {
    if !is_lower_hex(raw, SUBMIT_NONCE_HEX_LEN) {
        return Err(SubmitSigError::InvalidNonce);
    }
    let bytes = hex::decode(raw).map_err(|_| SubmitSigError::InvalidNonce)?;
    <[u8; 32]>::try_from(bytes).map_err(|_| SubmitSigError::InvalidNonce)
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

/// The two manifest lists a submit JSON body carries (missing list = empty).
pub fn manifest_lists(
    body: &serde_json::Value,
) -> Result<(Vec<String>, Vec<String>), SubmitSigError> {
    let manifest = body.get("manifest").unwrap_or(&serde_json::Value::Null);
    Ok((
        string_list(manifest.get("train_content_hashes"))?,
        string_list(manifest.get("train_dataset_ids"))?,
    ))
}

fn string_list(v: Option<&serde_json::Value>) -> Result<Vec<String>, SubmitSigError> {
    let Some(v) = v else {
        return Ok(Vec::new());
    };
    v.as_array()
        .ok_or(SubmitSigError::NotAnObject)?
        .iter()
        .map(|e| {
            e.as_str()
                .map(str::to_owned)
                .ok_or(SubmitSigError::NotAnObject)
        })
        .collect()
}

/// Sign a submit JSON object in place: sets `miner_hotkey`,
/// `hotkey_signature`, and — when absent — a fresh `submit_nonce`.
pub fn attach_to_json(
    body: &mut serde_json::Value,
    secret: &[u8; 32],
) -> Result<(), SubmitSigError> {
    let (hashes, datasets) = manifest_lists(body)?;
    let obj = body.as_object_mut().ok_or(SubmitSigError::NotAnObject)?;
    let nonce = match obj.get("submit_nonce").and_then(serde_json::Value::as_str) {
        Some(n) => n.to_owned(),
        None => fresh_submit_nonce_hex(),
    };
    let hotkey = hotkey_hex(secret)?;
    let text = |k: &str| {
        obj.get(k)
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_owned()
    };
    let (topic_id, artifact, claim) = (text("topic_id"), text("artifact_digest"), text("claim"));
    let declared = obj
        .get("declared_flops")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let sig = sign_submit(
        secret,
        &SubmitFields {
            hotkey_hex: &hotkey,
            topic_id: &topic_id,
            artifact_digest: &artifact,
            declared_flops: declared,
            claim: &claim,
            train_content_hashes: &hashes,
            train_dataset_ids: &datasets,
            submit_nonce_hex: &nonce,
        },
    )?;
    obj.insert("miner_hotkey".into(), serde_json::Value::String(hotkey));
    obj.insert(
        "hotkey_signature".into(),
        serde_json::Value::String(hex::encode(sig)),
    );
    obj.insert("submit_nonce".into(), serde_json::Value::String(nonce));
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

    const NONCE: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn fields<'a>(
        hotkey: &'a str,
        claim: &'a str,
        datasets: &'a [String],
        nonce: &'a str,
    ) -> SubmitFields<'a> {
        SubmitFields {
            hotkey_hex: hotkey,
            topic_id: "dt-no-ib-v0",
            artifact_digest: "abababababababababababababababababababababababababababababababab",
            declared_flops: 1,
            claim,
            train_content_hashes: &[],
            train_dataset_ids: datasets,
            submit_nonce_hex: nonce,
        }
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
    fn manifest_canonical_is_sorted_counted_and_injective() {
        let empty: [String; 0] = [];
        assert_eq!(manifest_canonical(&empty, &empty), b"0\xff0");
        let one = ["my-mix-v0".to_owned()];
        assert_eq!(manifest_canonical(&empty, &one), b"0\xff1\xffmy-mix-v0");
        assert_eq!(manifest_canonical(&one, &empty), b"1\xffmy-mix-v0\xff0");
        let ab = ["b".to_owned(), "a".to_owned()];
        let ba = ["a".to_owned(), "b".to_owned()];
        assert_eq!(manifest_canonical(&ab, &empty), b"2\xffa\xffb\xff0");
        assert_eq!(
            manifest_canonical(&ab, &empty),
            manifest_canonical(&ba, &empty)
        );
        // Entries that spell the other list's shape cannot slide across lists.
        let tricky_h = ["1".to_owned(), "a".to_owned()];
        let tricky_d = ["a".to_owned()];
        assert_ne!(
            manifest_canonical(&tricky_h, &empty),
            manifest_canonical(&["1".to_owned()], &tricky_d)
        );
    }

    #[test]
    fn payload_is_the_locked_concatenation() {
        let datasets = ["my-mix-v0".to_owned()];
        let f = SubmitFields {
            hotkey_hex: "aa",
            topic_id: "topic",
            artifact_digest: "bb",
            declared_flops: 12,
            claim: "claim",
            train_content_hashes: &[],
            train_dataset_ids: &datasets,
            submit_nonce_hex: "cc",
        };
        assert_eq!(
            f.signing_payload(),
            b"aa\xfftopic\xffbb\xff12\xffclaim\xff0\xff1\xffmy-mix-v0\xffcc"
        );
        let mut flops = f;
        flops.declared_flops = 13;
        assert_ne!(f.signing_payload(), flops.signing_payload());
        let mut claim = f;
        claim.claim = "other";
        assert_ne!(f.signing_payload(), claim.signing_payload());
        let mut nonce = f;
        nonce.submit_nonce_hex = "cd";
        assert_ne!(f.signing_payload(), nonce.signing_payload());
        let other_ds = ["other-mix".to_owned()];
        let mut manifest = f;
        manifest.train_dataset_ids = &other_ds;
        assert_ne!(f.signing_payload(), manifest.signing_payload());
    }

    #[test]
    fn round_trip_wrong_key_and_tampered_fields_fail() {
        let hotkey = hotkey_hex(&sk()).expect("pk");
        let pk = parse_hotkey_hex(&hotkey).expect("pk bytes");
        let datasets = ["my-mix-v0".to_owned()];
        let f = fields(&hotkey, "beat", &datasets, NONCE);
        let sig = sign_submit(&sk(), &f).expect("sign");
        verify_submit(&pk, &f, &sig).expect("verify");

        let other = parse_hotkey_hex(&hotkey_hex(&other_sk()).expect("other")).expect("other pk");
        assert!(verify_submit(&other, &f, &sig).is_err());
        assert!(
            verify_submit(&pk, &fields(&hotkey, "other claim", &datasets, NONCE), &sig).is_err()
        );
        let tampered = ["leaked-holdout".to_owned()];
        assert!(verify_submit(&pk, &fields(&hotkey, "beat", &tampered, NONCE), &sig).is_err());
        let replay_nonce = "f".repeat(64);
        assert!(verify_submit(
            &pk,
            &fields(&hotkey, "beat", &datasets, &replay_nonce),
            &sig
        )
        .is_err());
    }

    #[test]
    fn signature_and_nonce_hex_are_strict_lowercase() {
        let sig = "ab".repeat(64);
        assert!(parse_signature_hex(&sig).is_ok());
        assert!(parse_signature_hex(&format!("0x{sig}")).is_err());
        assert!(parse_signature_hex(&sig.to_ascii_uppercase()).is_err());
        assert!(parse_signature_hex(&format!(" {sig}")).is_err());
        assert!(parse_signature_hex(&sig[..126]).is_err());

        assert!(parse_submit_nonce_hex(NONCE).is_ok());
        assert!(parse_submit_nonce_hex(&NONCE.to_ascii_uppercase()).is_err());
        assert!(parse_submit_nonce_hex(&format!("0x{NONCE}")).is_err());
        assert!(parse_submit_nonce_hex(&NONCE[..62]).is_err());
        assert!(parse_submit_nonce_hex("").is_err());
        let fresh = fresh_submit_nonce_hex();
        assert!(parse_submit_nonce_hex(&fresh).is_ok());
        assert_ne!(fresh, fresh_submit_nonce_hex());
    }

    #[test]
    fn attach_to_json_sets_hotkey_signature_and_nonce() {
        let mut body = serde_json::json!({
            "topic_id": "dt-no-ib-v0",
            "artifact_digest": "ab".repeat(32),
            "declared_flops": 7u64,
            "claim": "beat baseline",
            "manifest": { "train_dataset_ids": ["my-mix-v0"] },
        });
        attach_to_json(&mut body, &sk()).expect("attach");
        let hotkey = hotkey_hex(&sk()).expect("pk");
        assert_eq!(body["miner_hotkey"], hotkey);
        let nonce = body["submit_nonce"].as_str().expect("nonce").to_owned();
        parse_submit_nonce_hex(&nonce).expect("fresh nonce");
        let sig =
            parse_signature_hex(body["hotkey_signature"].as_str().expect("sig")).expect("hex");
        let pk = parse_hotkey_hex(&hotkey).expect("pk");
        let (hashes, datasets) = manifest_lists(&body).expect("lists");
        assert!(hashes.is_empty());
        assert_eq!(datasets, ["my-mix-v0".to_owned()]);
        let digest = "ab".repeat(32);
        verify_submit(
            &pk,
            &SubmitFields {
                hotkey_hex: &hotkey,
                topic_id: "dt-no-ib-v0",
                artifact_digest: &digest,
                declared_flops: 7,
                claim: "beat baseline",
                train_content_hashes: &hashes,
                train_dataset_ids: &datasets,
                submit_nonce_hex: &nonce,
            },
            &sig,
        )
        .expect("verify attached");

        let mut pinned = serde_json::json!({ "topic_id": "t", "submit_nonce": NONCE });
        attach_to_json(&mut pinned, &sk()).expect("attach pinned");
        assert_eq!(pinned["submit_nonce"], NONCE);

        let mut bad = serde_json::json!({ "manifest": { "train_dataset_ids": [1] } });
        assert_eq!(
            attach_to_json(&mut bad, &sk()),
            Err(SubmitSigError::NotAnObject)
        );
    }
}
