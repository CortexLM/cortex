#![allow(clippy::unwrap_used, clippy::expect_used)]

//! The committed `config/proof-pin.toml` must load, match the trust-root
//! `proof` row, and never carry a topic catalog, holdout records, or secrets.

use std::path::{Path, PathBuf};

use proof_task::{
    ProofPin, ALLOWED_MODES, CHALLENGE_ID, EVAL_IMAGE, HOLDOUT_SIZE,
    INFERENCE_CONFIG_SCHEMA_VERSION, INFERENCE_OFFER_COMMITMENT_ALG, MAX_INPUT_TOKENS_CEILING,
    MAX_OUTPUT_TOKENS_CEILING, STRATUM_SIZE,
};

fn pin_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../config/proof-pin.toml")
        .canonicalize()
        .expect("pin path")
}

fn challenges_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../config/challenges.toml")
        .canonicalize()
        .expect("challenges path")
}

fn body() -> String {
    std::fs::read_to_string(pin_path()).expect("read pin")
}

fn pin() -> ProofPin {
    let p = ProofPin::from_toml(&body()).expect("committed pin must parse");
    p.validate().expect("committed pin must validate");
    p
}

fn proof_row_pubkey() -> String {
    let doc: toml::Value =
        toml::from_str(&std::fs::read_to_string(challenges_path()).expect("challenges")).unwrap();
    for row in doc
        .get("challenges")
        .and_then(|c| c.as_array())
        .expect("challenges array")
    {
        if row.get("id").and_then(|v| v.as_str()) == Some("proof") {
            return row
                .get("public_key")
                .and_then(|v| v.as_str())
                .expect("proof public_key")
                .to_ascii_lowercase();
        }
    }
    panic!("proof row missing from challenges.toml");
}

#[test]
fn committed_pin_is_proof_with_a_real_eval_digest() {
    let p = pin();
    assert_eq!(p.challenge_id, CHALLENGE_ID);
    assert_eq!(p.eval_image, EVAL_IMAGE);
    assert!(
        !p.eval_image.contains(':'),
        "the tag belongs in eval_image_digest, not eval_image"
    );
    let digest = p.eval_image_digest.trim();
    assert!(
        digest.starts_with("sha256:") && digest.len() == 71,
        "committed digest must be a real sha256 pin, not invented or empty: {digest:?}"
    );
    let hex = digest.trim_start_matches("sha256:");
    assert!(
        hex.len() == 64 && hex.chars().all(|c| c.is_ascii_hexdigit()),
        "{digest}"
    );
    assert!(p.can_rent(), "pinned digest must be rentable");
    assert_eq!(p.holdout_size, HOLDOUT_SIZE);
    assert_eq!(p.stratum_size, STRATUM_SIZE);
    assert!(p.proxy_model.trim().is_empty(), "no HF proxy bake");
    assert!(p.proxy_models.is_empty(), "no HF proxy bake list");
    assert_eq!(
        p.inference_config_schema_version,
        INFERENCE_CONFIG_SCHEMA_VERSION
    );
    assert_eq!(
        p.inference_offer_commitment_alg,
        INFERENCE_OFFER_COMMITMENT_ALG
    );
    assert_eq!(p.allowed_modes.as_slice(), ALLOWED_MODES.as_slice());
    assert_eq!(p.max_input_tokens_ceiling, MAX_INPUT_TOKENS_CEILING);
    assert_eq!(p.max_output_tokens_ceiling, MAX_OUTPUT_TOKENS_CEILING);
    assert_eq!(
        p.inference.provider,
        proof_task::InferenceProviderKind::OpenaiCompatible
    );
    assert!(
        p.inference.base_url.trim().is_empty(),
        "url is secret-backed"
    );
    assert!(
        p.inference.model.trim().is_empty(),
        "empty model is pre-launch 503"
    );
    assert_eq!(p.inference.mode, proof_task::InferenceMode::Chat);
    assert_eq!(p.inference.max_input_tokens, MAX_INPUT_TOKENS_CEILING);
    assert_eq!(p.inference.max_output_tokens, MAX_OUTPUT_TOKENS_CEILING);
}

#[test]
fn topic_pubkey_matches_the_trust_root_proof_row() {
    assert_eq!(pin().topic_pubkey.to_ascii_lowercase(), proof_row_pubkey());
}

#[test]
fn pin_carries_complete_inference_defaults_table() {
    let text = body();
    assert!(text.contains("[inference]"), "pin must declare [inference]");
    for key in [
        "provider",
        "base_url",
        "model",
        "mode",
        "max_input_tokens",
        "max_output_tokens",
    ] {
        assert!(text.contains(key), "pin [inference] must name {key}");
    }
}

#[test]
fn pin_carries_no_endpoint_secret_or_topic_catalog() {
    let lower = body().to_ascii_lowercase();
    for banned in [
        "api_key",
        "bearer",
        "admin_token",
        "mnemonic",
        "https://api.",
        "modal",
        "[[topics",
        "holdout_records",
        "content_sha256",
        "qwen/qwen3-0.6b",
        "qwen/qwen3.8-0.6b",
    ] {
        assert!(!lower.contains(banned), "pin mentions {banned:?}");
    }
}

#[test]
fn pin_points_at_the_ceremony_and_says_it_is_not_the_live_seal() {
    let text = body();
    assert!(
        text.contains("CEREMONY.md"),
        "pin must point at the ceremony"
    );
    let lower = text.to_ascii_lowercase();
    assert!(
        lower.contains("not the live seal"),
        "pin must not present the CI topic key as production"
    );
}
