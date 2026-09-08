//! The backend contract the agent drives.
//!
//! A [`Hypervisor`] boots one RLM VM per topic from a digest-pinned image,
//! runs jobs inside it, boots a **sister** miner guest when a paid job asks
//! for one, and tears the VM down or retains it. The Firecracker + jailer
//! implementation lives in `proof-fc-host`; tests use the fake behind the
//! `test-fixtures` feature. Nothing in this crate spawns a process.

use async_trait::async_trait;
use proof_rlm::{RetainPolicy, TopicVmSpec, VmJob, VmJobOutput};
use proof_vm_proto::SisterAttestation;

/// Why the backend refused or failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HvError {
    /// Cannot boot anything right now (binaries, `/dev/kvm`, kernel pin).
    #[error("hypervisor not ready: {0}")]
    NotReady(String),
    /// The spec asks for something this host refuses.
    #[error("spec: {0}")]
    Spec(String),
    /// The pinned image is absent or its bytes do not hash to the digest.
    #[error("image {0}: not present or does not verify")]
    Image(String),
    /// The guest agent failed or answered badly.
    #[error("guest: {0}")]
    Guest(String),
    /// Process / filesystem / network plumbing failed.
    #[error("hypervisor: {0}")]
    Backend(String),
    /// The job's deadline passed before the guest answered.
    #[error("job deadline of {0}s passed")]
    Deadline(u64),
}

/// One booted topic VM as the backend tracks it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootedVm {
    /// Host VM id (also the jail id).
    pub vm_id: String,
    /// Topic the VM is bound to.
    pub topic_id: String,
    /// `sha256:` digest verified before boot.
    pub image_digest: String,
}

/// What one job produced, with the host's view of any sister run.
#[derive(Debug, Clone, PartialEq)]
pub struct JobOutcome {
    /// The guest's document (paid outputs are re-stamped by the agent).
    pub output: VmJobOutput,
    /// Filled by the **host** iff it booted a sister guest for this job.
    pub sister: Option<SisterAttestation>,
}

/// Boot / run / teardown for topic VMs.
#[async_trait]
pub trait Hypervisor: Send + Sync {
    /// Backend name shown on `/v1/health`.
    fn name(&self) -> &'static str;

    /// Whether a VM could boot right now.
    ///
    /// # Errors
    ///
    /// [`HvError::NotReady`] naming what is missing (never a secret).
    fn ready(&self) -> Result<(), HvError>;

    /// Boot the RLM VM for `spec` under `vm_id`, verify the image digest
    /// first, wait for the guest agent, stage owner key material.
    async fn boot(&self, vm_id: &str, spec: &TopicVmSpec) -> Result<BootedVm, HvError>;

    /// Run one job inside the VM, booting a sister guest if the RLM asks.
    async fn run_job(&self, vm: &BootedVm, job: &VmJob) -> Result<JobOutcome, HvError>;

    /// Stop the VM; keep its scratch under `Retain`. `Ok(true)` only when the
    /// requested end state was reached.
    async fn teardown(&self, vm: &BootedVm, policy: RetainPolicy) -> Result<bool, HvError>;
}
