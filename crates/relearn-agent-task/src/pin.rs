//! Pins Cortex stores for the Relearn Agent eval image and episode set.
//!
//! Git carries the model ids, the eval image digest, and the holdout
//! commitment. It never carries the episodes themselves.

use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{episode::MIN_HOLDOUT_EPISODES, BASE_MODEL_ID, RELEARN_GIT_URL, TEACHER_MODEL_ID};

/// CI / local fixture commitment: `AgentEpisode::synthetic` over ids `41..=160`.
///
/// Labeled in `config/relearn-agent-pin.toml`. Never a live emissions pin —
/// reconstructing that range reproduces this digest exactly.
pub const FIXTURE_HOLDOUT_COMMITMENT: &str =
    "5b7bd02741082bf73212d96834465f941406277660894b034ae5b4dd608dd0fe";

/// Private live commitment (64 hex). Not a file in git.
pub const LIVE_HOLDOUT_COMMITMENT_ENV: &str = "RELEARN_AGENT_HOLDOUT_COMMITMENT";

/// Secret-store file whose whole body is the private live commitment.
pub const LIVE_HOLDOUT_COMMITMENT_FILE_ENV: &str = "RELEARN_AGENT_HOLDOUT_COMMITMENT_FILE";

/// True when `hex` is the public CI fixture.
#[must_use]
pub fn is_fixture_holdout_commitment(hex: &str) -> bool {
    hex.trim().eq_ignore_ascii_case(FIXTURE_HOLDOUT_COMMITMENT)
}

/// Read a private live commitment from the environment or secret-store file.
///
/// # Errors
///
/// [`PinError::BadHoldoutCommitment`] when the override is present but not
/// 64 hex, [`PinError::FixtureHoldoutNotLive`] when it equals the fixture,
/// [`PinError::LiveHoldoutIo`] when the file cannot be read.
pub fn read_private_holdout_commitment() -> Result<Option<String>, PinError> {
    if let Ok(path) = std::env::var(LIVE_HOLDOUT_COMMITMENT_FILE_ENV) {
        let path = path.trim();
        if !path.is_empty() {
            return parse_private_commitment(
                &std::fs::read_to_string(Path::new(path))
                    .map_err(|e| PinError::LiveHoldoutIo(format!("read {path}: {e}")))?,
            )
            .map(Some);
        }
    }
    match std::env::var(LIVE_HOLDOUT_COMMITMENT_ENV) {
        Ok(raw) if !raw.trim().is_empty() => parse_private_commitment(&raw).map(Some),
        _ => Ok(None),
    }
}

fn parse_private_commitment(raw: &str) -> Result<String, PinError> {
    let c = raw.trim();
    if c.len() != 64 || !c.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Err(PinError::BadHoldoutCommitment);
    }
    if is_fixture_holdout_commitment(c) {
        return Err(PinError::FixtureHoldoutNotLive);
    }
    Ok(c.to_ascii_lowercase())
}

/// Pinned eval image and episode set for the Agent challenge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RelearnAgentPin {
    /// Base checkpoint miners post-train.
    pub base_model: String,
    /// Teacher wire id. Judge-only; actions are graded against the recording.
    #[serde(default = "default_teacher")]
    pub teacher_model: String,
    /// Eval image reference (no floating tag in prod).
    pub eval_image: String,
    /// `sha256:…` digest. Empty until the first green relearn CI image.
    pub eval_image_digest: String,
    /// `https://github.com/CortexLM/relearn`.
    pub relearn_git: String,
    /// Pinned git SHA of the harness repo.
    pub relearn_git_sha: String,
    /// Commitment over the operator episode file. Required.
    pub holdout_commitment: String,
    /// Expected holdout episode count.
    pub holdout_size: usize,
    /// Published episode ids. Miners may train on these.
    pub public_ids: Vec<u32>,
}

fn default_teacher() -> String {
    TEACHER_MODEL_ID.into()
}

impl Default for RelearnAgentPin {
    fn default() -> Self {
        Self {
            base_model: BASE_MODEL_ID.into(),
            teacher_model: TEACHER_MODEL_ID.into(),
            eval_image: "ghcr.io/cortexlm/relearn-agent-eval".into(),
            eval_image_digest: String::new(),
            relearn_git: RELEARN_GIT_URL.into(),
            relearn_git_sha: String::new(),
            holdout_commitment: String::new(),
            holdout_size: 0,
            public_ids: Vec::new(),
        }
    }
}

/// Why a pin was refused.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PinError {
    /// TOML did not parse.
    #[error("parse relearn-agent pin: {0}")]
    Parse(String),
    /// Holdout commitment is not a 64-hex digest.
    #[error("holdout_commitment must be 64 hex chars")]
    BadHoldoutCommitment,
    /// Holdout split is too thin for a verdict.
    #[error("holdout_size {got} is below the {min} floor")]
    TooFewEpisodes {
        /// Count the pin declared.
        got: usize,
        /// Required floor.
        min: usize,
    },
    /// The pin names a base this challenge does not post-train.
    #[error("base_model {got:?} is not the pinned {want:?}")]
    WrongBase {
        /// What the pin said.
        got: String,
        /// What the challenge requires.
        want: &'static str,
    },
    /// Live scoring was asked to use the public CI fixture.
    #[error(
        "live holdout must not be the public CI fixture; set {LIVE_HOLDOUT_COMMITMENT_ENV} or {LIVE_HOLDOUT_COMMITMENT_FILE_ENV}"
    )]
    FixtureHoldoutNotLive,
    /// Live scoring has no private commitment from the secret store.
    #[error(
        "live holdout unconfigured; set {LIVE_HOLDOUT_COMMITMENT_ENV} or {LIVE_HOLDOUT_COMMITMENT_FILE_ENV}"
    )]
    LiveHoldoutUnconfigured,
    /// Secret-store file for the live commitment could not be read.
    #[error("live holdout secret: {0}")]
    LiveHoldoutIo(String),
}

impl RelearnAgentPin {
    /// Load from `config/relearn-agent-pin.toml`.
    ///
    /// # Errors
    ///
    /// [`PinError::Parse`] on malformed TOML. Call [`Self::validate`] before boot.
    pub fn from_toml(body: &str) -> Result<Self, PinError> {
        toml::from_str(body).map_err(|e| PinError::Parse(e.to_string()))
    }

    /// Enforce the base pin, the holdout commitment, and the episode floor.
    ///
    /// # Errors
    ///
    /// [`PinError::WrongBase`], [`PinError::BadHoldoutCommitment`], or
    /// [`PinError::TooFewEpisodes`].
    pub fn validate(&self) -> Result<(), PinError> {
        if self.base_model.trim() != BASE_MODEL_ID {
            return Err(PinError::WrongBase {
                got: self.base_model.clone(),
                want: BASE_MODEL_ID,
            });
        }
        let commitment = self.holdout_commitment.trim();
        if commitment.len() != 64 || !commitment.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(PinError::BadHoldoutCommitment);
        }
        if self.holdout_size < MIN_HOLDOUT_EPISODES {
            return Err(PinError::TooFewEpisodes {
                got: self.holdout_size,
                min: MIN_HOLDOUT_EPISODES,
            });
        }
        Ok(())
    }

    /// True when a live rent is allowed (real digest pin present).
    #[must_use]
    pub fn can_rent(&self) -> bool {
        self.eval_image_digest.starts_with("sha256:") && self.eval_image_digest.len() >= 71
    }

    /// True when this pin still carries the public CI fixture commitment.
    #[must_use]
    pub fn is_fixture_holdout(&self) -> bool {
        is_fixture_holdout_commitment(&self.holdout_commitment)
    }

    /// Replace the git fixture with a private live commitment.
    ///
    /// # Errors
    ///
    /// [`PinError::BadHoldoutCommitment`] or [`PinError::FixtureHoldoutNotLive`].
    pub fn bind_private_holdout(&mut self, commitment: &str) -> Result<(), PinError> {
        self.holdout_commitment = parse_private_commitment(commitment)?;
        Ok(())
    }

    /// Bind the private live commitment from the environment / secret store.
    ///
    /// # Errors
    ///
    /// [`PinError::LiveHoldoutUnconfigured`] when neither override is set,
    /// otherwise the same errors as [`read_private_holdout_commitment`].
    pub fn bind_live_holdout_from_env(&mut self) -> Result<(), PinError> {
        let Some(c) = read_private_holdout_commitment()? else {
            return Err(PinError::LiveHoldoutUnconfigured);
        };
        self.bind_private_holdout(&c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toml_round_trip_and_validate() {
        let body = format!(
            r#"
base_model = "{BASE_MODEL_ID}"
eval_image = "ghcr.io/cortexlm/relearn-agent-eval"
holdout_commitment = "{}"
holdout_size = 120
public_ids = [1, 2, 3]
"#,
            "aa".repeat(32)
        );
        let p = RelearnAgentPin::from_toml(&body).expect("parse");
        assert_eq!(p.base_model, BASE_MODEL_ID);
        assert_eq!(p.holdout_size, 120);
        p.validate().expect("validates");
        assert!(!p.can_rent(), "no digest pinned yet");
    }

    #[test]
    fn a_pin_without_a_commitment_or_with_a_thin_split_is_refused() {
        let bare = RelearnAgentPin::default();
        assert!(matches!(
            bare.validate(),
            Err(PinError::BadHoldoutCommitment)
        ));
        let thin = RelearnAgentPin {
            holdout_commitment: "aa".repeat(32),
            holdout_size: 4,
            ..RelearnAgentPin::default()
        };
        assert!(matches!(
            thin.validate(),
            Err(PinError::TooFewEpisodes { .. })
        ));
    }

    #[test]
    fn a_pin_that_swaps_the_base_is_refused() {
        let swapped = RelearnAgentPin {
            base_model: "Qwen/Qwen3.8-Flash-Next".into(),
            holdout_commitment: "aa".repeat(32),
            holdout_size: 120,
            ..RelearnAgentPin::default()
        };
        assert!(matches!(
            swapped.validate(),
            Err(PinError::WrongBase { .. })
        ));
    }

    #[test]
    fn rent_needs_a_full_sha256_digest() {
        let pinned = RelearnAgentPin {
            eval_image_digest: format!("sha256:{}", "ab".repeat(32)),
            ..RelearnAgentPin::default()
        };
        assert!(pinned.can_rent());
        let stub = RelearnAgentPin {
            eval_image_digest: "sha256:abc".into(),
            ..RelearnAgentPin::default()
        };
        assert!(!stub.can_rent());
    }

    #[test]
    fn fixture_commitment_is_refused_as_a_live_override() {
        assert!(is_fixture_holdout_commitment(FIXTURE_HOLDOUT_COMMITMENT));
        assert!(is_fixture_holdout_commitment(
            &FIXTURE_HOLDOUT_COMMITMENT.to_ascii_uppercase()
        ));
        let mut pin = RelearnAgentPin {
            holdout_commitment: FIXTURE_HOLDOUT_COMMITMENT.into(),
            holdout_size: 120,
            ..RelearnAgentPin::default()
        };
        assert!(pin.is_fixture_holdout());
        assert!(matches!(
            pin.bind_private_holdout(FIXTURE_HOLDOUT_COMMITMENT),
            Err(PinError::FixtureHoldoutNotLive)
        ));
        pin.bind_private_holdout(&"ab".repeat(32))
            .expect("private hex");
        assert!(!pin.is_fixture_holdout());
    }
}
