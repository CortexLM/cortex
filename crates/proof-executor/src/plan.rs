//! Resolved rent plan: pin ceilings + live offer + topic tighten + operator
//! env hot-swap, collapsed into what one harvest is allowed to do.
//!
//! The env overrides exist so an operator can swap the template, width, or
//! deadline without a rebuild. They replace the **offer's** values; the pin
//! ceilings still bind, and a value outside them refuses the rent instead of
//! being clamped. An unparseable override is also a refusal — never a silent
//! fall back to the offer.

use proof_task::{ProofPin, TopicDocument, EVAL_EXECUTOR_GPU_COUNT};

use crate::{
    check_template_id, executor_config_commitment, require_open_executor, shape_gpu_count,
    EvalExecutorOffer, ExecutorOfferError,
};

/// Env override: Lium template id / digest-scoped template name to rent.
pub const HARVEST_TEMPLATE_ID_ENV: &str = "PROOF_HARVEST_TEMPLATE_ID";

/// Env override: GPUs to rent. Anything but the pinned width aborts.
pub const HARVEST_GPU_COUNT_ENV: &str = "PROOF_HARVEST_GPU_COUNT";

/// Env override: proof deadline in seconds. Still `<=` the pin ceiling.
pub const HARVEST_DEADLINE_SECS_ENV: &str = "PROOF_HARVEST_DEADLINE_SECS";

/// Operator env hot-swap of the executor plan. `None` = use the offer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HarvestOverrides {
    /// [`HARVEST_TEMPLATE_ID_ENV`].
    pub template_id: Option<String>,
    /// [`HARVEST_GPU_COUNT_ENV`].
    pub gpu_count: Option<u32>,
    /// [`HARVEST_DEADLINE_SECS_ENV`].
    pub deadline_secs: Option<u64>,
}

fn env_value(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

impl HarvestOverrides {
    /// Read the three `PROOF_HARVEST_*` variables from the process env.
    ///
    /// # Errors
    ///
    /// [`ExecutorOfferError::BadOverride`] when a set variable is unparseable.
    pub fn from_env() -> Result<Self, ExecutorOfferError> {
        Self::parse(
            env_value(HARVEST_TEMPLATE_ID_ENV).as_deref(),
            env_value(HARVEST_GPU_COUNT_ENV).as_deref(),
            env_value(HARVEST_DEADLINE_SECS_ENV).as_deref(),
        )
    }

    /// Pure core of [`Self::from_env`]. Blank values are unset.
    ///
    /// # Errors
    ///
    /// [`ExecutorOfferError::BadOverride`] when a value is present but unparseable.
    pub fn parse(
        template_id: Option<&str>,
        gpu_count: Option<&str>,
        deadline_secs: Option<&str>,
    ) -> Result<Self, ExecutorOfferError> {
        fn blank(v: Option<&str>) -> Option<&str> {
            v.map(str::trim).filter(|s| !s.is_empty())
        }
        let gpu_count = match blank(gpu_count) {
            Some(raw) => Some(
                raw.parse::<u32>()
                    .map_err(|_| ExecutorOfferError::BadOverride(HARVEST_GPU_COUNT_ENV))?,
            ),
            None => None,
        };
        let deadline_secs = match blank(deadline_secs) {
            Some(raw) => Some(
                raw.parse::<u64>()
                    .map_err(|_| ExecutorOfferError::BadOverride(HARVEST_DEADLINE_SECS_ENV))?,
            ),
            None => None,
        };
        Ok(Self {
            template_id: blank(template_id).map(str::to_owned),
            gpu_count,
            deadline_secs,
        })
    }

    /// True when no override is set.
    pub fn is_empty(&self) -> bool {
        self.template_id.is_none() && self.gpu_count.is_none() && self.deadline_secs.is_none()
    }
}

/// What one harvest rent is allowed to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutorPlan {
    /// Offer this plan was resolved from.
    pub offer_id: String,
    /// Topic the rent is scoped to. Carried so a topic-scoped attach (the
    /// per-topic judge VM the pod reports to) can key on the plan without
    /// changing its shape; this crate does not interpret the id.
    pub topic_id: String,
    /// Digest-scoped template name to rent (resolved / created bound to the
    /// pinned `eval_image@digest`; never a raw Lium UUID).
    pub template_id: String,
    /// Exact GPUs to rent. Always [`EVAL_EXECUTOR_GPU_COUNT`].
    pub gpu_count: u32,
    /// Proof deadline the pod-side `timeout` and the harvest wait enforce.
    pub deadline_s: u64,
    /// The live offer's `config_commitment` (what a topic may pin).
    pub offer_commitment: String,
    /// Commitment of the configuration that actually runs: `template_id`,
    /// shape, `deadline_s`, digest. Equals `offer_commitment` only when no
    /// override and no topic tighten changed the offer's knobs. This is what
    /// the run request and the scored row carry as executor provenance.
    pub config_commitment: String,
    /// `true` when an operator `PROOF_HARVEST_*` override changed the
    /// template or the deadline away from the offer.
    pub overridden: bool,
}

/// Resolve the plan for scoring `topic` on `offer` under `pin`.
///
/// # Errors
///
/// Any [`ExecutorOfferError`]: the offer must be open and valid, the
/// template legal, the width exactly the pin, the deadline within the
/// ceiling after the topic tighten and the operator override, and — when
/// the topic pins the offer commitment — no override may change what runs.
pub fn executor_plan(
    pin: &ProofPin,
    offer: Option<&EvalExecutorOffer>,
    topic: &TopicDocument,
    overrides: &HarvestOverrides,
) -> Result<ExecutorPlan, ExecutorOfferError> {
    let offer = require_open_executor(offer, pin)?;
    offer.serves_topic(topic)?;

    let template_id = overrides
        .template_id
        .as_deref()
        .map_or(offer.lium_template_id.trim(), str::trim)
        .to_owned();
    check_template_id(pin, &template_id)?;

    let want = shape_gpu_count(&pin.gpu_class).unwrap_or(EVAL_EXECUTOR_GPU_COUNT);
    let gpu_count = overrides
        .gpu_count
        .or_else(|| offer.gpu_count())
        .unwrap_or(0);
    if gpu_count != want || want != EVAL_EXECUTOR_GPU_COUNT {
        return Err(ExecutorOfferError::GpuCount(
            gpu_count,
            EVAL_EXECUTOR_GPU_COUNT,
        ));
    }

    let base = overrides
        .deadline_secs
        .unwrap_or(offer.max_proof_deadline_s);
    let overridden =
        template_id != offer.lium_template_id.trim() || base != offer.max_proof_deadline_s;
    // A topic that pinned the offer commitment approved *that* template and
    // deadline. An operator override that changes either would run a
    // configuration the topic never approved: refuse, never re-stamp.
    if overridden && topic.eval_executor.require_offer_commitment.is_some() {
        return Err(ExecutorOfferError::OverrideBreaksCommitment);
    }
    let deadline_s = topic
        .eval_executor
        .max_proof_deadline_s
        .map_or(base, |t| t.min(base));
    if deadline_s == 0 || deadline_s > pin.max_proof_deadline_s_ceiling {
        return Err(ExecutorOfferError::BadDeadline(
            deadline_s,
            pin.max_proof_deadline_s_ceiling,
        ));
    }

    let config_commitment = executor_config_commitment(
        &template_id,
        &offer.machine_shape,
        deadline_s,
        &offer.eval_image_digest,
    );
    Ok(ExecutorPlan {
        offer_id: offer.offer_id.clone(),
        topic_id: topic.id.clone(),
        template_id,
        gpu_count,
        deadline_s,
        offer_commitment: offer.config_commitment.clone(),
        config_commitment,
        overridden,
    })
}

#[cfg(test)]
mod tests {
    use proof_task::{OfferStatus, TopicEvalExecutor};

    use super::*;
    use crate::fixtures::{offer, offer_for, pin};

    fn topic() -> TopicDocument {
        TopicDocument {
            id: "any-open-topic-v0".into(),
            ..TopicDocument::default()
        }
    }

    #[test]
    fn plan_uses_the_open_offer_template_deadline_and_exactly_one_gpu() {
        let plan = executor_plan(
            &pin(),
            Some(&offer()),
            &topic(),
            &HarvestOverrides::default(),
        )
        .expect("plan");
        assert_eq!(plan.offer_id, "lium-1x-v0");
        assert_eq!(
            plan.topic_id, "any-open-topic-v0",
            "topic scope rides on the plan"
        );
        assert_eq!(plan.template_id, "proof-eval-78b614a1f51c");
        assert_eq!(plan.gpu_count, 1);
        assert_eq!(plan.deadline_s, 7_200);
        assert!(!plan.overridden);
        assert_eq!(plan.offer_commitment, offer().config_commitment);
        assert_eq!(
            plan.config_commitment, plan.offer_commitment,
            "untouched offer: the executed configuration is the committed one"
        );
    }

    #[test]
    fn missing_closed_or_wrong_shape_offer_has_no_plan() {
        let none = HarvestOverrides::default();
        assert!(matches!(
            executor_plan(&pin(), None, &topic(), &none),
            Err(ExecutorOfferError::Missing)
        ));
        let mut closed = offer();
        closed.status = OfferStatus::Closed;
        assert!(matches!(
            executor_plan(&pin(), Some(&closed), &topic(), &none),
            Err(ExecutorOfferError::Closed)
        ));
        let mut wide = offer();
        wide.machine_shape = "8x".into();
        wide.config_commitment = wide.expected_commitment();
        assert!(matches!(
            executor_plan(&pin(), Some(&wide), &topic(), &none),
            Err(ExecutorOfferError::ShapeMismatch { .. })
        ));
    }

    #[test]
    fn topic_tightens_the_deadline_and_may_pin_the_commitment() {
        let mut t = topic();
        t.eval_executor = TopicEvalExecutor {
            require_offer_commitment: Some(offer().config_commitment),
            max_proof_deadline_s: Some(900),
        };
        let plan =
            executor_plan(&pin(), Some(&offer()), &t, &HarvestOverrides::default()).expect("plan");
        assert_eq!(plan.deadline_s, 900);
        assert!(
            !plan.overridden,
            "a topic tighten is signed, not an override"
        );
        assert_eq!(plan.offer_commitment, offer().config_commitment);
        assert_eq!(
            plan.config_commitment,
            executor_config_commitment(
                "proof-eval-78b614a1f51c",
                "1x",
                900,
                &pin().eval_image_digest
            ),
            "provenance commits the executed 900s, not the offer's 7200s"
        );
        t.eval_executor.require_offer_commitment = Some("ab".repeat(32));
        assert!(matches!(
            executor_plan(&pin(), Some(&offer()), &t, &HarvestOverrides::default()),
            Err(ExecutorOfferError::CannotServeTopic)
        ));
    }

    #[test]
    fn overrides_swap_template_and_deadline_but_pin_ceilings_still_bind() {
        let p = pin();
        let swapped = HarvestOverrides {
            template_id: Some("proof-eval-78b614a1f51c-hotfix".into()),
            gpu_count: Some(1),
            deadline_secs: Some(3_600),
        };
        let plan = executor_plan(&p, Some(&offer()), &topic(), &swapped).expect("plan");
        assert_eq!(plan.template_id, "proof-eval-78b614a1f51c-hotfix");
        assert_eq!(plan.deadline_s, 3_600);
        assert!(plan.overridden);
        assert_eq!(plan.offer_commitment, offer().config_commitment);
        assert_ne!(
            plan.config_commitment, plan.offer_commitment,
            "an override must not be stamped as the offer's committed config"
        );
        assert_eq!(
            plan.config_commitment,
            executor_config_commitment(
                "proof-eval-78b614a1f51c-hotfix",
                "1x",
                3_600,
                &p.eval_image_digest
            )
        );

        // The override replaces the offer deadline, so a longer one is legal
        // up to the ceiling — never past it.
        let short_offer = offer_for("proof-eval-78b614a1f51c", 600, &p);
        let longer = HarvestOverrides {
            deadline_secs: Some(7_200),
            ..HarvestOverrides::default()
        };
        assert_eq!(
            executor_plan(&p, Some(&short_offer), &topic(), &longer)
                .expect("plan")
                .deadline_s,
            7_200
        );
        let past = HarvestOverrides {
            deadline_secs: Some(7_201),
            ..HarvestOverrides::default()
        };
        assert!(matches!(
            executor_plan(&p, Some(&offer()), &topic(), &past),
            Err(ExecutorOfferError::BadDeadline(7_201, 7_200))
        ));
        let zero = HarvestOverrides {
            deadline_secs: Some(0),
            ..HarvestOverrides::default()
        };
        assert!(matches!(
            executor_plan(&p, Some(&offer()), &topic(), &zero),
            Err(ExecutorOfferError::BadDeadline(0, 7_200))
        ));
        // A topic tighten still applies on top of the override.
        let mut t = topic();
        t.eval_executor.max_proof_deadline_s = Some(300);
        assert_eq!(
            executor_plan(&p, Some(&offer()), &t, &longer)
                .expect("plan")
                .deadline_s,
            300
        );
        // The template override obeys the pinned digest and the allowlist.
        let unbound = HarvestOverrides {
            template_id: Some("prism-recipe-v10".into()),
            ..HarvestOverrides::default()
        };
        assert!(matches!(
            executor_plan(&p, Some(&offer()), &topic(), &unbound),
            Err(ExecutorOfferError::TemplateDigestMismatch(..))
        ));
        let off_list = HarvestOverrides {
            template_id: Some("other-78b614a1f51c".into()),
            ..HarvestOverrides::default()
        };
        assert!(matches!(
            executor_plan(&p, Some(&offer()), &topic(), &off_list),
            Err(ExecutorOfferError::TemplateNotAllowed(_))
        ));
        // A raw UUID override is refused like a raw UUID offer would be.
        let mut open = p.clone();
        open.allowed_lium_template_prefixes.clear();
        let uuid = HarvestOverrides {
            template_id: Some("f2f5e84c-3b09-4090-be83-1913eabd009e".into()),
            ..HarvestOverrides::default()
        };
        assert!(matches!(
            executor_plan(&open, Some(&offer()), &topic(), &uuid),
            Err(ExecutorOfferError::RawTemplateId(_))
        ));
    }

    /// A topic that pinned the offer commitment approved that template and
    /// deadline; an override that changes either is refused rather than run
    /// under the old stamp. A no-op override (same width) is still fine.
    #[test]
    fn commitment_pinned_topic_refuses_config_changing_overrides() {
        let p = pin();
        let mut t = topic();
        t.eval_executor.require_offer_commitment = Some(offer().config_commitment);
        for over in [
            HarvestOverrides {
                template_id: Some("proof-eval-78b614a1f51c-hotfix".into()),
                ..HarvestOverrides::default()
            },
            HarvestOverrides {
                deadline_secs: Some(3_600),
                ..HarvestOverrides::default()
            },
            HarvestOverrides {
                deadline_secs: Some(7_200 - 1),
                ..HarvestOverrides::default()
            },
        ] {
            assert!(
                matches!(
                    executor_plan(&p, Some(&offer()), &t, &over),
                    Err(ExecutorOfferError::OverrideBreaksCommitment)
                ),
                "{over:?} must not run under a pinned commitment"
            );
        }
        // Same values as the offer, or only the (no-op) width: not a change.
        for over in [
            HarvestOverrides {
                template_id: Some("proof-eval-78b614a1f51c".into()),
                gpu_count: Some(1),
                deadline_secs: Some(7_200),
            },
            HarvestOverrides {
                gpu_count: Some(1),
                ..HarvestOverrides::default()
            },
        ] {
            let plan = executor_plan(&p, Some(&offer()), &t, &over).expect("no-op override");
            assert!(!plan.overridden);
            assert_eq!(plan.config_commitment, plan.offer_commitment);
        }
        // Without the pin the same override is legal and re-committed.
        let mut unpinned = topic();
        unpinned.eval_executor.max_proof_deadline_s = Some(900);
        let plan = executor_plan(
            &p,
            Some(&offer()),
            &unpinned,
            &HarvestOverrides {
                template_id: Some("proof-eval-78b614a1f51c-hotfix".into()),
                ..HarvestOverrides::default()
            },
        )
        .expect("plan");
        assert!(plan.overridden);
        assert_ne!(plan.config_commitment, plan.offer_commitment);
    }

    #[test]
    fn any_gpu_count_but_one_aborts() {
        for n in [0u32, 2, 8] {
            let over = HarvestOverrides {
                gpu_count: Some(n),
                ..HarvestOverrides::default()
            };
            assert!(
                matches!(
                    executor_plan(&pin(), Some(&offer()), &topic(), &over),
                    Err(ExecutorOfferError::GpuCount(got, 1)) if got == n
                ),
                "{n}x must abort"
            );
        }
        let one = HarvestOverrides {
            gpu_count: Some(1),
            ..HarvestOverrides::default()
        };
        executor_plan(&pin(), Some(&offer()), &topic(), &one).expect("1x is the pin");
    }

    #[test]
    fn overrides_parse_blank_as_unset_and_garbage_as_refusal() {
        let none = HarvestOverrides::parse(None, Some("  "), Some("")).expect("blank");
        assert!(none.is_empty());
        let set = HarvestOverrides::parse(Some(" tmpl-1 "), Some("1"), Some("600")).expect("set");
        assert_eq!(
            set,
            HarvestOverrides {
                template_id: Some("tmpl-1".into()),
                gpu_count: Some(1),
                deadline_secs: Some(600),
            }
        );
        assert!(matches!(
            HarvestOverrides::parse(None, Some("one"), None),
            Err(ExecutorOfferError::BadOverride(HARVEST_GPU_COUNT_ENV))
        ));
        assert!(matches!(
            HarvestOverrides::parse(None, None, Some("-5")),
            Err(ExecutorOfferError::BadOverride(HARVEST_DEADLINE_SECS_ENV))
        ));
        assert_eq!(HARVEST_TEMPLATE_ID_ENV, "PROOF_HARVEST_TEMPLATE_ID");
        assert_eq!(HARVEST_GPU_COUNT_ENV, "PROOF_HARVEST_GPU_COUNT");
        assert_eq!(HARVEST_DEADLINE_SECS_ENV, "PROOF_HARVEST_DEADLINE_SECS");
    }
}
