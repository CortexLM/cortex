//! RLM lifecycle for one topic.
//!
//! ```text
//! draft → owner_presend → awaiting_owner_keys → provisioning → baselining
//!       → open ⇄ evaluating → promoting → open … → closed
//! ```
//!
//! The machine is a pure transition table plus two owner hooks:
//!
//! - **`owner_presend`** asks the owner (an `askUser`-style prompt) before
//!   anything is sent anywhere. Without a configured hook the machine cannot
//!   leave `owner_presend`; a decline returns it to `draft`.
//! - **`awaiting_owner_keys`** waits for the owner's paid-inference key file
//!   to be present. The probe reports presence only; the value never enters
//!   logs or the store.
//!
//! Nothing here spends, rents, or scores. It records where a topic is and
//! refuses moves the product rules forbid (for example `open` before a
//! sealed baseline). Every transition is meant to be persisted by the store
//! so the topic's history survives a control-plane restart.

use std::path::{Path, PathBuf};

use proof_task::{MetricFamily, TopicDocument, TopicStatus};
use serde::{Deserialize, Serialize};

/// Env var naming the owner's paid-inference key **file** used for the
/// baseline run. Only the name lives in git; the file is operator state and
/// is staged into the topic VM, never read by the control plane.
pub const OWNER_INFERENCE_KEY_FILE_ENV: &str = "PROOF_RLM_OWNER_INFERENCE_KEY_FILE";

/// Lifecycle states, in ship order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RlmState {
    /// Topic drafted (signed or not, sealed or not). Nothing sent.
    Draft,
    /// Owner review before any send / spend. Needs an [`OwnerHook`] answer.
    OwnerPresend,
    /// Owner approved; waiting for the owner key file.
    AwaitingOwnerKeys,
    /// Topic VM being provisioned through the orchestrator.
    Provisioning,
    /// RLM writes rules and runs the baseline inside its VM; seals `custom_value`.
    Baselining,
    /// Accepting submissions and earning emission share.
    Open,
    /// One submission is being inspected and run.
    Evaluating,
    /// A pass beat the bar; artefact is being promoted.
    Promoting,
    /// Frozen. No new submissions, no emission share.
    Closed,
}

impl RlmState {
    /// Every state, in ship order.
    pub const ORDER: [RlmState; 9] = [
        Self::Draft,
        Self::OwnerPresend,
        Self::AwaitingOwnerKeys,
        Self::Provisioning,
        Self::Baselining,
        Self::Open,
        Self::Evaluating,
        Self::Promoting,
        Self::Closed,
    ];

    /// Wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::OwnerPresend => "owner_presend",
            Self::AwaitingOwnerKeys => "awaiting_owner_keys",
            Self::Provisioning => "provisioning",
            Self::Baselining => "baselining",
            Self::Open => "open",
            Self::Evaluating => "evaluating",
            Self::Promoting => "promoting",
            Self::Closed => "closed",
        }
    }

    /// Parse a wire name.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Self::ORDER.into_iter().find(|st| st.as_str() == s.trim())
    }

    /// Only `open` takes new submissions.
    #[must_use]
    pub const fn accepts_submissions(self) -> bool {
        matches!(self, Self::Open)
    }

    /// `closed` never moves again.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Closed)
    }
}

/// Events that move the machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RlmEvent {
    /// Operator asks for owner review of the draft.
    SubmitForReview,
    /// Owner answered yes at `owner_presend`.
    OwnerApproved,
    /// Owner answered no at `owner_presend`; back to draft.
    OwnerDeclined,
    /// Owner key file is present.
    OwnerKeysPresent,
    /// Topic VM provisioned; the RLM may write rules and baseline.
    Provisioned,
    /// Provisioning failed; back to draft for owner review.
    ProvisionFailed,
    /// Baseline measured and sealed into the topic.
    BaselineSealed,
    /// Baseline run failed; back to draft.
    BaselineFailed,
    /// A submission arrived on an open topic.
    SubmissionReceived,
    /// Verdict persisted; no promotion.
    VerdictRecorded,
    /// Verdict passed and beat the bar; promotion starts.
    PromotionCandidate,
    /// Artefact promoted; back to open.
    Promoted,
    /// Promotion refused (bar moved, artefact missing); back to open.
    PromotionRefused,
    /// Operator freezes the topic.
    Close,
}

impl RlmEvent {
    /// Wire name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SubmitForReview => "submit_for_review",
            Self::OwnerApproved => "owner_approved",
            Self::OwnerDeclined => "owner_declined",
            Self::OwnerKeysPresent => "owner_keys_present",
            Self::Provisioned => "provisioned",
            Self::ProvisionFailed => "provision_failed",
            Self::BaselineSealed => "baseline_sealed",
            Self::BaselineFailed => "baseline_failed",
            Self::SubmissionReceived => "submission_received",
            Self::VerdictRecorded => "verdict_recorded",
            Self::PromotionCandidate => "promotion_candidate",
            Self::Promoted => "promoted",
            Self::PromotionRefused => "promotion_refused",
            Self::Close => "close",
        }
    }
}

/// Why a move was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StateError {
    /// The table has no edge for this pair.
    #[error("illegal transition {} --{event:?}-->", from.as_str())]
    Illegal {
        /// Current state.
        from: RlmState,
        /// Event that was applied.
        event: RlmEvent,
    },
    /// The owner hook could not answer.
    #[error("owner hook: {0}")]
    Hook(#[from] HookError),
    /// The owner key file is absent or empty.
    #[error("owner keys not present ({0})")]
    KeysMissing(String),
    /// The topic is not a custom-family topic (nothing for an RLM to run).
    #[error("topic {0:?} is not a custom-family topic")]
    NotCustom(String),
}

/// The transition table. Every edge is explicit; anything else is illegal.
///
/// # Errors
///
/// [`StateError::Illegal`] for a pair the table does not name.
pub fn transition(from: RlmState, event: RlmEvent) -> Result<RlmState, StateError> {
    use RlmEvent as E;
    use RlmState as S;
    let to = match (from, event) {
        (S::Draft, E::SubmitForReview) => S::OwnerPresend,
        (S::OwnerPresend, E::OwnerApproved) => S::AwaitingOwnerKeys,
        (S::AwaitingOwnerKeys, E::OwnerKeysPresent) => S::Provisioning,
        (S::Provisioning, E::Provisioned) => S::Baselining,
        (S::Open, E::SubmissionReceived) => S::Evaluating,
        (S::Evaluating, E::PromotionCandidate) => S::Promoting,
        // Back to draft: the owner said no, or the ceremony failed.
        (S::OwnerPresend, E::OwnerDeclined)
        | (S::Provisioning, E::ProvisionFailed)
        | (S::Baselining, E::BaselineFailed) => S::Draft,
        // Back to open: sealed, verdict recorded, or promotion settled.
        (S::Baselining, E::BaselineSealed)
        | (S::Evaluating, E::VerdictRecorded)
        | (S::Promoting, E::Promoted | E::PromotionRefused) => S::Open,
        (S::Closed, _) => return Err(StateError::Illegal { from, event }),
        (_, E::Close) => S::Closed,
        _ => return Err(StateError::Illegal { from, event }),
    };
    Ok(to)
}

/// One recorded move.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transition {
    /// State before.
    pub from: RlmState,
    /// Event applied.
    pub event: RlmEvent,
    /// State after.
    pub to: RlmState,
    /// Operator-readable note (never a secret).
    pub note: String,
}

/// Where one topic is, plus how it got there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lifecycle {
    /// Topic id.
    pub topic_id: String,
    /// Current state.
    pub state: RlmState,
    /// Every applied transition, oldest first.
    pub history: Vec<Transition>,
}

impl Lifecycle {
    /// A fresh draft.
    #[must_use]
    pub fn draft(topic_id: &str) -> Self {
        Self::at(topic_id, RlmState::Draft)
    }

    /// Start at `state` with no history (a topic loaded from a signed
    /// document that already went through the ceremony off-host).
    #[must_use]
    pub fn at(topic_id: &str, state: RlmState) -> Self {
        Self {
            topic_id: topic_id.trim().to_owned(),
            state,
            history: Vec::new(),
        }
    }

    /// Lifecycle position implied by a signed topic document's `status`.
    ///
    /// An `open` document without a sealed baseline never maps to `open`:
    /// it sits at `baselining`, because nobody is paid for beating a number
    /// nobody measured.
    #[must_use]
    pub fn from_topic(doc: &TopicDocument) -> Self {
        let state = match doc.status {
            TopicStatus::Draft => RlmState::Draft,
            TopicStatus::Open if doc.baseline.is_sealed() => RlmState::Open,
            TopicStatus::Open => RlmState::Baselining,
            TopicStatus::Closed => RlmState::Closed,
        };
        Self::at(&doc.id, state)
    }

    /// Apply one event.
    ///
    /// # Errors
    ///
    /// [`StateError::Illegal`]; the state is unchanged on error.
    pub fn apply(&mut self, event: RlmEvent, note: &str) -> Result<RlmState, StateError> {
        let to = transition(self.state, event)?;
        self.history.push(Transition {
            from: self.state,
            event,
            to,
            note: note.trim().to_owned(),
        });
        self.state = to;
        Ok(to)
    }
}

/// What the owner is shown at `owner_presend`. Public topic fields only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OwnerPrompt {
    /// Topic id.
    pub topic_id: String,
    /// The research problem, as signed.
    pub statement: String,
    /// Custom metric the RLM will compute.
    pub custom_id: String,
    /// Model every paid call must name, when the topic pins one.
    pub model_pin: Option<String>,
    /// Opaque task slice, when the topic names one.
    pub task_slice: Option<String>,
    /// Whether miner code is confined to a Firecracker guest.
    pub firecracker_required: bool,
    /// Seed shared by baseline and challengers.
    pub seed: u64,
    /// Anti-cheat rule ids the topic ships (version 1).
    pub checklist: Vec<String>,
    /// Operator-declared spend cap for the baseline run, if any.
    pub spend_cap_usd: Option<f64>,
}

impl OwnerPrompt {
    /// Build the prompt from a custom-family topic document.
    ///
    /// # Errors
    ///
    /// [`StateError::NotCustom`] for `nll` / `throughput` topics.
    pub fn from_topic(doc: &TopicDocument, spend_cap_usd: Option<f64>) -> Result<Self, StateError> {
        if doc.metric.family != MetricFamily::Custom {
            return Err(StateError::NotCustom(doc.id.clone()));
        }
        Ok(Self {
            topic_id: doc.id.clone(),
            statement: doc.statement.clone(),
            custom_id: doc.metric.custom_id.trim().to_owned(),
            model_pin: doc.constraints.model_pin.clone(),
            task_slice: doc.constraints.task_slice.clone(),
            firecracker_required: doc.constraints.firecracker_required,
            seed: doc.baseline.seed,
            checklist: doc.checklist.iter().map(|r| r.id.clone()).collect(),
            spend_cap_usd,
        })
    }

    /// Human text an `askUser`-style hook can show verbatim.
    #[must_use]
    pub fn render(&self) -> String {
        let cap = self
            .spend_cap_usd
            .map_or_else(|| "unset".to_owned(), |c| format!("{c:.2} USD"));
        format!(
            "Proof topic {}: {} Metric {} (seed {}); model pin {}; task slice {}; firecracker \
             required: {}. Anti-cheat rules ticked before any paid inference: {}. Baseline \
             spend cap: {}. Approve provisioning a topic VM and running the baseline?",
            self.topic_id,
            self.statement.trim(),
            self.custom_id,
            self.seed,
            self.model_pin.as_deref().unwrap_or("none"),
            self.task_slice.as_deref().unwrap_or("none"),
            self.firecracker_required,
            if self.checklist.is_empty() {
                "none".to_owned()
            } else {
                self.checklist.join(", ")
            },
            cap
        )
    }
}

/// The owner's answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnerDecision {
    /// Proceed to `awaiting_owner_keys`.
    Approve,
    /// Back to `draft`.
    Decline {
        /// Why (operator note).
        reason: String,
    },
}

/// Why a hook could not answer.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HookError {
    /// No hook configured on this host. Fail-closed: `owner_presend` cannot advance.
    #[error("no owner hook configured; owner_presend cannot advance")]
    NoHook,
    /// The hook ran and failed.
    #[error("owner hook failed: {0}")]
    Failed(String),
}

/// `askUser`-style hook: show the prompt, return the owner's decision.
pub trait OwnerHook: Send + Sync {
    /// Ask the owner. Implementations must not answer on the owner's behalf.
    ///
    /// # Errors
    ///
    /// [`HookError`] when no answer could be obtained.
    fn ask_owner(&self, prompt: &OwnerPrompt) -> Result<OwnerDecision, HookError>;
}

/// No hook wired: every ask refuses.
pub struct NoOwnerHook;

impl OwnerHook for NoOwnerHook {
    fn ask_owner(&self, _prompt: &OwnerPrompt) -> Result<OwnerDecision, HookError> {
        Err(HookError::NoHook)
    }
}

/// A decision the owner already recorded out of band (tests, `--owner-approved` flows).
pub struct StaticOwnerHook(pub OwnerDecision);

impl OwnerHook for StaticOwnerHook {
    fn ask_owner(&self, _prompt: &OwnerPrompt) -> Result<OwnerDecision, HookError> {
        Ok(self.0.clone())
    }
}

/// Presence probe for the owner key. Presence only, never the value.
pub trait OwnerKeysProbe: Send + Sync {
    /// `Ok` when the key is present and non-empty.
    ///
    /// # Errors
    ///
    /// [`HookError::Failed`] naming what is missing (a path or env name, never a value).
    fn owner_keys_present(&self) -> Result<(), HookError>;
}

/// Key lives in a file ([`OWNER_INFERENCE_KEY_FILE_ENV`]).
pub struct FileKeysProbe {
    /// File to probe.
    pub path: PathBuf,
}

impl FileKeysProbe {
    /// Probe `path`.
    #[must_use]
    pub fn new(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
        }
    }

    /// Probe the file named by [`OWNER_INFERENCE_KEY_FILE_ENV`], if set.
    #[must_use]
    pub fn from_env() -> Option<Self> {
        std::env::var(OWNER_INFERENCE_KEY_FILE_ENV)
            .ok()
            .map(|p| p.trim().to_owned())
            .filter(|p| !p.is_empty())
            .map(|p| Self::new(Path::new(&p)))
    }
}

impl OwnerKeysProbe for FileKeysProbe {
    fn owner_keys_present(&self) -> Result<(), HookError> {
        let present = std::fs::read_to_string(&self.path).is_ok_and(|s| !s.trim().is_empty());
        if present {
            Ok(())
        } else {
            Err(HookError::Failed(format!(
                "{} ({}) missing or empty",
                OWNER_INFERENCE_KEY_FILE_ENV,
                self.path.display()
            )))
        }
    }
}

/// Drive `owner_presend`: ask the owner, then approve or decline.
///
/// # Errors
///
/// [`StateError::Illegal`] when not at `owner_presend`; [`StateError::Hook`]
/// when the hook could not answer (state unchanged, nothing sent).
pub fn owner_presend(
    lc: &mut Lifecycle,
    hook: &dyn OwnerHook,
    prompt: &OwnerPrompt,
) -> Result<RlmState, StateError> {
    if lc.state != RlmState::OwnerPresend {
        return Err(StateError::Illegal {
            from: lc.state,
            event: RlmEvent::OwnerApproved,
        });
    }
    match hook.ask_owner(prompt)? {
        OwnerDecision::Approve => lc.apply(RlmEvent::OwnerApproved, "owner approved presend"),
        OwnerDecision::Decline { reason } => lc.apply(
            RlmEvent::OwnerDeclined,
            &format!("owner declined: {reason}"),
        ),
    }
}

/// Drive `awaiting_owner_keys`: advance only when the key file is present.
///
/// # Errors
///
/// [`StateError::Illegal`] when not at `awaiting_owner_keys`;
/// [`StateError::KeysMissing`] when the probe says no (state unchanged).
pub fn await_owner_keys(
    lc: &mut Lifecycle,
    probe: &dyn OwnerKeysProbe,
) -> Result<RlmState, StateError> {
    if lc.state != RlmState::AwaitingOwnerKeys {
        return Err(StateError::Illegal {
            from: lc.state,
            event: RlmEvent::OwnerKeysPresent,
        });
    }
    probe
        .owner_keys_present()
        .map_err(|e| StateError::KeysMissing(e.to_string()))?;
    lc.apply(RlmEvent::OwnerKeysPresent, "owner key file present")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::topic;

    #[test]
    fn the_happy_path_walks_every_state_in_ship_order() {
        let mut lc = Lifecycle::draft("topic-a");
        let steps = [
            (RlmEvent::SubmitForReview, RlmState::OwnerPresend),
            (RlmEvent::OwnerApproved, RlmState::AwaitingOwnerKeys),
            (RlmEvent::OwnerKeysPresent, RlmState::Provisioning),
            (RlmEvent::Provisioned, RlmState::Baselining),
            (RlmEvent::BaselineSealed, RlmState::Open),
            (RlmEvent::SubmissionReceived, RlmState::Evaluating),
            (RlmEvent::PromotionCandidate, RlmState::Promoting),
            (RlmEvent::Promoted, RlmState::Open),
            (RlmEvent::SubmissionReceived, RlmState::Evaluating),
            (RlmEvent::VerdictRecorded, RlmState::Open),
            (RlmEvent::Close, RlmState::Closed),
        ];
        for (event, want) in steps {
            assert_eq!(lc.apply(event, "t").expect("legal"), want);
        }
        assert_eq!(lc.history.len(), steps.len());
        assert!(lc.state.is_terminal());
        assert!(!lc.state.accepts_submissions());
        let names: Vec<&str> = RlmState::ORDER.iter().map(|s| s.as_str()).collect();
        assert_eq!(
            names,
            [
                "draft",
                "owner_presend",
                "awaiting_owner_keys",
                "provisioning",
                "baselining",
                "open",
                "evaluating",
                "promoting",
                "closed"
            ]
        );
        for s in RlmState::ORDER {
            assert_eq!(RlmState::parse(s.as_str()), Some(s));
        }
        assert_eq!(RlmState::parse("nope"), None);
        assert_eq!(RlmEvent::Close.as_str(), "close");
    }

    /// Skipping owner review, keys, provisioning, or the baseline is illegal.
    #[test]
    fn no_state_can_be_skipped_and_closed_is_terminal() {
        let mut lc = Lifecycle::draft("t");
        for event in [
            RlmEvent::OwnerApproved,
            RlmEvent::OwnerKeysPresent,
            RlmEvent::Provisioned,
            RlmEvent::BaselineSealed,
            RlmEvent::SubmissionReceived,
            RlmEvent::Promoted,
        ] {
            assert!(matches!(
                lc.apply(event, ""),
                Err(StateError::Illegal {
                    from: RlmState::Draft,
                    ..
                })
            ));
        }
        assert_eq!(
            lc.state,
            RlmState::Draft,
            "a refused move leaves state alone"
        );
        assert!(lc.history.is_empty());
        for (from, event) in [
            (RlmState::Open, RlmEvent::BaselineSealed),
            (RlmState::Open, RlmEvent::Promoted),
            (RlmState::Closed, RlmEvent::SubmissionReceived),
            (RlmState::Closed, RlmEvent::Close),
        ] {
            assert!(matches!(
                transition(from, event),
                Err(StateError::Illegal { .. })
            ));
        }
        for s in RlmState::ORDER {
            if s != RlmState::Closed {
                assert_eq!(
                    transition(s, RlmEvent::Close),
                    Ok(RlmState::Closed),
                    "{s:?}"
                );
            }
        }
        assert_eq!(
            transition(RlmState::Provisioning, RlmEvent::ProvisionFailed),
            Ok(RlmState::Draft)
        );
        assert_eq!(
            transition(RlmState::Baselining, RlmEvent::BaselineFailed),
            Ok(RlmState::Draft)
        );
        assert_eq!(
            transition(RlmState::Promoting, RlmEvent::PromotionRefused),
            Ok(RlmState::Open)
        );
    }

    /// Without a hook the machine cannot leave owner_presend: nothing is
    /// sent on the owner's behalf.
    #[test]
    fn owner_presend_needs_an_answer_and_a_decline_returns_to_draft() {
        let t = topic();
        let prompt = OwnerPrompt::from_topic(&t, Some(25.0)).expect("prompt");
        let text = prompt.render();
        for needle in [
            t.id.as_str(),
            t.statement.trim(),
            t.metric.custom_id.as_str(),
            t.constraints.model_pin.as_deref().unwrap_or("none"),
            t.constraints.task_slice.as_deref().unwrap_or("none"),
            "seed 42",
            t.checklist[0].id.as_str(),
            "25.00 USD",
        ] {
            assert!(text.contains(needle), "{needle} missing in {text}");
        }
        let mut lc = Lifecycle::draft(&t.id);
        assert!(matches!(
            owner_presend(&mut lc, &NoOwnerHook, &prompt),
            Err(StateError::Illegal { .. })
        ));
        lc.apply(RlmEvent::SubmitForReview, "").expect("review");
        assert!(matches!(
            owner_presend(&mut lc, &NoOwnerHook, &prompt),
            Err(StateError::Hook(HookError::NoHook))
        ));
        assert_eq!(lc.state, RlmState::OwnerPresend);

        let decline = StaticOwnerHook(OwnerDecision::Decline {
            reason: "budget".into(),
        });
        assert_eq!(
            owner_presend(&mut lc, &decline, &prompt).expect("declined"),
            RlmState::Draft
        );
        assert!(lc.history.last().expect("h").note.contains("budget"));

        lc.apply(RlmEvent::SubmitForReview, "")
            .expect("review again");
        assert_eq!(
            owner_presend(&mut lc, &StaticOwnerHook(OwnerDecision::Approve), &prompt)
                .expect("approved"),
            RlmState::AwaitingOwnerKeys
        );
    }

    #[test]
    fn prompts_are_for_custom_topics_only() {
        let plain = TopicDocument {
            id: "adamw-beater-v0".into(),
            ..TopicDocument::default()
        };
        assert!(matches!(
            OwnerPrompt::from_topic(&plain, None),
            Err(StateError::NotCustom(_))
        ));
        let mut bare = topic();
        bare.constraints.model_pin = None;
        bare.checklist.clear();
        let p = OwnerPrompt::from_topic(&bare, None).expect("prompt");
        let text = p.render();
        assert!(text.contains("model pin none"), "{text}");
        assert!(text.contains("inference: none"), "{text}");
        assert!(text.contains("cap: unset"), "{text}");
    }

    #[test]
    fn owner_keys_are_probed_for_presence_only() {
        let dir = std::env::temp_dir().join(format!(
            "proof-rlm-keys-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("dir");
        let key = dir.join("owner_inference_key");
        let probe = FileKeysProbe::new(&key);

        let mut lc = Lifecycle::at("t", RlmState::AwaitingOwnerKeys);
        let err = await_owner_keys(&mut lc, &probe).expect_err("missing file");
        assert!(matches!(err, StateError::KeysMissing(_)), "{err}");
        assert!(err.to_string().contains(OWNER_INFERENCE_KEY_FILE_ENV));
        assert_eq!(lc.state, RlmState::AwaitingOwnerKeys);

        std::fs::write(&key, "  \n").expect("write");
        assert!(matches!(
            await_owner_keys(&mut lc, &probe),
            Err(StateError::KeysMissing(_))
        ));

        std::fs::write(&key, "not-a-real-secret-value\n").expect("write");
        assert_eq!(
            await_owner_keys(&mut lc, &probe).expect("present"),
            RlmState::Provisioning
        );
        let dump = serde_json::to_string(&lc).expect("json");
        assert!(
            !dump.contains("not-a-real-secret-value"),
            "key value must never be recorded: {dump}"
        );

        let mut wrong = Lifecycle::draft("t");
        assert!(matches!(
            await_owner_keys(&mut wrong, &probe),
            Err(StateError::Illegal { .. })
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_signed_open_topic_without_a_seal_sits_at_baselining() {
        let mut doc = topic();
        doc.baseline.script_sha256.clear();
        doc.status = TopicStatus::Open;
        assert_eq!(Lifecycle::from_topic(&doc).state, RlmState::Baselining);
        doc.baseline.script_sha256 = "11".repeat(32);
        doc.baseline.metrics_commitment = "22".repeat(32);
        assert_eq!(Lifecycle::from_topic(&doc).state, RlmState::Open);
        doc.status = TopicStatus::Draft;
        assert_eq!(Lifecycle::from_topic(&doc).state, RlmState::Draft);
        doc.status = TopicStatus::Closed;
        assert_eq!(Lifecycle::from_topic(&doc).state, RlmState::Closed);
        let json = serde_json::to_string(&Lifecycle::from_topic(&doc)).expect("json");
        assert!(json.contains("\"state\":\"closed\""));
    }
}
