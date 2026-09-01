//! Relearn Agent challenge identity, frozen episodes, and model pins.
//!
//! ```text
//! challenge_id     = "relearn-agent"
//! scoring_version  = 1
//! task_id domain   = b"base-relearn-agent-task-id-v1"
//! receipt domain   = b"base-relearn-agent-receipt-v1"
//! ```
//!
//! Distinct from `relearn` / `relearn-image` / `bounty` so leaf digests never
//! collide. Master-centralized eval; miners pay Lium.
//!
//! What makes this challenge different from `relearn` is the unit of work. A
//! `relearn` item is a prompt with a reference answer, and a model that has
//! memorised the answer scores. A Relearn Agent **episode** is a goal plus a
//! tool environment, and the only thing that counts is a run that *reached*
//! the answer through the tools: the eval replays the emitted trace and
//! re-runs the same episode with the tools stubbed and with the observation
//! swapped. A model that answers either of those just as well never used the
//! environment, and this challenge does not pay for that.

#![forbid(unsafe_code)]
#![allow(clippy::doc_markdown, clippy::module_name_repetitions)]

mod episode;
mod pin;

pub use episode::{
    contamination, episode_commitment, verify_episodes, AgentEpisode, EpisodeError, ToolKind,
    ToolSchema, TraceStep, MIN_HOLDOUT_EPISODES,
};
pub use pin::{PinError, RelearnAgentPin};

/// Normative challenge id (trust-root / leaf `challenge_id` string).
pub const CHALLENGE_ID: &str = "relearn-agent";

/// UTF-8 bytes of [`CHALLENGE_ID`].
pub const CHALLENGE_ID_BYTES: &[u8] = b"relearn-agent";

/// Live `challenge_scoring_version` (trace-replayed displacement + gates).
pub const SCORING_VERSION: u16 = 1;

/// Domain tag for task id digests.
pub const TASK_ID_DOMAIN: &[u8] = b"base-relearn-agent-task-id-v1";

/// Domain tag for holdout episode-set commitments.
pub const HOLDOUT_DOMAIN: &[u8] = b"base-relearn-agent-holdout-v1";

/// Domain tag for eval-receipt digests.
pub const RECEIPT_DOMAIN: &[u8] = b"base-relearn-agent-receipt-v1";

/// Domain tag for promotion attestations.
pub const PROMOTE_DOMAIN: &[u8] = b"base-relearn-agent-promote-v1";

/// Integer score lattice max (same scale as every other challenge).
pub const SCORE_MAX: u64 = 1_000_000;

/// Base model miners post-train into an agent.
///
/// The same checkpoint the `relearn` challenge pins: public, ungated,
/// Apache-2.0, native VLM. <https://huggingface.co/Qwen/Qwen3.8-27B>
pub const BASE_MODEL_ID: &str = "Qwen/Qwen3.8-27B";

/// Teacher wire id. Judge-only, and only for the free-text final answer.
pub const TEACHER_MODEL_ID: &str = "glm-5.3";

/// Public miner / eval-image repo (shared with the other Relearn challenges).
pub const RELEARN_GIT_URL: &str = "https://github.com/CortexLM/relearn";

/// Slice id bound into the paired test. Both sides must carry it.
pub const HOLDOUT_SLICE_ID: &str = "relearn-agent-holdout";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn challenge_id_is_distinct() {
        assert_eq!(CHALLENGE_ID, "relearn-agent");
        assert_eq!(CHALLENGE_ID_BYTES, b"relearn-agent");
        for other in ["relearn", "relearn-image", "relearn-mm", "bounty"] {
            assert_ne!(CHALLENGE_ID, other);
        }
    }

    #[test]
    fn domain_tags_are_agent_prefixed_and_unique() {
        let tags = [
            TASK_ID_DOMAIN,
            HOLDOUT_DOMAIN,
            RECEIPT_DOMAIN,
            PROMOTE_DOMAIN,
        ];
        for t in tags {
            let s = std::str::from_utf8(t).unwrap_or("");
            assert!(s.contains("relearn-agent"), "{s}");
        }
        for i in 0..tags.len() {
            for j in (i + 1)..tags.len() {
                assert_ne!(tags[i], tags[j]);
            }
        }
    }

    /// The Agent challenge is a second post-train of the same checkpoint, not
    /// a rename of `relearn`: same base, different id and different scoring.
    #[test]
    fn base_is_the_locked_checkpoint() {
        assert_eq!(BASE_MODEL_ID, "Qwen/Qwen3.8-27B");
    }
}
