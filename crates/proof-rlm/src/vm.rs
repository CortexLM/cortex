//! Topic-VM orchestrator boundary.
//!
//! Every topic's RLM runs **inside a VM attributed to that topic** — it
//! writes rules, runs the baseline, inspects and runs miner submissions
//! there, and never touches the control-plane host filesystem or secrets.
//! Miner code runs in a Firecracker guest under that VM when the topic says
//! `firecracker_required`. The control plane is the orchestrator: it asks
//! for a VM, hands it [`VmJob`]s (public topic data, digests, rule versions —
//! never host paths, never keys), reads back documents, and tears the VM down
//! or retains it by policy.
//!
//! The only orchestrator shipped in this crate is [`UnwiredVmOrchestrator`]:
//! it refuses every call and names the env vars a live one reads. The live
//! implementation (`FirecrackerOrchestrator`, crate `proof-vm-fc`) is a thin
//! HTTPS client of the `proof-vm-orchestrator` agent on a dedicated KVM host,
//! where jailer boots one Firecracker RLM VM per topic and every miner run is
//! a **sister** Firecracker guest. There is no host-local execution path
//! anywhere — a missing orchestrator is a 503, not a fallback.
//!
//! A topic whose signed `constraints.params` select an **in-guest runner**
//! (`proof_experiment`: runner id + pinned pack digest, optional size) runs
//! each paid job in a **dedicated experiment VM** instead — one VM per
//! experiment, created for the job and stopped after it (destroyed when the
//! job succeeded and its report passed final verification, **retained** on
//! the KVM host when it failed or the `Evaluated` report failed binding /
//! sandbox checks, so the guest console and `report.json` scratch survive
//! for root-cause analysis), sized under the operator ceilings
//! ([`run_paid_job`]). Parallel experiments are
//! parallel VMs, never containers sharing one VM. Nothing here knows what
//! the runner or the pack are; both are topic data resolved inside the guest.

use std::sync::Arc;

use async_trait::async_trait;
use proof_canon::is_slug;
use proof_experiment::{
    ExperimentBinding, ExperimentError, ExperimentPolicy, ExperimentSpec, RunPolicy,
};
use proof_task::{ChecklistRule, TopicDocument};
use serde::{Deserialize, Serialize};

use crate::gate::SpendToken;
use crate::rules::RuleSet;
use crate::runner::{
    CustomRunReport, CustomRunRequest, CustomRunner, InspectOutcome, ReportError, RunOutcome,
    RunnerError, SandboxPolicy,
};

/// Env var naming the orchestrator base URL (operator state, never git).
pub const VM_ORCHESTRATOR_URL_ENV: &str = "PROOF_VM_ORCHESTRATOR_URL";

/// Env var naming the orchestrator bearer **file**. Never logged.
pub const VM_ORCHESTRATOR_TOKEN_FILE_ENV: &str = "PROOF_VM_ORCHESTRATOR_TOKEN_FILE";

/// Env var naming the `sha256:` digest of the RLM VM image the orchestrator boots.
pub const RLM_VM_IMAGE_DIGEST_ENV: &str = "PROOF_RLM_VM_IMAGE_DIGEST";

/// What happens to a topic VM when the topic leaves service.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetainPolicy {
    /// Destroy the VM; artefacts already live in the topic-scoped store.
    Destroy,
    /// Keep the VM (and its scratch) for audit.
    Retain,
}

/// Host-independent VM shape: an image digest plus sizes. Operator config.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VmTemplate {
    /// `sha256:` digest of the RLM VM image. Empty = unpinned = never boots.
    pub image_digest: String,
    /// vCPUs (1..=64).
    pub vcpus: u32,
    /// Guest memory in MiB (512..=131072).
    pub mem_mib: u32,
}

impl VmTemplate {
    /// A template with no image pin: validates false, so nothing boots.
    #[must_use]
    pub fn unpinned() -> Self {
        Self {
            image_digest: String::new(),
            vcpus: 2,
            mem_mib: 4_096,
        }
    }

    /// Template from [`RLM_VM_IMAGE_DIGEST_ENV`] (unpinned when unset).
    #[must_use]
    pub fn from_env() -> Self {
        Self {
            image_digest: std::env::var(RLM_VM_IMAGE_DIGEST_ENV)
                .map(|s| s.trim().to_owned())
                .unwrap_or_default(),
            ..Self::unpinned()
        }
    }

    /// Ranges and digest shape.
    ///
    /// # Errors
    ///
    /// [`VmError::Spec`] naming the first bad field.
    pub fn validate(&self) -> Result<(), VmError> {
        let hex = self
            .image_digest
            .trim()
            .strip_prefix("sha256:")
            .unwrap_or("");
        let checks: [(&'static str, bool); 3] = [
            ("image_digest", proof_canon::is_hex64(hex)),
            ("vcpus", (1..=64).contains(&self.vcpus)),
            ("mem_mib", (512..=131_072).contains(&self.mem_mib)),
        ];
        match checks.iter().find(|(_, ok)| !ok) {
            Some((field, _)) => Err(VmError::Spec(field)),
            None => Ok(()),
        }
    }
}

/// What the orchestrator is asked to create for one topic — its RLM VM, or,
/// with `experiment` set, a dedicated experiment VM for one paid job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopicVmSpec {
    /// Topic the VM is attributed to (one topic ↔ its own RLM VM; any number
    /// of experiment VMs, each for one job).
    pub topic_id: String,
    /// Image + sizes.
    pub template: VmTemplate,
    /// Sandbox policy for miner code inside the VM.
    pub sandbox: SandboxPolicy,
    /// What to do with the VM when the topic closes.
    pub retain: RetainPolicy,
    /// Present iff this is an experiment VM: the runner the guest resolves,
    /// the pack the host stages, the writable disk. Public data only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub experiment: Option<ExperimentSpec>,
}

impl TopicVmSpec {
    /// Spec for `topic` from a template and the topic's own sandbox policy.
    #[must_use]
    pub fn for_topic(topic_id: &str, template: VmTemplate, sandbox: SandboxPolicy) -> Self {
        Self {
            topic_id: topic_id.trim().to_owned(),
            template,
            sandbox,
            retain: RetainPolicy::Destroy,
            experiment: None,
        }
    }

    /// Spec for one experiment VM of `topic`. The teardown after its job
    /// picks the policy from the outcome ([`run_paid_job`]: destroy on
    /// success, retain on failure); this create-time `retain` only governs
    /// how the agent reaps the VM should its process die outside a teardown.
    #[must_use]
    pub fn for_experiment(
        topic_id: &str,
        template: VmTemplate,
        sandbox: SandboxPolicy,
        experiment: ExperimentSpec,
    ) -> Self {
        Self {
            experiment: Some(experiment),
            ..Self::for_topic(topic_id, template, sandbox)
        }
    }

    /// Slug topic id + valid template (+ valid experiment shape).
    ///
    /// # Errors
    ///
    /// [`VmError::Spec`] / [`VmError::Experiment`].
    pub fn validate(&self) -> Result<(), VmError> {
        if !is_slug(&self.topic_id) {
            return Err(VmError::Spec("topic_id"));
        }
        self.template.validate()?;
        if let Some(e) = &self.experiment {
            e.validate()?;
        }
        Ok(())
    }
}

/// One provisioned topic VM.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VmHandle {
    /// Topic the VM belongs to.
    pub topic_id: String,
    /// Orchestrator VM id.
    pub vm_id: String,
}

/// Work handed to the RLM inside its VM. Public data only: a job never
/// carries a host path, a key, or an origin.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "job", rename_all = "snake_case")]
pub enum VmJob {
    /// Let the RLM read the signed topic and write (or rewrite) its rules.
    ProposeRules {
        /// The signed topic (public document).
        topic: Box<TopicDocument>,
        /// Rule version to supersede, if any.
        current_version: Option<u32>,
    },
    /// Run the baseline artefact so the operator can seal `custom_value`.
    Baseline {
        /// Request shaped exactly like a miner run.
        request: CustomRunRequest,
    },
    /// Tick every rule over a miner artefact. No paid inference.
    Inspect {
        /// The run request.
        request: CustomRunRequest,
        /// Rules to tick.
        rules: RuleSet,
    },
    /// Run a miner artefact behind a spend token.
    Evaluate {
        /// The run request.
        request: CustomRunRequest,
        /// Digest of the checklist that minted the token (audit binding).
        checklist_digest: String,
        /// Rule version the token was minted for.
        rules_version: u32,
    },
    /// Flush the VM's scratch into the topic-scoped artefact store.
    Archive {
        /// Topic id.
        topic_id: String,
    },
}

impl VmJob {
    /// The topic every job is attributed to. An orchestrator refuses a job
    /// whose topic is not the one its VM is bound to.
    #[must_use]
    pub fn topic_id(&self) -> &str {
        match self {
            Self::ProposeRules { topic, .. } => &topic.id,
            Self::Baseline { request }
            | Self::Inspect { request, .. }
            | Self::Evaluate { request, .. } => &request.topic_id,
            Self::Archive { topic_id } => topic_id,
        }
    }

    /// Wall-clock budget the job's run request carries, if it carries one.
    #[must_use]
    pub fn deadline_s(&self) -> Option<u64> {
        match self {
            Self::Baseline { request }
            | Self::Inspect { request, .. }
            | Self::Evaluate { request, .. } => Some(request.sandbox.deadline_s),
            Self::ProposeRules { .. } | Self::Archive { .. } => None,
        }
    }

    /// The run request a job carries, if it carries one.
    #[must_use]
    pub fn request(&self) -> Option<&CustomRunRequest> {
        match self {
            Self::Baseline { request }
            | Self::Inspect { request, .. }
            | Self::Evaluate { request, .. } => Some(request),
            Self::ProposeRules { .. } | Self::Archive { .. } => None,
        }
    }

    /// Strip vault bytes from a job the host already injected over vsock.
    #[must_use]
    pub fn without_artifact_tar(self) -> Self {
        match self {
            Self::Baseline { request } => Self::Baseline {
                request: request.without_artifact_tar(),
            },
            Self::Inspect { request, rules } => Self::Inspect {
                request: request.without_artifact_tar(),
                rules,
            },
            Self::Evaluate {
                request,
                checklist_digest,
                rules_version,
            } => Self::Evaluate {
                request: request.without_artifact_tar(),
                checklist_digest,
                rules_version,
            },
            other => other,
        }
    }

    /// Whether the job runs miner code and the topic demands the guest.
    #[must_use]
    pub fn requires_firecracker(&self) -> bool {
        match self {
            Self::Baseline { request } | Self::Evaluate { request, .. } => {
                request.sandbox.firecracker_required
            }
            Self::ProposeRules { .. } | Self::Inspect { .. } | Self::Archive { .. } => false,
        }
    }
}

/// What a job produced. Serialised adjacently tagged (`output` / `body`) so
/// an orchestrator can answer over the wire with the same type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "output", content = "body", rename_all = "snake_case")]
pub enum VmJobOutput {
    /// Rules the RLM proposes; the store versions them.
    Rules(Vec<ChecklistRule>),
    /// Baseline measurement.
    Baseline(CustomRunReport),
    /// Inspection result.
    Inspected(InspectOutcome),
    /// Paid run result.
    Evaluated(RunOutcome),
    /// Scratch archived.
    Archived,
}

/// Why the orchestrator refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum VmError {
    /// No live orchestrator configured.
    #[error("topic-vm orchestrator not wired: {0}")]
    NotWired(String),
    /// A spec field is missing or out of range.
    #[error("topic-vm spec: {0} is missing or out of range")]
    Spec(&'static str),
    /// The orchestrator failed.
    #[error("topic-vm orchestrator: {0}")]
    Backend(String),
    /// The job's output was not the shape the job asked for.
    #[error("topic-vm returned the wrong output for {0}")]
    WrongOutput(&'static str),
    /// The job returned a document that is not evidence (binding / sandbox).
    #[error("run report: {0}")]
    Report(#[from] ReportError),
    /// The topic's in-guest experiment binding is malformed or over a ceiling.
    #[error("experiment vm: {0}")]
    Experiment(#[from] ExperimentError),
    /// An experiment VM was not confirmed destroyed after its **successful**
    /// paid job. The job's outcome is withheld: nothing is scored while the
    /// VM may still hold host capacity. (A job that failed returns its own
    /// error; a retain that was not confirmed is logged, never scored.)
    #[error("experiment vm {vm_id} not confirmed destroyed after its job ({reason}); the outcome is withheld, not scored — reconcile the vm on the KVM host")]
    TeardownUnconfirmed {
        /// The VM the orchestrator did not confirm gone.
        vm_id: String,
        /// What the orchestrator answered.
        reason: String,
    },
}

/// Create / attach / run / teardown for topic VMs.
#[async_trait]
pub trait TopicVmOrchestrator: Send + Sync {
    /// Whether the orchestrator is configured (fail-closed).
    ///
    /// # Errors
    ///
    /// [`VmError::NotWired`].
    fn ready(&self) -> Result<(), VmError>;

    /// Provision a VM for `spec.topic_id`.
    async fn create(&self, spec: &TopicVmSpec) -> Result<VmHandle, VmError>;

    /// The existing VM for `topic_id`, if any.
    async fn attach(&self, topic_id: &str) -> Result<Option<VmHandle>, VmError>;

    /// Run one job inside the VM.
    async fn run(&self, handle: &VmHandle, job: VmJob) -> Result<VmJobOutput, VmError>;

    /// Tear down (or retain) the VM. `Ok(true)` only when the orchestrator
    /// confirms the requested end state.
    async fn teardown(&self, handle: &VmHandle, policy: RetainPolicy) -> Result<bool, VmError>;
}

/// The CI-safe orchestrator: nothing is configured, every call refuses.
pub struct UnwiredVmOrchestrator;

impl UnwiredVmOrchestrator {
    fn refuse() -> VmError {
        VmError::NotWired(format!(
            "no orchestrator configured ({VM_ORCHESTRATOR_URL_ENV} / {VM_ORCHESTRATOR_TOKEN_FILE_ENV})"
        ))
    }
}

#[async_trait]
impl TopicVmOrchestrator for UnwiredVmOrchestrator {
    fn ready(&self) -> Result<(), VmError> {
        Err(Self::refuse())
    }

    async fn create(&self, _spec: &TopicVmSpec) -> Result<VmHandle, VmError> {
        Err(Self::refuse())
    }

    async fn attach(&self, _topic_id: &str) -> Result<Option<VmHandle>, VmError> {
        Err(Self::refuse())
    }

    async fn run(&self, _handle: &VmHandle, _job: VmJob) -> Result<VmJobOutput, VmError> {
        Err(Self::refuse())
    }

    async fn teardown(&self, _handle: &VmHandle, _policy: RetainPolicy) -> Result<bool, VmError> {
        Err(Self::refuse())
    }
}

fn map_vm(e: VmError) -> RunnerError {
    match e {
        VmError::NotWired(m) => RunnerError::NotWired(m),
        VmError::Report(e) => RunnerError::Report(e),
        other => RunnerError::Backend(other.to_string()),
    }
}

impl CustomRunRequest {
    /// The in-guest experiment binding the topic's signed params carry, if
    /// the topic selects one (`proof_experiment::PARAM_RUNNER`).
    ///
    /// # Errors
    ///
    /// [`ExperimentError`] for a half-selected or malformed binding — refused,
    /// never ignored.
    pub fn experiment(&self) -> Result<Option<ExperimentBinding>, ExperimentError> {
        ExperimentBinding::from_params(&self.constraints.params)
    }
}

/// Run one paid job (`Baseline` / `Evaluate`) where its topic says it runs.
///
/// A topic that selects no in-guest runner runs the job in `topic_vm` as
/// before. A topic that does gets **one dedicated experiment VM for this
/// job**: created from `policy` (its image, or the RLM `template`'s; sized
/// by the topic's ask under the ceilings — an ask over a ceiling is refused,
/// never clamped), handed the job, then **stopped** whatever the outcome —
/// [`RetainPolicy::Destroy`] after a successful job whose report passes final
/// verification, [`RetainPolicy::Retain`] after a failed one **or** an
/// `Evaluated` report that fails binding / sandbox checks, so the guest
/// console and `report.json` scratch land under the host's retain dir
/// (`PROOF_VM_AGENT_RETAIN_DIR`) for root-cause analysis instead of vanishing
/// with the jail. A retained VM is never
/// reused and holds no capacity slot. The KVM host stages the pinned pack at
/// boot and attests the run as `experiment_vm`; the guest resolves the runner
/// id itself. Non-paid jobs always run in `topic_vm`.
///
/// # Errors
///
/// [`VmError::Experiment`] for a malformed / over-ceiling binding (before
/// any VM exists), else whatever the orchestrator returned. A successful
/// outcome is returned **only when the orchestrator confirms the VM
/// destroyed** (`teardown` → `Ok(true)`): an unconfirmed (`Ok(false)`) or
/// failed teardown is [`VmError::TeardownUnconfirmed`] even for a successful
/// run — fail-closed, so a result is never scored while its VM may still
/// consume host capacity. A failed job returns **its own error** whatever
/// the retain answered (nothing is scored either way); a retain the
/// orchestrator did not confirm is logged with the VM id for the operator
/// to reconcile, never folded into the miner-visible failure.
pub async fn run_paid_job(
    orchestrator: &dyn TopicVmOrchestrator,
    policy: &ExperimentPolicy,
    template: &VmTemplate,
    topic_vm: &VmHandle,
    job: VmJob,
) -> Result<VmJobOutput, VmError> {
    let request = match &job {
        VmJob::Baseline { request } | VmJob::Evaluate { request, .. } => request,
        VmJob::ProposeRules { .. } | VmJob::Inspect { .. } | VmJob::Archive { .. } => {
            return orchestrator.run(topic_vm, job).await;
        }
    };
    let Some(binding) = request.experiment()? else {
        return orchestrator.run(topic_vm, job).await;
    };
    // The generic run policy (tasks, gates, wall clocks, exception policy)
    // is shape-checked here, before any VM exists — and again in the guest.
    let run_policy = RunPolicy::from_params(&request.constraints.params)?;
    let shape = policy.ceilings.shape(&binding)?;
    let spec = TopicVmSpec::for_experiment(
        &request.topic_id,
        VmTemplate {
            image_digest: policy.image_for(&template.image_digest).to_owned(),
            vcpus: shape.vcpus,
            mem_mib: shape.mem_mib,
        },
        request.sandbox.clone(),
        ExperimentSpec {
            runner: binding.runner.clone(),
            pack: binding.pack.clone(),
            disk_mib: shape.disk_mib,
        },
    );
    spec.validate()?;
    let submission = request.submission_digest.clone();
    let verify = request.clone();
    let vm = orchestrator.create(&spec).await?;
    tracing::info!(
        topic_id = %vm.topic_id, vm_id = %vm.vm_id, runner = %binding.runner,
        pack = %binding.pack.digest, vcpus = shape.vcpus, mem_mib = shape.mem_mib,
        disk_mib = shape.disk_mib, run_policy = %run_policy.summary(),
        "experiment vm created for one paid job"
    );
    let outcome = match orchestrator.run(&vm, job).await {
        Ok(VmJobOutput::Evaluated(out)) => match out.report.verify(&verify) {
            Ok(()) => Ok(VmJobOutput::Evaluated(out)),
            Err(e) => Err(VmError::Report(e)),
        },
        Ok(VmJobOutput::Baseline(r)) => match r.verify(&verify) {
            Ok(()) => Ok(VmJobOutput::Baseline(r)),
            Err(e) => Err(VmError::Report(e)),
        },
        Ok(_) => Err(VmError::WrongOutput("paid")),
        Err(e) => Err(e),
    };
    // A failed job — including an Evaluated report that fails final
    // verification — keeps its jail (guest console, report.json scratch) on
    // the KVM host for root-cause analysis; a successful one that passed
    // those checks frees it. Either way the VM is stopped and never reused.
    let (policy, verb) = if outcome.is_ok() {
        (RetainPolicy::Destroy, "destroy")
    } else {
        (RetainPolicy::Retain, "retain")
    };
    let ended = match orchestrator.teardown(&vm, policy).await {
        Ok(true) => Ok(()),
        Ok(false) => Err(format!("the orchestrator did not confirm the {verb}")),
        Err(e) => Err(format!("teardown failed: {e}")),
    };
    match (outcome, ended) {
        (Ok(output), Ok(())) => {
            tracing::info!(vm_id = %vm.vm_id, "experiment vm destroyed after its job");
            Ok(output)
        }
        (Ok(_), Err(teardown)) => {
            tracing::error!(
                vm_id = %vm.vm_id, topic_id = %vm.topic_id,
                "experiment vm not confirmed destroyed; withholding the job outcome: {teardown}"
            );
            Err(VmError::TeardownUnconfirmed {
                vm_id: vm.vm_id.clone(),
                reason: teardown,
            })
        }
        (Err(job), Ok(())) => {
            tracing::error!(
                vm_id = %vm.vm_id, topic_id = %vm.topic_id, %submission, error = %job,
                "experiment vm job failed; vm retained on the kvm host for root-cause analysis"
            );
            Err(job)
        }
        (Err(job), Err(teardown)) => {
            tracing::error!(
                vm_id = %vm.vm_id, topic_id = %vm.topic_id, %submission, error = %job,
                "experiment vm job failed and the vm was not confirmed retained ({teardown}); reconcile it on the kvm host"
            );
            Err(job)
        }
    }
}

/// The generic runner: every inspect / evaluate is a job inside the topic's
/// VM — or, for a topic whose params select an in-guest runner, inside a
/// dedicated experiment VM per paid job ([`run_paid_job`]). Registering it
/// under a `custom_id` is an operator action; nothing registers it by default.
pub struct VmBackedRunner {
    orchestrator: Arc<dyn TopicVmOrchestrator>,
    template: VmTemplate,
    experiments: ExperimentPolicy,
}

impl VmBackedRunner {
    /// Runner over `orchestrator` booting `template` for topics without a VM.
    /// Experiment VMs follow the default policy until
    /// [`with_experiments`](Self::with_experiments) sets the operator's.
    #[must_use]
    pub fn new(orchestrator: Arc<dyn TopicVmOrchestrator>, template: VmTemplate) -> Self {
        Self {
            orchestrator,
            template,
            experiments: ExperimentPolicy::default(),
        }
    }

    /// Operator policy (ceilings, image) for per-experiment VMs.
    #[must_use]
    pub fn with_experiments(mut self, experiments: ExperimentPolicy) -> Self {
        self.experiments = experiments;
        self
    }

    /// The experiment policy in force.
    #[must_use]
    pub fn experiments(&self) -> &ExperimentPolicy {
        &self.experiments
    }

    /// The runner an unconfigured host would get: unwired orchestrator,
    /// unpinned image. `ready()` names the orchestrator as the root cause.
    #[must_use]
    pub fn unwired() -> Self {
        Self::new(Arc::new(UnwiredVmOrchestrator), VmTemplate::unpinned())
    }

    /// The topic's VM, created on first use.
    async fn vm_for(&self, req: &CustomRunRequest) -> Result<VmHandle, RunnerError> {
        self.ready()?;
        if let Some(h) = self
            .orchestrator
            .attach(&req.topic_id)
            .await
            .map_err(map_vm)?
        {
            return Ok(h);
        }
        let spec =
            TopicVmSpec::for_topic(&req.topic_id, self.template.clone(), req.sandbox.clone());
        spec.validate().map_err(map_vm)?;
        self.orchestrator.create(&spec).await.map_err(map_vm)
    }
}

#[async_trait]
impl CustomRunner for VmBackedRunner {
    fn ready(&self) -> Result<(), RunnerError> {
        self.orchestrator.ready().map_err(map_vm)?;
        self.template.validate().map_err(map_vm)
    }

    async fn inspect(
        &self,
        req: &CustomRunRequest,
        rules: &RuleSet,
    ) -> Result<InspectOutcome, RunnerError> {
        let vm = self.vm_for(req).await?;
        let job = VmJob::Inspect {
            request: req.clone(),
            rules: rules.clone(),
        };
        match self.orchestrator.run(&vm, job).await.map_err(map_vm)? {
            VmJobOutput::Inspected(out) => Ok(out),
            _ => Err(map_vm(VmError::WrongOutput("inspect"))),
        }
    }

    async fn evaluate(
        &self,
        req: &CustomRunRequest,
        spend: &SpendToken,
    ) -> Result<RunOutcome, RunnerError> {
        if !spend.covers(&req.topic_id, &req.submission_digest)
            || spend.rules_version() != req.rules_version
        {
            return Err(RunnerError::SpendTokenMismatch);
        }
        let vm = self.vm_for(req).await?;
        let job = VmJob::Evaluate {
            request: req.clone(),
            checklist_digest: spend.checklist_digest().to_owned(),
            rules_version: spend.rules_version(),
        };
        let output = run_paid_job(
            self.orchestrator.as_ref(),
            &self.experiments,
            &self.template,
            &vm,
            job,
        )
        .await
        .map_err(map_vm)?;
        match output {
            VmJobOutput::Evaluated(out) => {
                out.report.verify(req)?;
                Ok(out)
            }
            _ => Err(map_vm(VmError::WrongOutput("evaluate"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{pinned_template, request, rules, token_for, FakeOrchestrator};

    #[tokio::test]
    async fn the_unwired_runner_refuses_before_any_call() {
        let runner = VmBackedRunner::unwired();
        let err = runner.ready().expect_err("unwired");
        assert!(matches!(err, RunnerError::NotWired(_)), "{err}");
        assert!(err.to_string().contains(VM_ORCHESTRATOR_URL_ENV), "{err}");
        let req = request();
        assert!(matches!(
            runner.inspect(&req, &rules()).await,
            Err(RunnerError::NotWired(_))
        ));
        assert!(matches!(
            runner.evaluate(&req, &token_for(&req)).await,
            Err(RunnerError::NotWired(_))
        ));
        assert!(UnwiredVmOrchestrator.ready().is_err());
    }

    #[tokio::test]
    async fn an_unpinned_image_over_a_live_orchestrator_still_refuses() {
        let runner = VmBackedRunner::new(FakeOrchestrator::new(0.5), VmTemplate::unpinned());
        assert_eq!(
            runner.ready(),
            Err(RunnerError::Backend(
                VmError::Spec("image_digest").to_string()
            ))
        );
        assert!(VmTemplate::unpinned().validate().is_err());
        pinned_template().validate().expect("pinned");
        let mut tiny = pinned_template();
        tiny.mem_mib = 64;
        assert_eq!(tiny.validate(), Err(VmError::Spec("mem_mib")));
        let spec = TopicVmSpec::for_topic(
            "Bad Topic",
            pinned_template(),
            SandboxPolicy {
                firecracker_required: true,
                deadline_s: 60,
            },
        );
        assert_eq!(spec.validate(), Err(VmError::Spec("topic_id")));
    }

    /// One topic, one VM: the first job creates it, later jobs attach.
    #[tokio::test]
    async fn one_topic_one_vm_and_jobs_carry_public_data_only() {
        let orch = FakeOrchestrator::new(0.8);
        let runner = VmBackedRunner::new(orch.clone(), pinned_template());
        runner.ready().expect("ready");
        let req = request();
        let inspected = runner.inspect(&req, &rules()).await.expect("inspect");
        assert!(inspected.checklist.is_green(&rules()));
        let run = runner
            .evaluate(&req, &token_for(&req))
            .await
            .expect("evaluate");
        assert!((run.report.primary_value - 0.8).abs() < 1e-12);
        assert_eq!(
            orch.created(),
            1,
            "the second job attached, it did not create"
        );
        let jobs = orch.jobs();
        assert_eq!(jobs.len(), 2);
        for job in &jobs {
            let dump = serde_json::to_string(job).expect("json");
            for forbidden in ["/run/base", "/opt/base", "api_key", "127.0.0.1", "base_url"] {
                assert!(!dump.contains(forbidden), "job leaked {forbidden}: {dump}");
            }
        }
        assert!(matches!(jobs[0], VmJob::Inspect { .. }));
        assert!(matches!(jobs[1], VmJob::Evaluate { .. }));
    }

    #[tokio::test]
    async fn a_token_for_another_submission_or_rule_version_never_runs() {
        let orch = FakeOrchestrator::new(0.8);
        let runner = VmBackedRunner::new(orch.clone(), pinned_template());
        let req = request();
        let mut other = req.clone();
        other.submission_digest = "digest-b".into();
        assert_eq!(
            runner.evaluate(&req, &token_for(&other)).await,
            Err(RunnerError::SpendTokenMismatch)
        );
        let mut stale = req.clone();
        stale.rules_version = 2;
        assert_eq!(
            runner.evaluate(&stale, &token_for(&req)).await,
            Err(RunnerError::SpendTokenMismatch)
        );
        assert!(orch.jobs().is_empty(), "no job before the token checks");
    }

    #[tokio::test]
    async fn a_report_that_escaped_the_sandbox_is_not_evidence() {
        let orch = FakeOrchestrator::new(0.8);
        orch.set_sandboxed(false);
        let runner = VmBackedRunner::new(orch.clone(), pinned_template());
        let req = request();
        let err = runner
            .evaluate(&req, &token_for(&req))
            .await
            .expect_err("unsandboxed");
        assert!(matches!(
            err,
            RunnerError::Report(crate::runner::ReportError::NotSandboxed)
        ));
        assert!(orch.teardowns().is_empty(), "topic vm is not torn down");
    }

    /// An experiment-backed evaluate that returns `Ok(Evaluated)` with a
    /// report that fails final verification (`sandboxed=false` on a
    /// `firecracker_required` topic) must **retain** the jail. Policy used to
    /// follow `orchestrator.run(...).is_ok()` alone, so this Destroyed the
    /// evidence before `VmBackedRunner::evaluate` rejected the report.
    #[tokio::test]
    async fn an_invalid_evaluated_report_retains_the_experiment_vm() {
        use crate::fixtures::experiment_request;
        let orch = FakeOrchestrator::new(0.8);
        orch.set_sandboxed(false);
        let runner = VmBackedRunner::new(orch.clone(), pinned_template());
        let req = experiment_request(None);
        let err = runner
            .evaluate(&req, &token_for(&req))
            .await
            .expect_err("unsandboxed report is not evidence");
        assert!(matches!(
            err,
            RunnerError::Report(crate::runner::ReportError::NotSandboxed)
        ));
        let downs = orch.teardowns();
        assert_eq!(downs.len(), 1, "the experiment vm is stopped");
        assert_eq!(
            downs[0].1,
            RetainPolicy::Retain,
            "final-verification failure keeps the jail for RCA"
        );
        assert_eq!(
            orch.vms().len(),
            1,
            "the topic vm only; a retained vm is stopped"
        );
        assert_eq!(orch.experiments().len(), 1);

        orch.set_sandboxed(false);
        let topic_vm = orch
            .attach(&req.topic_id)
            .await
            .expect("attach")
            .expect("topic vm");
        let err = run_paid_job(
            orch.as_ref(),
            &ExperimentPolicy::default(),
            &pinned_template(),
            &topic_vm,
            VmJob::Baseline {
                request: req.clone(),
            },
        )
        .await
        .expect_err("unsandboxed baseline is not evidence");
        assert!(matches!(
            err,
            VmError::Report(crate::runner::ReportError::NotSandboxed)
        ));
        assert_eq!(
            orch.teardowns().last().map(|(_, p)| *p),
            Some(RetainPolicy::Retain),
            "a failed baseline verify retains too"
        );
    }

    /// Every job names its topic (the orchestrator's hard bind), the paid
    /// jobs carry the deadline the guest is held to, and outputs round-trip
    /// over the wire as the same type.
    #[test]
    fn jobs_name_their_topic_and_outputs_round_trip() {
        let req = request();
        let jobs = [
            VmJob::ProposeRules {
                topic: Box::new(crate::fixtures::topic()),
                current_version: None,
            },
            VmJob::Baseline {
                request: req.clone(),
            },
            VmJob::Inspect {
                request: req.clone(),
                rules: rules(),
            },
            VmJob::Evaluate {
                request: req.clone(),
                checklist_digest: "c".into(),
                rules_version: 1,
            },
            VmJob::Archive {
                topic_id: req.topic_id.clone(),
            },
        ];
        for job in &jobs {
            assert_eq!(job.topic_id(), req.topic_id);
        }
        assert_eq!(jobs[0].deadline_s(), None);
        assert_eq!(jobs[1].deadline_s(), Some(req.sandbox.deadline_s));
        assert_eq!(jobs[3].deadline_s(), Some(req.sandbox.deadline_s));
        assert!(jobs[1].requires_firecracker() && jobs[3].requires_firecracker());
        assert!(
            !jobs[2].requires_firecracker(),
            "inspection runs no miner code"
        );
        let outputs = [
            VmJobOutput::Rules(rules().rules),
            VmJobOutput::Baseline(crate::fixtures::report_for(&req, 0.5)),
            VmJobOutput::Inspected(crate::runner::InspectOutcome {
                checklist: crate::fixtures::green(&rules(), &req.submission_digest),
                artifact: vec![],
            }),
            VmJobOutput::Evaluated(crate::runner::RunOutcome {
                report: crate::fixtures::report_for(&req, 0.5),
                logs: vec![],
            }),
            VmJobOutput::Archived,
        ];
        for out in outputs {
            let json = serde_json::to_string(&out).expect("json");
            assert!(json.contains("\"output\""), "{json}");
            let back: VmJobOutput = serde_json::from_str(&json).expect("round trip");
            assert_eq!(back, out);
        }
    }

    /// A topic whose params select an in-guest runner gets **one experiment
    /// VM per paid job**: created for the job, sized by the topic's ask under
    /// the ceilings (silent knobs = the operator defaults), destroyed
    /// afterwards. The topic's RLM VM stays for inspection and is what
    /// `attach` returns.
    #[tokio::test]
    async fn an_experiment_topic_gets_one_vm_per_paid_job_destroyed_after_it() {
        use crate::fixtures::experiment_request;
        let orch = FakeOrchestrator::new(0.8);
        let runner = VmBackedRunner::new(orch.clone(), pinned_template());
        let req = experiment_request(Some(8));
        runner.inspect(&req, &rules()).await.expect("inspect");
        assert_eq!(orch.created(), 1, "inspection uses the topic vm");
        assert!(orch.experiments().is_empty());
        let run = runner
            .evaluate(&req, &token_for(&req))
            .await
            .expect("evaluate");
        assert!((run.report.primary_value - 0.8).abs() < 1e-12);
        assert_eq!(orch.created(), 2, "one experiment vm for the paid job");
        let specs = orch.experiments();
        assert_eq!(specs.len(), 1);
        let spec = &specs[0];
        let exp = spec.experiment.as_ref().expect("experiment spec");
        assert_eq!(exp.runner, "placeholder_in_guest_runner");
        assert_eq!(exp.pack.digest, format!("sha256:{}", "ee".repeat(32)));
        assert_eq!(exp.disk_mib, 32_768, "default writable disk");
        assert_eq!(spec.template.vcpus, 8, "the topic's ask");
        assert_eq!(spec.template.mem_mib, 32_768, "silent = the default");
        assert_eq!(spec.template.image_digest, pinned_template().image_digest);
        assert_eq!(spec.retain, RetainPolicy::Destroy);
        let runs = orch.runs();
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].0, "vm-0", "inspect on the topic vm");
        assert_eq!(runs[1].0, "vm-1", "evaluate on the experiment vm");
        assert!(matches!(runs[1].1, VmJob::Evaluate { .. }));
        let downs = orch.teardowns();
        assert_eq!(downs.len(), 1);
        assert_eq!(downs[0].0.vm_id, "vm-1");
        assert_eq!(downs[0].1, RetainPolicy::Destroy);
        let alive = orch.vms();
        assert_eq!(alive.len(), 1, "only the topic vm survives");
        assert_eq!(alive[0].vm_id, "vm-0");
        assert_eq!(
            orch.attach(&req.topic_id).await.expect("attach"),
            Some(alive[0].clone()),
            "attach names the topic vm, never an experiment"
        );
        // A second paid job is a second VM: parallel experiments are VMs.
        runner
            .evaluate(&req, &token_for(&req))
            .await
            .expect("second evaluate");
        assert_eq!(orch.created(), 3);
        assert_eq!(orch.teardowns().len(), 2);
        assert_eq!(orch.vms().len(), 1);
        // The policy's own image pin wins over the RLM image for experiments.
        let own = VmBackedRunner::new(orch.clone(), pinned_template()).with_experiments(
            ExperimentPolicy {
                image_digest: Some(format!("sha256:{}", "ff".repeat(32))),
                ..ExperimentPolicy::default()
            },
        );
        own.evaluate(&req, &token_for(&req))
            .await
            .expect("evaluate");
        let last = orch.experiments().pop().expect("spec");
        assert_eq!(
            last.template.image_digest,
            format!("sha256:{}", "ff".repeat(32))
        );
        assert_eq!(own.experiments().ceilings.max_vcpus, 16);
        assert_eq!(own.experiments().ceilings.default_vcpus, 16);
    }

    /// A malformed generic run-policy knob on an experiment topic is
    /// refused **before any experiment VM is created** (no boot, no spend),
    /// and a well-formed one — including the single-task smoke shape —
    /// travels to the guest untouched inside the job's params.
    #[tokio::test]
    async fn a_malformed_run_policy_is_refused_before_any_experiment_vm() {
        use crate::fixtures::experiment_request;
        let orch = FakeOrchestrator::new(0.8);
        let runner = VmBackedRunner::new(orch.clone(), pinned_template());
        let mut typo = experiment_request(None);
        typo.constraints.params.insert(
            proof_experiment::policy::PARAM_AGENT_EXCEPTION_POLICY.into(),
            "zer0".into(),
        );
        let err = runner
            .evaluate(&typo, &token_for(&typo))
            .await
            .expect_err("typo in a signed knob");
        assert!(matches!(err, RunnerError::Backend(_)), "{err}");
        assert!(
            err.to_string().contains("agent_exception_policy"),
            "names the knob: {err}"
        );
        assert!(err.to_string().contains("re-sign the topic"), "{err}");
        assert_eq!(orch.created(), 1, "only the topic vm; no experiment vm");
        assert!(orch.experiments().is_empty(), "nothing booted for the job");

        let mut smoke = experiment_request(None);
        smoke.constraints.params.insert(
            proof_experiment::policy::PARAM_TASKS.into(),
            "one-item".into(),
        );
        smoke.constraints.params.insert(
            proof_experiment::policy::PARAM_AGENT_EXCEPTION_POLICY.into(),
            "zero".into(),
        );
        let out = runner
            .evaluate(&smoke, &token_for(&smoke))
            .await
            .expect("a one-item selection is a topic shape, not a code path");
        assert!((out.report.primary_value - 0.8).abs() < 1e-12);
        assert_eq!(orch.experiments().len(), 1);
        let (_, job) = orch.runs().pop().expect("the paid job ran");
        let carried = job.request().expect("paid").constraints.params.clone();
        assert_eq!(
            carried
                .get(proof_experiment::policy::PARAM_TASKS)
                .map(String::as_str),
            Some("one-item"),
            "the policy reaches the guest as the signed params, verbatim"
        );
        assert!(RunPolicy::from_params(&carried)
            .expect("well-formed")
            .selects_single_item());
    }

    /// An ask over the operator ceiling is refused before any VM exists, a
    /// job that fails inside the experiment VM still stops it — **retained**
    /// on the host so its console and scratch survive for root-cause
    /// analysis, never destroyed — and a topic that selects no runner keeps
    /// the topic-VM path untouched.
    #[tokio::test]
    async fn experiment_vms_fail_closed_and_are_never_left_behind() {
        use crate::fixtures::experiment_request;
        let orch = FakeOrchestrator::new(0.8);
        let runner = VmBackedRunner::new(orch.clone(), pinned_template());
        let greedy = experiment_request(Some(32));
        let err = runner
            .evaluate(&greedy, &token_for(&greedy))
            .await
            .expect_err("over the ceiling");
        assert!(matches!(err, RunnerError::Backend(_)), "{err}");
        assert!(err.to_string().contains("exceeds the ceiling 16"), "{err}");
        assert_eq!(orch.created(), 1, "only the topic vm; no experiment vm");
        assert!(orch.experiments().is_empty());

        let req = experiment_request(None);
        orch.set_fail_run(true);
        let err = runner
            .evaluate(&req, &token_for(&req))
            .await
            .expect_err("guest failure");
        assert!(err.to_string().contains("injected run failure"), "{err}");
        orch.set_fail_run(false);
        assert_eq!(orch.experiments().len(), 1);
        let downs = orch.teardowns();
        assert_eq!(downs.len(), 1, "torn down despite the failure");
        assert_eq!(downs[0].0.vm_id, "vm-1");
        assert_eq!(
            downs[0].1,
            RetainPolicy::Retain,
            "a failed job keeps its jail on the host for root-cause analysis"
        );
        assert_eq!(
            orch.vms().len(),
            1,
            "the topic vm only; a retained vm is stopped"
        );

        let plain = request();
        let out = runner
            .evaluate(&plain, &token_for(&plain))
            .await
            .expect("plain topic");
        assert!((out.report.primary_value - 0.8).abs() < 1e-12);
        assert_eq!(orch.experiments().len(), 1, "no new experiment vm");
        assert_eq!(orch.teardowns().len(), 1);
        let last = orch.runs().pop().expect("run");
        assert_eq!(last.0, "vm-0", "ran on the topic vm");

        // A half-selected binding (runner, no pack digest) is refused, not
        // routed to the ordinary path.
        let mut half = request();
        half.constraints
            .params
            .insert(proof_experiment::PARAM_RUNNER.into(), "some_runner".into());
        let err = runner
            .evaluate(&half, &token_for(&half))
            .await
            .expect_err("no pack digest");
        assert!(
            err.to_string()
                .contains(proof_experiment::PARAM_PACK_DIGEST),
            "{err}"
        );
        assert_eq!(orch.experiments().len(), 1);
        // Non-paid jobs never leave the topic vm, whatever the params say.
        let vm = orch
            .attach(&req.topic_id)
            .await
            .expect("attach")
            .expect("vm");
        let out = run_paid_job(
            orch.as_ref(),
            &ExperimentPolicy::default(),
            &pinned_template(),
            &vm,
            VmJob::Archive {
                topic_id: req.topic_id.clone(),
            },
        )
        .await
        .expect("archive");
        assert_eq!(out, VmJobOutput::Archived);
        assert_eq!(orch.experiments().len(), 1);
        let spec = TopicVmSpec::for_experiment(
            &req.topic_id,
            pinned_template(),
            req.sandbox.clone(),
            ExperimentSpec {
                runner: "Bad Runner".into(),
                pack: proof_experiment::PackRef {
                    path: None,
                    digest: format!("sha256:{}", "ee".repeat(32)),
                },
                disk_mib: 32_768,
            },
        );
        assert!(matches!(spec.validate(), Err(VmError::Experiment(_))));
        let json = serde_json::to_string(&TopicVmSpec::for_topic(
            &req.topic_id,
            pinned_template(),
            req.sandbox.clone(),
        ))
        .expect("json");
        assert!(!json.contains("experiment"), "absent on the wire: {json}");
    }

    /// A paid run is scored only once its experiment VM is **confirmed**
    /// destroyed. `Ok(false)` and a teardown error both withhold a
    /// successful outcome (fail-closed: the VM may still hold capacity), the
    /// error names the VM, a job that failed keeps **its own** error even
    /// when its retain is not confirmed (the leak is logged for the host to
    /// reconcile, never hidden behind a teardown error), and a confirmed
    /// destroy scores again. Nothing here is warn-only.
    #[tokio::test]
    async fn a_paid_run_is_withheld_unless_its_experiment_vm_is_confirmed_destroyed() {
        use crate::fixtures::experiment_request;
        let orch = FakeOrchestrator::new(0.8);
        let runner = VmBackedRunner::new(orch.clone(), pinned_template());
        let req = experiment_request(None);
        runner.inspect(&req, &rules()).await.expect("topic vm");

        orch.set_teardown(Ok(false));
        let err = runner
            .evaluate(&req, &token_for(&req))
            .await
            .expect_err("unconfirmed destroy is not a scored run");
        assert!(matches!(err, RunnerError::Backend(_)), "{err}");
        let msg = err.to_string();
        assert!(msg.contains("not confirmed destroyed"), "{msg}");
        assert!(msg.contains("vm-1"), "names the leaked vm: {msg}");
        assert!(msg.contains("did not confirm the destroy"), "{msg}");
        assert!(msg.contains("withheld"), "{msg}");
        assert_eq!(orch.teardowns().len(), 1, "the destroy was attempted");
        assert_eq!(orch.vms().len(), 2, "the fake kept the vm, as the host did");

        orch.set_teardown(Err("injected transport error"));
        let err = runner
            .evaluate(&req, &token_for(&req))
            .await
            .expect_err("failed destroy is not a scored run");
        let msg = err.to_string();
        assert!(
            msg.contains("teardown failed: topic-vm orchestrator: injected transport error"),
            "{msg}"
        );
        assert!(msg.contains("vm-2"), "{msg}");
        assert!(
            !msg.contains("the job itself failed"),
            "the run itself succeeded: {msg}"
        );

        orch.set_fail_run(true);
        let err = runner
            .evaluate(&req, &token_for(&req))
            .await
            .expect_err("job failed and the vm leaked");
        let msg = err.to_string();
        assert!(
            msg.contains("injected run failure"),
            "the job's own failure is the error: {msg}"
        );
        assert!(
            !msg.contains("injected transport error") && !msg.contains("withheld"),
            "an unconfirmed retain never hides the real failure: {msg}"
        );
        let (leaked, policy) = orch.teardowns().pop().expect("teardown attempted");
        assert_eq!(leaked.vm_id, "vm-3");
        assert_eq!(policy, RetainPolicy::Retain, "a failed job asks to retain");
        orch.set_fail_run(false);

        let direct = run_paid_job(
            orch.as_ref(),
            &ExperimentPolicy::default(),
            &pinned_template(),
            &orch
                .attach(&req.topic_id)
                .await
                .expect("attach")
                .expect("vm"),
            VmJob::Baseline {
                request: req.clone(),
            },
        )
        .await
        .expect_err("baseline is a paid job too");
        assert!(
            matches!(direct, VmError::TeardownUnconfirmed { ref vm_id, .. } if vm_id == "vm-4"),
            "{direct}"
        );
        assert_eq!(
            orch.vms().len(),
            5,
            "four leaked experiment vms + the topic vm"
        );

        orch.set_teardown(Ok(true));
        let run = runner
            .evaluate(&req, &token_for(&req))
            .await
            .expect("confirmed destroy scores");
        assert!((run.report.primary_value - 0.8).abs() < 1e-12);
        assert_eq!(
            orch.vms().len(),
            5,
            "the new vm is gone; the leaked ones are the host's to reconcile"
        );
    }

    /// A run request carries exactly one secret — the miner's own BYOK, and
    /// only when the topic asked for it. It is absent from the wire until
    /// then, its `Debug` never prints a value, and the operator's judge offer
    /// still travels without a key or an origin.
    #[test]
    fn the_only_secret_a_job_carries_is_the_miners_own() {
        let plain = request();
        assert!(plain.miner_env.is_empty());
        let json = serde_json::to_string(&plain).expect("json");
        assert!(
            !json.contains("miner_env"),
            "absent on the wire until a topic asks: {json}"
        );

        let mut env = crate::MinerEnv::new();
        env.insert("MINER_PROVIDED_API_KEY", "miner-supplied-value");
        let byok = request().with_miner_env(env);
        let json = serde_json::to_string(&byok).expect("json");
        assert!(json.contains("MINER_PROVIDED_API_KEY"), "{json}");
        assert!(json.contains("miner-supplied-value"), "{json}");
        for forbidden in ["/run/base", "api_key", "127.0.0.1", "base_url"] {
            assert!(!json.contains(forbidden), "job leaked {forbidden}: {json}");
        }
        let back: CustomRunRequest = serde_json::from_str(&json).expect("round trip");
        assert_eq!(back, byok);
        // Formatting a job never prints the value, however deeply nested.
        let job = VmJob::Evaluate {
            request: byok.clone(),
            checklist_digest: "c".into(),
            rules_version: 1,
        };
        let printed = format!("{job:?}");
        assert!(!printed.contains("miner-supplied-value"), "{printed}");
        assert!(printed.contains("MINER_PROVIDED_API_KEY"), "{printed}");
        assert!(printed.contains("[REDACTED]"), "{printed}");

        // The sister forwarding flag is the topic's, and it is off by default.
        assert!(!byok.miner_env_in_sister());
        let mut forwards = byok;
        forwards.constraints.params.insert(
            crate::PARAM_INJECT_MINER_ENV_SISTER.to_owned(),
            "true".to_owned(),
        );
        assert!(forwards.miner_env_in_sister());
    }

    #[test]
    fn env_names_are_names_only() {
        assert_eq!(VM_ORCHESTRATOR_URL_ENV, "PROOF_VM_ORCHESTRATOR_URL");
        assert_eq!(
            VM_ORCHESTRATOR_TOKEN_FILE_ENV,
            "PROOF_VM_ORCHESTRATOR_TOKEN_FILE"
        );
        assert_eq!(RLM_VM_IMAGE_DIGEST_ENV, "PROOF_RLM_VM_IMAGE_DIGEST");
        assert!(
            VmTemplate::from_env().image_digest.is_empty()
                || VmTemplate::from_env().validate().is_ok()
        );
    }
}
