//! Topic setup driver: the agentic lifecycle from `draft` to a sealed
//! baseline, with the RLM doing its work **inside the topic VM**.
//!
//! ```text
//! draft ──submit_for_review──▶ owner_presend ──(owner hook)──▶ awaiting_owner_keys
//!   ──(key probe)──▶ provisioning ──(orchestrator.create)──▶ baselining
//!   ──(ProposeRules job → rules vN in store; Baseline job → measurement in store)──▶
//!   returns SetupOutcome; the operator seals custom_value, re-signs `open`,
//!   and calls `mark_sealed` (baselining → open) with the signed open
//!   document and the sealed measurement — both are checked before the move.
//! ```
//!
//! The control plane never runs RLM logic: it forwards jobs through
//! [`TopicVmOrchestrator`] and persists what comes back. Every transition
//! lands in the store. A missing orchestrator, a declined owner, or a
//! missing key file stops the driver where it is, with the reason, and a
//! re-run resumes from the persisted state.
//!
//! # This crate exists so `proof-admin` can drive it
//!
//! The driver lives in its own crate because it has **two** callers with
//! different lifetimes: the challenge service drives it per submission, and
//! the operator CLI (`proof-admin topic install --drive-rlm`) drives it once
//! at install time. Keeping it beside the scorer would have pushed that crate
//! past the repository's per-crate LOC cap, and the driver has no dependency
//! on scoring: it needs the VM boundary, the store, and the lifecycle.

#![forbid(unsafe_code)]
#![allow(
    clippy::missing_errors_doc,
    clippy::doc_markdown,
    clippy::module_name_repetitions,
    clippy::must_use_candidate
)]

use std::sync::Arc;

use proof_eval::BaselineMeasurement;
use proof_rlm::{
    await_owner_keys, owner_presend, run_paid_job, CustomRunReport, CustomRunRequest,
    ExperimentPolicy, Lifecycle, OwnerHook, OwnerKeysProbe, OwnerPrompt, RlmEvent, RlmState,
    RuleSet, RuleSource, SandboxPolicy, StateError, TopicVmOrchestrator, TopicVmSpec, VmError,
    VmHandle, VmJob, VmJobOutput, VmTemplate,
};
use proof_rlm_store::{BaselineRow, RlmStore, StoreError, TransitionRow};
use proof_task::{InferenceOffer, MetricFamily, ProofPin, TopicDocument, TopicError, TopicStatus};

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
    /// `mark_sealed` was handed a document that is not `status: open`.
    #[error("topic {0:?} is not an open document; nothing to open")]
    NotOpen(String),
    /// The document does not validate as an open topic or does not verify
    /// under the pin's topic key.
    #[error("topic document: {0}")]
    Topic(#[from] TopicError),
    /// The sealed measurement does not bind to the document, or is not what
    /// the RLM measured in the topic VM.
    #[error("seal: {0}")]
    Seal(String),
    /// The measured baseline is a bar no challenger can ever clear.
    ///
    /// A relative-win family (`throughput` / `custom`) compares a challenger
    /// against the sealed value with `challenger >= bar * (1 + epsilon_rel)`,
    /// so a bar at ~zero has no solution: the topic would be open, scorable,
    /// and permanently unwinnable by anyone. That is what an all-zero
    /// reference run measures (LIVE Gate 1: five Harbor tasks, every one
    /// `0.0`), and sealing it would publish a dead topic.
    ///
    /// Refused **without** touching the stored measurement and **without**
    /// auto-resealing: the operator re-runs the baseline against a reference
    /// that can score, or re-scopes the task set, and seals the new number.
    #[error(
        "topic {topic_id:?}: the measured baseline {primary} is a degenerate bar — this family \
         scores a relative win (`challenger >= bar * (1 + epsilon_rel)`), so a bar at zero can \
         never be cleared by anyone and the topic would be open but unwinnable. Nothing was \
         changed. Re-run the baseline against a reference that can score (or fix the task \
         selection so the reference run actually measures something), then seal that number"
    )]
    DegenerateBar {
        /// The topic whose baseline is degenerate.
        topic_id: String,
        /// The measured primary that cannot be a bar.
        primary: f64,
    },
    /// A baseline would be measured but there is no judge offer to bind it to.
    #[error(
        "no inference offer: the baseline is a paid run and needs a live judge offer to bind \
         (or set skip_baseline, which measures none)"
    )]
    NoOffer,
    /// The rule version in force is not RLM-authored.
    ///
    /// The topic's behavior has to be authored by its own RLM inside the topic
    /// VM. A vector still carrying the signed document's provenance
    /// (`topic_document`) or an operator's edit (`operator`) means setup never
    /// got the RLM to write rules, so nothing downstream may treat this topic
    /// as set up.
    #[error(
        "topic {topic_id:?}: rule version {version} is not RLM-authored (source {provenance}); the \
         topic's behavior is still the operator's document, so setup did not complete"
    )]
    RulesNotRlmAuthored {
        /// The topic whose rules were read back.
        topic_id: String,
        /// The provenance found (`topic_document` / `operator` / no version).
        provenance: String,
        /// The version that was read back.
        version: u32,
    },
    /// A rule version other than the one this run wrote is in force.
    ///
    /// The baseline is measured and persisted **against a rule version**, so
    /// the two have to agree: a concurrent writer that advanced the store
    /// while this run was measuring would otherwise leave a sealed bar
    /// measured under rules nobody scores with. Nothing is persisted — the
    /// lifecycle stays where it stopped, so a re-run resumes.
    #[error(
        "topic {topic_id:?}: rule version {wrote} was written but version {} is now in force; \
         the baseline would have sealed a measurement taken under rules that no longer apply, so \
         nothing was persisted",
        match in_force {
            Some(v) => v.to_string(),
            None => "none".to_owned(),
        }
    )]
    RulesSuperseded {
        /// The topic whose rules moved.
        topic_id: String,
        /// The version this run wrote and measured against.
        wrote: u32,
        /// The version now in force (`None` when no rule row exists).
        in_force: Option<u32>,
    },
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
    ///
    /// `None` when [`TopicSetup::skip_baseline`] was set: no baseline was
    /// measured, so there is nothing to seal and the topic cannot open yet.
    pub baseline_primary: Option<f64>,
}

impl SetupOutcome {
    /// Whether this run measured a baseline.
    #[must_use]
    pub const fn measured_baseline(&self) -> bool {
        self.baseline_primary.is_some()
    }
}

/// Everything the driver needs; no secrets.
pub struct TopicSetup {
    /// VM boundary.
    pub orchestrator: Arc<dyn TopicVmOrchestrator>,
    /// Persistence.
    pub store: Arc<dyn RlmStore>,
    /// RLM VM image + sizes.
    pub template: VmTemplate,
    /// Per-experiment VM policy: a topic whose params select an in-guest
    /// runner measures its baseline in a dedicated VM, destroyed afterwards
    /// (retained on the host for root-cause analysis when the run failed).
    pub experiments: ExperimentPolicy,
    /// `askUser`-style owner hook.
    pub owner: Arc<dyn OwnerHook>,
    /// Owner key presence probe.
    pub keys: Arc<dyn OwnerKeysProbe>,
    /// Spend cap shown to the owner, if any.
    pub spend_cap_usd: Option<f64>,
    /// Stop after the RLM's rules are installed, before the baseline job.
    ///
    /// The operator CLI sets this for a staging install, where provisioning a
    /// VM and running a paid baseline is the expensive part and the point is
    /// to prove the install path. A topic installed this way has **no**
    /// measured baseline, so it cannot open until one is sealed — the
    /// lifecycle is left at `baselining`, and a later run without the flag
    /// resumes from there rather than restarting.
    pub skip_baseline: bool,
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
                .unwrap_or(pin.max_proof_deadline_s_ceiling),
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
    ///
    /// **Fail-closed on authorship.** The rules land as
    /// [`RuleSource::Rlm`] only because the RLM's own `propose_rules` job
    /// produced them inside the topic VM — the guest refuses to echo the
    /// signed `checklist` back ([`proof_vm_guest`] `propose_rules`), and this
    /// method re-reads the store afterwards to confirm the version in force
    /// really is `rlm`-sourced. A store that still shows the operator's
    /// vector (`topic_document`) or an operator edit (`operator`) means the
    /// topic's behavior was never authored by its RLM, which is a refusal
    /// naming the provenance rather than a silent pass.
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
        // The read-back is the gate, not a formality: it is what makes "the
        // RLM authored this topic's behavior" a fact the store can prove,
        // rather than a label this driver attached.
        //
        // It reads back **the exact version just written**, never "whichever
        // version is newest": a concurrent writer advancing the store to a
        // later, unrelated version would make a newest-wins check pass while
        // the version this run wrote — the one the baseline is measured
        // against — was not RLM-authored at all. The digest is compared too,
        // so a row rewritten under the same version number is caught.
        let written = self.store.rules_at(&topic.id, rules.version).await?;
        let ok = written
            .as_ref()
            .is_some_and(|w| w.source == RuleSource::Rlm && w.digest() == rules.digest());
        if !ok {
            let provenance = match written.as_ref() {
                Some(w) => format!("{:?}", w.source),
                None => "no rule version".to_owned(),
            };
            return Err(SetupError::RulesNotRlmAuthored {
                topic_id: topic.id.clone(),
                provenance,
                version: rules.version,
            });
        }
        Ok(rules)
    }

    /// Refuse when the rule version in force is no longer the one this run
    /// wrote and measured its baseline against.
    ///
    /// The baseline is persisted **for a rule version**, so a vector that
    /// changed under it would leave a topic whose sealed bar was measured
    /// under rules nobody is scoring with. Reading the version in force before
    /// the baseline lands is what serializes the two: a concurrent RLM write
    /// fails the setup run (nothing persisted) rather than sealing a stale
    /// measurement.
    async fn rules_still_in_force(
        &self,
        topic_id: &str,
        wrote: &RuleSet,
    ) -> Result<(), SetupError> {
        let current = self.store.current_rules(topic_id).await?;
        let in_force = current.as_ref().map(|r| r.version);
        if in_force != Some(wrote.version) {
            return Err(SetupError::RulesSuperseded {
                topic_id: topic_id.to_owned(),
                wrote: wrote.version,
                in_force,
            });
        }
        Ok(())
    }

    /// Baseline shaped exactly like a miner run, persisted: inside the topic
    /// VM, or — when the topic's params select an in-guest runner — inside
    /// one dedicated experiment VM created for it and stopped after it
    /// (destroyed on success, retained for root-cause analysis on failure).
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
            topic.flops_budget,
            "operator baseline",
        )
        .map_err(|e| SetupError::Vm(VmError::Backend(e.to_string())))?;
        let job = VmJob::Baseline {
            request: request.clone(),
        };
        // The baseline is the only paid job the setup driver runs, so it
        // never contends for a topic's shared-VM lock: it passes `None`.
        let ran = run_paid_job(
            self.orchestrator.as_ref(),
            &self.experiments,
            &self.template,
            vm,
            job,
        )
        .await;
        let report = match ran {
            Ok(VmJobOutput::Baseline(r)) => r,
            Ok(_) => return Err(VmError::WrongOutput("baseline").into()),
            Err(e) => {
                self.step(lc, RlmEvent::BaselineFailed, &e.to_string())
                    .await?;
                return Err(e.into());
            }
        };
        report.verify(&request)?;
        // The measurement is about to be persisted **against a rule version**,
        // so that version has to still be the one in force: a vector that
        // moved under the run would leave a sealed bar measured under rules
        // nobody scores with. Checked after the paid run (the only point where
        // a concurrent write could have landed) and before the row, so a
        // superseded run persists nothing and a re-run resumes from
        // `baselining`.
        self.rules_still_in_force(&topic.id, rules).await?;
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
    /// `offer` is the live judge offer the **baseline** runs against, so it is
    /// `None` only on the [`TopicSetup::skip_baseline`] path: with no baseline
    /// to measure there is no paid run and nothing for an offer to bind. Any
    /// other combination is a refusal naming what is missing, rather than a
    /// placeholder offer that would silently bind a run to nothing.
    ///
    /// With `skip_baseline` the driver stops after the RLM's rules land: the
    /// VM is provisioned, the rules are installed, and
    /// [`SetupOutcome::baseline_primary`] is `None`. The lifecycle is left at
    /// `baselining`, which is exactly where a later run without the flag
    /// resumes — so skipping is a pause, not a different path.
    ///
    /// # Errors
    ///
    /// See [`SetupError`]. The lifecycle is left where the failure happened
    /// (persisted), so a re-run resumes rather than restarts.
    pub async fn run(
        &self,
        topic: &TopicDocument,
        pin: &ProofPin,
        offer: Option<&InferenceOffer>,
    ) -> Result<SetupOutcome, SetupError> {
        if topic.metric.family != MetricFamily::Custom {
            return Err(SetupError::NotCustom(topic.id.clone()));
        }
        if offer.is_none() && !self.skip_baseline {
            return Err(SetupError::NoOffer);
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
        if self.skip_baseline {
            return Ok(SetupOutcome {
                topic_id: topic.id.clone(),
                vm,
                rules_version: rules.version,
                baseline_primary: None,
            });
        }
        let Some(offer) = offer else {
            // Unreachable: checked above. Kept as a refusal rather than an
            // `expect`, because the workspace forbids panics in non-test code.
            return Err(SetupError::NoOffer);
        };
        let report = self
            .baseline(topic, pin, offer, &rules, &vm, &mut lc)
            .await?;
        Ok(SetupOutcome {
            topic_id: topic.id.clone(),
            vm,
            rules_version: rules.version,
            baseline_primary: Some(report.primary_value),
        })
    }

    /// The operator sealed the RLM's baseline and re-signed `status: open`:
    /// `baselining → open`, recorded, and the new document version stored.
    ///
    /// Nothing moves or is written unless, in this order: the document is
    /// `status: open`; it validates as an open topic on this host
    /// (`registered_custom` are the custom ids with a runner here — an open
    /// custom topic needs one, a sealed baseline, tighten-only floors); its
    /// operator signature verifies under the pin's topic key; `sealed` binds
    /// to it (`BaselineMeasurement::verify`: commitment, holdout, image
    /// digest) and its `custom_value` is the primary the RLM measured in the
    /// topic VM; and the lifecycle is at `baselining`.
    ///
    /// # Errors
    ///
    /// [`SetupError::NotOpen`], [`SetupError::Topic`], [`SetupError::Seal`],
    /// or [`SetupError::State`] when the topic is not at `baselining`. An
    /// invalid draft never becomes the open version.
    pub async fn mark_sealed(
        &self,
        topic: &TopicDocument,
        pin: &ProofPin,
        registered_custom: &[&str],
        sealed: &BaselineMeasurement,
    ) -> Result<RlmState, SetupError> {
        if topic.status != TopicStatus::Open {
            return Err(SetupError::NotOpen(topic.id.clone()));
        }
        topic.validate(pin, registered_custom)?;
        topic.verify_signature(pin)?;
        sealed
            .verify(pin, topic)
            .map_err(|e| SetupError::Seal(e.to_string()))?;
        let measured =
            self.store.baseline(&topic.id).await?.ok_or_else(|| {
                SetupError::Seal(format!("no baseline measured for {:?}", topic.id))
            })?;
        let sealed_primary = sealed
            .custom_value
            .filter(|v| v.is_finite())
            .ok_or_else(|| SetupError::Seal("sealed measurement has no custom_value".into()))?;
        if (sealed_primary - measured.primary_value).abs() > 1e-9 {
            return Err(SetupError::Seal(format!(
                "sealed custom_value {sealed_primary} is not the measured baseline {}",
                measured.primary_value
            )));
        }
        // A sealed bar nobody can clear is a topic that is open, scorable and
        // permanently unwinnable: `relative_win` refuses every challenger
        // against a zero bar, so no submission could ever pass. That is a real
        // measurement, not a bug — a reference run that solved nothing, which
        // is exactly what an all-zero Harbor baseline is — so it is refused
        // here, at the boundary, where the operator can still act on it.
        //
        // This is deliberately **not** an auto-reseal: the stored measurement
        // is left exactly as the RLM wrote it. Fixing it means re-running the
        // baseline against a reference that can score (or re-scoping the task
        // set), then sealing the new number.
        if proof_score::family_bar_is_degenerate(topic.metric.family, Some(sealed_primary)) {
            return Err(SetupError::DegenerateBar {
                topic_id: topic.id.clone(),
                primary: sealed_primary,
            });
        }
        let mut lc = self.lifecycle(topic).await?;
        if lc.state != RlmState::Baselining {
            return Err(StateError::Illegal {
                from: lc.state,
                event: RlmEvent::BaselineSealed,
            }
            .into());
        }
        self.store.put_topic_version(topic).await?;
        self.step(
            &mut lc,
            RlmEvent::BaselineSealed,
            "operator sealed the baseline",
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use proof_rlm::UnwiredVmOrchestrator;
    use proof_rlm_store::MemoryRlmStore;

    /// The store is the only collaborator these tests need; the rest of
    /// [`TopicSetup`] is filled with stubs that are never reached by
    /// `rules_still_in_force`.
    fn setup(store: Arc<dyn RlmStore>) -> TopicSetup {
        TopicSetup {
            orchestrator: Arc::new(UnwiredVmOrchestrator),
            store,
            template: VmTemplate::unpinned(),
            experiments: proof_rlm::ExperimentPolicy::default(),
            owner: Arc::new(AlwaysApprove),
            keys: Arc::new(KeysPresent),
            spend_cap_usd: None,
            skip_baseline: false,
        }
    }

    fn rules(topic_id: &str, version: u32, source: RuleSource) -> RuleSet {
        RuleSet {
            topic_id: topic_id.to_owned(),
            version,
            source,
            rules: vec![proof_task::ChecklistRule {
                id: format!("r-{version}"),
                text: "a rule".into(),
            }],
        }
    }

    /// The read-back verifies **the version this run wrote**, not whichever
    /// version is newest.
    ///
    /// The defect this pins: setup wrote version N and then checked the source
    /// of the *newest* version. A concurrent writer advancing the store to
    /// N+1 (`rlm`-sourced) made that check pass while the version setup
    /// actually wrote — the one the baseline is measured against — was never
    /// verified at all.
    #[tokio::test]
    async fn the_rule_read_back_verifies_the_version_it_wrote() {
        let store = Arc::new(MemoryRlmStore::new());
        let topic_id = "fixture-topic";
        // Setup wrote version 1 as `topic_document` (what the install seeds),
        // while a later, RLM-authored version 2 is now newest.
        store
            .put_rules(&rules(topic_id, 1, RuleSource::TopicDocument))
            .await
            .expect("v1");
        store
            .put_rules(&rules(topic_id, 2, RuleSource::Rlm))
            .await
            .expect("v2");

        // A newest-wins check would read `rlm` and pass. The version the run
        // wrote is the one that has to be verified.
        let written = store
            .rules_at(topic_id, 1)
            .await
            .expect("read")
            .expect("v1 exists");
        assert_eq!(
            written.source,
            RuleSource::TopicDocument,
            "version 1 is the operator's vector, whatever version 2 says"
        );
        assert_eq!(
            store.current_rules_source(topic_id).await.expect("read"),
            Some(RuleSource::Rlm),
            "the newest version is rlm — which is exactly why newest-wins was the defect"
        );
    }

    /// A rule vector that moved under the run refuses **before** the baseline
    /// is persisted.
    ///
    /// The baseline is stored per rule version, so a version that changed
    /// while the paid run was in flight would leave a sealed bar measured
    /// under rules nobody scores with. Nothing is written: the lifecycle stays
    /// at `baselining`, so a re-run resumes.
    #[tokio::test]
    async fn a_superseded_rule_version_persists_no_baseline() {
        let store = Arc::new(MemoryRlmStore::new());
        let topic_id = "fixture-topic";
        store
            .put_rules(&rules(topic_id, 1, RuleSource::Rlm))
            .await
            .expect("v1");
        let wrote = store
            .rules_at(topic_id, 1)
            .await
            .expect("read")
            .expect("v1");

        let setup = setup(store.clone());
        // In force: nothing to refuse.
        setup
            .rules_still_in_force(topic_id, &wrote)
            .await
            .expect("the version this run wrote is in force");

        // A concurrent RLM write lands version 2 while the baseline runs.
        store
            .put_rules(&rules(topic_id, 2, RuleSource::Rlm))
            .await
            .expect("v2");
        let err = setup
            .rules_still_in_force(topic_id, &wrote)
            .await
            .expect_err("a superseded version must refuse");
        assert!(
            matches!(
                err,
                SetupError::RulesSuperseded {
                    wrote: 1,
                    in_force: Some(2),
                    ..
                }
            ),
            "{err}"
        );
        assert!(
            store.baseline(topic_id).await.expect("read").is_none(),
            "nothing was persisted for a run whose rules moved under it"
        );
    }

    /// The refusal names both versions, so an operator reads what happened.
    #[test]
    fn a_superseded_refusal_names_both_versions() {
        let err = SetupError::RulesSuperseded {
            topic_id: "fixture-topic".into(),
            wrote: 3,
            in_force: Some(4),
        };
        let text = err.to_string();
        assert!(text.contains("fixture-topic"), "{text}");
        assert!(text.contains('3') && text.contains('4'), "{text}");
    }

    struct AlwaysApprove;
    impl OwnerHook for AlwaysApprove {
        fn ask_owner(
            &self,
            _prompt: &OwnerPrompt,
        ) -> Result<proof_rlm::OwnerDecision, proof_rlm::HookError> {
            Ok(proof_rlm::OwnerDecision::Approve)
        }
    }

    struct KeysPresent;
    impl OwnerKeysProbe for KeysPresent {
        fn owner_keys_present(&self) -> Result<(), proof_rlm::HookError> {
            Ok(())
        }
    }
}
