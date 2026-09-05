//! Global Proof pin: eval image digest, inference ceilings, and the floors a
//! topic may tighten but never loosen.
//!
//! There is **no topic catalog here** and **no live InferenceOffer**. Problems
//! are operator-published signed documents ([`crate::TopicDocument`]); the
//! master's **RLM judge** backend lives in operator state. Git carries only
//! what every topic is measured against: which image may score, which key may
//! publish, which judge modes/token ceilings are legal, and how generous a
//! topic is allowed to be. No secrets.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    is_hex64, is_http_origin, InferenceMode, PinInference, ALLOWED_MODES, BASE_MODEL_FAMILY,
    CHALLENGE_ID, EPSILON_NLL_MIN, EPSILON_THROUGHPUT_REL_MIN, EPSILON_TOPIC_MAX_REGRESS_MIN,
    EVAL_IMAGE, FLOPS_BUDGET_MAX, HOLDOUT_SIZE, INFERENCE_CONFIG_SCHEMA_VERSION,
    INFERENCE_OFFER_COMMITMENT_ALG, MAX_INPUT_TOKENS_CEILING, MAX_OUTPUT_TOKENS_CEILING,
    PROOF_GIT_URL, QUALITY_FLOOR_NLL_MAX, SCORING_VERSION, STRATUM_SIZE,
};

/// `config/proof-pin.toml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProofPin {
    /// Must be `proof`.
    pub challenge_id: String,
    /// Must equal [`SCORING_VERSION`].
    pub scoring_version: u16,
    /// Research family name. Not an architecture lock and not an HF bake.
    pub base_model_family: String,
    /// Deprecated HF proxy id. Must stay empty — Proof does not bake weights.
    #[serde(default)]
    pub proxy_model: String,
    /// Deprecated HF proxy list. Must stay empty.
    #[serde(default)]
    pub proxy_models: Vec<String>,
    /// Inference config schema (`1`).
    pub inference_config_schema_version: u32,
    /// Modes a topic / offer may name. Subset of chat, completions, embeddings.
    pub allowed_modes: Vec<InferenceMode>,
    /// Largest input cap a topic or offer may declare.
    pub max_input_tokens_ceiling: u32,
    /// Largest output cap a topic or offer may declare.
    pub max_output_tokens_ceiling: u32,
    /// Hash algorithm for `config_commitment` (`sha256`).
    pub inference_offer_commitment_alg: String,
    /// Complete provider defaults. Empty model/url is pre-launch fail-closed.
    pub inference: PinInference,
    /// Eval image reference (no floating tag in prod).
    pub eval_image: String,
    /// `sha256:…` digest. Empty until the first green proof-eval CI image.
    pub eval_image_digest: String,
    /// Public docs pointer for miners.
    pub proof_git: String,
    /// Pinned git SHA of the docs / harness repo.
    pub proof_git_sha: String,
    /// Hex public key topic documents must verify under (the `proof` row).
    pub topic_pubkey: String,
    /// Largest FLOP budget a topic may declare.
    pub flops_budget_max: u64,
    /// Floor on a topic's absolute NLL epsilon.
    pub epsilon_nll_min: f64,
    /// Floor on a topic's per-split NLL regression tolerance.
    pub epsilon_topic_max_regress_min: f64,
    /// Floor on a throughput topic's relative win.
    pub epsilon_throughput_rel_min: f64,
    /// Largest NLL a throughput topic may trade for speed.
    pub quality_floor_nll_max: f64,
    /// Holdout records per topic.
    pub holdout_size: usize,
    /// Records per scored split.
    pub stratum_size: usize,
}

impl Default for ProofPin {
    fn default() -> Self {
        Self {
            challenge_id: CHALLENGE_ID.into(),
            scoring_version: SCORING_VERSION,
            base_model_family: BASE_MODEL_FAMILY.into(),
            proxy_model: String::new(),
            proxy_models: Vec::new(),
            inference_config_schema_version: INFERENCE_CONFIG_SCHEMA_VERSION,
            allowed_modes: ALLOWED_MODES.to_vec(),
            max_input_tokens_ceiling: MAX_INPUT_TOKENS_CEILING,
            max_output_tokens_ceiling: MAX_OUTPUT_TOKENS_CEILING,
            inference_offer_commitment_alg: INFERENCE_OFFER_COMMITMENT_ALG.into(),
            inference: PinInference::default(),
            eval_image: EVAL_IMAGE.into(),
            eval_image_digest: String::new(),
            proof_git: PROOF_GIT_URL.into(),
            proof_git_sha: String::new(),
            topic_pubkey: String::new(),
            flops_budget_max: FLOPS_BUDGET_MAX,
            epsilon_nll_min: EPSILON_NLL_MIN,
            epsilon_topic_max_regress_min: EPSILON_TOPIC_MAX_REGRESS_MIN,
            epsilon_throughput_rel_min: EPSILON_THROUGHPUT_REL_MIN,
            quality_floor_nll_max: QUALITY_FLOOR_NLL_MAX,
            holdout_size: HOLDOUT_SIZE,
            stratum_size: STRATUM_SIZE,
        }
    }
}

/// Why a pin was refused.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum PinError {
    /// TOML did not parse.
    #[error("parse proof pin: {0}")]
    Parse(String),
    /// The pin claims to be another challenge.
    #[error("challenge_id {got:?} is not {want:?}")]
    WrongChallenge {
        /// What the pin said.
        got: String,
        /// What this crate scores.
        want: &'static str,
    },
    /// Scoring version drift would change consensus meaning silently.
    #[error("scoring_version {got}, this build scores {want}")]
    WrongScoringVersion {
        /// What the pin said.
        got: u16,
        /// What this build implements.
        want: u16,
    },
    /// The pin points the challenge at another image repository.
    #[error("eval_image {got:?} is not {want:?}")]
    WrongEvalImage {
        /// What the pin said.
        got: String,
        /// The only image that may score.
        want: &'static str,
    },
    /// The research family name drifted.
    #[error("base_model_family {got:?} is not {want:?}")]
    WrongFamily {
        /// What the pin said.
        got: String,
        /// Locked family name.
        want: &'static str,
    },
    /// A non-empty HF proxy would reintroduce a bake lock.
    #[error("proxy_model / proxy_models must stay empty (no HF bake)")]
    DeprecatedProxyBake,
    /// Inference schema version drift.
    #[error("inference_config_schema_version {got}, this build reads {want}")]
    WrongInferenceSchema {
        /// What the pin said.
        got: u32,
        /// What this build reads.
        want: u32,
    },
    /// Commitment algorithm is not sha256.
    #[error("inference_offer_commitment_alg {got:?} is not {want:?}")]
    WrongCommitmentAlg {
        /// What the pin said.
        got: String,
        /// Locked algorithm.
        want: &'static str,
    },
    /// `allowed_modes` is empty or names something this build does not score.
    #[error("allowed_modes must be a non-empty subset of chat, completions, embeddings")]
    BadAllowedModes,
    /// A token ceiling was raised above the crate lock, or is zero.
    #[error("{0} = {1} must be 1..={2}")]
    TokenCeiling(&'static str, u32, u32),
    /// `[inference].base_url` is set but is not an http(s) origin.
    #[error("inference.base_url must be empty (secret-backed) or an http(s) origin")]
    BadInferenceUrl,
    /// The topic key is not a 64-hex sr25519 public key.
    #[error("topic_pubkey must be 64 hex chars (the challenges.toml `proof` row key)")]
    BadTopicPubkey,
    /// A floor was loosened.
    #[error("{field} = {got} loosens the locked floor {floor}")]
    LoosenedFloor {
        /// Which knob.
        field: &'static str,
        /// Pin value.
        got: f64,
        /// Locked floor.
        floor: f64,
    },
    /// The FLOP ceiling was raised.
    #[error("flops_budget_max {got} exceeds the locked {max}")]
    BudgetTooLarge {
        /// Pin value.
        got: u64,
        /// Locked ceiling.
        max: u64,
    },
    /// The stratification no longer covers the holdout exactly.
    #[error("holdout_size {size} is not {strata} scored splits x stratum_size {stratum}")]
    BadStratification {
        /// Declared holdout size.
        size: usize,
        /// Declared per-split size.
        stratum: usize,
        /// Scored split count.
        strata: usize,
    },
}

impl ProofPin {
    /// Load from `config/proof-pin.toml`.
    ///
    /// # Errors
    ///
    /// [`PinError::Parse`] on malformed TOML. Call [`Self::validate`] before boot.
    pub fn from_toml(body: &str) -> Result<Self, PinError> {
        toml::from_str(body).map_err(|e| PinError::Parse(e.to_string()))
    }

    /// Enforce identity, the image lock, the family lock, and every floor.
    ///
    /// # Errors
    ///
    /// See [`PinError`]. Every case is fail-closed: a host with a pin it
    /// cannot validate refuses to boot rather than scoring under looser rules
    /// than the trust root was signed for.
    pub fn validate(&self) -> Result<(), PinError> {
        if self.challenge_id.trim() != CHALLENGE_ID {
            return Err(PinError::WrongChallenge {
                got: self.challenge_id.clone(),
                want: CHALLENGE_ID,
            });
        }
        if self.scoring_version != SCORING_VERSION {
            return Err(PinError::WrongScoringVersion {
                got: self.scoring_version,
                want: SCORING_VERSION,
            });
        }
        if self.eval_image.trim() != EVAL_IMAGE {
            return Err(PinError::WrongEvalImage {
                got: self.eval_image.clone(),
                want: EVAL_IMAGE,
            });
        }
        if self.base_model_family.trim() != BASE_MODEL_FAMILY {
            return Err(PinError::WrongFamily {
                got: self.base_model_family.clone(),
                want: BASE_MODEL_FAMILY,
            });
        }
        self.validate_inference()?;
        if !is_hex64(&self.topic_pubkey) {
            return Err(PinError::BadTopicPubkey);
        }
        if self.flops_budget_max == 0 || self.flops_budget_max > FLOPS_BUDGET_MAX {
            return Err(PinError::BudgetTooLarge {
                got: self.flops_budget_max,
                max: FLOPS_BUDGET_MAX,
            });
        }
        for (field, got, floor) in [
            ("epsilon_nll_min", self.epsilon_nll_min, EPSILON_NLL_MIN),
            (
                "epsilon_topic_max_regress_min",
                self.epsilon_topic_max_regress_min,
                EPSILON_TOPIC_MAX_REGRESS_MIN,
            ),
            (
                "epsilon_throughput_rel_min",
                self.epsilon_throughput_rel_min,
                EPSILON_THROUGHPUT_REL_MIN,
            ),
        ] {
            if got.is_nan() || got < floor {
                return Err(PinError::LoosenedFloor { field, got, floor });
            }
        }
        // The quality floor is a ceiling on traded NLL: a larger number lets
        // a throughput topic sell more quality for speed, so it may only
        // shrink.
        if self.quality_floor_nll_max.is_nan() || self.quality_floor_nll_max > QUALITY_FLOOR_NLL_MAX
        {
            return Err(PinError::LoosenedFloor {
                field: "quality_floor_nll_max",
                got: self.quality_floor_nll_max,
                floor: QUALITY_FLOOR_NLL_MAX,
            });
        }
        let strata = crate::HoldoutSplit::SCORED.len();
        if self.stratum_size == 0
            || self.holdout_size != self.stratum_size.saturating_mul(strata)
            || self.holdout_size != HOLDOUT_SIZE
        {
            return Err(PinError::BadStratification {
                size: self.holdout_size,
                stratum: self.stratum_size,
                strata,
            });
        }
        Ok(())
    }

    fn validate_inference(&self) -> Result<(), PinError> {
        if !self.proxy_model.trim().is_empty()
            || self.proxy_models.iter().any(|m| !m.trim().is_empty())
        {
            return Err(PinError::DeprecatedProxyBake);
        }
        if self.inference_config_schema_version != INFERENCE_CONFIG_SCHEMA_VERSION {
            return Err(PinError::WrongInferenceSchema {
                got: self.inference_config_schema_version,
                want: INFERENCE_CONFIG_SCHEMA_VERSION,
            });
        }
        if self.inference_offer_commitment_alg.trim() != INFERENCE_OFFER_COMMITMENT_ALG {
            return Err(PinError::WrongCommitmentAlg {
                got: self.inference_offer_commitment_alg.clone(),
                want: INFERENCE_OFFER_COMMITMENT_ALG,
            });
        }
        if self.allowed_modes.is_empty() {
            return Err(PinError::BadAllowedModes);
        }
        let mut seen = Vec::new();
        for mode in &self.allowed_modes {
            if seen.contains(mode) || !ALLOWED_MODES.contains(mode) {
                return Err(PinError::BadAllowedModes);
            }
            seen.push(*mode);
        }
        if !self.allows_mode(self.inference.mode) {
            return Err(PinError::BadAllowedModes);
        }
        let inf = &self.inference;
        for (field, got, ceiling) in [
            (
                "max_input_tokens_ceiling",
                self.max_input_tokens_ceiling,
                MAX_INPUT_TOKENS_CEILING,
            ),
            (
                "max_output_tokens_ceiling",
                self.max_output_tokens_ceiling,
                MAX_OUTPUT_TOKENS_CEILING,
            ),
            (
                "inference.max_input_tokens",
                inf.max_input_tokens,
                self.max_input_tokens_ceiling,
            ),
            (
                "inference.max_output_tokens",
                inf.max_output_tokens,
                self.max_output_tokens_ceiling,
            ),
        ] {
            if got == 0 || got > ceiling {
                return Err(PinError::TokenCeiling(field, got, ceiling));
            }
        }
        if (!inf.base_url.trim().is_empty() && !is_http_origin(&inf.base_url))
            || inf.model.len() > 256
        {
            return Err(PinError::BadInferenceUrl);
        }
        Ok(())
    }

    /// True when a live rent is allowed (real digest pin present).
    ///
    /// An empty digest is the normal pre-launch state and the reason submits
    /// answer `503`. There is no sim fallback on this challenge.
    pub fn can_rent(&self) -> bool {
        let d = self.eval_image_digest.trim();
        d.starts_with("sha256:") && d.len() >= 71
    }

    /// Topic-signing public key bytes, when the pin carries a well-formed one.
    pub fn topic_pubkey_bytes(&self) -> Option<[u8; 32]> {
        let raw = hex::decode(self.topic_pubkey.trim()).ok()?;
        <[u8; 32]>::try_from(raw).ok()
    }

    /// Whether `mode` is in this pin's allowlist.
    pub fn allows_mode(&self, mode: InferenceMode) -> bool {
        self.allowed_modes.contains(&mode)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pin() -> ProofPin {
        ProofPin {
            topic_pubkey: "ab".repeat(32),
            ..ProofPin::default()
        }
    }

    #[test]
    fn a_pin_with_no_image_digest_validates_but_cannot_rent() {
        let p = pin();
        p.validate().expect("floors are the defaults");
        assert!(
            !p.can_rent(),
            "no published proof-eval digest yet; submits must 503"
        );
    }

    #[test]
    fn toml_round_trip() {
        let body = format!(
            r#"
challenge_id = "proof"
scoring_version = 1
base_model_family = "{BASE_MODEL_FAMILY}"
inference_config_schema_version = 1
allowed_modes = ["chat", "completions", "embeddings"]
max_input_tokens_ceiling = 32768
max_output_tokens_ceiling = 8192
inference_offer_commitment_alg = "sha256"
eval_image = "{EVAL_IMAGE}"
eval_image_digest = ""
topic_pubkey = "{}"
flops_budget_max = 2000000000000000000
epsilon_nll_min = 0.02
epsilon_topic_max_regress_min = 0.05
holdout_size = 120
stratum_size = 24
"#,
            "cd".repeat(32)
        );
        let p = ProofPin::from_toml(&body).expect("parse");
        p.validate().expect("validates");
        assert_eq!(p.flops_budget_max, 2_000_000_000_000_000_000);
        assert_eq!(p.max_input_tokens_ceiling, 32_768);
        assert_eq!(p.topic_pubkey_bytes().expect("key"), [0xcd; 32]);
        assert!(p.proxy_model.is_empty());
        assert!(p.proxy_models.is_empty());
    }

    #[test]
    fn a_pin_that_loosens_a_floor_is_refused() {
        for (field, mutate) in [
            ("epsilon_nll_min", 0usize),
            ("epsilon_topic_max_regress_min", 1),
            ("epsilon_throughput_rel_min", 2),
            ("quality_floor_nll_max", 3),
        ] {
            let mut p = pin();
            match mutate {
                0 => p.epsilon_nll_min = 0.01,
                1 => p.epsilon_topic_max_regress_min = 0.04,
                2 => p.epsilon_throughput_rel_min = 0.01,
                _ => p.quality_floor_nll_max = 0.5,
            }
            assert!(
                matches!(p.validate(), Err(PinError::LoosenedFloor { .. })),
                "{field} must not be loosened"
            );
        }
    }

    #[test]
    fn a_pin_that_raises_the_flop_ceiling_or_swaps_the_image_is_refused() {
        let mut big = pin();
        big.flops_budget_max = FLOPS_BUDGET_MAX + 1;
        assert!(matches!(
            big.validate(),
            Err(PinError::BudgetTooLarge { .. })
        ));

        let mut other = pin();
        other.eval_image = "ghcr.io/cortexlm/relearn-eval".into();
        assert!(matches!(
            other.validate(),
            Err(PinError::WrongEvalImage { .. })
        ));
    }

    #[test]
    fn a_named_hf_proxy_bake_is_refused() {
        let mut p = pin();
        p.proxy_model = "Qwen/Qwen3.8-0.6B".into();
        assert!(matches!(p.validate(), Err(PinError::DeprecatedProxyBake)));
        p.proxy_model.clear();
        p.proxy_models = vec!["Qwen/Qwen3-0.6B".into()];
        assert!(matches!(p.validate(), Err(PinError::DeprecatedProxyBake)));
        p.proxy_models.clear();
        p.validate().expect("empty deprecated fields");
    }

    #[test]
    fn inference_schema_and_ceilings_are_locked() {
        let mut p = pin();
        p.inference_config_schema_version = 2;
        assert!(matches!(
            p.validate(),
            Err(PinError::WrongInferenceSchema { .. })
        ));
        p = pin();
        p.inference_offer_commitment_alg = "blake3".into();
        assert!(matches!(
            p.validate(),
            Err(PinError::WrongCommitmentAlg { .. })
        ));
        p = pin();
        p.allowed_modes.clear();
        assert!(matches!(p.validate(), Err(PinError::BadAllowedModes)));
        p = pin();
        p.max_input_tokens_ceiling = MAX_INPUT_TOKENS_CEILING + 1;
        assert!(matches!(p.validate(), Err(PinError::TokenCeiling(..))));
        p = pin();
        p.allowed_modes = vec![InferenceMode::Chat];
        p.validate().expect("subset is a tighten");
        assert!(p.allows_mode(InferenceMode::Chat));
        assert!(!p.allows_mode(InferenceMode::Embeddings));
        p = pin();
        p.inference.base_url = "not-a-url".into();
        assert!(matches!(p.validate(), Err(PinError::BadInferenceUrl)));
        p = pin();
        p.inference.base_url = "https://example.invalid/v1".into();
        p.validate().expect("public origin may live in the pin");
    }

    #[test]
    fn identity_and_stratification_are_pinned() {
        let mut wrong_id = pin();
        wrong_id.challenge_id = "relearn-proof".into();
        assert!(matches!(
            wrong_id.validate(),
            Err(PinError::WrongChallenge { .. })
        ));

        let mut wrong_version = pin();
        wrong_version.scoring_version = 2;
        assert!(matches!(
            wrong_version.validate(),
            Err(PinError::WrongScoringVersion { .. })
        ));

        let mut thin = pin();
        thin.stratum_size = 10;
        assert!(matches!(
            thin.validate(),
            Err(PinError::BadStratification { .. })
        ));
    }

    #[test]
    fn a_pin_without_a_topic_key_cannot_verify_any_topic() {
        let mut p = pin();
        p.topic_pubkey = String::new();
        assert!(matches!(p.validate(), Err(PinError::BadTopicPubkey)));
        assert!(p.topic_pubkey_bytes().is_none());
    }

    #[test]
    fn default_pin_has_the_locked_inference_schema() {
        let p = pin();
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
            crate::InferenceProviderKind::OpenaiCompatible
        );
        assert!(p.inference.base_url.is_empty());
        assert!(p.inference.model.is_empty());
        assert_eq!(p.inference.mode, InferenceMode::Chat);
        assert_eq!(p.inference.max_input_tokens, MAX_INPUT_TOKENS_CEILING);
        assert_eq!(p.inference.max_output_tokens, MAX_OUTPUT_TOKENS_CEILING);
    }

    #[test]
    fn rent_needs_a_full_sha256_digest() {
        let mut pinned = pin();
        pinned.eval_image_digest = format!("sha256:{}", "ab".repeat(32));
        assert!(pinned.can_rent());
        pinned.eval_image_digest = "sha256:abc".into();
        assert!(!pinned.can_rent());
    }
}
