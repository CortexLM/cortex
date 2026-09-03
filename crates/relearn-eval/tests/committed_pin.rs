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

/// Every digest built so far failed the live harvest, so the committed pin is
/// empty and live scoring is refused. An empty digest is a 503 on submit — the
/// fail-closed state — not a fallback to the sim harness.
#[test]
fn committed_pin_is_fail_closed_until_a_working_image_ships() {
    let p = pin();
    assert!(
        p.eval_image_digest.is_empty(),
        "no CortexLM/relearn image has printed RELEARN_EVAL_OK on a rented pod; \
         an empty digest 503s, a guessed one rents a B200 that cannot score"
    );
    assert!(!p.can_rent(), "an unpinned host must not rent");
    assert!(
        p.relearn_git_sha.is_empty(),
        "no digest, no commit that built it"
    );
}

/// Every digest that reached a pod and failed is named in the pin, with why.
/// Re-pinning one of these is the failure mode this list exists to stop.
#[test]
fn pin_names_the_digests_that_must_not_be_re_harvested() {
    let body = std::fs::read_to_string(pin_path()).expect("read pin");
    for (prefix, why) in [
        (
            "cbc4bbb8",
            "no vLLM / torchvision on the CUDA scoring image",
        ),
        ("201cc5d2", "judge generation cap 32 / empty content"),
        ("303c6357", "printed no RELEARN_EVAL_OK"),
        ("00839671", "exit 127, no /usr/bin/relearn-eval"),
        ("86240d76", "pre-CUDA"),
    ] {
        assert!(body.contains(prefix), "pin must warn off {prefix} ({why})");
    }
    // The next image has to fix the crash that killed cbc4bbb8, so the pin
    // says what that is rather than leaving the next operator to rediscover it.
    let lower = body.to_ascii_lowercase();
    assert!(lower.contains("vllm"), "pin must name the missing runtime");
    assert!(lower.contains("torchvision"), "{lower}");
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
    if p.eval_image_digest.is_empty() {
        return;
    }
    let hex = p.eval_image_digest.trim_start_matches("sha256:");
    assert_eq!(hex.len(), 64);
    assert!(hex
        .chars()
        .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase()));
    for dead in [
        "cbc4bbb80e",
        "201cc5d29c",
        "303c63573c",
        "0083967170",
        "86240d7617",
    ] {
        assert!(
            !p.eval_image_digest.contains(dead),
            "{dead} already failed the live harvest; it must never be re-pinned"
        );
    }
}

#[test]
fn pin_does_not_embed_holdout_prompts() {
    let body = std::fs::read_to_string(pin_path()).expect("read pin");
    assert!(!body.contains("[[holdout"));
    assert!(!body.contains("holdout_item"));
}
