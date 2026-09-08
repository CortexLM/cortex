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
//! The only orchestrator shipped here is [`UnwiredVmOrchestrator`]: it
//! refuses every call and names the env vars a live one would read. There is
//! no host-local execution path in this crate — a missing orchestrator is a
//! 503, not a fallback.

use std::sync::Arc;

use async_trait::async_trait;
use proof_canon::is_slug;
use proof_task::{ChecklistRule, TopicDocument};
use serde::{Deserialize, Serialize};

use crate::gate::SpendToken;
use crate::rules::RuleSet;
use crate::runner::{
    CustomRunReport, CustomRunRequest, CustomRunner, InspectOutcome, RunOutcome, RunnerError,
    SandboxPolicy,
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

/// What the orchestrator is asked to create for one topic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopicVmSpec {
    /// Topic the VM is attributed to (one topic ↔ its own VM).
    pub topic_id: String,
    /// Image + sizes.
    pub template: VmTemplate,
    /// Sandbox policy for miner code inside the VM.
    pub sandbox: SandboxPolicy,
    /// What to do with the VM when the topic closes.
    pub retain: RetainPolicy,
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
        }
    }

    /// Slug topic id + valid template.
    ///
    /// # Errors
    ///
    /// [`VmError::Spec`].
    pub fn validate(&self) -> Result<(), VmError> {
        if !is_slug(&self.topic_id) {
            return Err(VmError::Spec("topic_id"));
        }
        self.template.validate()
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

/// What a job produced.
#[derive(Debug, Clone, PartialEq)]
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
        other => RunnerError::Backend(other.to_string()),
    }
}

/// The generic runner: every inspect / evaluate is a job inside the topic's
/// VM. Registering it under a `custom_id` is an operator action; nothing
/// registers it by default.
pub struct VmBackedRunner {
    orchestrator: Arc<dyn TopicVmOrchestrator>,
    template: VmTemplate,
}

impl VmBackedRunner {
    /// Runner over `orchestrator` booting `template` for topics without a VM.
    #[must_use]
    pub fn new(orchestrator: Arc<dyn TopicVmOrchestrator>, template: VmTemplate) -> Self {
        Self {
            orchestrator,
            template,
        }
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
        match self.orchestrator.run(&vm, job).await.map_err(map_vm)? {
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
        let runner = VmBackedRunner::new(orch, pinned_template());
        let req = request();
        let err = runner
            .evaluate(&req, &token_for(&req))
            .await
            .expect_err("unsandboxed");
        assert!(matches!(
            err,
            RunnerError::Report(crate::runner::ReportError::NotSandboxed)
        ));
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
