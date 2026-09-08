//! `FirecrackerOrchestrator` — the live [`TopicVmOrchestrator`].
//!
//! The control plane never touches Firecracker. It talks HTTPS to the
//! `proof-vm-orchestrator` agent on a **dedicated KVM host**, which boots one
//! jailed Firecracker RLM VM per topic from the digest the control plane
//! pins (`PROOF_RLM_VM_IMAGE_DIGEST`) and runs every miner artefact in a
//! **sister** Firecracker guest. Jobs cross the wire as [`VmJob`]s — public
//! topic data, digests, rule versions — never a host path, a key, or an
//! origin. Owner key material is staged by the agent from the host's own
//! files; this crate only ever sends the bearer it reads from
//! `PROOF_VM_ORCHESTRATOR_TOKEN_FILE`, and never logs it.
//!
//! Fail-closed, in this order:
//!
//! - `PROOF_VM_ORCHESTRATOR_URL` unset → [`FirecrackerOrchestrator::from_env`]
//!   is `None` and the host keeps `UnwiredVmOrchestrator` (503).
//! - URL set but not `https://` (plain `http://` is accepted on loopback
//!   only, for tests) → configuration error at boot, nothing wired.
//! - Token file missing / empty, RLM image digest unpinned → `ready()` is
//!   [`VmError::NotWired`] naming the env var → 503 with the root cause.
//! - Agent unreachable, bearer refused, hypervisor not ready →
//!   [`VmError::Backend`] → 503. There is no host-local execution path.
//!
//! Hard binds the client enforces on top of the agent's: a job must name
//! the handle's topic before any request leaves; the agent must echo the
//! same topic and VM; a created VM must report the digest that was pinned;
//! and a `firecracker_required` run must come back with the host's sister
//! attestation (`sandboxed: true`) or the output is not evidence.

#![forbid(unsafe_code)]
#![allow(clippy::missing_errors_doc, clippy::module_name_repetitions)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use proof_rlm::{
    RetainPolicy, TopicVmOrchestrator, TopicVmSpec, VmError, VmHandle, VmJob, VmJobOutput,
    VmTemplate, RLM_VM_IMAGE_DIGEST_ENV, VM_ORCHESTRATOR_TOKEN_FILE_ENV, VM_ORCHESTRATOR_URL_ENV,
};
use proof_vm_proto::{
    paths, AgentHealth, CreateVmRequest, ErrorBody, RunJobRequest, RunJobResponse, TeardownRequest,
    TeardownResponse, VmRecord, API_VERSION,
};
use reqwest::{Method, StatusCode};
use serde::de::DeserializeOwned;
use serde::Serialize;
use url::Url;

/// Optional extra PEM root the agent's certificate chains to (private CA).
pub const VM_ORCHESTRATOR_CA_FILE_ENV: &str = "PROOF_VM_ORCHESTRATOR_CA_FILE";
/// RLM VM vCPUs (default [`DEFAULT_RLM_VCPUS`]).
pub const RLM_VM_VCPUS_ENV: &str = "PROOF_RLM_VM_VCPUS";
/// RLM VM memory in MiB (default [`DEFAULT_RLM_MEM_MIB`]).
pub const RLM_VM_MEM_MIB_ENV: &str = "PROOF_RLM_VM_MEM_MIB";
/// Comma-separated custom ids the generic `VmBackedRunner` serves over this
/// orchestrator. Registration is an operator action; unset = nothing registered.
pub const VM_RUNNER_CUSTOM_IDS_ENV: &str = "PROOF_VM_RUNNER_CUSTOM_IDS";

/// Locked RLM VM shape: 4 vCPU.
pub const DEFAULT_RLM_VCPUS: u32 = 4;
/// Locked RLM VM shape: 8192 MiB.
pub const DEFAULT_RLM_MEM_MIB: u32 = 8_192;
/// Wall-clock for a `create` (image verify + boot + guest hello + staging).
pub const DEFAULT_CREATE_TIMEOUT_S: u64 = 600;
/// Wall-clock for a job that carries no deadline (`ProposeRules`, `Archive`).
pub const DEFAULT_JOB_TIMEOUT_S: u64 = 3_600;
/// Added to a job's own deadline before the client gives up on the agent.
pub const JOB_GRACE_S: u64 = 60;

/// Why the orchestrator could not be configured.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FcConfigError {
    /// Not a URL.
    #[error("{VM_ORCHESTRATOR_URL_ENV} is not a URL: {0}")]
    BadUrl(String),
    /// Plain `http://` to a non-loopback host.
    #[error("{VM_ORCHESTRATOR_URL_ENV} must be https:// (plain http only on loopback): {0}")]
    Insecure(String),
    /// URL set, token file env missing.
    #[error("{VM_ORCHESTRATOR_TOKEN_FILE_ENV} is not set (the bearer is a file, never a value)")]
    NoTokenFile,
    /// A sizing knob did not parse or is out of range.
    #[error("{0} is not a number in range: {1:?}")]
    BadNumber(&'static str, String),
    /// The CA file is unreadable or not PEM.
    #[error("{VM_ORCHESTRATOR_CA_FILE_ENV}: {0}")]
    BadCa(String),
    /// The HTTP client could not be built.
    #[error("http client: {0}")]
    Client(String),
}

/// Operator configuration. Holds paths, never secrets.
#[derive(Debug, Clone)]
pub struct FcConfig {
    /// Agent base URL (`https://kvm-host:8200`).
    pub url: Url,
    /// Bearer file, re-read per request.
    pub token_file: PathBuf,
    /// RLM VM image pin + size.
    pub template: VmTemplate,
    /// Extra PEM root, if the agent uses a private CA.
    pub ca_file: Option<PathBuf>,
    /// TCP + TLS connect budget.
    pub connect_timeout: Duration,
    /// `create` budget.
    pub create_timeout: Duration,
    /// Budget for jobs with no deadline of their own.
    pub default_job_timeout: Duration,
}

fn env_trimmed(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

fn env_u32(name: &'static str, default: u32) -> Result<u32, FcConfigError> {
    match env_trimmed(name) {
        None => Ok(default),
        Some(raw) => raw
            .parse::<u32>()
            .map_err(|_| FcConfigError::BadNumber(name, raw)),
    }
}

fn is_loopback(url: &Url) -> bool {
    matches!(
        url.host(),
        Some(
            url::Host::Domain("localhost")
                | url::Host::Ipv4(std::net::Ipv4Addr::LOCALHOST)
                | url::Host::Ipv6(std::net::Ipv6Addr::LOCALHOST)
        )
    )
}

impl FcConfig {
    /// Config for `url` + `token_file` with the locked RLM shape.
    #[must_use]
    pub fn new(url: Url, token_file: &Path, image_digest: &str) -> Self {
        Self {
            url,
            token_file: token_file.to_path_buf(),
            template: VmTemplate {
                image_digest: image_digest.trim().to_owned(),
                vcpus: DEFAULT_RLM_VCPUS,
                mem_mib: DEFAULT_RLM_MEM_MIB,
            },
            ca_file: None,
            connect_timeout: Duration::from_secs(10),
            create_timeout: Duration::from_secs(DEFAULT_CREATE_TIMEOUT_S),
            default_job_timeout: Duration::from_secs(DEFAULT_JOB_TIMEOUT_S),
        }
    }

    /// Read the operator env. `Ok(None)` when the URL is unset (unwired).
    pub fn from_env() -> Result<Option<Self>, FcConfigError> {
        let Some(raw) = env_trimmed(VM_ORCHESTRATOR_URL_ENV) else {
            return Ok(None);
        };
        let url = Url::parse(&raw).map_err(|e| FcConfigError::BadUrl(format!("{raw}: {e}")))?;
        let token_file =
            env_trimmed(VM_ORCHESTRATOR_TOKEN_FILE_ENV).ok_or(FcConfigError::NoTokenFile)?;
        let digest = env_trimmed(RLM_VM_IMAGE_DIGEST_ENV).unwrap_or_default();
        let mut cfg = Self::new(url, Path::new(&token_file), &digest);
        cfg.template.vcpus = env_u32(RLM_VM_VCPUS_ENV, DEFAULT_RLM_VCPUS)?;
        cfg.template.mem_mib = env_u32(RLM_VM_MEM_MIB_ENV, DEFAULT_RLM_MEM_MIB)?;
        cfg.ca_file = env_trimmed(VM_ORCHESTRATOR_CA_FILE_ENV).map(PathBuf::from);
        cfg.validate()?;
        Ok(Some(cfg))
    }

    /// `https://`, or `http://` on loopback only. Sizes are checked by the
    /// template at `ready()` so an unpinned digest is a 503, not a boot error.
    pub fn validate(&self) -> Result<(), FcConfigError> {
        match self.url.scheme() {
            "https" => Ok(()),
            "http" if is_loopback(&self.url) => Ok(()),
            _ => Err(FcConfigError::Insecure(self.url.to_string())),
        }
    }
}

/// Live orchestrator client.
pub struct FirecrackerOrchestrator {
    config: FcConfig,
    http: reqwest::Client,
}

impl std::fmt::Debug for FirecrackerOrchestrator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FirecrackerOrchestrator")
            .field("url", &self.config.url.as_str())
            .field("token_file", &self.config.token_file)
            .field("template", &self.config.template)
            .finish_non_exhaustive()
    }
}

fn backend(msg: impl Into<String>) -> VmError {
    VmError::Backend(msg.into())
}

impl FirecrackerOrchestrator {
    /// Build the client. Reads the CA file (if any) now; the token per request.
    pub fn new(config: FcConfig) -> Result<Self, FcConfigError> {
        config.validate()?;
        let mut builder = reqwest::Client::builder()
            .connect_timeout(config.connect_timeout)
            .user_agent(format!("proof-vm-fc/{API_VERSION}"));
        if let Some(ca) = &config.ca_file {
            let pem = std::fs::read(ca)
                .map_err(|e| FcConfigError::BadCa(format!("{}: {e}", ca.display())))?;
            let cert = reqwest::Certificate::from_pem(&pem)
                .map_err(|e| FcConfigError::BadCa(format!("{}: {e}", ca.display())))?;
            builder = builder.add_root_certificate(cert);
        }
        let http = builder
            .build()
            .map_err(|e| FcConfigError::Client(e.to_string()))?;
        Ok(Self { config, http })
    }

    /// From the operator env. `Ok(None)` = unwired (keep `UnwiredVmOrchestrator`).
    pub fn from_env() -> Result<Option<Self>, FcConfigError> {
        FcConfig::from_env()?.map(Self::new).transpose()
    }

    /// The RLM VM template this client asks the agent to boot.
    #[must_use]
    pub fn template(&self) -> &VmTemplate {
        &self.config.template
    }

    /// Agent base URL.
    #[must_use]
    pub fn url(&self) -> &Url {
        &self.config.url
    }

    /// The bearer, read fresh. Never logged.
    fn token(&self) -> Result<String, VmError> {
        std::fs::read_to_string(&self.config.token_file)
            .ok()
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                VmError::NotWired(format!(
                    "{VM_ORCHESTRATOR_TOKEN_FILE_ENV} ({}) missing or empty",
                    self.config.token_file.display()
                ))
            })
    }

    fn endpoint(&self, path: &str) -> Result<Url, VmError> {
        self.config
            .url
            .join(path.trim_start_matches('/'))
            .map_err(|e| backend(format!("orchestrator url: {e}")))
    }

    /// One authenticated call. `Ok(None)` for 404 (callers decide if that is
    /// an answer); every other non-2xx is a [`VmError::Backend`] carrying the
    /// agent's error code — never the bearer.
    async fn call<B: Serialize, T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<&B>,
        timeout: Duration,
    ) -> Result<Option<T>, VmError> {
        let token = self.token()?;
        let mut req = self
            .http
            .request(method.clone(), self.endpoint(path)?)
            .bearer_auth(token)
            .timeout(timeout);
        if let Some(b) = body {
            req = req.json(b);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| backend(format!("orchestrator unreachable ({method} {path}): {e}")))?;
        let status = resp.status();
        if status == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
            return Err(backend(format!(
                "orchestrator refused the bearer ({status}); rotate {VM_ORCHESTRATOR_TOKEN_FILE_ENV} on both sides"
            )));
        }
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| backend(format!("orchestrator body ({method} {path}): {e}")))?;
        if !status.is_success() {
            let detail = serde_json::from_slice::<ErrorBody>(&bytes).map_or_else(
                |_| {
                    let text = String::from_utf8_lossy(&bytes);
                    text.chars().take(240).collect::<String>()
                },
                |e| format!("{:?}: {}", e.code, e.error),
            );
            return Err(backend(format!(
                "orchestrator {status} on {method} {path}: {detail}"
            )));
        }
        serde_json::from_slice::<T>(&bytes).map(Some).map_err(|e| {
            backend(format!(
                "orchestrator answer ({method} {path}) did not parse: {e}"
            ))
        })
    }

    /// `GET /v1/health`.
    pub async fn health(&self) -> Result<AgentHealth, VmError> {
        self.call::<(), AgentHealth>(
            Method::GET,
            paths::HEALTH,
            None,
            self.config.connect_timeout,
        )
        .await?
        .ok_or_else(|| backend("orchestrator has no health route"))
    }

    fn job_timeout(&self, job: &VmJob) -> Duration {
        job.deadline_s()
            .map_or(self.config.default_job_timeout, |d| {
                Duration::from_secs(d.saturating_add(JOB_GRACE_S))
            })
    }
}

fn check_echo(handle: &VmHandle, topic_id: &str, vm_id: &str) -> Result<(), VmError> {
    if handle.topic_id == topic_id && handle.vm_id == vm_id {
        Ok(())
    } else {
        Err(backend(format!(
            "orchestrator answered for {topic_id}/{vm_id}, asked about {}/{}",
            handle.topic_id, handle.vm_id
        )))
    }
}

#[async_trait]
impl TopicVmOrchestrator for FirecrackerOrchestrator {
    fn ready(&self) -> Result<(), VmError> {
        self.token()?;
        self.config
            .template
            .validate()
            .map_err(|e| VmError::NotWired(format!("{RLM_VM_IMAGE_DIGEST_ENV}: {e}")))
    }

    async fn create(&self, spec: &TopicVmSpec) -> Result<VmHandle, VmError> {
        self.ready()?;
        spec.validate()?;
        let record: VmRecord = self
            .call(
                Method::POST,
                paths::VMS,
                Some(&CreateVmRequest { spec: spec.clone() }),
                self.config.create_timeout,
            )
            .await?
            .ok_or_else(|| backend("orchestrator has no create route"))?;
        if record.handle.topic_id != spec.topic_id {
            return Err(backend(format!(
                "orchestrator bound the vm to {:?}, asked for {:?}",
                record.handle.topic_id, spec.topic_id
            )));
        }
        if !record
            .image_digest
            .eq_ignore_ascii_case(&spec.template.image_digest)
        {
            return Err(backend(format!(
                "orchestrator booted image {} instead of the pinned {}",
                record.image_digest, spec.template.image_digest
            )));
        }
        tracing::info!(topic_id = %spec.topic_id, vm_id = %record.handle.vm_id, "topic vm created");
        Ok(record.handle)
    }

    async fn attach(&self, topic_id: &str) -> Result<Option<VmHandle>, VmError> {
        self.ready()?;
        let found: Option<VmRecord> = self
            .call::<(), VmRecord>(
                Method::GET,
                &paths::vm_by_topic(topic_id.trim()),
                None,
                self.config.connect_timeout,
            )
            .await?;
        match found {
            Some(r) if r.handle.topic_id == topic_id.trim() => Ok(Some(r.handle)),
            Some(r) => Err(backend(format!(
                "orchestrator returned vm {} of topic {:?} for topic {topic_id:?}",
                r.handle.vm_id, r.handle.topic_id
            ))),
            None => Ok(None),
        }
    }

    async fn run(&self, handle: &VmHandle, job: VmJob) -> Result<VmJobOutput, VmError> {
        self.ready()?;
        if job.topic_id() != handle.topic_id {
            return Err(VmError::Spec("topic_id"));
        }
        let needs_sister = job.requires_firecracker();
        let timeout = self.job_timeout(&job);
        let resp: RunJobResponse = self
            .call(
                Method::POST,
                &paths::vm_jobs(&handle.vm_id),
                Some(&RunJobRequest {
                    topic_id: handle.topic_id.clone(),
                    job,
                }),
                timeout,
            )
            .await?
            .ok_or_else(|| backend(format!("orchestrator knows no vm {}", handle.vm_id)))?;
        check_echo(handle, &resp.topic_id, &resp.vm_id)?;
        let attested = resp.sister.as_ref().is_some_and(|s| s.sandboxed);
        if needs_sister && !attested {
            return Err(backend(
                "firecracker_required run came back without the host's sister-guest attestation",
            ));
        }
        let claims_sandbox = match &resp.output {
            VmJobOutput::Baseline(r) => r.sandboxed,
            VmJobOutput::Evaluated(run) => run.report.sandboxed,
            VmJobOutput::Rules(_) | VmJobOutput::Inspected(_) | VmJobOutput::Archived => false,
        };
        if claims_sandbox && !attested {
            return Err(backend(
                "report claims a sandbox the orchestrator did not attest",
            ));
        }
        Ok(resp.output)
    }

    async fn teardown(&self, handle: &VmHandle, policy: RetainPolicy) -> Result<bool, VmError> {
        self.ready()?;
        let resp: TeardownResponse = self
            .call(
                Method::DELETE,
                &paths::vm(&handle.vm_id),
                Some(&TeardownRequest {
                    topic_id: handle.topic_id.clone(),
                    policy,
                }),
                self.config.create_timeout,
            )
            .await?
            .ok_or_else(|| backend(format!("orchestrator knows no vm {}", handle.vm_id)))?;
        check_echo(handle, &resp.topic_id, &resp.vm_id)?;
        tracing::info!(
            topic_id = %handle.topic_id, vm_id = %handle.vm_id, ?policy,
            state = ?resp.state, confirmed = resp.confirmed, "topic vm teardown"
        );
        Ok(resp.confirmed)
    }
}

/// Parse [`VM_RUNNER_CUSTOM_IDS_ENV`]-style lists: comma-separated, trimmed,
/// empty entries dropped, order kept, duplicates removed.
#[must_use]
pub fn parse_custom_ids(raw: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for id in raw.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        if !out.iter().any(|o| o == id) {
            out.push(id.to_owned());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scheme_rule_is_https_or_loopback_http() {
        let tok = Path::new("/nonexistent/token");
        let ok = |u: &str| FcConfig::new(Url::parse(u).expect("url"), tok, "").validate();
        ok("https://kvm.example.invalid:8200").expect("https anywhere");
        ok("http://127.0.0.1:8200").expect("loopback http");
        ok("http://localhost:8200").expect("localhost http");
        ok("http://[::1]:8200").expect("v6 loopback http");
        assert!(matches!(
            ok("http://10.0.0.5:8200"),
            Err(FcConfigError::Insecure(_))
        ));
        assert!(matches!(
            ok("http://kvm.example.invalid:8200"),
            Err(FcConfigError::Insecure(_))
        ));
        assert!(FirecrackerOrchestrator::new(FcConfig::new(
            Url::parse("http://10.0.0.5:8200").expect("url"),
            tok,
            ""
        ))
        .is_err());
    }

    #[test]
    fn defaults_are_the_locked_rlm_shape_and_ids_parse() {
        let cfg = FcConfig::new(
            Url::parse("https://kvm.example.invalid").expect("url"),
            Path::new("/x"),
            "  sha256:abc ",
        );
        assert_eq!(cfg.template.vcpus, 4);
        assert_eq!(cfg.template.mem_mib, 8_192);
        assert_eq!(cfg.template.image_digest, "sha256:abc");
        assert_eq!(
            parse_custom_ids(" a_metric, b-metric ,,a_metric, "),
            vec!["a_metric".to_owned(), "b-metric".to_owned()]
        );
        assert!(parse_custom_ids("").is_empty());
        assert_eq!(VM_RUNNER_CUSTOM_IDS_ENV, "PROOF_VM_RUNNER_CUSTOM_IDS");
        assert_eq!(VM_ORCHESTRATOR_CA_FILE_ENV, "PROOF_VM_ORCHESTRATOR_CA_FILE");
        assert_eq!(RLM_VM_VCPUS_ENV, "PROOF_RLM_VM_VCPUS");
        assert_eq!(RLM_VM_MEM_MIB_ENV, "PROOF_RLM_VM_MEM_MIB");
    }
}
