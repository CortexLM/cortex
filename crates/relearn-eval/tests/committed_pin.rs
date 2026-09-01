#![allow(clippy::unwrap_used, clippy::expect_used)]

//! The committed `config/relearn-pin.toml` must load and carry a real
//! holdout commitment. Model ids are not rewritten here.

use std::path::{Path, PathBuf};

use relearn_eval::RelearnPin;

fn pin_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../config/relearn-pin.toml")
        .canonicalize()
        .expect("pin path")
}

fn pin() -> RelearnPin {
    let body = std::fs::read_to_string(pin_path()).expect("read pin");
    let p = RelearnPin::from_toml(&body).expect("committed pin must parse");
    p.validate().expect("committed pin must validate");
    p
}

#[test]
fn committed_pin_has_a_holdout_commitment() {
    let p = pin();
    assert_eq!(p.holdout_commitment.len(), 64);
    assert!(p.holdout_size >= relearn_challenge_task::MIN_HOLDOUT_ITEMS);
    assert!(!p.public_ids.is_empty());
}

#[test]
fn committed_pin_has_the_locked_model_ids() {
    let p = pin();
    assert_eq!(p.base_model, "Qwen/Qwen3.8-27B");
    assert_eq!(p.teacher_nvfp4, "incoai/GLM-5.3-NVFP4");
    assert_eq!(p.teacher_model, "glm-5.3");
}

#[test]
fn pin_carries_no_endpoint_or_secret_or_t2i_salt() {
    let body = std::fs::read_to_string(pin_path()).expect("read pin");
    let lower = body.to_ascii_lowercase();
    for banned in [
        "api_key",
        "bearer",
        "_token",
        "mnemonic",
        "https://api.",
        "modal",
        "cortex-t2i-dev-holdout-v0",
        // Naming the dev salt here reads as the live seal. The pin says the
        // commitment is the CI one and points at the ceremony instead.
        "cortex-relearn-dev-holdout-v0",
    ] {
        assert!(!lower.contains(banned), "pin mentions {banned:?}");
    }
}

#[test]
fn pin_says_its_commitment_is_not_the_live_seal() {
    let body = std::fs::read_to_string(pin_path()).expect("read pin");
    assert!(
        body.contains("CEREMONY.md"),
        "pin must point at the ceremony"
    );
    let lower = body.to_ascii_lowercase();
    assert!(
        lower.contains("not the live seal"),
        "pin must not present the CI commitment as production"
    );
    assert!(
        lower.contains("private catalog"),
        "prod must rotate the catalog as well as the salt"
    );
}

/// The eval image is pinned, so a live host may rent. `can_rent` is only the
/// image half: the harvest and the champion baseline are still operator state.
#[test]
fn committed_pin_allows_live_rent() {
    let p = pin();
    assert!(p.can_rent(), "eval_image_digest must be a sha256 pin");
    assert_eq!(
        p.eval_image_digest,
        "sha256:86240d7617d296dc12c9f215b6156b127c60dc2baafe87db6dea7a3b7bbb68ba"
    );
    assert_eq!(
        p.relearn_git_sha,
        "9998154fec288cafc185a0478748db2243fada5f"
    );
    assert_eq!(p.relearn_git_sha.len(), 40);
    assert!(p.relearn_git_sha.chars().all(|c| c.is_ascii_hexdigit()));
    assert!(
        !p.eval_image_digest.contains("0083967170"),
        "do not pin the digest that exited 127 (no /usr/bin/relearn-eval)"
    );
    assert!(
        !p.eval_image_digest.contains("303c63573c"),
        "do not pin the digest that printed no RELEARN_EVAL_OK"
    );
}

/// A floating tag in a deploy path is how a "pinned" image silently changes
/// under the trust root.
#[test]
fn eval_image_is_digest_only() {
    let p = pin();
    assert_eq!(p.eval_image, "ghcr.io/cortexlm/relearn-eval");
    assert!(
        !p.eval_image.contains(':'),
        "the tag belongs in eval_image_digest, not eval_image"
    );
    let hex = p.eval_image_digest.trim_start_matches("sha256:");
    assert_eq!(hex.len(), 64);
    assert!(hex
        .chars()
        .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase()));
}

#[test]
fn pin_does_not_embed_holdout_prompts() {
    let body = std::fs::read_to_string(pin_path()).expect("read pin");
    assert!(!body.contains("[[holdout"));
    assert!(!body.contains("holdout_item"));
}
