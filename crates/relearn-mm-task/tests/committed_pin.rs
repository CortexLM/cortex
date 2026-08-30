#![allow(clippy::unwrap_used, clippy::expect_used)]

//! The committed `config/relearn-mm-pin.toml` must load and satisfy every
//! product rule: the Relearn champion base on the language side, a verified
//! permissive encoder, splits thick enough for a verdict, and no secrets.

use std::path::{Path, PathBuf};

use relearn_mm_task::{
    license_is_permissive, RelearnMmPin, ENCODER_LICENSE, ENCODER_MODEL_ID, LM_BASE_MODEL_ID,
    MIN_AGENTIC_TRACES, MIN_TEXT_ITEMS, MIN_VISION_ITEMS_PER_TASK,
};

fn pin_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../config/relearn-mm-pin.toml")
        .canonicalize()
        .expect("pin path")
}

fn pin() -> RelearnMmPin {
    let body = std::fs::read_to_string(pin_path()).expect("read pin");
    RelearnMmPin::from_toml(&body).expect("committed pin must validate")
}

#[test]
fn committed_pin_validates() {
    let p = pin();
    assert_eq!(p.lm_base_model, LM_BASE_MODEL_ID);
    assert_eq!(p.encoder_model, ENCODER_MODEL_ID);
    assert_eq!(p.encoder_license, ENCODER_LICENSE);
    assert!(license_is_permissive(&p.encoder_license));
}

#[test]
fn committed_splits_clear_the_evidence_floors() {
    let p = pin();
    assert!(p.text_holdout_items >= MIN_TEXT_ITEMS);
    assert!(p.vision_items_per_task >= MIN_VISION_ITEMS_PER_TASK);
    assert!(p.agentic_traces >= MIN_AGENTIC_TRACES);
    assert!((p.vision_weights.total() - 1.0).abs() < 1e-9);
}

#[test]
fn pin_carries_no_endpoint_or_secret() {
    let body = std::fs::read_to_string(pin_path()).expect("read pin");
    let lower = body.to_ascii_lowercase();
    for banned in ["api_key", "bearer", "_token", "mnemonic", "https://api."] {
        assert!(!lower.contains(banned), "pin mentions {banned:?}");
    }
}

#[test]
fn pin_documents_that_imagenet_and_coco_test_are_off_limits() {
    let body = std::fs::read_to_string(pin_path()).expect("read pin");
    assert!(
        body.contains("ImageNet") && body.contains("COCO"),
        "the contamination rationale must stay next to the split sizes"
    );
}
