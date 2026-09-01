//! Frozen Relearn Agent episodes and the pin commitment.
//!
//! An episode is a **recorded tool-use trace**, not a prompt. The published
//! eval image ([`CortexLM/relearn`](https://github.com/CortexLM/relearn)
//! PR #3) replays each step on the recorded prefix and refuses a request
//! whose traces do not hash to `holdout_commitment`. Git carries only that
//! commitment + `holdout_size`; the traces live in an operator file
//! (`RELEARN_AGENT_HOLDOUT_FILE`) and are checked at boot.

use std::collections::{BTreeSet, HashSet};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::HOLDOUT_DOMAIN;

/// Minimum holdout episodes. Matches the paired test's evidence floor.
pub const MIN_HOLDOUT_EPISODES: usize = 100;

/// Largest Jaccard similarity on goal 3-grams allowed between public and
/// holdout episodes.
pub const NGRAM_JACCARD_MAX: f64 = 0.80;

/// A tool the synthetic / CI harness names. Live catalogues use free-form
/// [`ToolSchema`] names; this closed set is only a convenience for fixtures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    /// Read a region of the episode's document / image observation.
    Inspect,
    /// Query the episode's offline corpus.
    Search,
    /// Run a sandboxed snippet against the episode's data.
    Execute,
    /// Fetch a record from the episode's offline table.
    Lookup,
}

impl ToolKind {
    /// Every named tool the synthetic harness uses.
    pub const ALL: [Self; 4] = [Self::Inspect, Self::Search, Self::Execute, Self::Lookup];

    /// Wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Inspect => "inspect",
            Self::Search => "search",
            Self::Execute => "execute",
            Self::Lookup => "lookup",
        }
    }
}

/// One tool the episode made available, as the eval image sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolSchema {
    /// Tool name the model is shown.
    pub name: String,
    /// Human description. Not part of the commitment.
    #[serde(default)]
    pub description: String,
    /// JSON-schema-ish parameter object. Not part of the commitment.
    #[serde(default)]
    pub parameters: serde_json::Value,
}

impl ToolSchema {
    /// Named tool with an empty schema.
    #[must_use]
    pub fn named(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: String::new(),
            parameters: serde_json::json!({}),
        }
    }
}

/// One recorded turn: the action that was taken, and what came back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceStep {
    /// Tool that was called.
    pub tool: String,
    /// Arguments, as an object. Canonicalized before hashing.
    #[serde(default)]
    pub arguments: serde_json::Value,
    /// What the tool returned. Pixels stay out: those are named by hash.
    #[serde(default)]
    pub observation: String,
    /// SHA-256 hex of a screenshot observation, when the step has one.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub observation_image_hash: String,
}

impl TraceStep {
    /// Arguments encoded the way the eval image commits them.
    ///
    /// Sorted keys, no whitespace — so the commitment does not depend on how
    /// the operator's exporter happened to serialize the same object.
    #[must_use]
    pub fn arguments_json(&self) -> String {
        canonical_json(&self.arguments)
    }
}

/// One frozen episode: a recorded tool-use trace.
///
/// This is the harvest request's `holdout[]` item. The eval image refuses a
/// request whose traces do not hash to the pin's `holdout_commitment`, so the
/// preimage here **is** the image's preimage — goal, dataset, tools, every
/// step's tool / arguments / observation / image hash, and the final answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentEpisode {
    /// Stable episode id. Never published for the holdout split.
    pub id: u32,
    /// What the agent is asked to accomplish.
    pub goal: String,
    /// Source catalogue / environment id (fingerprint only).
    #[serde(default, alias = "environment_id")]
    pub dataset_id: String,
    /// Tools this episode's environment exposes.
    pub tools: Vec<ToolSchema>,
    /// Recorded steps, in order. An episode with none cannot be replayed.
    pub steps: Vec<TraceStep>,
    /// Reference final answer. Graded by the teacher; actions are not.
    #[serde(default)]
    pub final_answer: String,
}

impl AgentEpisode {
    /// Canonical fingerprint used by the contamination gate.
    #[must_use]
    pub fn fingerprint(&self) -> String {
        format!("episode:{}", self.id)
    }

    /// Tool calls the recorded solution made. Zero is refused at load.
    #[must_use]
    pub fn min_tool_calls(&self) -> u32 {
        u32::try_from(self.steps.len()).unwrap_or(u32::MAX)
    }

    /// SHA-256 hex of the first image observation, else of the first text one.
    #[must_use]
    pub fn observation_hash(&self) -> String {
        if let Some(img) = self
            .steps
            .iter()
            .map(|s| s.observation_image_hash.trim())
            .find(|h| !h.is_empty())
        {
            return img.to_ascii_lowercase();
        }
        let mut h = Sha256::new();
        for step in &self.steps {
            h.update(step.observation.as_bytes());
            h.update([0xff]);
        }
        hex::encode(h.finalize())
    }

    /// Observation fingerprint, when one exists.
    #[must_use]
    pub fn observation_fingerprint(&self) -> Option<String> {
        let hash = self.observation_hash();
        (hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit()))
            .then(|| format!("observation:{hash}"))
    }

    /// A minimal valid recorded episode, for CI and local catalogues.
    ///
    /// Production must use a private catalogue. This constructor exists so
    /// every test fixture speaks the same wire the eval image accepts.
    #[must_use]
    pub fn synthetic(id: u32, goal: impl Into<String>) -> Self {
        let figure = 1_000 + id;
        Self {
            id,
            goal: goal.into(),
            dataset_id: "synthetic-dev".into(),
            tools: vec![
                ToolSchema {
                    name: "inspect".into(),
                    description: "Read a region of the attached record".into(),
                    parameters: serde_json::json!({"path": {"type": "string"}}),
                },
                ToolSchema {
                    name: "lookup".into(),
                    description: "Fetch a row from the offline table".into(),
                    parameters: serde_json::json!({"key": {"type": "string"}}),
                },
            ],
            steps: vec![
                TraceStep {
                    tool: "inspect".into(),
                    arguments: serde_json::json!({"path": format!("record/{id}")}),
                    observation: format!("{{\"figure\":{figure}}}"),
                    observation_image_hash: String::new(),
                },
                TraceStep {
                    tool: "lookup".into(),
                    arguments: serde_json::json!({"key": format!("k-{id}")}),
                    observation: format!("{{\"value\":{figure}}}"),
                    observation_image_hash: String::new(),
                },
            ],
            final_answer: format!("The figure is {figure}."),
        }
    }
}

/// Why an episode set was refused.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum EpisodeError {
    /// Nothing to score.
    #[error("episode set is empty")]
    Empty,
    /// Two episodes claim the same id.
    #[error("duplicate episode id {0}")]
    DuplicateId(u32),
    /// An episode has no goal body.
    #[error("episode {0} has an empty goal")]
    EmptyGoal(u32),
    /// An episode exposes no tools, so nothing distinguishes it from a prompt.
    #[error("episode {0} exposes no tools")]
    NoTools(u32),
    /// An episode has no recorded steps, so nothing can be replayed.
    #[error("episode {0} has no recorded steps; it cannot detect a memorised answer")]
    NoToolCallRequired(u32),
    /// A recorded step names a tool the episode does not expose.
    #[error("episode {0} records a call to {1:?}, which is not in its schema")]
    UnknownTool(u32, String),
    /// An image observation hash is not 64 hex chars.
    #[error("episode {0} has a malformed observation_image_hash")]
    MalformedHash(u32, &'static str),
    /// The supplied set does not match the committed digest.
    #[error("episode commitment mismatch (expected {expected}, got {got})")]
    CommitmentMismatch {
        /// Digest pinned in git.
        expected: String,
        /// Digest of what the operator supplied.
        got: String,
    },
    /// Episode count disagrees with the pin.
    #[error("episode count mismatch (expected {expected}, got {got})")]
    SizeMismatch {
        /// Count pinned in git.
        expected: usize,
        /// Count the operator supplied.
        got: usize,
    },
    /// A holdout episode id is also in the published split.
    #[error("episode {0} is also in the public split")]
    OverlapsPublic(u32),
    /// Public and holdout share a near-duplicate goal or observation.
    #[error("holdout near-duplicate of a public episode ({0})")]
    NearDuplicate(String),
}

/// Canonical JSON matching the eval image (`separators=(",", ":")`, sorted keys).
fn canonical_json(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => "null".into(),
        serde_json::Value::Bool(true) => "true".into(),
        serde_json::Value::Bool(false) => "false".into(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::String(s) => serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into()),
        serde_json::Value::Array(items) => {
            let parts: Vec<String> = items.iter().map(canonical_json).collect();
            format!("[{}]", parts.join(","))
        }
        serde_json::Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let parts: Vec<String> = keys
                .into_iter()
                .map(|k| {
                    format!(
                        "{}:{}",
                        serde_json::to_string(k).unwrap_or_else(|_| "\"\"".into()),
                        canonical_json(&map[k])
                    )
                })
                .collect();
            format!("{{{}}}", parts.join(","))
        }
    }
}

fn field(h: &mut Sha256, value: &str) {
    let body = value.as_bytes();
    h.update(u64::try_from(body.len()).unwrap_or(u64::MAX).to_le_bytes());
    h.update(body);
}

/// Commitment over a recorded-trace set.
///
/// Byte-for-byte the eval image's `trace_commitment`: domain-separated,
/// id-sorted, length-prefixed, covering every field that decides what counts
/// as a correct action. Editing an observation, a recorded argument, or the
/// step order would otherwise be an editable knob on a "verified" holdout.
#[must_use]
pub fn episode_commitment(episodes: &[AgentEpisode]) -> String {
    let mut sorted: Vec<&AgentEpisode> = episodes.iter().collect();
    sorted.sort_by_key(|e| e.id);
    let mut h = Sha256::new();
    h.update(HOLDOUT_DOMAIN);
    h.update([0xff]);
    h.update(
        u64::try_from(sorted.len())
            .unwrap_or(u64::MAX)
            .to_le_bytes(),
    );
    for e in sorted {
        h.update(e.id.to_le_bytes());
        for value in [&e.goal, &e.dataset_id, &e.final_answer] {
            field(&mut h, value);
        }
        h.update(
            u64::try_from(e.tools.len())
                .unwrap_or(u64::MAX)
                .to_le_bytes(),
        );
        for tool in &e.tools {
            field(&mut h, &tool.name);
        }
        h.update(
            u64::try_from(e.steps.len())
                .unwrap_or(u64::MAX)
                .to_le_bytes(),
        );
        for step in &e.steps {
            let args = step.arguments_json();
            let img = step.observation_image_hash.trim().to_ascii_lowercase();
            for value in [
                step.tool.as_str(),
                args.as_str(),
                step.observation.as_str(),
                img.as_str(),
            ] {
                field(&mut h, value);
            }
        }
    }
    hex::encode(h.finalize())
}

fn hex64(s: &str) -> bool {
    let t = s.trim();
    t.len() == 64 && t.chars().all(|c| c.is_ascii_hexdigit())
}

fn validate(episodes: &[AgentEpisode]) -> Result<(), EpisodeError> {
    if episodes.is_empty() {
        return Err(EpisodeError::Empty);
    }
    let mut seen = BTreeSet::new();
    for e in episodes {
        if e.goal.trim().is_empty() {
            return Err(EpisodeError::EmptyGoal(e.id));
        }
        if e.tools.is_empty() {
            return Err(EpisodeError::NoTools(e.id));
        }
        if e.steps.is_empty() {
            return Err(EpisodeError::NoToolCallRequired(e.id));
        }
        let names: BTreeSet<&str> = e.tools.iter().map(|t| t.name.as_str()).collect();
        for step in &e.steps {
            if !names.contains(step.tool.as_str()) {
                return Err(EpisodeError::UnknownTool(e.id, step.tool.clone()));
            }
            let img = step.observation_image_hash.trim();
            if !img.is_empty() && !hex64(img) {
                return Err(EpisodeError::MalformedHash(e.id, "observation_image"));
            }
        }
        if !seen.insert(e.id) {
            return Err(EpisodeError::DuplicateId(e.id));
        }
    }
    Ok(())
}

/// Word 3-grams of a goal, lowercased.
fn trigrams(text: &str) -> HashSet<String> {
    let words: Vec<String> = text
        .split_whitespace()
        .map(|w| {
            w.chars()
                .filter(|c| c.is_alphanumeric())
                .collect::<String>()
                .to_ascii_lowercase()
        })
        .filter(|w| !w.is_empty())
        .collect();
    if words.len() < 3 {
        return HashSet::new();
    }
    words
        .windows(3)
        .map(|w| format!("{} {} {}", w[0], w[1], w[2]))
        .collect()
}

fn jaccard(a: &HashSet<String>, b: &HashSet<String>) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    #[allow(clippy::cast_precision_loss)]
    let (inter, union) = (a.intersection(b).count() as f64, a.union(b).count() as f64);
    if union <= 0.0 {
        0.0
    } else {
        inter / union
    }
}

/// Public↔holdout near-duplicates (id, observation hash, goal n-gram).
#[must_use]
pub fn near_duplicates(public: &[AgentEpisode], holdout: &[AgentEpisode]) -> Vec<String> {
    let mut hits = Vec::new();
    let published_ids: BTreeSet<u32> = public.iter().map(|p| p.id).collect();
    let published_obs: BTreeSet<String> = public
        .iter()
        .filter_map(AgentEpisode::observation_fingerprint)
        .collect();
    let goal_grams: Vec<(u32, HashSet<String>)> = public
        .iter()
        .map(|p| (p.id, trigrams(&p.goal)))
        .filter(|(_, g)| g.len() >= 4)
        .collect();

    for h in holdout {
        if published_ids.contains(&h.id) {
            hits.push(format!("episode:{}", h.id));
        }
        if let Some(obs) = h.observation_fingerprint() {
            if published_obs.contains(&obs) {
                hits.push(obs);
            }
        }
        let hg = trigrams(&h.goal);
        if hg.len() >= 4 {
            for (pid, pg) in &goal_grams {
                if jaccard(&hg, pg) > NGRAM_JACCARD_MAX {
                    hits.push(format!("ngram:{}~{pid}", h.id));
                }
            }
        }
    }
    hits.sort();
    hits.dedup();
    hits
}

/// Verify an operator-supplied episode set against the committed digest.
///
/// # Errors
///
/// Structural problems, size disagreement, public-split overlap,
/// near-duplicates, or a commitment mismatch. Every one is fail-closed.
pub fn verify_episodes(
    episodes: &[AgentEpisode],
    public: &[AgentEpisode],
    public_ids: &[u32],
    expected_commitment: &str,
    expected_size: usize,
) -> Result<(), EpisodeError> {
    validate(episodes)?;
    if episodes.len() != expected_size {
        return Err(EpisodeError::SizeMismatch {
            expected: expected_size,
            got: episodes.len(),
        });
    }
    for e in episodes {
        if public_ids.contains(&e.id) {
            return Err(EpisodeError::OverlapsPublic(e.id));
        }
    }
    if let Some(hit) = near_duplicates(public, episodes).into_iter().next() {
        return Err(EpisodeError::NearDuplicate(hit));
    }
    let got = episode_commitment(episodes);
    if !got.eq_ignore_ascii_case(expected_commitment.trim()) {
        return Err(EpisodeError::CommitmentMismatch {
            expected: expected_commitment.trim().to_ascii_lowercase(),
            got,
        });
    }
    Ok(())
}

/// Holdout fingerprints that leaked into a submission's training metadata.
#[must_use]
pub fn contamination(
    train_episode_ids: &BTreeSet<u32>,
    train_observation_hashes: &BTreeSet<String>,
    holdout: &[AgentEpisode],
) -> Vec<String> {
    let mut hits = Vec::new();
    for e in holdout {
        if train_episode_ids.contains(&e.id) {
            hits.push(e.fingerprint());
        }
        if let Some(obs) = e.observation_fingerprint() {
            let raw = obs.trim_start_matches("observation:");
            if train_observation_hashes
                .iter()
                .any(|t| t.trim().eq_ignore_ascii_case(raw))
            {
                hits.push(obs);
            }
        }
    }
    hits.sort();
    hits.dedup();
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    fn episode(id: u32, goal: &str) -> AgentEpisode {
        AgentEpisode::synthetic(id, goal)
    }

    fn holdout() -> Vec<AgentEpisode> {
        vec![
            episode(900, "find the invoice total hidden in the scanned ledger"),
            episode(
                901,
                "reconcile the shipment rows against the manifest table",
            ),
        ]
    }

    #[test]
    fn commitment_is_order_independent_and_length_prefixed() {
        let a = episode_commitment(&holdout());
        let mut rev = holdout();
        rev.reverse();
        assert_eq!(a, episode_commitment(&rev));
        assert_eq!(a.len(), 64);
        let spliced_ab = episode_commitment(&[episode(1, "ab"), episode(2, "c")]);
        let spliced_bc = episode_commitment(&[episode(1, "a"), episode(2, "bc")]);
        assert_ne!(spliced_ab, spliced_bc);
    }

    #[test]
    fn commitment_tracks_the_environment_not_just_the_goal() {
        let base = episode_commitment(&holdout());
        let mut tools = holdout();
        tools[0].tools = vec![ToolSchema::named("search")];
        tools[0].steps[0].tool = "search".into();
        tools[0].steps[1].tool = "search".into();
        assert_ne!(base, episode_commitment(&tools));
        let mut steps = holdout();
        steps[0].steps.pop();
        assert_ne!(base, episode_commitment(&steps));
        let mut obs = holdout();
        obs[0].steps[0].observation = "edited".into();
        assert_ne!(base, episode_commitment(&obs));
        let mut args = holdout();
        args[0].steps[0].arguments = serde_json::json!({"path": "other"});
        assert_ne!(base, episode_commitment(&args));
    }

    #[test]
    fn verify_accepts_the_committed_set() {
        let recs = holdout();
        let c = episode_commitment(&recs);
        verify_episodes(&recs, &[], &[1, 2, 3], &c, 2).expect("ok");
    }

    /// An episode a model can answer without calling a tool measures recall,
    /// not agency, so it never enters the set.
    #[test]
    fn an_episode_with_no_required_tool_call_is_refused() {
        let mut recs = holdout();
        recs[0].steps.clear();
        assert!(matches!(
            verify_episodes(&recs, &[], &[], "00", 2),
            Err(EpisodeError::NoToolCallRequired(900))
        ));

        let mut toolless = holdout();
        toolless[1].tools.clear();
        assert!(matches!(
            verify_episodes(&toolless, &[], &[], "00", 2),
            Err(EpisodeError::NoTools(901))
        ));
    }

    #[test]
    fn verify_rejects_public_overlap_and_near_duplicates() {
        let recs = holdout();
        let c = episode_commitment(&recs);
        assert!(matches!(
            verify_episodes(&recs, &[], &[901], &c, 2),
            Err(EpisodeError::OverlapsPublic(901))
        ));
        let public = vec![episode(
            12,
            "find the invoice total hidden in the scanned ledger",
        )];
        assert!(matches!(
            verify_episodes(&recs, &public, &[], &c, 2),
            Err(EpisodeError::NearDuplicate(_))
        ));
    }

    #[test]
    fn a_shared_observation_is_a_near_duplicate() {
        let mut public = vec![episode(1, "a public episode about a warehouse audit")];
        public[0].steps[0].observation = "shared-payload".into();
        let mut hold = vec![episode(900, "a holdout episode about a harbour audit")];
        hold[0].steps[0].observation = "shared-payload".into();
        hold[0].steps[1].observation = public[0].steps[1].observation.clone();
        let hits = near_duplicates(&public, &hold);
        assert!(
            hits.iter().any(|h| h.starts_with("observation:")),
            "{hits:?}"
        );
    }

    #[test]
    fn verify_rejects_edited_sets_and_size_drift() {
        let recs = holdout();
        let c = episode_commitment(&recs);
        let mut edited = recs.clone();
        edited[1].goal = "something else entirely".into();
        assert!(matches!(
            verify_episodes(&edited, &[], &[], &c, 2),
            Err(EpisodeError::CommitmentMismatch { .. })
        ));
        assert!(matches!(
            verify_episodes(&recs, &[], &[], &c, 3),
            Err(EpisodeError::SizeMismatch { .. })
        ));
    }

    #[test]
    fn contamination_detects_episode_and_observation_overlap() {
        let hold = holdout();
        let ids: BTreeSet<u32> = [900].into_iter().collect();
        let obs: BTreeSet<String> = [hold[1].observation_hash()].into_iter().collect();
        let none = BTreeSet::new();
        assert!(contamination(&ids, &none, &hold)
            .iter()
            .any(|h| h == "episode:900"));
        assert!(contamination(&BTreeSet::new(), &obs, &hold)
            .iter()
            .any(|h| h.starts_with("observation:")));
        assert!(contamination(&BTreeSet::new(), &none, &hold).is_empty());
    }

    #[test]
    fn arguments_canonicalize_the_way_the_image_does() {
        let a = TraceStep {
            tool: "lookup".into(),
            arguments: serde_json::json!({"b": 1, "a": 2}),
            observation: String::new(),
            observation_image_hash: String::new(),
        };
        assert_eq!(a.arguments_json(), r#"{"a":2,"b":1}"#);
    }
}
