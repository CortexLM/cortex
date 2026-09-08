//! Wire protocol of the Proof topic-VM orchestrator.
//!
//! Two boundaries share these types:
//!
//! 1. **Control plane ↔ agent** (HTTPS, bearer): `FirecrackerOrchestrator`
//!    in `proof-vm-fc` calls the `proof-vm-orchestrator` agent on a dedicated
//!    KVM host. Requests carry a [`TopicVmSpec`] or a [`VmJob`] — public topic
//!    data, digests, rule versions — never a host path, a key, or an origin.
//!    Every VM is bound to exactly one `topic_id`; every job and teardown
//!    names that topic again and the agent refuses a mismatch.
//! 2. **Agent ↔ guests** (Firecracker vsock, length-prefixed JSON,
//!    [`guest`]): the agent hands the RLM guest its jobs, stages owner key
//!    material the control plane never reads, and boots a **sister** miner
//!    guest (no network, no host filesystem) when the RLM asks for a run.
//!    The host — not the RLM — stamps [`SisterAttestation`] and the report's
//!    `sandboxed` / `flops_used`.
//!
//! Nothing here names a benchmark, a model, or a repository.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

use proof_rlm::{RetainPolicy, SandboxPolicy, TopicVmSpec, VmHandle, VmJob, VmJobOutput};
use serde::{Deserialize, Serialize};

pub mod guest;

/// Only accepted `api_version` on both boundaries.
pub const API_VERSION: u32 = 1;

/// Port the agent listens on by default (HTTPS on the KVM host).
pub const DEFAULT_AGENT_PORT: u16 = 8200;

/// Agent HTTP paths.
pub mod paths {
    /// `GET` readiness (no auth beyond the bearer).
    pub const HEALTH: &str = "/v1/health";
    /// `POST` create a topic VM.
    pub const VMS: &str = "/v1/vms";

    /// `DELETE` teardown / retain one VM.
    #[must_use]
    pub fn vm(vm_id: &str) -> String {
        format!("/v1/vms/{vm_id}")
    }

    /// `POST` run one job inside the VM.
    #[must_use]
    pub fn vm_jobs(vm_id: &str) -> String {
        format!("/v1/vms/{vm_id}/jobs")
    }

    /// `GET` the VM bound to a topic (404 = none).
    #[must_use]
    pub fn vm_by_topic(topic_id: &str) -> String {
        format!("/v1/vms/by-topic/{topic_id}")
    }
}

/// `GET /v1/health`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentHealth {
    /// Equals [`API_VERSION`].
    pub api_version: u32,
    /// Whether the hypervisor could boot a VM right now.
    pub ready: bool,
    /// Why not (empty when ready). Never a secret.
    pub reason: String,
    /// Backend name (`firecracker`, or `fake` in tests).
    pub hypervisor: String,
    /// VMs currently bound (running or retained).
    pub vms: usize,
}

/// `POST /v1/vms` body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateVmRequest {
    /// What to boot, for which topic.
    pub spec: TopicVmSpec,
}

/// Where a VM is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VmState {
    /// Booted and taking jobs.
    Running,
    /// Torn down under [`RetainPolicy::Retain`]: process gone, scratch kept for audit.
    Retained,
    /// Torn down under [`RetainPolicy::Destroy`]: nothing left on the host.
    Destroyed,
}

/// One topic VM as the agent knows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VmRecord {
    /// Topic ↔ VM binding.
    pub handle: VmHandle,
    /// `sha256:` digest of the RLM image the host verified before boot.
    pub image_digest: String,
    /// vCPUs booted.
    pub vcpus: u32,
    /// Guest memory booted.
    pub mem_mib: u32,
    /// Sandbox policy the VM was created under.
    pub sandbox: SandboxPolicy,
    /// What teardown does by default.
    pub retain: RetainPolicy,
    /// Lifecycle state.
    pub state: VmState,
}

/// `POST /v1/vms/{vm_id}/jobs` body.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunJobRequest {
    /// Must equal the VM's bound topic **and** the job's own topic.
    pub topic_id: String,
    /// The work (public data only).
    pub job: VmJob,
}

/// What the host attests about the sister miner guest a job used.
///
/// Written by the agent from what it booted and observed, never copied from
/// the RLM guest. The control plane cross-checks it against the report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SisterAttestation {
    /// Host id of the sister VM (destroyed after the run).
    pub sister_vm_id: String,
    /// `sha256:` digest of the miner-guest image the host verified and booted.
    pub image_digest: String,
    /// The host booted the sister and the run happened inside it.
    pub sandboxed: bool,
    /// Network the sister had. Always `none`: the artefact travels over vsock.
    pub network: String,
    /// FLOPs the guest measured for the run (host-relayed, never RLM-authored).
    pub flops_used: Option<u64>,
    /// Wall-clock of the sister run.
    pub wall_ms: u64,
    /// Exit code of the run inside the guest (`None` = killed at the deadline).
    pub exit_code: Option<i32>,
}

/// `POST /v1/vms/{vm_id}/jobs` response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunJobResponse {
    /// Echo of the bound topic.
    pub topic_id: String,
    /// Echo of the VM.
    pub vm_id: String,
    /// The job's output, host-stamped for paid runs.
    pub output: VmJobOutput,
    /// Present iff the host booted a sister guest for this job.
    pub sister: Option<SisterAttestation>,
}

/// `DELETE /v1/vms/{vm_id}` body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeardownRequest {
    /// Must equal the VM's bound topic.
    pub topic_id: String,
    /// Destroy or retain.
    pub policy: RetainPolicy,
}

/// `DELETE /v1/vms/{vm_id}` response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeardownResponse {
    /// Echo of the bound topic.
    pub topic_id: String,
    /// Echo of the VM.
    pub vm_id: String,
    /// End state.
    pub state: VmState,
    /// `true` only when the host reached the requested end state.
    pub confirmed: bool,
}

/// Machine-readable error class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// Missing or wrong bearer.
    Unauthorized,
    /// Hypervisor cannot boot right now (binaries, `/dev/kvm`, image).
    NotReady,
    /// Spec failed validation on the host.
    BadSpec,
    /// Request topic ≠ VM's bound topic (or ≠ the job's topic).
    TopicMismatch,
    /// The topic already has a VM.
    AlreadyExists,
    /// No such VM.
    NotFound,
    /// The VM is running another job.
    Busy,
    /// The hypervisor or guest failed.
    Backend,
    /// The guest answered with the wrong output shape.
    WrongOutput,
}

impl ErrorCode {
    /// HTTP status the agent answers with.
    #[must_use]
    pub fn status(self) -> u16 {
        match self {
            Self::Unauthorized => 401,
            Self::NotReady => 503,
            Self::BadSpec => 400,
            Self::TopicMismatch | Self::AlreadyExists | Self::Busy => 409,
            Self::NotFound => 404,
            Self::Backend | Self::WrongOutput => 502,
        }
    }
}

/// Error body every non-2xx answer carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorBody {
    /// Class.
    pub code: ErrorCode,
    /// Human detail. Never a secret, never a host path the CP could act on.
    pub error: String,
}

/// Why a wire document is not usable.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProtoError {
    /// A frame was longer than [`guest::MAX_FRAME_BYTES`].
    #[error("frame of {0} bytes exceeds the {max} byte cap", max = guest::MAX_FRAME_BYTES)]
    FrameTooLarge(u32),
    /// JSON did not parse.
    #[error("decode: {0}")]
    Decode(String),
    /// I/O on the channel.
    #[error("channel: {0}")]
    Io(String),
    /// The peer speaks another `api_version`.
    #[error("peer api_version {got}, this build speaks {API_VERSION}")]
    WrongVersion {
        /// What the peer said.
        got: u32,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use proof_rlm::fixtures::{pinned_template, request};

    #[test]
    fn paths_are_stable_and_codes_map_to_statuses() {
        assert_eq!(paths::HEALTH, "/v1/health");
        assert_eq!(paths::VMS, "/v1/vms");
        assert_eq!(paths::vm("vm-1"), "/v1/vms/vm-1");
        assert_eq!(paths::vm_jobs("vm-1"), "/v1/vms/vm-1/jobs");
        assert_eq!(paths::vm_by_topic("topic-a"), "/v1/vms/by-topic/topic-a");
        assert_eq!(ErrorCode::Unauthorized.status(), 401);
        assert_eq!(ErrorCode::NotReady.status(), 503);
        assert_eq!(ErrorCode::BadSpec.status(), 400);
        assert_eq!(ErrorCode::TopicMismatch.status(), 409);
        assert_eq!(ErrorCode::NotFound.status(), 404);
        assert_eq!(ErrorCode::Backend.status(), 502);
        assert_eq!(DEFAULT_AGENT_PORT, 8200);
        assert_eq!(API_VERSION, 1);
    }

    #[test]
    fn documents_round_trip_and_carry_public_data_only() {
        let req = request();
        let spec = TopicVmSpec::for_topic(&req.topic_id, pinned_template(), req.sandbox.clone());
        let create = CreateVmRequest { spec };
        let json = serde_json::to_string(&create).expect("json");
        let back: CreateVmRequest = serde_json::from_str(&json).expect("round trip");
        assert_eq!(back, create);
        let run = RunJobRequest {
            topic_id: req.topic_id.clone(),
            job: VmJob::Evaluate {
                request: req.clone(),
                checklist_digest: "c".into(),
                rules_version: 1,
            },
        };
        let json = serde_json::to_string(&run).expect("json");
        for forbidden in ["/run/base", "/opt/base", "api_key", "127.0.0.1", "base_url"] {
            assert!(!json.contains(forbidden), "leaked {forbidden}: {json}");
        }
        let back: RunJobRequest = serde_json::from_str(&json).expect("round trip");
        assert_eq!(back.job.topic_id(), req.topic_id);
        let err = ErrorBody {
            code: ErrorCode::TopicMismatch,
            error: "x".into(),
        };
        let json = serde_json::to_string(&err).expect("json");
        assert!(json.contains("topic_mismatch"), "{json}");
        let resp = TeardownResponse {
            topic_id: "t".into(),
            vm_id: "v".into(),
            state: VmState::Retained,
            confirmed: true,
        };
        assert!(serde_json::to_string(&resp)
            .expect("json")
            .contains("\"retained\""));
    }
}
