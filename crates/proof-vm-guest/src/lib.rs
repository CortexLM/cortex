//! Guest side of the Proof topic-VM protocol, run **inside** the RLM /
//! experiment microVM (`proof-vm-guest-agent`, vsock port 5000).
//!
//! The KVM host (`proof-vm-orchestrator`) speaks `proof_vm_proto::guest` to
//! this agent: `Hello` binds the VM to its topic, `StageSecrets` hands over
//! the owner key material (kept under a tmpfs directory, never echoed),
//! `StagePack` hands an experiment VM the topic-pinned pack (verified with
//! `proof_vm_proto::tar::verify_artifact` before it is unpacked on the
//! writable disk), `StageArtifact` injects a miner upload (`proof-artefact://`)
//! so the guest must not HTTP-fetch that scheme, and `Run` carries one job.
//! The agent is generic: it knows
//! the protocol, the filesystem layout, and how to **exec an operator
//! adaptor** — never a benchmark, a dataset, a model, or a result.
//!
//! # Runner adaptors (operator capability, baked into the image)
//!
//! A job's request carries the signed topic's `constraints.params`. When they
//! select an in-guest runner (`proof_experiment::PARAM_RUNNER`), the agent
//! resolves `<runners_dir>/<runner id>/` and execs the entrypoint for the job
//! kind — `run` (baseline / evaluate), `inspect`, `propose_rules` — with the
//! environment contract in [`runner`]. The adaptor writes its answer under
//! `PROOF_OUTPUT_DIR` (`report.json`, `checklist.json`, `rules.json`); the
//! agent turns it into the protocol document with the identities copied
//! from the request. **Nothing is defaulted:** no runner selected, no adaptor
//! installed under that id, no staged pack matching the topic's digest, no
//! artefact that verifies, no report, a non-finite value, or a run cut at the
//! deadline is `RlmToHost::Failed` — the host answers 502, the control plane
//! 503, and no row is written. A placeholder `primary_value` never leaves
//! this process.
//!
//! One deliberate default exists: `ProposeRules` without a `propose_rules`
//! entrypoint answers with the signed topic's own `checklist` (v1 of the
//! rule set is that vector by contract), so a topic whose adaptor writes no
//! rules still scores under the rules its operator signed.
//!
//! Secrets are files the adaptor reads (`PROOF_SECRETS_DIR`); their bytes
//! are redacted from every log tail and evidence document the agent sends
//! back, and never appear in an environment variable the agent sets.

#![forbid(unsafe_code)]
#![allow(clippy::missing_errors_doc, clippy::module_name_repetitions)]

pub mod fetch;
pub mod runner;
pub mod staging;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use proof_rlm::{CustomRunRequest, VmJob, VmJobOutput};
use proof_vm_proto::guest::{check_version, read_frame, write_frame, HostToRlm, RlmToHost};
use proof_vm_proto::{ProtoError, API_VERSION};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::Mutex;

pub use runner::{JobKind, RunnerReport};
pub use staging::{StagedArtifact, StagedPack};

/// Agent name reported on `Ready`.
pub const AGENT_NAME: &str = concat!("proof-vm-guest-agent/", env!("CARGO_PKG_VERSION"));
/// Bounded retries when `write_frame(Done/Failed)` hits a broken vsock after
/// the job has already finished (host harvest still recovers without this).
pub const TERMINAL_WRITE_ATTEMPTS: u32 = 4;
/// Backoff between terminal-frame retries.
pub const TERMINAL_WRITE_BACKOFF: Duration = Duration::from_millis(50);

/// Where the guest keeps what the host stages and what runs produce.
#[derive(Debug, Clone)]
pub struct GuestConfig {
    /// Owner key material (tmpfs; mode 0700). Never on the writable disk.
    pub secrets_dir: PathBuf,
    /// Unpacked experiment packs, one directory per digest (writable disk).
    pub pack_root: PathBuf,
    /// Per-job work directories (artefact tree, output, logs).
    pub work_root: PathBuf,
    /// Operator adaptors: `<runners_dir>/<runner id>/{run,inspect,propose_rules}`.
    pub runners_dir: PathBuf,
    /// uid / gid adaptors run as (`None` = the agent's own; rootless
    /// container runtimes want an unprivileged user with subuid ranges).
    pub run_as: Option<(u32, u32)>,
    /// Where an artefact fetch may take its bytes from: `https://` always;
    /// plain `http://` only when this is `true` (staging artefact hosts).
    pub allow_plain_http: bool,
}

impl GuestConfig {
    /// The layout the baked image uses.
    #[must_use]
    pub fn defaults() -> Self {
        Self {
            secrets_dir: PathBuf::from("/run/proof/secrets"),
            pack_root: PathBuf::from("/var/lib/proof/packs"),
            work_root: PathBuf::from("/var/lib/proof/work"),
            runners_dir: PathBuf::from("/opt/proof/runners"),
            run_as: None,
            allow_plain_http: false,
        }
    }
}

/// What the host told this VM it is.
#[derive(Debug, Clone, Default)]
struct Binding {
    topic_id: String,
    vm_id: String,
}

/// The agent: one per VM, shared by every connection the host opens.
pub struct GuestAgent {
    cfg: GuestConfig,
    binding: Mutex<Option<Binding>>,
    pack: Mutex<Option<StagedPack>>,
    /// Miner artefact injected for the next job (`proof-artefact://`).
    artifact: Mutex<Option<StagedArtifact>>,
    /// One job at a time (the host enforces the same).
    job: Mutex<()>,
    seq: std::sync::atomic::AtomicU64,
}

impl GuestAgent {
    /// Agent over `cfg`. Directories are created lazily as messages arrive.
    #[must_use]
    pub fn new(cfg: GuestConfig) -> Arc<Self> {
        Arc::new(Self {
            cfg,
            binding: Mutex::new(None),
            pack: Mutex::new(None),
            artifact: Mutex::new(None),
            job: Mutex::new(()),
            seq: std::sync::atomic::AtomicU64::new(1),
        })
    }

    /// The config in force.
    #[must_use]
    pub fn config(&self) -> &GuestConfig {
        &self.cfg
    }

    /// Serve one host connection: frames in, frames out, until the peer
    /// closes. The host opens a fresh connection per message group (hello +
    /// staging at boot, then one per job), so state lives on the agent.
    pub async fn serve_connection<S: AsyncRead + AsyncWrite + Unpin>(
        &self,
        mut stream: S,
    ) -> Result<(), ProtoError> {
        loop {
            let msg: HostToRlm = match read_frame(&mut stream).await {
                Ok(m) => m,
                Err(ProtoError::Io(_)) => return Ok(()),
                Err(e) => return Err(e),
            };
            let terminal = matches!(&msg, HostToRlm::Run { .. });
            let answer = self.handle(msg).await;
            if let Err(e) = write_frame_retry(&mut stream, &answer).await {
                if terminal && broken_pipe(&e) {
                    tracing::warn!(
                        "terminal frame not delivered ({e}); host harvest recovers the report"
                    );
                    return Ok(());
                }
                return Err(e);
            }
        }
    }

    /// Answer one host message. Never panics, never leaks a secret.
    pub async fn handle(&self, msg: HostToRlm) -> RlmToHost {
        match msg {
            HostToRlm::Hello {
                api_version,
                topic_id,
                vm_id,
            } => {
                if let Err(e) = check_version(api_version) {
                    return RlmToHost::Failed {
                        error: e.to_string(),
                    };
                }
                tracing::info!(%topic_id, %vm_id, "bound to topic by the host");
                *self.binding.lock().await = Some(Binding { topic_id, vm_id });
                RlmToHost::Ready {
                    agent: AGENT_NAME.into(),
                    api_version: API_VERSION,
                }
            }
            HostToRlm::StageSecrets { files } => {
                match staging::stage_secrets(&self.cfg.secrets_dir, &files, self.cfg.run_as) {
                    Ok(count) => {
                        tracing::info!(count, "owner key material staged (contents not logged)");
                        RlmToHost::Staged { count }
                    }
                    Err(e) => RlmToHost::Failed {
                        error: format!("stage secrets: {e}"),
                    },
                }
            }
            HostToRlm::StagePack { digest, pack_tar } => {
                match staging::stage_pack(&self.cfg.pack_root, &digest, &pack_tar) {
                    Ok(pack) => {
                        tracing::info!(digest = %pack.digest, bytes = pack.bytes, "experiment pack staged");
                        let answer = RlmToHost::PackStaged {
                            digest: pack.digest.clone(),
                            bytes: pack.bytes,
                        };
                        *self.pack.lock().await = Some(pack);
                        answer
                    }
                    Err(e) => RlmToHost::Failed {
                        error: format!("stage pack: {e}"),
                    },
                }
            }
            HostToRlm::StageArtifact {
                digest,
                artifact_tar,
            } => match staging::stage_artifact(&digest, &artifact_tar) {
                Ok(art) => {
                    tracing::info!(digest = %art.digest, bytes = art.bytes.len(), "miner artefact staged");
                    let answer = RlmToHost::ArtifactStaged {
                        digest: art.digest.clone(),
                        bytes: art.bytes.len() as u64,
                    };
                    *self.artifact.lock().await = Some(art);
                    answer
                }
                Err(e) => RlmToHost::Failed {
                    error: format!("stage artefact: {e}"),
                },
            },
            HostToRlm::Run { job } => {
                let _one = self.job.lock().await;
                match self.run(*job).await {
                    // Never persist work_root here: a stale sibling from an
                    // earlier job (or Archive, which has no tree) must not
                    // turn this result into Failed. Each job seals its own
                    // directory in `run` / `run_paid`.
                    Ok(output) => RlmToHost::Done { output },
                    Err(error) => {
                        tracing::warn!("job failed: {error}");
                        RlmToHost::Failed { error }
                    }
                }
            }
        }
    }

    /// A fresh work directory for one job.
    fn work_dir(&self, kind: JobKind) -> PathBuf {
        let n = self.seq.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.cfg.work_root.join(format!("{n:04}-{}", kind.as_str()))
    }

    async fn check_topic(&self, topic_id: &str) -> Result<(), String> {
        match self.binding.lock().await.as_ref() {
            Some(b) if b.topic_id == topic_id => Ok(()),
            Some(b) => Err(format!(
                "job names topic {topic_id:?}, vm {} is bound to {:?}",
                b.vm_id, b.topic_id
            )),
            None => Err("no hello from the host yet; refusing a job".into()),
        }
    }

    async fn run(&self, job: VmJob) -> Result<VmJobOutput, String> {
        self.check_topic(job.topic_id()).await?;
        // One inject per job: the host stages then Runs on this connection.
        let injected = self.artifact.lock().await.take();
        match job {
            VmJob::Baseline { request } => {
                let report = self
                    .paid(&request, JobKind::Baseline, injected.as_ref())
                    .await?;
                Ok(VmJobOutput::Baseline(report.report))
            }
            VmJob::Evaluate { request, .. } => {
                let run = self
                    .paid(&request, JobKind::Evaluate, injected.as_ref())
                    .await?;
                Ok(VmJobOutput::Evaluated(run))
            }
            VmJob::Inspect { request, rules } => {
                let work = self.work_dir(JobKind::Inspect);
                let result = async {
                    let adaptor = runner::Adaptor::resolve(&self.cfg.runners_dir, &request)?;
                    let artifact = fetch::fetch_artifact(
                        &request,
                        &work,
                        self.cfg.allow_plain_http,
                        injected.as_ref(),
                    )
                    .await?;
                    let out = runner::inspect(
                        &self.cfg,
                        &adaptor,
                        &request,
                        &rules,
                        &work,
                        artifact.as_deref(),
                    )
                    .await?;
                    Ok(VmJobOutput::Inspected(out))
                }
                .await;
                seal_job(&work, result)
            }
            VmJob::ProposeRules {
                topic,
                current_version,
            } => {
                let work = self.work_dir(JobKind::ProposeRules);
                let result = runner::propose_rules(&self.cfg, &topic, current_version, &work).await;
                seal_job(&work, result.map(VmJobOutput::Rules))
            }
            VmJob::Archive { .. } => {
                // No work directory and no completion sync. The host
                // retains or destroys the writable disk with the VM.
                Ok(VmJobOutput::Archived)
            }
        }
    }

    /// Baseline / evaluate: resolve the adaptor, check the staged pack is the
    /// topic's, fetch + verify the artefact (evaluate), exec `run`.
    async fn paid(
        &self,
        request: &CustomRunRequest,
        kind: JobKind,
        injected: Option<&StagedArtifact>,
    ) -> Result<proof_rlm::RunOutcome, String> {
        let adaptor = runner::Adaptor::resolve(&self.cfg.runners_dir, request)?;
        let pack = self.pack.lock().await.clone();
        let pack = staging::pack_for(pack.as_ref(), &adaptor.binding.pack)?;
        let work = self.work_dir(kind);
        let result = async {
            let artifact = match kind {
                JobKind::Evaluate => {
                    let dir =
                        fetch::fetch_artifact(request, &work, self.cfg.allow_plain_http, injected)
                            .await?
                            .ok_or_else(|| {
                                "evaluate needs the miner's artifact_uri; the request carries none"
                                    .to_owned()
                            })?;
                    Some(dir)
                }
                // The baseline is the topic's own reference run: the pack is its
                // input, an artefact only if the topic serves one.
                JobKind::Baseline | JobKind::Inspect | JobKind::ProposeRules => {
                    fetch::fetch_artifact(request, &work, self.cfg.allow_plain_http, injected)
                        .await?
                }
            };
            runner::run_paid(
                &self.cfg,
                &adaptor,
                request,
                kind,
                &pack,
                &work,
                artifact.as_deref(),
            )
            .await
        }
        .await;
        // run_paid already seals `work` on the exec path. Fetch/resolve
        // failures still need this job dir on the virtio-blk — never
        // work_root (stale siblings).
        if result.is_err() {
            let _ = runner::persist_work(&work);
        }
        result
    }
}

/// Flush one job tree. Success is fail-closed (no Done if sync fails).
/// Failure is best-effort so the job error stays the job error.
fn seal_job<T>(work: &Path, result: Result<T, String>) -> Result<T, String> {
    match result {
        Ok(v) => runner::persist_work(work).map(|()| v),
        Err(e) => {
            let _ = runner::persist_work(work);
            Err(e)
        }
    }
}

fn broken_pipe(err: &ProtoError) -> bool {
    match err {
        ProtoError::Io(s) => {
            let s = s.to_ascii_lowercase();
            s.contains("broken pipe")
                || s.contains("brokenpipe")
                || s.contains("connection reset")
                || s.contains("connection abort")
                || s.contains("not connected")
                || s.contains("os error 32")
                || s.contains("os error 104")
                || s.contains("unexpected eof")
                || s.contains("early eof")
        }
        ProtoError::FrameTooLarge(_) | ProtoError::Decode(_) | ProtoError::WrongVersion { .. } => {
            false
        }
    }
}

async fn write_frame_retry<W: AsyncWrite + Unpin>(
    w: &mut W,
    value: &RlmToHost,
) -> Result<(), ProtoError> {
    let mut last = None;
    for i in 0..TERMINAL_WRITE_ATTEMPTS {
        match write_frame(w, value).await {
            Ok(()) => return Ok(()),
            Err(e) if broken_pipe(&e) => {
                tracing::warn!(
                    "terminal frame write failed ({e}); retry {}/{TERMINAL_WRITE_ATTEMPTS}",
                    i + 1
                );
                last = Some(e);
                tokio::time::sleep(TERMINAL_WRITE_BACKOFF.saturating_mul(i + 1)).await;
            }
            Err(e) => return Err(e),
        }
    }
    Err(last.unwrap_or_else(|| ProtoError::Io("terminal frame not delivered".into())))
}

/// Where the agent resolves relative layout paths from (tests point this at
/// a temp dir).
#[must_use]
pub fn under(root: &Path, cfg: &GuestConfig) -> GuestConfig {
    GuestConfig {
        secrets_dir: root.join("secrets"),
        pack_root: root.join("packs"),
        work_root: root.join("work"),
        runners_dir: root.join("runners"),
        ..cfg.clone()
    }
}

#[cfg(test)]
#[path = "agent_tests.rs"]
mod agent_tests;
