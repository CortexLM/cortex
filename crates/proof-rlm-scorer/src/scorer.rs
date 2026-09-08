//! [`RlmScorer`]: the `LiveScorer` for the whole `custom` metric family.
//!
//! Per submission, in this order and never reordered:
//!
//! 1. resolve the topic's `custom_id` in the [`RunnerRegistry`] — unknown or
//!    unwired → `RunnerUnwired` (503, no row);
//! 2. load the topic's **current rule version** from the store (seeding
//!    version 1 from the signed document the first time);
//! 3. **inspect** through the runner (a job in the topic VM) → checklist,
//!    persisted red or green;
//! 4. red → **reject document, no paid inference**;
//! 5. green → [`SpendToken`] → **evaluate** (the only paid step) → report →
//!    `custom_value = primary_value`;
//! 6. on persist: artefact zip + metadata row + public event; on promotion:
//!    promotion row, `best.json`, lifecycle `promoting → open`.
//!
//! Evaluations are serialised per topic so the lifecycle mirror is exact
//! and every transition is written to the store.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use proof_eval::{EvalError, LiveScorer, ProofEvalDocument, PROOF_METRICS_SCHEMA};
use proof_rlm::{
    authorize_spend, decide_promote, ArtifactFile, Checklist, CustomRunReport, CustomRunRequest,
    Lifecycle, LogFile, PromoteDecision, RlmEvent, RlmState, RuleSet, RunnerError, RunnerRegistry,
};
use proof_rlm_store::{ArtefactRow, ChecklistRow, PromotionRow, RlmStore, TransitionRow};
use proof_score::{AgentVerdict, HarnessMetrics, ProofCheatCode, ProofKind};
use proof_task::{HoldoutRecord, InferenceOffer, MetricFamily, ProofPin, TopicDocument};

use crate::artefact::{ArtefactBundle, ArtefactStore, BaselineRef, BestRef, PublicEvent};

/// Scored-but-not-yet-persisted state for one submission.
struct Pending {
    bundle: ArtefactBundle,
    decision: Option<PromoteDecision>,
}

/// Family scorer over the runner registry, the RLM store, and the artefact store.
pub struct RlmScorer {
    registry: Arc<RunnerRegistry>,
    store: Arc<dyn RlmStore>,
    artefacts: Option<ArtefactStore>,
    pending: Mutex<BTreeMap<String, Pending>>,
    locks: Mutex<BTreeMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

fn unwired(custom_id: &str, detail: String) -> EvalError {
    EvalError::RunnerUnwired {
        custom_id: custom_id.to_owned(),
        detail,
    }
}

fn map_runner(custom_id: &str, e: RunnerError) -> EvalError {
    match e {
        RunnerError::Unregistered(_) | RunnerError::NotWired(_) => {
            unwired(custom_id, e.to_string())
        }
        other => EvalError::Backend(other.to_string()),
    }
}

fn store_err<E: std::fmt::Display>(e: E) -> EvalError {
    EvalError::Backend(format!("rlm store: {e}"))
}

impl RlmScorer {
    /// Scorer over `registry` and `store`, no artefact store.
    #[must_use]
    pub fn new(registry: Arc<RunnerRegistry>, store: Arc<dyn RlmStore>) -> Self {
        Self {
            registry,
            store,
            artefacts: None,
            pending: Mutex::new(BTreeMap::new()),
            locks: Mutex::new(BTreeMap::new()),
        }
    }

    /// Where artefact zips go. `None` keeps bundles in memory only.
    #[must_use]
    pub fn with_artefacts(mut self, store: Option<ArtefactStore>) -> Self {
        self.artefacts = store;
        self
    }

    /// Bundles scored but not yet persisted.
    #[must_use]
    pub fn pending_len(&self) -> usize {
        self.pending.lock().map_or(0, |m| m.len())
    }

    fn topic_lock(&self, topic_id: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.locks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(topic_id.to_owned())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }

    /// Make sure the signed document this run is scored under is in the store
    /// (a new signature is a new topic version).
    async fn ensure_topic(&self, topic: &TopicDocument) -> Result<(), EvalError> {
        let latest = self
            .store
            .latest_topic(&topic.id)
            .await
            .map_err(store_err)?;
        if latest.is_none_or(|(_, d)| d.signature != topic.signature) {
            self.store
                .put_topic_version(topic)
                .await
                .map_err(store_err)?;
        }
        Ok(())
    }

    /// Persisted lifecycle, or the position the signed document implies.
    async fn lifecycle(&self, topic: &TopicDocument) -> Result<Lifecycle, EvalError> {
        Ok(self
            .store
            .lifecycle(&topic.id)
            .await
            .map_err(store_err)?
            .unwrap_or_else(|| Lifecycle::from_topic(topic)))
    }

    /// Apply `event` to the persisted lifecycle and record the move.
    async fn apply(
        &self,
        topic: &TopicDocument,
        event: RlmEvent,
        note: &str,
    ) -> Result<RlmState, EvalError> {
        let mut lc = self.lifecycle(topic).await?;
        let to = lc
            .apply(event, note)
            .map_err(|e| EvalError::Backend(format!("topic lifecycle: {e}")))?;
        let last = lc.history.last().cloned();
        if let Some(t) = last {
            self.store
                .record_transition(&TransitionRow {
                    topic_id: topic.id.clone(),
                    from: t.from,
                    event: t.event,
                    to: t.to,
                    note: t.note,
                })
                .await
                .map_err(store_err)?;
        }
        Ok(to)
    }

    async fn apply_logged(&self, topic: &TopicDocument, event: RlmEvent, note: &str) {
        if let Err(e) = self.apply(topic, event, note).await {
            tracing::warn!(topic_id = %topic.id, %e, "rlm lifecycle event refused");
        }
    }

    /// Called with the topic lock held: a persisted `evaluating` /
    /// `promoting` means the previous run's row never landed. Close that
    /// phase rather than refusing the topic forever.
    async fn recover_stale(&self, topic: &TopicDocument) -> Result<(), EvalError> {
        match self.lifecycle(topic).await?.state {
            RlmState::Evaluating => {
                self.apply(
                    topic,
                    RlmEvent::VerdictRecorded,
                    "previous run never persisted; recovered",
                )
                .await?;
            }
            RlmState::Promoting => {
                self.apply(
                    topic,
                    RlmEvent::PromotionRefused,
                    "previous promotion never persisted; recovered",
                )
                .await?;
            }
            _ => {}
        }
        Ok(())
    }

    /// The topic's current rule version, seeding version 1 from the signed
    /// document on first use so the gate a run was ticked against is in the
    /// store, not only in the document.
    async fn rules_for(&self, topic: &TopicDocument) -> Result<RuleSet, EvalError> {
        if let Some(current) = self
            .store
            .current_rules(&topic.id)
            .await
            .map_err(store_err)?
        {
            return Ok(current);
        }
        let v1 = RuleSet::from_topic(topic).map_err(|e| {
            EvalError::Backend(format!(
                "topic carries no usable anti-cheat rules ({e}); refuse"
            ))
        })?;
        self.store.put_rules(&v1).await.map_err(store_err)?;
        Ok(v1)
    }

    fn agent(
        topic: &TopicDocument,
        kind: ProofKind,
        reproduced: bool,
        claim_holds: bool,
        rationale: String,
    ) -> AgentVerdict {
        AgentVerdict {
            verdict: kind,
            reproduced,
            claim_holds_public: claim_holds,
            contamination: false,
            canary_hit: false,
            flops_used: 0,
            flops_budget: topic.flops_budget,
            cheat_codes: if kind == ProofKind::Clean {
                Vec::new()
            } else {
                vec![ProofCheatCode::Other]
            },
            rationale,
            topic_id: topic.id.clone(),
            family: topic.metric.family,
        }
    }

    fn document(
        pin: &ProofPin,
        topic: &TopicDocument,
        req: &CustomRunRequest,
        agent: AgentVerdict,
        custom_value: Option<f64>,
    ) -> ProofEvalDocument {
        ProofEvalDocument {
            schema_version: PROOF_METRICS_SCHEMA,
            submission_digest: req.submission_digest.clone(),
            artifact_digest: req.artifact_digest.clone(),
            topic_id: topic.id.clone(),
            eval_image_digest: pin.eval_image_digest.clone(),
            holdout_commitment: topic.holdout_commitment.clone(),
            agent,
            harness: HarnessMetrics {
                custom_value,
                ..HarnessMetrics::default()
            },
        }
    }

    fn stash(
        &self,
        pin: &ProofPin,
        topic: &TopicDocument,
        req: &CustomRunRequest,
        checklist: Checklist,
        checklist_green: bool,
        artifact: Vec<ArtifactFile>,
        report: Option<CustomRunReport>,
        logs: Vec<LogFile>,
    ) {
        let bundle = ArtefactBundle {
            topic_id: topic.id.clone(),
            custom_id: req.custom_id.clone(),
            submission_digest: req.submission_digest.clone(),
            artifact_digest: req.artifact_digest.clone(),
            checklist_green,
            checklist,
            report,
            baseline_ref: BaselineRef::from_topic(topic, pin),
            artifact,
            logs,
        };
        self.pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(
                req.submission_digest.clone(),
                Pending {
                    bundle,
                    decision: None,
                },
            );
    }

    async fn evaluate(
        &self,
        pin: &ProofPin,
        topic: &TopicDocument,
        offer: &InferenceOffer,
        frozen_digest: &str,
        artifact_digest: &str,
        claim: &str,
    ) -> Result<ProofEvalDocument, EvalError> {
        let custom_id = topic.metric.custom_id.trim().to_owned();
        let runner = self
            .registry
            .resolve(&custom_id)
            .map_err(|e| map_runner(&custom_id, e))?;
        let rules = self.rules_for(topic).await?;
        let req = CustomRunRequest::from_topic(
            topic,
            pin,
            offer,
            &rules,
            frozen_digest,
            artifact_digest,
            None,
            claim,
        )
        .map_err(|e| map_runner(&custom_id, e))?;
        let inspected = runner
            .inspect(&req, &rules)
            .await
            .map_err(|e| map_runner(&custom_id, e))?;
        let checklist = inspected.checklist;
        let row = ChecklistRow::from_checklist(&checklist, &rules);
        self.store.put_checklist(&row).await.map_err(store_err)?;
        if let Err(red) = checklist.verify(&rules) {
            let rationale = format!(
                "anti-cheat {red} (rules v{}); no paid inference",
                rules.version
            );
            let agent = Self::agent(topic, ProofKind::Reject, false, false, rationale);
            self.stash(
                pin,
                topic,
                &req,
                checklist,
                false,
                inspected.artifact,
                None,
                Vec::new(),
            );
            return Ok(Self::document(pin, topic, &req, agent, None));
        }
        let token = authorize_spend(&checklist, &rules, &req.topic_id, &req.submission_digest)
            .map_err(|e| EvalError::Backend(e.to_string()))?;
        let run = runner
            .evaluate(&req, &token)
            .await
            .map_err(|e| map_runner(&custom_id, e))?;
        run.report
            .verify(&req)
            .map_err(|e| EvalError::NoVerdict(e.to_string()))?;
        let rationale = format!(
            "{}: {} = {:.6}; checklist green (rules v{}); sandboxed={}",
            req.custom_id,
            req.primary,
            run.report.primary_value,
            rules.version,
            run.report.sandboxed
        );
        let agent = Self::agent(
            topic,
            ProofKind::Clean,
            true,
            run.report.claim_holds,
            rationale,
        );
        let doc = Self::document(pin, topic, &req, agent, Some(run.report.primary_value));
        self.stash(
            pin,
            topic,
            &req,
            checklist,
            true,
            inspected.artifact,
            Some(run.report),
            run.logs,
        );
        Ok(doc)
    }

    async fn persist(
        &self,
        topic_id: &str,
        submission_digest: &str,
        submission_id: &str,
        promoted: bool,
    ) {
        let Some(pending) = self
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(submission_digest)
        else {
            return;
        };
        let bundle = pending.bundle;
        let primary = bundle.report.as_ref().map(|r| r.primary_value);
        let Some(store) = &self.artefacts else {
            return;
        };
        match store.write(&bundle, submission_id, promoted) {
            Ok(written) => {
                let row = ArtefactRow {
                    topic_id: topic_id.to_owned(),
                    submission_id: submission_id.to_owned(),
                    submission_digest: submission_digest.to_owned(),
                    path: written.path.display().to_string(),
                    sha256: written.sha256,
                    bytes: written.bytes,
                    primary_value: primary,
                    checklist_green: bundle.checklist_green,
                    promoted,
                };
                if let Err(e) = self.store.put_artefact(&row).await {
                    tracing::error!(topic_id, submission_id, %e, "artefact metadata not persisted");
                }
                let _ = store.append_event(
                    topic_id,
                    &PublicEvent::Scored {
                        submission_id: submission_id.to_owned(),
                        primary_value: primary,
                        checklist_green: bundle.checklist_green,
                    },
                );
                tracing::info!(topic_id, submission_id, path = %written.path.display(), "artefact written");
            }
            Err(e) => tracing::error!(topic_id, submission_id, %e, "artefact not written"),
        }
        if !promoted {
            return;
        }
        let (Some(primary_value), Some(PromoteDecision::Promote { bar, .. })) =
            (primary, pending.decision)
        else {
            tracing::error!(
                topic_id,
                submission_id,
                "promoted without a primary or a decision"
            );
            return;
        };
        let previous_best = self
            .store
            .best(topic_id)
            .await
            .ok()
            .flatten()
            .map(|b| b.submission_id);
        let row = PromotionRow {
            topic_id: topic_id.to_owned(),
            submission_id: submission_id.to_owned(),
            submission_digest: submission_digest.to_owned(),
            primary_value,
            bar: Some(bar),
            previous_best: previous_best.clone(),
        };
        if let Err(e) = self.store.record_promotion(&row).await {
            tracing::error!(topic_id, submission_id, %e, "promotion not persisted");
        }
        if let Err(e) = store.mark_best(&BestRef {
            topic_id: topic_id.to_owned(),
            submission_id: submission_id.to_owned(),
            submission_digest: submission_digest.to_owned(),
            primary_value,
            bar: Some(bar),
            artefact: format!("{submission_id}.zip"),
        }) {
            tracing::error!(topic_id, submission_id, %e, "best pointer not written");
        }
        let _ = store.append_event(
            topic_id,
            &PublicEvent::Promoted {
                submission_id: submission_id.to_owned(),
                primary_value,
                bar: Some(bar),
                previous_best,
            },
        );
    }
}

#[async_trait]
impl LiveScorer for RlmScorer {
    async fn score(
        &self,
        pin: &ProofPin,
        topic: &TopicDocument,
        offer: &InferenceOffer,
        frozen_digest: &str,
        artifact_digest: &str,
        _holdout: &[HoldoutRecord],
        claim: &str,
    ) -> Result<ProofEvalDocument, EvalError> {
        self.ready_for_topic(topic)?;
        let lock = self.topic_lock(&topic.id);
        let _serial = lock.lock().await;
        self.ensure_topic(topic).await?;
        self.recover_stale(topic).await?;
        self.apply(topic, RlmEvent::SubmissionReceived, frozen_digest)
            .await?;
        let out = self
            .evaluate(pin, topic, offer, frozen_digest, artifact_digest, claim)
            .await;
        if out.is_err() {
            // No row will follow a refusal, so the verdict phase is over now.
            self.apply_logged(topic, RlmEvent::VerdictRecorded, "refused; no row")
                .await;
        }
        out
    }

    fn ready(&self) -> Result<(), EvalError> {
        Ok(())
    }

    fn ready_for_topic(&self, topic: &TopicDocument) -> Result<(), EvalError> {
        if topic.metric.family != MetricFamily::Custom {
            return Err(EvalError::Backend(format!(
                "rlm scorer asked to score non-custom topic {}",
                topic.id
            )));
        }
        let custom_id = topic.metric.custom_id.trim();
        let runner = self
            .registry
            .resolve(custom_id)
            .map_err(|e| map_runner(custom_id, e))?;
        runner.ready().map_err(|e| map_runner(custom_id, e))
    }

    fn custom_ids(&self) -> Vec<String> {
        self.registry.ids()
    }

    async fn auto_promote(
        &self,
        topic: &TopicDocument,
        submission_digest: &str,
        pass: bool,
        primary: Option<f64>,
        bar: Option<f64>,
    ) -> bool {
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(p) = pending.get_mut(submission_digest) else {
            return false;
        };
        let reported = p.bundle.report.as_ref().map(|r| r.primary_value);
        // The payout primary and the report must agree, or neither is evidence.
        let agree = matches!((primary, reported), (Some(a), Some(b)) if (a - b).abs() < 1e-9);
        let decision = decide_promote(
            pass && agree,
            p.bundle.checklist_green,
            &p.bundle.checklist.failed_ids(),
            reported,
            bar,
            topic.metric.direction,
            topic.metric.epsilon_rel,
        );
        let promote = decision.is_promote();
        p.decision = Some(decision);
        promote
    }

    async fn on_persisted(
        &self,
        topic_id: &str,
        submission_digest: &str,
        submission_id: &str,
        promoted: bool,
    ) {
        let owned = self
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains_key(submission_digest);
        if !owned {
            return;
        }
        let topic = self
            .store
            .latest_topic(topic_id)
            .await
            .ok()
            .flatten()
            .map(|(_, d)| d);
        if promoted {
            if let Some(t) = &topic {
                self.apply_logged(t, RlmEvent::PromotionCandidate, submission_id)
                    .await;
            }
        }
        self.persist(topic_id, submission_digest, submission_id, promoted)
            .await;
        if let Some(t) = &topic {
            let event = if promoted {
                RlmEvent::Promoted
            } else {
                RlmEvent::VerdictRecorded
            };
            self.apply_logged(t, event, submission_id).await;
        }
    }
}
