//! Proof challenge identity, signed topic documents, and holdout commitments.
//!
//! ```text
//! challenge_id     = "proof"
//! scoring_version  = 1
//! topic domain     = b"base-proof-topic-v1"
//! holdout domain   = b"base-proof-holdout-v1"
//! receipt domain   = b"base-proof-receipt-v1"
//! ```
//!
//! Distinct from other challenge ids (`bounty`, and historical `relearn*` /
//! `design` / `prism`) so leaf digests never collide.
//!
//! The **problem itself is data**. A Proof unit of work is a
//! [`TopicDocument`]: an operator-published research problem — statement,
//! machine-checkable constraints, a metric family, a FLOP/wall budget, a
//! *sealed* baseline recipe, and a holdout commitment — signed by the same key
//! that signs this challenge's leaves. Miners submit a reproducible experiment
//! against a `topic_id`, and a digest-pinned RLM agent re-runs the recipe.
//!
//! So git carries no problem catalog. It carries the global floors a topic may
//! tighten but never loosen ([`ProofPin`]), and nothing about any holdout
//! except its commitment. An unsigned topic, a topic whose baseline is not
//! sealed, and a topic whose holdout records are absent are all the same
//! answer: this host cannot score, and nobody is paid.

#![forbid(unsafe_code)]
#![allow(clippy::doc_markdown, clippy::module_name_repetitions)]

mod canonical;
mod holdout;
mod pin;
mod topic;

pub use canonical::canonical_json;
pub use holdout::{
    contamination, holdout_commitment, synthetic_holdout, verify_holdout, HoldoutError,
    HoldoutRecord, HoldoutSplit, LONGCTX_MAX_TOKENS, LONGCTX_MIN_TOKENS,
};
pub use pin::{PinError, ProofPin};
pub use topic::{
    default_adamw, topic_signing_payload, Baseline, Constraints, DiscoverySpec, MetricDirection,
    MetricFamily, MetricSpec, PayoutMode, TopicDocument, TopicError, TopicStatus, ValidationSpec,
    BPS_DENOM, DISCOVERY_NOVELTY_POOL_SHARE_BPS, DISCOVERY_PASS_FLOOR_SHARE_BPS, MAX_STATEMENT_LEN,
    MAX_TOPIC_ID_LEN, MAX_VALIDATION_LEN, METRIC_STEP_LATENCY_MS, METRIC_TOKENS_PER_SEC,
    MIN_TOPIC_ID_LEN, PRIMARY_HOLDOUT_NLL, TOPIC_SCHEMA_VERSION,
};

/// Normative challenge id (trust-root / leaf `challenge_id` string).
pub const CHALLENGE_ID: &str = "proof";

/// UTF-8 bytes of [`CHALLENGE_ID`].
pub const CHALLENGE_ID_BYTES: &[u8] = b"proof";

/// Live `challenge_scoring_version` (agent-reproduced experiment + harness gates).
pub const SCORING_VERSION: u16 = 1;

/// Domain tag for task id digests.
pub const TASK_ID_DOMAIN: &[u8] = b"base-proof-task-id-v1";

/// Domain tag for per-topic holdout commitments.
pub const HOLDOUT_DOMAIN: &[u8] = b"base-proof-holdout-v1";

/// Domain tag for eval-receipt digests.
pub const RECEIPT_DOMAIN: &[u8] = b"base-proof-receipt-v1";

/// Domain tag for the sealed baseline metric vector commitment.
pub const BASELINE_DOMAIN: &[u8] = b"base-proof-baseline-v1";

/// Signature domain for operator-published topic documents.
///
/// The signing key is the `proof` row's key in `config/challenges.toml` — the
/// same key that signs this challenge's weight leaves — so a topic nobody with
/// that key published cannot be scored. That key is sr25519 (every trust-root
/// row is), so the document signature is sr25519 over the canonical JSON bytes
/// under this tag, not ed25519: a signature scheme the row's key cannot
/// produce would need a second key, and a second key is a second trust root.
pub const TOPIC_DOMAIN: crypto::DomainTag = crypto::DomainTag::new(b"base-proof-topic-v1");

/// Integer score lattice max (same scale as every other challenge).
pub const SCORE_MAX: u64 = 1_000_000;

/// Base model family the proxy must belong to (same family as `relearn`).
///
/// Family lock for the RLM judge the eval image bakes. The exact judge id
/// lives in [`ProofPin::proxy_model`] and must be a model the pinned image
/// contains. This is not a miner training proxy.
pub const BASE_MODEL_FAMILY: &str = "Qwen/Qwen3.8";

/// Eval image repository (digest-pinned; never a floating tag in prod).
pub const EVAL_IMAGE: &str = "ghcr.io/cortexlm/proof-eval";

/// Public docs pointer (this control-plane repo).
pub const PROOF_GIT_URL: &str = "https://github.com/CortexLM/cortex";

/// Custom metric id for the agent-harness success-rate topic. Listed so an
/// operator can publish the document; the eval image fail-closes until a
/// real harness fills `custom_value`.
pub const CUSTOM_HARNESS_SUCCESS_RATE: &str = "harness_success_rate";

/// Proof challenge emission share (basis points of the subnet).
pub const PROOF_EMISSION_BPS: u16 = 8_000;

/// Bounty challenge emission share (basis points of the subnet).
pub const BOUNTY_EMISSION_BPS: u16 = 2_000;

/// Largest FLOP budget any topic may declare.
pub const FLOPS_BUDGET_MAX: u64 = 2_000_000_000_000_000_000;

/// Floor on a topic's `epsilon_nll` (absolute NLL a challenger must win by).
pub const EPSILON_NLL_MIN: f64 = 0.02;

/// Floor on a topic's `epsilon_topic_max_regress` (per-split NLL regression).
pub const EPSILON_TOPIC_MAX_REGRESS_MIN: f64 = 0.05;

/// Floor on a throughput topic's relative win over the sealed comms baseline.
pub const EPSILON_THROUGHPUT_REL_MIN: f64 = 0.05;

/// Quality floor a throughput topic must keep: speed is not free.
pub const QUALITY_FLOOR_NLL_MAX: f64 = 0.02;

/// Holdout records per topic.
pub const HOLDOUT_SIZE: usize = 120;

/// Records per scored split (`HOLDOUT_SIZE / scored splits`).
pub const STRATUM_SIZE: usize = 24;

/// Slice id prefix bound into per-topic measurements.
pub const HOLDOUT_SLICE_PREFIX: &str = "proof-holdout";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn challenge_id_is_distinct() {
        assert_eq!(CHALLENGE_ID, "proof");
        assert_eq!(CHALLENGE_ID_BYTES, b"proof");
        for other in [
            "relearn",
            "relearn-image",
            "relearn-agent",
            "relearn-mm",
            "bounty",
        ] {
            assert_ne!(CHALLENGE_ID, other);
        }
    }

    #[test]
    fn domain_tags_are_proof_prefixed_and_unique() {
        let tags: [&[u8]; 5] = [
            TASK_ID_DOMAIN,
            HOLDOUT_DOMAIN,
            RECEIPT_DOMAIN,
            BASELINE_DOMAIN,
            TOPIC_DOMAIN.as_bytes(),
        ];
        for t in tags {
            let s = std::str::from_utf8(t).unwrap_or("");
            assert!(s.contains("proof"), "{s}");
        }
        for i in 0..tags.len() {
            for j in (i + 1)..tags.len() {
                assert_ne!(tags[i], tags[j]);
            }
        }
    }

    /// The stratification is what makes the per-split regression gate
    /// meaningful: 120 records over 5 scored splits, 24 each.
    #[test]
    fn holdout_stratification_is_exact() {
        assert_eq!(HOLDOUT_SIZE, STRATUM_SIZE * HoldoutSplit::SCORED.len());
        assert_eq!(STRATUM_SIZE, 24);
        assert!(!HoldoutSplit::SCORED.contains(&HoldoutSplit::CanaryOffpath));
    }

    #[test]
    fn global_floors_match_the_locked_pin() {
        assert_eq!(FLOPS_BUDGET_MAX, 2_000_000_000_000_000_000);
        assert!((EPSILON_NLL_MIN - 0.02).abs() < 1e-12);
        assert!((EPSILON_TOPIC_MAX_REGRESS_MIN - 0.05).abs() < 1e-12);
        assert!((EPSILON_THROUGHPUT_REL_MIN - 0.05).abs() < 1e-12);
        assert!((QUALITY_FLOOR_NLL_MAX - 0.02).abs() < 1e-12);
        assert_eq!(PROOF_EMISSION_BPS, 8_000);
        assert_eq!(BOUNTY_EMISSION_BPS, 2_000);
        assert_eq!(
            u32::from(PROOF_EMISSION_BPS) + u32::from(BOUNTY_EMISSION_BPS),
            10_000
        );
        assert_eq!(CUSTOM_HARNESS_SUCCESS_RATE, "harness_success_rate");
    }
}
