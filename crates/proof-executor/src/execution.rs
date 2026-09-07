use std::{
    collections::{BTreeMap, BTreeSet},
    time::{Duration, Instant},
};

use proof_autonomy::{CapabilityOperation, ResourceGrant};
use proof_autonomy_pg::ControllerLease;
use proof_measure::MeasurementRequest;
use proof_research::{
    artifact_digest, Measurement, PairedMeasurement, RetainedArtifacts, ScientificEvidence,
    ScientificRecipe,
};
use proof_score::{AgentVerdict, ProofKind};
use proof_task::HoldoutRecord;
use serde_json::{json, Value};
use tokio::sync::watch;

use crate::{DurableExecutor, Failure, Intent, Invocation, Outcome, Plan, RunObservation, POLL};

struct Science {
    recipe: ScientificRecipe,
    chain_epoch: u64,
    contamination_hits: Vec<String>,
}

struct Preflight {
    scripts: BTreeMap<String, String>,
    science: Option<Science>,
}

/// Controller scan of the committed script text for holdout shard hashes or
/// corpus ids. Scripts carry no declared training manifest, so this is the
/// only declaration available; a fingerprint literal in the source is a hit.
fn scan_contamination(
    scripts: &BTreeMap<String, String>,
    holdout: &[HoldoutRecord],
) -> Vec<String> {
    let mut hits = Vec::new();
    for source in scripts.values() {
        let lower = source.to_ascii_lowercase();
        let hashes: BTreeSet<String> = holdout
            .iter()
            .map(|r| r.content_sha256.to_ascii_lowercase())
            .filter(|h| lower.contains(h.as_str()))
            .collect();
        let datasets: BTreeSet<String> = holdout
            .iter()
            .map(|r| r.dataset_id.trim().to_owned())
            .filter(|d| !d.is_empty() && source.contains(d.as_str()))
            .collect();
        hits.extend(proof_task::contamination(&hashes, &datasets, holdout));
    }
    hits.sort();
    hits.dedup();
    hits
}

impl DurableExecutor {
    pub(crate) async fn work(
        &self,
        lease: &ControllerLease,
        grant: &ResourceGrant,
        plan: Plan,
        cancel: watch::Receiver<bool>,
    ) -> Result<Outcome, Failure> {
        let intent = self.journal.begin(lease, grant, plan, &self.target).await?;
        let result = self.perform(&intent, lease, grant, cancel).await;
        let stopped = self.cleanup(&intent).await;
        // If create/start was ambiguous, absence alone cannot certify that a
        // delayed daemon request will not execute; keep reconciliation pending.
        let unambiguous = !matches!(result, Err(Failure::Target | Failure::StopUnconfirmed));
        self.journal
            .finish(
                &intent,
                result.as_ref().err().copied(),
                stopped && unambiguous,
            )
            .await?;
        if !stopped {
            return Err(Failure::StopUnconfirmed);
        }
        result
    }

    async fn preflight(&self, intent: &Intent) -> Result<Preflight, Failure> {
        // Preflight and retain ALL committed sources before the first side effect.
        if let Some(source) = &intent.plan.kernel_source {
            self.journal.put_script(source.as_bytes()).await?;
        }
        let mut scripts = BTreeMap::new();
        for run in &intent.plan.runs {
            if let Invocation::Script { script_digest, .. } = run {
                if scripts.contains_key(script_digest) {
                    continue;
                }
                let bytes = self.journal.script(script_digest).await?;
                self.journal
                    .append(
                        intent,
                        json!({"event": "source", "digest": script_digest}),
                        std::slice::from_ref(&bytes),
                    )
                    .await?;
                scripts.insert(
                    script_digest.clone(),
                    String::from_utf8(bytes).map_err(|_| Failure::Input)?,
                );
            }
        }
        let mut science = None;
        if let Some(digest) = &intent.plan.recipe_digest {
            let recipe = self
                .research
                .recipe(digest)
                .await
                .map_err(|_| Failure::Commitment)?;
            let epoch = tokio::time::timeout(Duration::from_secs(10), self.chain.finalized_epoch())
                .await
                .map_err(|_| Failure::Chain)??;
            if epoch.chain_epoch == 0
                || epoch.hash == [0; 32]
                || !recipe.topic.is_open_at(epoch.chain_epoch)
            {
                return Err(Failure::Chain);
            }
            self.journal
                .append(
                    intent,
                    json!({"event": "finalized_epoch", "observation": epoch}),
                    &[],
                )
                .await?;
            // Holdout and observer availability are checked before any run
            // executes, so an unobservable collect never spends the sandbox.
            let holdout = self.observer.holdout(&recipe.topic)?;
            let hits = scan_contamination(&scripts, &holdout);
            science = Some(Science {
                recipe,
                chain_epoch: epoch.chain_epoch,
                contamination_hits: hits,
            });
        }
        Ok(Preflight { scripts, science })
    }

    async fn observe(
        &self,
        intent: &Intent,
        index: usize,
        run: &Invocation,
        science: &Science,
        receipt: (&RunObservation, &[u8]),
        artifacts: &mut RetainedArtifacts,
    ) -> Result<(Measurement, AgentVerdict), Failure> {
        let (receipt, receipt_bytes) = receipt;
        let log_digest = artifact_digest(receipt_bytes);
        artifacts.insert(log_digest.clone(), receipt_bytes.to_vec());
        let Invocation::Script {
            script_digest,
            seed: Some(seed),
            timeout_ms,
        } = run
        else {
            return Err(Failure::Input);
        };
        let artifact = self.target.artifact(intent, index).await?;
        let artifact_digest = artifact_digest(&artifact);
        self.journal
            .append(
                intent,
                json!({"event": "artifact", "index": index, "digest": artifact_digest}),
                std::slice::from_ref(&artifact),
            )
            .await?;
        let request = MeasurementRequest {
            experiment_id: intent.experiment_id,
            intent_id: intent.id,
            run_index: index,
            seed: *seed,
            script_digest: script_digest.clone(),
            artifact,
            artifact_digest,
            topic: science.recipe.topic.clone(),
            pin: self.research.pin().clone(),
            deadline_ms: intent.deadline_ms,
            timeout_ms: *timeout_ms,
        };
        let observation = self.observer.measure(request).await?;
        self.journal
            .append(
                intent,
                json!({"event": "observed", "index": index, "observer_image": observation.observer_image,
                    "observer_log_digest": observation.log_digest, "flops_used": observation.flops_used}),
                std::slice::from_ref(&observation.log),
            )
            .await?;
        artifacts.insert(observation.log_digest, observation.log);
        let measurement = Measurement {
            seed: *seed,
            script_digest: script_digest.clone(),
            log_digest,
            exit_code: receipt.exit_code.ok_or(Failure::Execution)?,
            wall_ms: receipt.wall_ms,
            // A declared zero is not a measurement; only an observed count admits.
            flops_used: observation
                .flops_used
                .ok_or(Failure::UnobservedMeasurements)?,
            metrics: observation.metrics,
        };
        Ok((measurement, observation.verdict))
    }

    async fn perform(
        &self,
        intent: &Intent,
        lease: &ControllerLease,
        grant: &ResourceGrant,
        mut cancel: watch::Receiver<bool>,
    ) -> Result<Outcome, Failure> {
        if *cancel.borrow() {
            return Err(Failure::Interrupted);
        }
        let Preflight { scripts, science } = self.preflight(intent).await?;
        let mut last = Value::Null;
        let mut artifacts = RetainedArtifacts::new();
        let mut measurements = Vec::new();
        for (index, run) in intent.plan.runs.iter().enumerate() {
            self.check(lease, grant, &cancel).await?;
            let mut payload = serde_json::to_value(run).map_err(|_| Failure::Input)?;
            if let Invocation::Script { script_digest, .. } = run {
                payload["script"] =
                    json!(scripts.get(script_digest).ok_or(Failure::MissingScript)?);
            }
            self.target.create(intent, index, payload).await?;
            self.journal
                .append(intent, json!({"event": "created", "index": index}), &[])
                .await?;
            self.check(lease, grant, &cancel).await?;
            self.target.start(intent, index).await?;
            self.journal
                .append(intent, json!({"event": "started", "index": index}), &[])
                .await?;
            let started = Instant::now();
            loop {
                let check = self.check(lease, grant, &cancel).await;
                if check.is_err()
                    || started.elapsed()
                        > Duration::from_millis(run.timeout_ms())
                            .saturating_add(Duration::from_secs(3))
                {
                    // Preserve any supervisor bytes already emitted before force removal.
                    let _ = self.target.interrupt(intent, index).await;
                    self.retain_available(intent, index).await?;
                    return Err(Failure::Interrupted);
                }
                if self.target.stopped(intent, index).await? {
                    break;
                }
                tokio::select! {
                    () = tokio::time::sleep(POLL) => {}
                    _ = cancel.changed() => {}
                }
            }
            let bytes = self.target.logs(intent, index).await?;
            let digest = artifact_digest(&bytes);
            // Retain before parsing so failed dependencies / malformed receipts
            // cannot erase evidence merely by preventing a successful decode.
            self.journal
                .append(
                    intent,
                    json!({"event": "run", "index": index, "log_digest": digest}),
                    std::slice::from_ref(&bytes),
                )
                .await?;
            let observation: RunObservation =
                serde_json::from_slice(&bytes).map_err(|_| Failure::Execution)?;
            validate(&observation, run)?;
            if observation.failure.is_some() || observation.exit_code != Some(0) {
                self.target.remove(intent, index).await?;
                return Err(Failure::Execution);
            }
            if let Some(science) = &science {
                let measurement = self
                    .observe(
                        intent,
                        index,
                        run,
                        science,
                        (&observation, &bytes),
                        &mut artifacts,
                    )
                    .await?;
                measurements.push(measurement);
            }
            self.target.remove(intent, index).await?;
            last = json!({"intent_id": intent.id, "index": index, "exit_code": observation.exit_code,
                "wall_ms": observation.wall_ms, "log_digest": digest,
                "log": String::from_utf8_lossy(&observation.log), "scientific_measurement": science.is_some()});
        }
        let Some(science) = science else {
            return Ok(Outcome {
                value: last,
                evidence: None,
            });
        };
        let evidence = self
            .evidence(intent, grant, science, measurements, &mut artifacts)
            .await?;
        Ok(Outcome {
            value: last,
            evidence: Some((evidence, artifacts)),
        })
    }

    async fn evidence(
        &self,
        intent: &Intent,
        grant: &ResourceGrant,
        science: Science,
        measurements: Vec<(Measurement, AgentVerdict)>,
        artifacts: &mut RetainedArtifacts,
    ) -> Result<ScientificEvidence, Failure> {
        let recipe = &science.recipe;
        for digest in [
            &recipe.topic.baseline.script_sha256,
            &recipe.candidate_script_digest,
        ] {
            artifacts.insert(digest.clone(), self.journal.script(digest).await?);
        }
        let (measurements, verdicts): (Vec<_>, Vec<_>) = measurements.into_iter().unzip();
        let mut pairs = measurements.into_iter();
        let mut paired = Vec::new();
        while let (Some(baseline), Some(candidate)) = (pairs.next(), pairs.next()) {
            paired.push(PairedMeasurement {
                baseline,
                candidate,
            });
        }
        // The observer's verdicts are the only agent envelope; the controller
        // never authors one. Any non-clean run fails the whole evidence.
        let mut verdict = verdicts
            .last()
            .cloned()
            .ok_or(Failure::UnobservedMeasurements)?;
        if verdicts.iter().any(|v| v.verdict != ProofKind::Clean) {
            verdict.verdict = ProofKind::Reject;
        }
        verdict.reproduced = verdicts.iter().all(|v| v.reproduced);
        verdict.claim_holds_public = verdicts.iter().all(|v| v.claim_holds_public);
        verdict.contamination = verdicts.iter().any(|v| v.contamination);
        verdict.cheat_codes = verdicts
            .iter()
            .flat_map(|v| v.cheat_codes.clone())
            .collect();
        verdict = verdict.truncated();
        let evidence = ScientificEvidence {
            schema_version: 1,
            experiment_id: intent.experiment_id,
            chain_epoch: science.chain_epoch,
            recipe_digest: intent.plan.recipe_digest.clone().ok_or(Failure::Input)?,
            resource_id: grant.resource_id.clone(),
            measurements: paired,
            contamination_hits: science.contamination_hits,
            verdict,
        };
        let summary = evidence
            .evaluate(recipe, artifacts)
            .map_err(|_| Failure::UnobservedMeasurements)?;
        self.journal
            .append(
                intent,
                json!({"event": "evidence", "digest": summary.evidence_digest, "passed": summary.passed}),
                &[],
            )
            .await?;
        Ok(evidence)
    }

    async fn check(
        &self,
        lease: &ControllerLease,
        grant: &ResourceGrant,
        cancel: &watch::Receiver<bool>,
    ) -> Result<(), Failure> {
        if *cancel.borrow() || cancel.has_changed().is_err() {
            return Err(Failure::Interrupted);
        }
        tokio::time::timeout(
            Duration::from_secs(1),
            self.journal
                .authorize(lease, grant, CapabilityOperation::Inspect),
        )
        .await
        .map_err(|_| Failure::Interrupted)?
    }

    pub(crate) async fn retain_available(
        &self,
        intent: &Intent,
        index: usize,
    ) -> Result<(), Failure> {
        if let Ok(bytes) = self.target.logs(intent, index).await {
            if !bytes.is_empty() {
                self.journal
                    .append(
                        intent,
                        json!({"event": "interrupted_log", "index": index,
                    "digest": artifact_digest(&bytes)}),
                        &[bytes],
                    )
                    .await?;
            }
        }
        Ok(())
    }

    pub(crate) async fn cleanup(&self, intent: &Intent) -> bool {
        let mut stopped = true;
        for index in 0..intent.plan.runs.len() {
            stopped &= self.target.remove(intent, index).await.is_ok();
        }
        stopped
    }
}

fn validate(observation: &RunObservation, run: &Invocation) -> Result<(), Failure> {
    let (digest, seed) = match run {
        Invocation::Terminal { .. } => (None, None),
        Invocation::Script {
            script_digest,
            seed,
            ..
        } => (Some(script_digest), *seed),
    };
    if observation.schema_version != 1
        || observation.script_digest.as_ref() != digest
        || observation.seed != seed
        || observation.wall_ms == 0
        || observation.log.len() > 65536
        || observation.flops_used.is_some()
        || observation.metrics.is_some()
    {
        return Err(Failure::Commitment);
    }
    if observation.wall_ms > run.timeout_ms() {
        return Err(Failure::Interrupted);
    }
    Ok(())
}
