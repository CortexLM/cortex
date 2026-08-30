#![allow(clippy::unwrap_used, clippy::expect_used, clippy::doc_markdown)]

//! The committed `config/relearn-t2i-pin.toml` must load and satisfy every
//! product rule: Cosmos3 base, OpenMDW 1.1, Q-Judger as the only judge, a
//! frozen public split large enough for the paired test, and a holdout that is
//! present only as a commitment.

use std::path::{Path, PathBuf};

use relearn_t2i_task::{
    base_is_rejected, is_bench_prompt_id, RelearnT2iPin, BASE_MODEL_ID, BASE_MODEL_LICENSE,
    JUDGE_MODEL_ID, MIN_SCORED_CELLS,
};

fn pin_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../config/relearn-t2i-pin.toml")
        .canonicalize()
        .expect("pin path")
}

fn pin() -> RelearnT2iPin {
    let body = std::fs::read_to_string(pin_path()).expect("read pin");
    RelearnT2iPin::from_toml(&body).expect("committed pin must validate")
}

#[test]
fn committed_pin_validates() {
    let p = pin();
    assert_eq!(p.base, BASE_MODEL_ID);
    assert_eq!(p.base_license, BASE_MODEL_LICENSE);
    assert_eq!(p.base_license_url, "https://openmdw.ai/license/1-1/");
    assert_eq!(p.judge_model, JUDGE_MODEL_ID);
    assert!(!base_is_rejected(&p.base));
}

#[test]
fn committed_pin_freezes_the_card_sampler_recipe() {
    let s = pin().sampler;
    assert_eq!((s.width, s.height), (1024, 1024));
    assert_eq!(s.num_inference_steps, 50);
    assert!((s.guidance_scale - 4.0).abs() < f64::EPSILON);
    assert!((s.flow_shift - 3.0).abs() < f64::EPSILON);
    assert_eq!(s.num_frames, 1);
    assert_eq!(s.dtype, "bfloat16");
}

#[test]
fn public_split_is_frozen_and_large_enough() {
    let p = pin();
    assert_eq!(p.frozen_prompts.len(), p.prompts.public_ids.len());
    assert!(p.frozen_prompts.len() >= 40);
    for record in &p.frozen_prompts {
        assert!(is_bench_prompt_id(record.id), "id {} off-bench", record.id);
        assert!(
            record.generator_input().len() > 40,
            "prompt {} looks like a placeholder: {:?}",
            record.id,
            record.generator_input()
        );
    }
    let cells = p.seed_cells(&p.prompts.public_ids);
    assert!(cells.len() >= MIN_SCORED_CELLS, "{} cells", cells.len());
}

#[test]
fn every_public_cell_has_a_distinct_frozen_seed() {
    let p = pin();
    let cells = p.seed_cells(&p.prompts.public_ids);
    let mut seeds: Vec<u64> = cells.iter().map(|c| c.seed).collect();
    let total = seeds.len();
    seeds.sort_unstable();
    seeds.dedup();
    assert_eq!(seeds.len(), total, "seed collision across public cells");
}

#[test]
fn holdout_is_committed_but_not_published() {
    let p = pin();
    assert_eq!(p.prompts.holdout_commitment.len(), 64);
    assert!(p.prompts.holdout_size >= 25);
    let cells = p
        .prompts
        .holdout_size
        .saturating_mul(p.prompts.variations_per_prompt as usize);
    assert!(cells >= MIN_SCORED_CELLS, "{cells} holdout cells");

    // No holdout prompt record may appear in git.
    let body = std::fs::read_to_string(pin_path()).expect("read pin");
    let public: std::collections::BTreeSet<u32> = p.prompts.public_ids.iter().copied().collect();
    for record in &p.frozen_prompts {
        assert!(public.contains(&record.id));
    }
    assert!(!body.contains("holdout_prompt"));
    assert!(!body.contains("holdout_ids"));
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
fn flux_never_appears_as_a_pinned_base() {
    let body = std::fs::read_to_string(pin_path()).expect("read pin");
    for line in body.lines() {
        let t = line.trim();
        if t.starts_with("base =") || t.starts_with("base=") {
            assert!(!base_is_rejected(t), "pinned base line is a refused family");
        }
    }
}
