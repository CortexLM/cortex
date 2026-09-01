#![allow(clippy::unwrap_used, clippy::expect_used)]

//! The committed `config/relearn-agent-pin.toml` must load and carry a real
//! episode commitment, and it must never carry the episodes themselves.

use std::path::{Path, PathBuf};

use relearn_agent_task::{RelearnAgentPin, BASE_MODEL_ID, MIN_HOLDOUT_EPISODES};

fn pin_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../config/relearn-agent-pin.toml")
        .canonicalize()
        .expect("pin path")
}

fn body() -> String {
    std::fs::read_to_string(pin_path()).expect("read pin")
}

fn pin() -> RelearnAgentPin {
    let p = RelearnAgentPin::from_toml(&body()).expect("committed pin must parse");
    p.validate().expect("committed pin must validate");
    p
}

#[test]
fn committed_pin_has_an_episode_commitment() {
    let p = pin();
    assert_eq!(p.holdout_commitment.len(), 64);
    assert!(p.holdout_size >= MIN_HOLDOUT_EPISODES);
    assert!(!p.public_ids.is_empty());
}

/// The Agent challenge post-trains the same checkpoint as `relearn`. Swapping
/// the base here would silently change what miners are competing on.
#[test]
fn committed_pin_has_the_locked_base() {
    assert_eq!(pin().base_model, BASE_MODEL_ID);
    assert_eq!(pin().base_model, "Qwen/Qwen3.8-27B");
}

#[test]
fn pin_carries_no_endpoint_or_secret() {
    let lower = body().to_ascii_lowercase();
    for banned in [
        "api_key",
        "bearer",
        "_token",
        "mnemonic",
        "https://api.",
        "modal",
    ] {
        assert!(!lower.contains(banned), "pin mentions {banned:?}");
    }
}

#[test]
fn pin_says_its_commitment_is_not_the_live_seal() {
    let text = body();
    assert!(
        text.contains("CEREMONY.md"),
        "pin must point at the ceremony"
    );
    let lower = text.to_ascii_lowercase();
    assert!(
        lower.contains("not the live seal"),
        "pin must not present the CI commitment as production"
    );
    assert!(
        lower.contains("private catalogue"),
        "prod must rotate the catalogue as well as the salt"
    );
}

/// A floating tag in a deploy path is how a "pinned" image silently changes
/// under the trust root.
#[test]
fn eval_image_is_digest_only() {
    let p = pin();
    assert_eq!(p.eval_image, "ghcr.io/cortexlm/relearn-agent-eval");
    assert!(
        !p.eval_image.contains(':'),
        "the tag belongs in eval_image_digest, not eval_image"
    );
    if p.eval_image_digest.is_empty() {
        // No agent eval image published yet: a live host must refuse rather
        // than score, and the pin must say so.
        assert!(!p.can_rent());
        assert!(body().contains("503"), "pin must state the live behaviour");
        return;
    }
    let hex = p.eval_image_digest.trim_start_matches("sha256:");
    assert_eq!(hex.len(), 64);
    assert!(hex
        .chars()
        .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase()));
    assert!(p.can_rent());
}

#[test]
fn pin_does_not_embed_episodes() {
    let text = body();
    assert!(!text.contains("[[episodes"));
    assert!(!text.contains("observation_hash"));
    assert!(!text.contains("answer_hash"));
}
