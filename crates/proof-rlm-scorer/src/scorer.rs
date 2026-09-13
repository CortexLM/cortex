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
//!    `custom_value = primary_value`; `flops_used` is telemetry from the
//!    runner when present, never a cheat/reject gate on this family;
//! 6. on persist: artefact zip + metadata row + public event; on promotion:
//!    promotion row, `best.json`, lifecycle `promoting → open`.
//!
//! Runs are serialised per topic by a **lease** the run holds from `score`
//! until the host reports the row persisted (`on_persisted`). Promotion is
//! decided under that lease against the store's current best and persisted
//! under the same lease with a compare-and-swap on the best pointer, so two
//! runs can never both promote against one stale bar and a later, worse run
//! can never displace a better champion. A run whose row never lands
//! releases its lease after [`DEFAULT_LEASE_TTL`].
//!
//! [`SpendToken`]: proof_rlm::SpendToken

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use proof_eval::{EvalError, LiveScorer, ProofEvalDocument, PROOF_METRICS_SCHEMA};
use proof_executor::ExecutorPlan;
use proof_results::{require_evaluate, ReportBind};
use proof_rlm::{
    authorize_spend, decide_promote, ArtifactFile, Checklist, CustomRunReport, CustomRunRequest,
    Lifecycle, LogFile, MinerEnv, PromoteDecision, RlmEvent, RlmState, RuleSet, RunnerError,
    RunnerRegistry,
};
use proof_rlm_store::{ArtefactRow, ChecklistRow, PromotionRow, RlmStore, TransitionRow};
use proof_score::{AgentVerdict, HarnessMetrics, ProofCheatCode, ProofKind};
use proof_task::{
    HoldoutRecord, InferenceOffer, MetricDirection, MetricFamily, ProofPin, TopicDocument,
};
use tokio::sync::OwnedMutexGuard;

use crate::artefact::{ArtefactBundle, ArtefactStore, BaselineRef, BestRef, PublicEvent};

/// How long a scored run may hold its topic lease waiting for the host to
/// report its row persisted. Past this the persist is treated as abandoned
/// (the row never landed) and the next run of the topic proceeds.
pub const DEFAULT_LEASE_TTL: Duration = Duration::from_mins(5);

/// How often a run waiting for a topic lease re-checks for abandoned holders.
const LEASE_POLL: Duration = Duration::from_secs(1);

/// A promotion decision and the world it was taken in.
struct Decided {
    outcome: PromoteDecision,
    /// The store's best when the decision was taken; persist refuses when it
    /// has moved (another writer crowned something in between).
    previous_best: Option<PromotionRow>,
    direction: MetricDirection,
}

/// Scored-but-not-yet-persisted state for one submission.
struct Pending {
    bundle: ArtefactBundle,
    decided: Option<Decided>,
    /// Topic lease held since `score` returned; dropped when the row is
    /// persisted (end of `on_persisted`) or the entry is reaped.
    lease: Option<OwnedMutexGuard<()>>,
    since: Instant,
}

/// Family scorer over the runner registry, the RLM store, and the artefact store.
pub struct RlmScorer {
    registry: Arc<RunnerRegistry>,
    store: Arc<dyn RlmStore>,
    artefacts: Option<ArtefactStore>,
    pending: Mutex<BTreeMap<String, Pending>>,
    locks: Mutex<BTreeMap<String, Arc<tokio::sync::Mutex<()>>>>,
    lease_ttl: Duration,
}

fn unwired(custom_id: &str, detail: String) -> EvalError {
    EvalError::RunnerUnwired {
        custom_id: custom_id.to_owned(),
        detail,
    }
}

fn map_runner(custom_id: &str, e: RunnerError) -> EvalError {
    match e {
        RunnerError::Unregistered { .. } | RunnerError::NotWired(_) => {
            unwired(custom_id, e.to_string())
        }
        other => EvalError::Backend(other.to_string()),
    }
}

fn store_err<E: std::fmt::Display>(e: E) -> EvalError {
    EvalError::Backend(format!("rlm store: {e}"))
}

fn bind_evaluate_results(
    report: &CustomRunReport,
    req: &CustomRunRequest,
) -> Result<(), EvalError> {
    report
        .verify(req)
        .map_err(|e| EvalError::NoVerdict(e.to_string()))?;
    require_evaluate(
        report.results.as_ref(),
        &ReportBind {
            topic_id: &report.topic_id,
            custom_id: &report.custom_id,
            submission_digest: &report.submission_digest,
            artifact_digest: &report.artifact_digest,
            primary_value: report.primary_value,
            claim_holds: report.claim_holds,
        },
        &req.constraints.params,
    )
    .map_err(|e| EvalError::NoVerdict(e.to_string()))?;
    Ok(())
}

/// The harder of two bars, direction-aware (`None` when neither exists).
fn tighter_bar(a: Option<f64>, b: Option<f64>, direction: MetricDirection) -> Option<f64> {
    match (a.filter(|v| v.is_finite()), b.filter(|v| v.is_finite())) {
        (None, None) => None,
        (Some(v), None) | (None, Some(v)) => Some(v),
        (Some(x), Some(y)) => Some(match direction {
            MetricDirection::Max => x.max(y),
            MetricDirection::Min => x.min(y),
        }),
    }
}

/// Strictly better, direction-aware.
fn strictly_better(candidate: f64, incumbent: f64, direction: MetricDirection) -> bool {
    match direction {
        MetricDirection::Max => candidate > incumbent,
        MetricDirection::Min => candidate < incumbent,
    }
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
            lease_ttl: DEFAULT_LEASE_TTL,
        }
    }

    /// Where artefact zips go. `None` keeps bundles in memory only.
    #[must_use]
    pub fn with_artefacts(mut self, store: Option<ArtefactStore>) -> Self {
        self.artefacts = store;
        self
    }

    /// How long a scored run may wait for its row before its topic lease is
    /// released (default [`DEFAULT_LEASE_TTL`]).
    #[must_use]
    pub fn with_lease_ttl(mut self, ttl: Duration) -> Self {
        self.lease_ttl = ttl;
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
            .unwrap_or_else(PoisonError::into_inner)
            .entry(topic_id.to_owned())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }

    /// Drop pending runs of `topic_id` whose row never landed within the TTL,
    /// releasing the lease they hold.
    fn reap_abandoned(&self, topic_id: &str) {
        let mut pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
        let stale: Vec<String> = pending
            .iter()
            .filter(|(_, p)| p.bundle.topic_id == topic_id && p.since.elapsed() >= self.lease_ttl)
            .map(|(digest, _)| digest.clone())
            .collect();
        for digest in stale {
            pending.remove(&digest);
            tracing::warn!(
                topic_id,
                submission_digest = %digest,
                "scored run never persisted within the lease ttl; lease released"
            );
        }
    }

    /// Take the topic lease, reaping abandoned holders while waiting.
    async fn lease(&self, topic_id: &str) -> OwnedMutexGuard<()> {
        let lock = self.topic_lock(topic_id);
        loop {
            self.reap_abandoned(topic_id);
            if let Ok(guard) = tokio::time::timeout(LEASE_POLL, lock.clone().lock_owned()).await {
                return guard;
            }
        }
    }

    /// Hand the lease to the pending run so it outlives `score`.
    fn hold(&self, submission_digest: &str, lease: OwnedMutexGuard<()>) {
        let mut pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
        match pending.get_mut(submission_digest.trim()) {
            Some(p) => p.lease = Some(lease),
            None => drop(lease),
        }
    }

    fn take(&self, submission_digest: &str) -> Option<Pending> {
        self.pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(submission_digest)
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

    /// Called with the topic lease held: a persisted `evaluating` /
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
        flops_used: u64,
        cheat_codes: Vec<ProofCheatCode>,
        rationale: String,
    ) -> AgentVerdict {
        AgentVerdict {
            verdict: kind,
            reproduced,
            claim_holds_public: claim_holds,
            contamination: false,
            canary_hit: false,
            flops_used,
            flops_budget: topic.flops_budget,
            cheat_codes,
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
        let results = report.as_ref().and_then(|r| r.results.clone());
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
            results,
        };
        self.pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(
                req.submission_digest.clone(),
                Pending {
                    bundle,
                    decided: None,
                    lease: None,
                    since: Instant::now(),
                },
            );
    }

    /// Verdict for a verified paid run. Custom / agent topics do not cheat-
    /// code or reject on FLOP accounting: the signed checklist and sister
    /// attestation are the gates. `flops_used` is telemetry when present.
    fn paid_verdict(
        topic: &TopicDocument,
        req: &CustomRunRequest,
        report: &CustomRunReport,
        rules_version: u32,
    ) -> AgentVerdict {
        let flops_used = report.flops_used.unwrap_or(0);
        let rationale = format!(
            "{}: {} = {:.6}; checklist green (rules v{rules_version}); sandboxed={}; flops_used={flops_used}",
            req.custom_id, req.primary, report.primary_value, report.sandboxed
        );
        Self::agent(
            topic,
            ProofKind::Clean,
            true,
            report.claim_holds,
            flops_used,
            Vec::new(),
            rationale,
        )
    }

    #[allow(clippy::too_many_arguments)]
    async fn evaluate(
        &self,
        pin: &ProofPin,
        topic: &TopicDocument,
        offer: &InferenceOffer,
        plan: &ExecutorPlan,
        frozen_digest: &str,
        artifact_digest: &str,
        artifact_uri: Option<&str>,
        declared_flops: u64,
        claim: &str,
        miner_env: &MinerEnv,
        artifact_tar: Option<&[u8]>,
    ) -> Result<ProofEvalDocument, EvalError> {
        let custom_id = topic.metric.custom_id.trim().to_owned();
        let runner = self
            .registry
            .resolve(&custom_id)
            .map_err(|e| map_runner(&custom_id, e))?;
        // Custom intake accepts an upload (preferred) or a miner URI (compat).
        // Upload records `proof-artefact://{digest}` and the vault bytes
        // travel here for vsock inject. URI-only still GETs https:// inside
        // the VM (64 MiB cap, unchanged).
        let Some(artifact_uri) = artifact_uri.map(str::trim).filter(|u| !u.is_empty()) else {
            return Err(EvalError::Backend(
                "custom submission carries no artefact (upload or artifact_uri)".into(),
            ));
        };
        if proof_rlm::is_staged_artifact_uri(artifact_uri) && artifact_tar.is_none() {
            return Err(EvalError::Backend(
                "staged artefact locator with no vault bytes; refusing to invent".into(),
            ));
        }
        let rules = self.rules_for(topic).await?;
        let mut req = CustomRunRequest::from_topic(
            topic,
            pin,
            offer,
            &rules,
            frozen_digest,
            artifact_digest,
            Some(artifact_uri),
            declared_flops,
            claim,
        )
        .map_err(|e| map_runner(&custom_id, e))?
        .with_executor_plan(plan.deadline_s, &plan.config_commitment)
        .with_miner_env(miner_env.clone());
        if let Some(bytes) = artifact_tar {
            req = req.with_artifact_tar(bytes);
        }
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
            let agent = Self::agent(
                topic,
                ProofKind::Reject,
                false,
                false,
                0,
                vec![ProofCheatCode::Other],
                rationale,
            );
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
        bind_evaluate_results(&run.report, &req)?;
        let agent = Self::paid_verdict(topic, &req, &run.report, rules.version);
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

    /// Whether a decided promotion still stands at persist time: the best
    /// pointer must be the one the decision was taken against (compare-and-
    /// swap) and this run must be strictly better than it. Returns the
    /// promotion row to append, or `None` when the crown is refused.
    async fn crown(
        &self,
        topic_id: &str,
        submission_digest: &str,
        submission_id: &str,
        pending: &Pending,
    ) -> Option<PromotionRow> {
        let primary = pending.bundle.report.as_ref().map(|r| r.primary_value);
        let (Some(primary_value), Some(decided)) = (primary, pending.decided.as_ref()) else {
            tracing::error!(
                topic_id,
                submission_id,
                "promoted without a primary or a decision"
            );
            return None;
        };
        let PromoteDecision::Promote { bar, .. } = decided.outcome else {
            tracing::error!(topic_id, submission_id, "promoted against a keep decision");
            return None;
        };
        let current = match self.store.best(topic_id).await {
            Ok(b) => b,
            Err(e) => {
                tracing::error!(topic_id, submission_id, %e, "best unreadable; promotion refused");
                return None;
            }
        };
        let expected = decided
            .previous_best
            .as_ref()
            .map(|b| b.submission_id.as_str());
        let unchanged = current.as_ref().map(|b| b.submission_id.as_str()) == expected;
        let better = current
            .as_ref()
            .is_none_or(|b| strictly_better(primary_value, b.primary_value, decided.direction));
        if !unchanged || !better {
            tracing::error!(
                topic_id,
                submission_id,
                primary_value,
                best = ?current.as_ref().map(|b| (&b.submission_id, b.primary_value)),
                "stale promotion refused: best moved since the decision"
            );
            return None;
        }
        Some(PromotionRow {
            topic_id: topic_id.to_owned(),
            submission_id: submission_id.to_owned(),
            submission_digest: submission_digest.to_owned(),
            primary_value,
            bar: Some(bar),
            previous_best: current.map(|b| b.submission_id),
        })
    }

    /// Write the artefact and, when the crown stands ([`Self::crown`]), the
    /// promotion row, best pointer, and public event. Returns whether the
    /// promotion landed; the manifest records that, not the caller's flag.
    async fn persist(
        &self,
        topic_id: &str,
        submission_digest: &str,
        submission_id: &str,
        promoted: bool,
        pending: &Pending,
    ) -> bool {
        let crown = if promoted {
            self.crown(topic_id, submission_digest, submission_id, pending)
                .await
        } else {
            None
        };
        let landed = crown.is_some();
        let bundle = &pending.bundle;
        let primary = bundle.report.as_ref().map(|r| r.primary_value);
        if let Some(store) = &self.artefacts {
            match store.write(bundle, submission_id, landed) {
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
                        promoted: landed,
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
        }
        let Some(row) = crown else {
            return false;
        };
        if let Err(e) = self.store.record_promotion(&row).await {
            tracing::error!(topic_id, submission_id, %e, "promotion not persisted");
            return false;
        }
        if let Some(store) = &self.artefacts {
            if let Err(e) = store.mark_best(&BestRef {
                topic_id: topic_id.to_owned(),
                submission_id: submission_id.to_owned(),
                submission_digest: submission_digest.to_owned(),
                primary_value: row.primary_value,
                bar: row.bar,
                artefact: format!("{submission_id}.zip"),
            }) {
                tracing::error!(topic_id, submission_id, %e, "best pointer not written");
            }
            let _ = store.append_event(
                topic_id,
                &PublicEvent::Promoted {
                    submission_id: submission_id.to_owned(),
                    primary_value: row.primary_value,
                    bar: row.bar,
                    previous_best: row.previous_best,
                },
            );
        }
        true
    }
}

#[async_trait]
impl LiveScorer for RlmScorer {
    async fn score(
        &self,
        pin: &ProofPin,
        topic: &TopicDocument,
        offer: &InferenceOffer,
        plan: &ExecutorPlan,
        frozen_digest: &str,
        artifact_digest: &str,
        artifact_uri: Option<&str>,
        declared_flops: u64,
        _holdout: &[HoldoutRecord],
        claim: &str,
        miner_env: &MinerEnv,
        artifact_tar: Option<&[u8]>,
    ) -> Result<ProofEvalDocument, EvalError> {
        self.ready_for_topic(topic)?;
        let lease = self.lease(&topic.id).await;
        self.ensure_topic(topic).await?;
        self.recover_stale(topic).await?;
        self.apply(topic, RlmEvent::SubmissionReceived, frozen_digest)
            .await?;
        let out = self
            .evaluate(
                pin,
                topic,
                offer,
                plan,
                frozen_digest,
                artifact_digest,
                artifact_uri,
                declared_flops,
                claim,
                miner_env,
                artifact_tar,
            )
            .await;
        match &out {
            Ok(_) => {
                // The lease now belongs to the pending run: promotion is
                // decided and persisted under it, then it is released in
                // `on_persisted`.
                self.hold(frozen_digest, lease);
            }
            Err(err) => {
                // The miner sees this string in the 503 body and nowhere
                // else; keep it in the host journal too so a failed paid run
                // can be traced without the miner's copy.
                tracing::error!(
                    topic_id = %topic.id,
                    frozen_digest,
                    error = %err,
                    "evaluate refused; no row"
                );
                // No row will follow a refusal, so the verdict phase is over now.
                self.apply_logged(topic, RlmEvent::VerdictRecorded, "refused; no row")
                    .await;
                drop(lease);
            }
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

    /// Registered ids whose runner reports ready (topic-VM orchestrator
    /// bearer file present, image pinned). Local checks only; no request
    /// leaves the host.
    fn ready_custom_ids(&self) -> Vec<String> {
        self.registry
            .ids()
            .into_iter()
            .filter(|id| {
                self.registry
                    .resolve(id)
                    .is_ok_and(|runner| runner.ready().is_ok())
            })
            .collect()
    }

    /// The RLM scorer is the custom family, never the Lium harvest.
    fn harvest_wired(&self) -> bool {
        false
    }

    /// Decided under the topic lease this run has held since `score`
    /// returned, against the harder of the caller's bar and the store's
    /// current best: no other run of this topic can be between score and
    /// persist, and a bar computed before an earlier crown cannot be reused.
    async fn auto_promote(
        &self,
        topic: &TopicDocument,
        submission_digest: &str,
        pass: bool,
        primary: Option<f64>,
        bar: Option<f64>,
    ) -> bool {
        let held = self
            .pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains_key(submission_digest);
        if !held {
            return false;
        }
        let current = match self.store.best(&topic.id).await {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(topic_id = %topic.id, %e, "best unreadable; no promotion");
                return false;
            }
        };
        let bar = tighter_bar(
            bar,
            current.as_ref().map(|b| b.primary_value),
            topic.metric.direction,
        );
        let mut pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(p) = pending.get_mut(submission_digest) else {
            return false;
        };
        let reported = p.bundle.report.as_ref().map(|r| r.primary_value);
        // The payout primary and the report must agree, or neither is evidence.
        let agree = matches!((primary, reported), (Some(a), Some(b)) if (a - b).abs() < 1e-9);
        let outcome = decide_promote(
            pass && agree,
            p.bundle.checklist_green,
            &p.bundle.checklist.failed_ids(),
            reported,
            bar,
            topic.metric.direction,
            topic.metric.epsilon_rel,
        );
        let promote = outcome.is_promote();
        p.decided = Some(Decided {
            outcome,
            previous_best: current,
            direction: topic.metric.direction,
        });
        promote
    }

    fn display_results(&self, submission_digest: &str) -> Option<serde_json::Value> {
        self.pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(submission_digest.trim())
            .and_then(|p| p.bundle.results.clone())
    }

    async fn on_persisted(
        &self,
        topic_id: &str,
        submission_digest: &str,
        submission_id: &str,
        promoted: bool,
    ) {
        // `pending` (and the topic lease inside it) lives to the end of this
        // function: the artefact, the promotion row, and the best pointer
        // land under the guard the decision was taken under.
        let Some(pending) = self.take(submission_digest) else {
            return;
        };
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
        let landed = self
            .persist(
                topic_id,
                submission_digest,
                submission_id,
                promoted,
                &pending,
            )
            .await;
        if let Some(t) = &topic {
            let event = match (promoted, landed) {
                (true, true) => RlmEvent::Promoted,
                (true, false) => RlmEvent::PromotionRefused,
                (false, _) => RlmEvent::VerdictRecorded,
            };
            self.apply_logged(t, event, submission_id).await;
        }
        drop(pending);
    }
}
