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
    assert_eq!(p.teacher_nvfp4, "LibertAIDAI/GLM-5.3-Flash-NVFP4");
    assert_eq!(p.teacher_model, "glm-5.3-flash");
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

#[test]
fn committed_pin_cannot_rent_or_score_live_yet() {
    let p = pin();
    assert!(
        !p.can_rent(),
        "an eval digest landed; drop this test and enable live scoring deliberately"
    );
}

#[test]
fn pin_does_not_embed_holdout_prompts() {
    let body = std::fs::read_to_string(pin_path()).expect("read pin");
    assert!(!body.contains("[[holdout"));
    assert!(!body.contains("holdout_item"));
}
