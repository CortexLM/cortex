//! Topic setup driver: the agentic lifecycle from `draft` to a sealed
//! baseline, with the RLM doing its work **inside the topic VM**.
//!
//! ```text
//! draft ──submit_for_review──▶ owner_presend ──(owner hook)──▶ awaiting_owner_keys
//!   ──(key probe)──▶ provisioning ──(orchestrator.create)──▶ baselining
//!   ──(ProposeRules job → rules vN in store; Baseline job → measurement in store)──▶
//!   returns SetupOutcome; the operator seals custom_value, re-signs `open`,
//!   and calls `mark_sealed` (baselining → open).
//! ```
//!
//! The control plane never runs RLM logic: it forwards jobs through
//! [`TopicVmOrchestrator`] and persists what comes back. Every transition
//! lands in the store. A missing orchestrator, a declined owner, or a
//! missing key file stops the driver where it is, with the reason, and a
//! re-run resumes from the persisted state.

use std::sync::Arc;

use proof_rlm::{
    await_owner_keys, owner_presend, CustomRunReport, CustomRunRequest, Lifecycle, OwnerHook,
    OwnerKeysProbe, OwnerPrompt, RlmEvent, RlmState, RuleSet, RuleSource, SandboxPolicy,
    StateError, TopicVmOrchestrator, TopicVmSpec, VmError, VmHandle, VmJob, VmJobOutput,
    VmTemplate,
};
use proof_rlm_store::{BaselineRow, RlmStore, StoreError, TransitionRow};
use proof_task::{InferenceOffer, MetricFamily, ProofPin, TopicDocument};

/// Why setup stopped.
#[derive(Debug, thiserror::Error)]
pub enum SetupError {
    /// Not a custom-family topic (nothing for an RLM to do).
    #[error("topic {0:?} is not a custom-family topic")]
    NotCustom(String),
    /// Lifecycle refused (owner declined, keys missing, illegal move).
    #[error("lifecycle: {0}")]
    State(#[from] StateError),
    /// The orchestrator refused or failed.
    #[error("topic vm: {0}")]
    Vm(#[from] VmError),
    /// The store refused.
    #[error("store: {0}")]
    Store(#[from] StoreError),
    /// The RLM proposed no usable rules.
    #[error("rules: {0}")]
    Rules(#[from] proof_rlm::ChecklistError),
    /// The baseline report did not bind to the request.
    #[error("baseline report: {0}")]
    Report(#[from] proof_rlm::ReportError),
    /// The owner declined at presend; the topic is back at draft.
    #[error("owner declined; topic {0:?} returned to draft")]
    Declined(String),
}

/// What setup produced for the operator to seal.
#[derive(Debug, Clone, PartialEq)]
pub struct SetupOutcome {
    /// Topic id.
    pub topic_id: String,
    /// The topic's VM.
    pub vm: VmHandle,
    /// Rule version in force after the RLM wrote its rules.
    pub rules_version: u32,
    /// Baseline primary the operator seals as `custom_value`.
    pub baseline_primary: f64,
}

/// Everything the driver needs; no secrets.
pub struct TopicSetup {
    /// VM boundary.
    pub orchestrator: Arc<dyn TopicVmOrchestrator>,
    /// Persistence.
    pub store: Arc<dyn RlmStore>,
    /// RLM VM image + sizes.
    pub template: VmTemplate,
    /// `askUser`-style owner hook.
    pub owner: Arc<dyn OwnerHook>,
    /// Owner key presence probe.
    pub keys: Arc<dyn OwnerKeysProbe>,
    /// Spend cap shown to the owner, if any.
    pub spend_cap_usd: Option<f64>,
}

impl TopicSetup {
    async fn lifecycle(&self, topic: &TopicDocument) -> Result<Lifecycle, SetupError> {
        Ok(self
            .store
            .lifecycle(&topic.id)
            .await?
            .unwrap_or_else(|| Lifecycle::from_topic(topic)))
    }

    /// Persist every transition after `from`.
    async fn record(&self, lc: &Lifecycle, from: usize) -> Result<(), SetupError> {
        for t in lc.history.iter().skip(from) {
            self.store
                .record_transition(&TransitionRow {
                    topic_id: lc.topic_id.clone(),
                    from: t.from,
                    event: t.event,
                    to: t.to,
                    note: t.note.clone(),
                })
                .await?;
        }
        Ok(())
    }

    /// Apply one event and persist it.
    async fn step(
        &self,
        lc: &mut Lifecycle,
        event: RlmEvent,
        note: &str,
    ) -> Result<RlmState, SetupError> {
        let mark = lc.history.len();
        let to = lc.apply(event, note)?;
        self.record(lc, mark).await?;
        Ok(to)
    }

    /// `draft → owner_presend → awaiting_owner_keys → provisioning`.
    async fn owner_phase(
        &self,
        topic: &TopicDocument,
        lc: &mut Lifecycle,
    ) -> Result<(), SetupError> {
        let prompt = OwnerPrompt::from_topic(topic, self.spend_cap_usd)?;
        if lc.state == RlmState::Draft {
            self.step(lc, RlmEvent::SubmitForReview, "setup").await?;
        }
        if lc.state == RlmState::OwnerPresend {
            let mark = lc.history.len();
            let to = owner_presend(lc, self.owner.as_ref(), &prompt)?;
            self.record(lc, mark).await?;
            if to == RlmState::Draft {
                return Err(SetupError::Declined(topic.id.clone()));
            }
        }
        if lc.state == RlmState::AwaitingOwnerKeys {
            await_owner_keys(lc, self.keys.as_ref())?;
            self.record(lc, lc.history.len().saturating_sub(1)).await?;
        }
        Ok(())
    }

    /// Provision (or attach to) the topic's own VM: `provisioning → baselining`.
    async fn provision(
        &self,
        topic: &TopicDocument,
        pin: &ProofPin,
        lc: &mut Lifecycle,
    ) -> Result<VmHandle, SetupError> {
        self.orchestrator.ready()?;
        if let Some(h) = self.orchestrator.attach(&topic.id).await? {
            if lc.state == RlmState::Provisioning {
                self.step(lc, RlmEvent::Provisioned, &h.vm_id).await?;
            }
            return Ok(h);
        }
        let sandbox = SandboxPolicy {
            firecracker_required: topic.constraints.firecracker_required,
            deadline_s: topic
                .eval_executor
                .max_proof_deadline_s
                .unwrap_or(pin.max_proof_deadline_s),
        };
        let spec = TopicVmSpec::for_topic(&topic.id, self.template.clone(), sandbox);
        spec.validate()?;
        match self.orchestrator.create(&spec).await {
            Ok(h) => {
                if lc.state == RlmState::Provisioning {
                    self.step(lc, RlmEvent::Provisioned, &h.vm_id).await?;
                }
                Ok(h)
            }
            Err(e) => {
                if lc.state == RlmState::Provisioning {
                    self.step(lc, RlmEvent::ProvisionFailed, &e.to_string())
                        .await?;
                }
                Err(e.into())
            }
        }
    }

    /// The RLM writes its rules inside the VM; the store versions them.
    async fn propose_rules(
        &self,
        topic: &TopicDocument,
        vm: &VmHandle,
    ) -> Result<RuleSet, SetupError> {
        let current = self.store.current_rules(&topic.id).await?;
        let job = VmJob::ProposeRules {
            topic: Box::new(topic.clone()),
            current_version: current.as_ref().map(|r| r.version),
        };
        let VmJobOutput::Rules(proposed) = self.orchestrator.run(vm, job).await? else {
            return Err(VmError::WrongOutput("propose_rules").into());
        };
        let rules = if let Some(cur) = current {
            cur.next(RuleSource::Rlm, proposed)?
        } else {
            let set = RuleSet {
                topic_id: topic.id.clone(),
                version: 1,
                source: RuleSource::Rlm,
                rules: proposed,
            };
            set.validate()?;
            set
        };
        self.store.put_rules(&rules).await?;
        Ok(rules)
    }

    /// Baseline inside the VM, shaped exactly like a miner run, persisted.
    async fn baseline(
        &self,
        topic: &TopicDocument,
        pin: &ProofPin,
        offer: &InferenceOffer,
        rules: &RuleSet,
        vm: &VmHandle,
        lc: &mut Lifecycle,
    ) -> Result<CustomRunReport, SetupError> {
        let request = CustomRunRequest::from_topic(
            topic,
            pin,
            offer,
            rules,
            &format!("baseline-{}", rules.digest()),
            &topic.baseline.script_sha256,
            None,
            "operator baseline",
        )
        .map_err(|e| SetupError::Vm(VmError::Backend(e.to_string())))?;
        let job = VmJob::Baseline {
            request: request.clone(),
        };
        let report = match self.orchestrator.run(vm, job).await {
            Ok(VmJobOutput::Baseline(r)) => r,
            Ok(_) => return Err(VmError::WrongOutput("baseline").into()),
            Err(e) => {
                self.step(lc, RlmEvent::BaselineFailed, &e.to_string())
                    .await?;
                return Err(e.into());
            }
        };
        report.verify(&request)?;
        self.store
            .put_baseline(&BaselineRow {
                topic_id: topic.id.clone(),
                rules_version: rules.version,
                primary_value: report.primary_value,
                report: report.clone(),
            })
            .await?;
        Ok(report)
    }

    /// Drive `draft → … → baselining` and run the RLM's rule + baseline jobs.
    ///
    /// # Errors
    ///
    /// See [`SetupError`]. The lifecycle is left where the failure happened
    /// (persisted), so a re-run resumes rather than restarts.
    pub async fn run(
        &self,
        topic: &TopicDocument,
        pin: &ProofPin,
        offer: &InferenceOffer,
    ) -> Result<SetupOutcome, SetupError> {
        if topic.metric.family != MetricFamily::Custom {
            return Err(SetupError::NotCustom(topic.id.clone()));
        }
        if self.store.latest_topic(&topic.id).await?.is_none() {
            self.store.put_topic_version(topic).await?;
        }
        let mut lc = self.lifecycle(topic).await?;
        self.owner_phase(topic, &mut lc).await?;
        if lc.state != RlmState::Provisioning && lc.state != RlmState::Baselining {
            return Err(StateError::Illegal {
                from: lc.state,
                event: RlmEvent::Provisioned,
            }
            .into());
        }
        let vm = self.provision(topic, pin, &mut lc).await?;
        let rules = self.propose_rules(topic, &vm).await?;
        let report = self
            .baseline(topic, pin, offer, &rules, &vm, &mut lc)
            .await?;
        Ok(SetupOutcome {
            topic_id: topic.id.clone(),
            vm,
            rules_version: rules.version,
            baseline_primary: report.primary_value,
        })
    }

    /// The operator sealed the baseline and re-signed `status: open`:
    /// `baselining → open`, recorded, and the new document version stored.
    ///
    /// # Errors
    ///
    /// [`SetupError::State`] when the topic is not at `baselining`.
    pub async fn mark_sealed(&self, topic: &TopicDocument) -> Result<RlmState, SetupError> {
        let mut lc = self.lifecycle(topic).await?;
        let to = self
            .step(
                &mut lc,
                RlmEvent::BaselineSealed,
                "operator sealed the baseline",
            )
            .await?;
        self.store.put_topic_version(topic).await?;
        Ok(to)
    }
}
