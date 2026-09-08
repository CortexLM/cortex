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
    check_template_id, is_lium_template_uuid, require_open_executor, shape_gpu_count,
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
    /// Lium template id or digest-scoped template name to rent.
    pub template_id: String,
    /// `true` when `template_id` is a raw Lium UUID (rent verbatim) rather
    /// than a template name to resolve / create bound to the pinned digest.
    pub template_is_uuid: bool,
    /// Exact GPUs to rent. Always [`EVAL_EXECUTOR_GPU_COUNT`].
    pub gpu_count: u32,
    /// Proof deadline the pod-side `timeout` and the harvest wait enforce.
    pub deadline_s: u64,
    /// Offer `config_commitment` stamped onto the run request.
    pub config_commitment: String,
}

/// Resolve the plan for scoring `topic` on `offer` under `pin`.
///
/// # Errors
///
/// Any [`ExecutorOfferError`]: the offer must be open and valid, the
/// template legal, the width exactly the pin, and the deadline within the
/// ceiling after the topic tighten and the operator override.
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

    Ok(ExecutorPlan {
        offer_id: offer.offer_id.clone(),
        topic_id: topic.id.clone(),
        template_is_uuid: is_lium_template_uuid(&template_id),
        template_id,
        gpu_count,
        deadline_s,
        config_commitment: offer.config_commitment.clone(),
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
        assert!(!plan.template_is_uuid);
        assert_eq!(plan.gpu_count, 1);
        assert_eq!(plan.deadline_s, 7_200);
        assert_eq!(plan.config_commitment, offer().config_commitment);
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
        // The template override obeys the allowlist and the pinned digest.
        let off_list = HarvestOverrides {
            template_id: Some("prism-recipe-v10".into()),
            ..HarvestOverrides::default()
        };
        assert!(matches!(
            executor_plan(&p, Some(&offer()), &topic(), &off_list),
            Err(ExecutorOfferError::TemplateNotAllowed(_))
        ));
        let mut open = p.clone();
        open.allowed_lium_template_prefixes.clear();
        let uuid = HarvestOverrides {
            template_id: Some("f2f5e84c-3b09-4090-be83-1913eabd009e".into()),
            ..HarvestOverrides::default()
        };
        let plan = executor_plan(&open, Some(&offer()), &topic(), &uuid).expect("plan");
        assert!(plan.template_is_uuid);
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
