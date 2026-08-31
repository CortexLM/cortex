//! Relearn challenge identity and verified model pins.
//!
//! ```text
//! challenge_id     = "relearn"
//! scoring_version  = 1
//! task_id domain   = b"base-relearn-task-id-v1"
//! receipt domain   = b"base-relearn-receipt-v1"
//! ```
//!
//! Distinct from `design` / `prism` so leaf digests never collide.
//! Master-centralized Lium eval; miners pay Lium.

#![forbid(unsafe_code)]
#![allow(clippy::doc_markdown, clippy::module_name_repetitions)]

mod holdout;

pub use holdout::{
    contamination, holdout_commitment, near_duplicates, verify_holdout_items, HoldoutError,
    HoldoutItem, HoldoutTask, MIN_HOLDOUT_ITEMS, NGRAM_JACCARD_MAX,
};

/// Normative challenge id (trust-root / leaf `challenge_id` string).
pub const CHALLENGE_ID: &str = "relearn";

/// UTF-8 bytes of [`CHALLENGE_ID`].
pub const CHALLENGE_ID_BYTES: &[u8] = b"relearn";

/// Live `challenge_scoring_version` (displacement vs champion + gates).
pub const SCORING_VERSION: u16 = 1;

/// Domain tag for task id digests.
pub const TASK_ID_DOMAIN: &[u8] = b"base-relearn-task-id-v1";

/// Domain tag for holdout slice ids.
pub const HOLDOUT_DOMAIN: &[u8] = b"base-relearn-holdout-v1";

/// Domain tag for eval-receipt digests.
pub const RECEIPT_DOMAIN: &[u8] = b"base-relearn-receipt-v1";

/// Domain tag for promotion attestations.
pub const PROMOTE_DOMAIN: &[u8] = b"base-relearn-promote-v1";

/// Integer score lattice max (same scale as other challenges).
pub const SCORE_MAX: u64 = 1_000_000;

/// Verified Hugging Face id for the base model miners improve.
///
/// Confirmed 2026-08-29: <https://huggingface.co/Qwen/Qwen3.8-Flash-Next>
pub const BASE_MODEL_ID: &str = "Qwen/Qwen3.8-Flash-Next";

/// Wire id for the v0 HTTP teacher. Override with `RELEARN_TEACHER_MODEL`.
pub const TEACHER_MODEL_ID: &str = "kimi-k3";

/// Hugging Face-style alias some OpenAI-compatible hosts use.
pub const TEACHER_MODEL_HF_ALIAS: &str = "moonshotai/Kimi-K3";

/// Frozen GLM teacher — optional override, not the v0 default.
pub const TEACHER_GLM_MODEL_ID: &str = "zai-org/GLM-5.3";

/// Community NVFP4 checkpoint (optional Lium serve; not the v0 default).
pub const TEACHER_NVFP4_ID: &str = "Inferact/GLM-5.3-NVFP4";

/// Public miner / eval-image repo.
pub const RELEARN_GIT_URL: &str = "https://github.com/CortexLM/relearn";

/// Teacher serving mode. v0 default is a teacher-only HTTP API.
/// Miner weights are never served through the teacher API.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TeacherBackend {
    /// Optional NVFP4 on a digest-pinned Lium pod (`RELEARN_TEACHER_BACKEND=lium`).
    LiumNvfp4,
    /// Teacher-only OpenAI-compatible HTTP API (v0 default).
    #[default]
    HttpApi,
    /// Deterministic offline judge (CI / `RELEARN_FORCE_SIM`).
    Sim,
}

impl TeacherBackend {
    /// Parse from env (`RELEARN_TEACHER_BACKEND`). Empty → HTTP API.
    #[must_use]
    pub fn from_env() -> Self {
        match std::env::var("RELEARN_TEACHER_BACKEND")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "lium" | "lium_nvfp4" | "nvfp4" => Self::LiumNvfp4,
            "sim" => Self::Sim,
            _ => Self::HttpApi,
        }
    }
}

/// Default teacher backend for v0: HTTP API unless sim is forced.
#[must_use]
pub fn default_teacher_backend(force_sim: bool) -> TeacherBackend {
    if force_sim {
        TeacherBackend::Sim
    } else {
        TeacherBackend::HttpApi
    }
}

/// `RELEARN_TEACHER_API_URL` when set. No baked host — missing means skip/sim.
#[must_use]
pub fn teacher_api_url() -> Option<String> {
    std::env::var("RELEARN_TEACHER_API_URL")
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

/// `RELEARN_TEACHER_MODEL`, or [`TEACHER_MODEL_ID`].
#[must_use]
pub fn teacher_model_from_env() -> String {
    std::env::var("RELEARN_TEACHER_MODEL")
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| TEACHER_MODEL_ID.to_owned())
}

/// Bearer for the teacher HTTP API. Never log the value.
///
/// Reads `RELEARN_TEACHER_API_KEY` only.
#[must_use]
pub fn teacher_api_key() -> Option<String> {
    std::env::var("RELEARN_TEACHER_API_KEY")
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

/// True when `model` is the configured HTTP teacher or the optional GLM pin.
#[must_use]
pub fn is_configured_teacher_model(model: &str) -> bool {
    let m = model.trim();
    if m.is_empty() {
        return false;
    }
    m == TEACHER_MODEL_ID
        || m.eq_ignore_ascii_case(TEACHER_MODEL_HF_ALIAS)
        || m == TEACHER_GLM_MODEL_ID
        || m == teacher_model_from_env()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn challenge_id_is_relearn() {
        assert_eq!(CHALLENGE_ID, "relearn");
        assert_eq!(CHALLENGE_ID_BYTES, b"relearn");
        assert_ne!(CHALLENGE_ID, "prism");
        assert_ne!(CHALLENGE_ID, "design");
    }

    #[test]
    fn verified_model_ids() {
        assert_eq!(BASE_MODEL_ID, "Qwen/Qwen3.8-Flash-Next");
        assert_eq!(TEACHER_MODEL_ID, "kimi-k3");
        assert_eq!(TEACHER_GLM_MODEL_ID, "zai-org/GLM-5.3");
        assert!(teacher_api_url().is_none());
    }

    #[test]
    fn teacher_backend_defaults_to_http_api() {
        assert_eq!(default_teacher_backend(false), TeacherBackend::HttpApi);
        assert_eq!(default_teacher_backend(true), TeacherBackend::Sim);
    }

    #[test]
    fn kimi_and_glm_are_configured_teachers() {
        assert!(is_configured_teacher_model("kimi-k3"));
        assert!(is_configured_teacher_model("moonshotai/Kimi-K3"));
        assert!(is_configured_teacher_model(TEACHER_GLM_MODEL_ID));
        assert!(!is_configured_teacher_model(""));
    }

    #[test]
    fn domain_tags_are_relearn_prefixed() {
        assert!(std::str::from_utf8(TASK_ID_DOMAIN)
            .unwrap_or("")
            .contains("relearn"));
        assert!(!std::str::from_utf8(TASK_ID_DOMAIN)
            .unwrap_or("")
            .contains("prism"));
    }
}
