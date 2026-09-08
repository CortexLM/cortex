//! Eval **executor** ceilings: the machine class the digest-pinned
//! `proof-eval` image is rented on, and what a topic may tighten about it.
//!
//! This is the pin side only. The live `EvalExecutorOffer` (operator state,
//! off git) lives in the `proof-executor` crate. It is a sibling of the RLM
//! judge [`crate::InferenceOffer`], never the same document: the judge is
//! *what* scores, the executor is *where* the proof runs.

use serde::{Deserialize, Serialize};

use crate::{is_hex64, ProofPin, TopicError};

/// Pin `eval_executor_schema_version`.
pub const EVAL_EXECUTOR_SCHEMA_VERSION: u32 = 1;

/// Pin `gpu_class`: the only machine shape a Proof executor may rent.
pub const EVAL_EXECUTOR_GPU_CLASS: &str = "1x";

/// GPUs behind [`EVAL_EXECUTOR_GPU_CLASS`]. Harvest aborts any other width.
pub const EVAL_EXECUTOR_GPU_COUNT: u32 = 1;

/// Pin `max_proof_deadline_s_ceiling`: longest proof deadline an offer or a
/// topic may declare (two hours).
pub const MAX_PROOF_DEADLINE_S_CEILING: u64 = 7_200;

/// Pin `eval_executor_commitment_alg`.
pub const EVAL_EXECUTOR_COMMITMENT_ALG: &str = "sha256";

/// Topic override of the executor contract. **Tighten-only**, and there is
/// deliberately no `machine_id`: a topic names how long a proof may take and
/// which committed executor may run it, never a specific machine.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct TopicEvalExecutor {
    /// Optional 64-hex pin of the live executor offer `config_commitment`.
    /// Not a miner-facing bind.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub require_offer_commitment: Option<String>,
    /// Proof deadline shorter than the pin ceiling (and the live offer).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_proof_deadline_s: Option<u64>,
}

impl TopicEvalExecutor {
    /// True when the topic tightens nothing. Such a topic serializes without
    /// an `eval_executor` key, so documents signed before this field existed
    /// keep verifying.
    pub fn is_empty(&self) -> bool {
        self.require_offer_commitment.is_none() && self.max_proof_deadline_s.is_none()
    }

    /// Tighten-only check against the pin ceiling.
    ///
    /// # Errors
    ///
    /// [`TopicError::BadExecutorCommitment`] or [`TopicError::ExecutorDeadlineCeiling`].
    pub fn validate(&self, pin: &ProofPin) -> Result<(), TopicError> {
        if let Some(need) = self.require_offer_commitment.as_deref() {
            if !is_hex64(need) {
                return Err(TopicError::BadExecutorCommitment);
            }
        }
        if let Some(deadline) = self.max_proof_deadline_s {
            if deadline == 0 || deadline > pin.max_proof_deadline_s_ceiling {
                return Err(TopicError::ExecutorDeadlineCeiling(
                    deadline,
                    pin.max_proof_deadline_s_ceiling,
                ));
            }
        }
        Ok(())
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
    fn locked_executor_constants() {
        assert_eq!(EVAL_EXECUTOR_SCHEMA_VERSION, 1);
        assert_eq!(EVAL_EXECUTOR_GPU_CLASS, "1x");
        assert_eq!(EVAL_EXECUTOR_GPU_COUNT, 1);
        assert_eq!(MAX_PROOF_DEADLINE_S_CEILING, 7_200);
        assert_eq!(EVAL_EXECUTOR_COMMITMENT_ALG, "sha256");
    }

    #[test]
    fn empty_override_validates_and_is_omitted_from_json() {
        let t = TopicEvalExecutor::default();
        assert!(t.is_empty());
        t.validate(&pin()).expect("nothing to tighten");
        assert_eq!(serde_json::to_string(&t).expect("json"), "{}");
    }

    #[test]
    fn topic_may_tighten_the_deadline_never_loosen_it() {
        let mut t = TopicEvalExecutor {
            max_proof_deadline_s: Some(1_800),
            ..TopicEvalExecutor::default()
        };
        t.validate(&pin()).expect("tighten");
        t.max_proof_deadline_s = Some(MAX_PROOF_DEADLINE_S_CEILING);
        t.validate(&pin()).expect("equal to ceiling is legal");
        t.max_proof_deadline_s = Some(MAX_PROOF_DEADLINE_S_CEILING + 1);
        assert!(matches!(
            t.validate(&pin()),
            Err(TopicError::ExecutorDeadlineCeiling(..))
        ));
        t.max_proof_deadline_s = Some(0);
        assert!(matches!(
            t.validate(&pin()),
            Err(TopicError::ExecutorDeadlineCeiling(..))
        ));
    }

    #[test]
    fn a_tightened_pin_ceiling_binds_the_topic() {
        let mut p = pin();
        p.max_proof_deadline_s_ceiling = 3_600;
        p.validate().expect("pin may tighten its own ceiling");
        let t = TopicEvalExecutor {
            max_proof_deadline_s: Some(3_601),
            ..TopicEvalExecutor::default()
        };
        assert!(matches!(
            t.validate(&p),
            Err(TopicError::ExecutorDeadlineCeiling(3_601, 3_600))
        ));
    }

    #[test]
    fn commitment_pin_must_be_hex64() {
        let mut t = TopicEvalExecutor {
            require_offer_commitment: Some("ab".repeat(32)),
            ..TopicEvalExecutor::default()
        };
        t.validate(&pin()).expect("hex64");
        t.require_offer_commitment = Some("not-hex".into());
        assert!(matches!(
            t.validate(&pin()),
            Err(TopicError::BadExecutorCommitment)
        ));
    }

    #[test]
    fn no_per_topic_machine_id() {
        let err = serde_json::from_str::<TopicEvalExecutor>(
            r#"{"machine_id":"pod-123","max_proof_deadline_s":600}"#,
        )
        .expect_err("machine_id is not a topic knob");
        assert!(err.to_string().contains("machine_id"), "{err}");
    }
}
