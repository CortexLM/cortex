//! Experiment-VM layer of the `proof-vm-orchestrator` host.
//!
//! [`ExperimentHypervisor`] wraps the Firecracker backend (or any
//! [`Hypervisor`]) and adds what a **dedicated experiment VM** needs on top
//! of an ordinary topic VM — without knowing what runs inside it:
//!
//! 1. **Ceilings.** A spec with `experiment` set is admitted only under this
//!    host's own copy of the per-VM ceilings ([`ExperimentCeilings`]; lock:
//!    16 vCPU / 32 GiB RAM, writable disk ≥ 16 GiB). Over a ceiling is
//!    `BadSpec`, never a silent clamp.
//! 2. **Pack.** The topic-pinned experiment pack is resolved under the host's
//!    pack directory ([`PackStore`]) from the digest the signed topic carries
//!    (and its optional relative locator), size-capped, and re-hashed with the
//!    artefact check every other side uses
//!    (`proof_vm_proto::tar::verify_artifact`: uncompressed tar with content
//!    whose sha256 is the pin) **before any jail is built**. A missing or
//!    mis-hashed pack is `Image` (503 on the control plane), never a
//!    substitute.
//! 3. **Staging.** Once the guest said hello, the pack travels over vsock
//!    ([`HostToRlm::StagePack`]) and the guest must answer
//!    [`RlmToHost::PackStaged`] naming the same digest; anything else
//!    destroys the VM and fails the boot.
//! 4. **Bind + attestation.** A paid job on an experiment VM must name the
//!    runner and pack the VM was created for, and its output is attested by
//!    the host as a [`GuestMode::ExperimentVm`] run — the VM the host booted
//!    from the pinned image for exactly this job's topic, submission, and
//!    artefact — so the agent's stamp and the control plane's
//!    `bind_evidence` work exactly as for a sister.
//!
//! Nothing here names a benchmark, a runner, or a pack; every value comes
//! from the spec the control plane derived from the signed topic. CI runs
//! this over the fake hypervisor and a recording stager: no Firecracker.

#![forbid(unsafe_code)]
#![allow(clippy::missing_errors_doc, clippy::module_name_repetitions)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use proof_experiment::{ExperimentCeilings, ExperimentSpec, PackRef, VmShape};
use proof_fc_host::vsock::GuestChannel;
use proof_fc_host::HostConfig;
use proof_rlm::{RetainPolicy, TopicVmSpec, VmJob, VmJobOutput};
use proof_vm_agent::{BootedVm, HvError, Hypervisor, JobOutcome};
use proof_vm_proto::guest::{HostToRlm, RlmToHost, StagedFile, MAX_PACK_TAR_BYTES, RLM_JOB_PORT};
use proof_vm_proto::tar::verify_artifact;
use proof_vm_proto::{EvidenceBinding, GuestMode, SisterAttestation};
use tokio::sync::Mutex;

/// Default pack directory on the KVM host (`sha256-<hex>.tar`, or the
/// topic's relative locator, under it).
pub const DEFAULT_PACK_DIR: &str = "/var/lib/proof-vm/packs";
/// How long the guest may take to verify and unpack a staged pack.
pub const DEFAULT_STAGE_TIMEOUT: Duration = Duration::from_mins(10);

/// Operator configuration of the experiment layer. Paths and ceilings only.
#[derive(Debug, Clone)]
pub struct ExperimentHostConfig {
    /// Where pack files live. Never written by the agent.
    pub pack_dir: PathBuf,
    /// This host's per-VM ceilings.
    pub ceilings: ExperimentCeilings,
    /// Budget for the guest's `PackStaged` answer.
    pub stage_timeout: Duration,
}

impl Default for ExperimentHostConfig {
    fn default() -> Self {
        Self {
            pack_dir: PathBuf::from(DEFAULT_PACK_DIR),
            ceilings: ExperimentCeilings::default(),
            stage_timeout: DEFAULT_STAGE_TIMEOUT,
        }
    }
}

impl ExperimentHostConfig {
    /// Ceilings in range.
    pub fn validate(&self) -> Result<(), HvError> {
        self.ceilings
            .validate()
            .map_err(|e| HvError::Spec(format!("experiment ceilings: {e}")))
    }
}

/// Resolves and verifies experiment packs under one directory.
#[derive(Debug, Clone)]
pub struct PackStore {
    dir: PathBuf,
}

impl PackStore {
    /// Store over `dir`.
    #[must_use]
    pub fn new(dir: &Path) -> Self {
        Self {
            dir: dir.to_path_buf(),
        }
    }

    /// The file `pack` names: the relative locator, else `sha256-<hex>.tar`.
    /// Must be a regular file **inside** the pack directory.
    pub fn resolve(&self, pack: &PackRef) -> Result<PathBuf, HvError> {
        pack.validate()
            .map_err(|e| HvError::Spec(format!("experiment pack: {e}")))?;
        let dir = std::fs::canonicalize(&self.dir).map_err(|e| {
            HvError::Image(format!(
                "pack dir {} is not readable on this host: {e}",
                self.dir.display()
            ))
        })?;
        let candidate = dir.join(pack.locator());
        let path = std::fs::canonicalize(&candidate).map_err(|_| {
            HvError::Image(format!(
                "experiment pack {} (no {} on this host)",
                pack.digest,
                candidate.display()
            ))
        })?;
        if !path.starts_with(&dir) || !path.is_file() {
            return Err(HvError::Image(format!(
                "experiment pack {} resolves outside the pack dir or is not a file",
                pack.digest
            )));
        }
        Ok(path)
    }

    /// Read the pack and check it is what the topic pinned: under the size
    /// cap, an uncompressed tar with content, hashing to `pack.digest`.
    pub fn load(&self, pack: &PackRef) -> Result<Vec<u8>, HvError> {
        let path = self.resolve(pack)?;
        let len = std::fs::metadata(&path)
            .map_err(|e| HvError::Backend(format!("stat {}: {e}", path.display())))?
            .len();
        if len == 0 || len > MAX_PACK_TAR_BYTES as u64 {
            return Err(HvError::Image(format!(
                "experiment pack {} is {len} bytes (1..={MAX_PACK_TAR_BYTES}); larger packs need a \
                 block-device staging path this protocol version does not have",
                path.display()
            )));
        }
        let bytes = std::fs::read(&path)
            .map_err(|e| HvError::Backend(format!("read {}: {e}", path.display())))?;
        let hex = pack
            .hex()
            .ok_or_else(|| HvError::Spec(format!("experiment pack digest {:?}", pack.digest)))?;
        verify_artifact(&bytes, hex).map_err(|e| {
            HvError::Image(format!(
                "experiment pack {}: {e} (stage the exact tar the topic pinned; never re-tar)",
                path.display()
            ))
        })?;
        Ok(bytes)
    }
}

/// How a verified pack reaches the guest.
#[async_trait]
pub trait PackStager: Send + Sync {
    /// Hand `tar` (already verified against `digest`) to the VM's guest agent
    /// and wait for it to confirm the same digest.
    async fn stage(&self, vm: &BootedVm, digest: &str, tar: &[u8]) -> Result<(), HvError>;
}

/// Production stager: one vsock connection to the RLM job port of the VM's
/// jail, one `StagePack` frame, one `PackStaged` answer.
pub struct VsockPackStager {
    host: HostConfig,
    stage_timeout: Duration,
}

impl VsockPackStager {
    /// Over the jail layout `host` describes.
    #[must_use]
    pub fn new(host: HostConfig, stage_timeout: Duration) -> Self {
        Self {
            host,
            stage_timeout,
        }
    }
}

#[async_trait]
impl PackStager for VsockPackStager {
    async fn stage(&self, vm: &BootedVm, digest: &str, tar: &[u8]) -> Result<(), HvError> {
        let root = self.host.jail_root(&vm.vm_id);
        let mut ch =
            GuestChannel::connect_within(&root, RLM_JOB_PORT, self.host.boot_timeout).await?;
        ch.send(&HostToRlm::StagePack {
            digest: digest.to_owned(),
            pack_tar: StagedFile::new("pack.tar", tar),
        })
        .await?;
        match ch.recv_within::<RlmToHost>(self.stage_timeout).await? {
            RlmToHost::PackStaged { digest: got, bytes }
                if got.trim().eq_ignore_ascii_case(digest.trim()) && bytes == tar.len() as u64 =>
            {
                tracing::info!(vm_id = %vm.vm_id, pack = %digest, bytes, "experiment pack staged");
                Ok(())
            }
            RlmToHost::PackStaged { digest: got, bytes } => Err(HvError::Guest(format!(
                "guest staged {got} ({bytes} bytes), the host sent {digest} ({} bytes)",
                tar.len()
            ))),
            RlmToHost::Failed { error } => Err(HvError::Guest(format!("pack staging: {error}"))),
            other => Err(HvError::Guest(format!("pack staging answered {other:?}"))),
        }
    }
}

#[derive(Debug, Clone)]
struct LiveExperiment {
    spec: ExperimentSpec,
}

/// The experiment-aware [`Hypervisor`]: `inner` for everything, plus the
/// ceilings, pack, staging, bind, and attestation for experiment VMs.
pub struct ExperimentHypervisor {
    inner: Arc<dyn Hypervisor>,
    cfg: ExperimentHostConfig,
    packs: PackStore,
    stager: Box<dyn PackStager>,
    live: Mutex<HashMap<String, LiveExperiment>>,
}

impl ExperimentHypervisor {
    /// Wrap `inner`.
    pub fn new(
        inner: Arc<dyn Hypervisor>,
        cfg: ExperimentHostConfig,
        stager: Box<dyn PackStager>,
    ) -> Result<Self, HvError> {
        cfg.validate()?;
        Ok(Self {
            packs: PackStore::new(&cfg.pack_dir),
            inner,
            cfg,
            stager,
            live: Mutex::new(HashMap::new()),
        })
    }

    /// The wrapped backend.
    pub fn inner(&self) -> &Arc<dyn Hypervisor> {
        &self.inner
    }

    /// The layer's config.
    #[must_use]
    pub fn config(&self) -> &ExperimentHostConfig {
        &self.cfg
    }

    /// Experiment VMs this layer knows as booted.
    pub async fn live_experiments(&self) -> usize {
        self.live.lock().await.len()
    }

    fn admit(&self, spec: &TopicVmSpec, exp: &ExperimentSpec) -> Result<(), HvError> {
        exp.validate()
            .map_err(|e| HvError::Spec(format!("experiment: {e}")))?;
        self.cfg
            .ceilings
            .admit(VmShape {
                vcpus: spec.template.vcpus,
                mem_mib: spec.template.mem_mib,
                disk_mib: exp.disk_mib,
            })
            .map_err(|e| HvError::Spec(e.to_string()))
    }

    /// A paid job on an experiment VM must be for the runner and pack the VM
    /// was created with; a job whose params select nothing is refused too.
    fn bind_job(live: &LiveExperiment, job: &VmJob) -> Result<(), HvError> {
        let request = match job {
            VmJob::Baseline { request } | VmJob::Evaluate { request, .. } => request,
            VmJob::ProposeRules { .. } | VmJob::Inspect { .. } | VmJob::Archive { .. } => {
                return Ok(())
            }
        };
        let binding = request
            .experiment()
            .map_err(|e| HvError::Spec(format!("job experiment binding: {e}")))?
            .ok_or_else(|| {
                HvError::Spec("job selects no in-guest runner, vm is an experiment vm".into())
            })?;
        if binding.runner != live.spec.runner || binding.pack.digest != live.spec.pack.digest {
            return Err(HvError::Spec(format!(
                "job names runner {:?} / pack {}, the vm was created for {:?} / {}",
                binding.runner, binding.pack.digest, live.spec.runner, live.spec.pack.digest
            )));
        }
        Ok(())
    }
}

fn reported_flops(output: &VmJobOutput) -> Option<u64> {
    match output {
        VmJobOutput::Baseline(r) => r.flops_used,
        VmJobOutput::Evaluated(run) => run.report.flops_used,
        VmJobOutput::Rules(_) | VmJobOutput::Inspected(_) | VmJobOutput::Archived => None,
    }
}

#[async_trait]
impl Hypervisor for ExperimentHypervisor {
    fn name(&self) -> &'static str {
        self.inner.name()
    }

    fn ready(&self) -> Result<(), HvError> {
        self.inner.ready()
    }

    async fn boot(&self, vm_id: &str, spec: &TopicVmSpec) -> Result<BootedVm, HvError> {
        let Some(exp) = &spec.experiment else {
            return self.inner.boot(vm_id, spec).await;
        };
        self.admit(spec, exp)?;
        // Resolve + verify the pack before any jail exists: a missing or
        // mis-hashed pack costs nothing on the host.
        let tar = self.packs.load(&exp.pack)?;
        let booted = self.inner.boot(vm_id, spec).await?;
        if let Err(e) = self.stager.stage(&booted, &exp.pack.digest, &tar).await {
            tracing::warn!(%vm_id, "experiment pack staging failed; destroying the vm: {e}");
            if let Err(down) = self.inner.teardown(&booted, RetainPolicy::Destroy).await {
                tracing::error!(%vm_id, "experiment vm not released after a failed staging: {down}");
            }
            return Err(e);
        }
        self.live
            .lock()
            .await
            .insert(vm_id.to_owned(), LiveExperiment { spec: exp.clone() });
        Ok(booted)
    }

    async fn alive(&self, vm: &BootedVm) -> bool {
        self.inner.alive(vm).await
    }

    async fn run_job(&self, vm: &BootedVm, job: &VmJob) -> Result<JobOutcome, HvError> {
        let live = self.live.lock().await.get(&vm.vm_id).cloned();
        let Some(live) = live else {
            return self.inner.run_job(vm, job).await;
        };
        Self::bind_job(&live, job)?;
        let started = Instant::now();
        let outcome = self.inner.run_job(vm, job).await?;
        let Some(binding) = EvidenceBinding::of_job(job) else {
            return Ok(outcome);
        };
        if outcome.sister.is_some() {
            // The guest asked for a sister after all; the host's sister
            // attestation is the stronger evidence and stands.
            return Ok(outcome);
        }
        let flops_used = reported_flops(&outcome.output);
        let attestation = SisterAttestation {
            mode: GuestMode::ExperimentVm,
            sister_vm_id: vm.vm_id.clone(),
            image_digest: vm.image_digest.clone(),
            topic_id: binding.topic_id,
            submission_digest: binding.submission_digest,
            artifact_digest: binding.artifact_digest,
            sandboxed: true,
            network: GuestMode::ExperimentVm.network().into(),
            flops_used,
            wall_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            exit_code: Some(0),
        };
        tracing::info!(
            vm_id = %vm.vm_id, runner = %live.spec.runner, pack = %live.spec.pack.digest,
            flops_used = ?flops_used, wall_ms = attestation.wall_ms, "experiment vm run attested"
        );
        Ok(JobOutcome {
            output: outcome.output,
            sister: Some(attestation),
        })
    }

    async fn teardown(&self, vm: &BootedVm, policy: RetainPolicy) -> Result<bool, HvError> {
        let confirmed = self.inner.teardown(vm, policy).await?;
        if confirmed {
            self.live.lock().await.remove(&vm.vm_id);
        }
        Ok(confirmed)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex as StdMutex;

    use super::*;
    use proof_rlm::fixtures::{experiment_request, pinned_template, request, token_for};
    use proof_rlm::VmTemplate;
    use proof_vm_agent::fixtures::FakeHypervisor;
    use proof_vm_proto::tar::fixtures::{archive, member};
    use sha2::{Digest, Sha256};

    struct RecordingStager {
        staged: StdMutex<Vec<(String, String, usize)>>,
        fail: bool,
    }

    #[async_trait]
    impl PackStager for RecordingStager {
        async fn stage(&self, vm: &BootedVm, digest: &str, tar: &[u8]) -> Result<(), HvError> {
            if self.fail {
                return Err(HvError::Guest("guest refused the pack".into()));
            }
            self.staged
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((vm.vm_id.clone(), digest.to_owned(), tar.len()));
            Ok(())
        }
    }

    fn pack_dir(tag: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("proof-fc-experiment-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("dir");
        d
    }

    /// A real pack: an uncompressed tar with one file of content, written
    /// under its digest name. Returns the `sha256:` pin.
    fn stage_pack(dir: &Path, payload: &[u8]) -> (String, Vec<u8>) {
        let tar = archive(&[member("pack/task.toml", b'0', payload)]);
        let hex = hex::encode(Sha256::digest(&tar));
        std::fs::write(dir.join(format!("sha256-{hex}.tar")), &tar).expect("pack");
        (format!("sha256:{hex}"), tar)
    }

    fn spec_for(digest: &str, vcpus: u32, disk_mib: u32) -> TopicVmSpec {
        let req = request();
        TopicVmSpec::for_experiment(
            &req.topic_id,
            VmTemplate {
                vcpus,
                mem_mib: 16_384,
                ..pinned_template()
            },
            req.sandbox,
            ExperimentSpec {
                runner: "placeholder_in_guest_runner".into(),
                pack: PackRef {
                    path: None,
                    digest: digest.to_owned(),
                },
                disk_mib,
            },
        )
    }

    fn layer(dir: &Path, hv: Arc<FakeHypervisor>, fail_stage: bool) -> ExperimentHypervisor {
        ExperimentHypervisor::new(
            hv as Arc<dyn Hypervisor>,
            ExperimentHostConfig {
                pack_dir: dir.to_path_buf(),
                ..ExperimentHostConfig::default()
            },
            Box::new(RecordingStager {
                staged: StdMutex::new(Vec::new()),
                fail: fail_stage,
            }),
        )
        .expect("config")
    }

    /// The pack is resolved by digest (or relative locator) inside the pack
    /// dir only, and must be the exact tar the topic pinned.
    #[test]
    fn packs_resolve_inside_the_dir_and_must_hash_to_the_pin() {
        let dir = pack_dir("store");
        let (digest, tar) = stage_pack(&dir, b"content");
        let store = PackStore::new(&dir);
        let by_digest = PackRef {
            path: None,
            digest: digest.clone(),
        };
        assert_eq!(store.load(&by_digest).expect("load"), tar);
        std::fs::create_dir_all(dir.join("slices")).expect("subdir");
        std::fs::write(dir.join("slices/first.tar"), &tar).expect("copy");
        let by_path = PackRef {
            path: Some("slices/first.tar".into()),
            digest: digest.clone(),
        };
        assert_eq!(store.load(&by_path).expect("by path"), tar);
        let missing = PackRef {
            path: None,
            digest: format!("sha256:{}", "77".repeat(32)),
        };
        let err = store.load(&missing).expect_err("absent");
        assert!(matches!(err, HvError::Image(_)), "{err}");
        assert!(err.to_string().contains("no "), "{err}");
        // Right bytes under the wrong pin: refused by the re-hash.
        let mislabeled = PackRef {
            path: Some("slices/first.tar".into()),
            digest: format!("sha256:{}", "77".repeat(32)),
        };
        let err = store.load(&mislabeled).expect_err("mismatch");
        assert!(err.to_string().contains("hashes to"), "{err}");
        // A pack that is not an artefact (gzip / empty) is refused by name.
        let mut gz = tar.clone();
        gz[0] = 0x1f;
        gz[1] = 0x8b;
        let gz_hex = hex::encode(Sha256::digest(&gz));
        std::fs::write(dir.join(format!("sha256-{gz_hex}.tar")), &gz).expect("gz");
        let err = store
            .load(&PackRef {
                path: None,
                digest: format!("sha256:{gz_hex}"),
            })
            .expect_err("gzip");
        assert!(err.to_string().contains("gzip"), "{err}");
        std::fs::write(dir.join("empty.tar"), b"").expect("empty");
        let err = store
            .load(&PackRef {
                path: Some("empty.tar".into()),
                digest: digest.clone(),
            })
            .expect_err("empty");
        assert!(err.to_string().contains("0 bytes"), "{err}");
        // Escaping the dir is refused even with a matching file outside.
        let outside = dir.parent().expect("parent").join("outside-pack.tar");
        std::fs::write(&outside, &tar).expect("outside");
        let escape = PackRef {
            path: Some("../outside-pack.tar".into()),
            digest: digest.clone(),
        };
        assert!(matches!(store.load(&escape), Err(HvError::Spec(_))));
        let _ = std::fs::remove_file(outside);
        assert!(PackStore::new(&dir.join("nonexistent"))
            .load(&by_digest)
            .is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An experiment boot: ceilings first, then the pack (before any boot),
    /// then the boot, then staging; a paid job on the VM is bound to its
    /// runner + pack and attested `experiment_vm`; teardown forgets it.
    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn an_experiment_vm_is_admitted_staged_bound_and_attested() {
        let dir = pack_dir("flow");
        let (digest, tar) = stage_pack(&dir, b"task content");
        let hv = FakeHypervisor::new(0.9);
        hv.set_rlm_flops(Some(5));
        let layer = layer(&dir, hv.clone(), false);
        assert_eq!(layer.name(), "fake");
        layer.ready().expect("ready");

        let over = spec_for(&digest, 32, 32_768);
        let err = layer
            .boot("topic-a-x0001", &over)
            .await
            .expect_err("over the ceiling");
        assert!(matches!(err, HvError::Spec(_)), "{err}");
        assert!(err.to_string().contains("exceeds the ceiling 16"), "{err}");
        assert!(hv.boots().is_empty(), "refused before any boot");

        let absent = spec_for(&format!("sha256:{}", "77".repeat(32)), 4, 32_768);
        let err = layer
            .boot("topic-a-x0001", &absent)
            .await
            .expect_err("no pack");
        assert!(matches!(err, HvError::Image(_)), "{err}");
        assert!(hv.boots().is_empty(), "no pack, no boot");

        let spec = spec_for(&digest, 4, 32_768);
        let vm = layer.boot("topic-a-x0001", &spec).await.expect("boot");
        assert_eq!(hv.boots().len(), 1);
        assert_eq!(layer.live_experiments().await, 1);
        assert!(layer.alive(&vm).await);

        // A job for another pack / runner never reaches the guest.
        let mut other = experiment_request(None);
        other.constraints.params.insert(
            proof_experiment::PARAM_PACK_DIGEST.into(),
            format!("sha256:{}", "77".repeat(32)),
        );
        let err = layer
            .run_job(
                &vm,
                &VmJob::Evaluate {
                    request: other,
                    checklist_digest: "c".into(),
                    rules_version: 1,
                },
            )
            .await
            .expect_err("wrong pack");
        assert!(matches!(err, HvError::Spec(_)), "{err}");
        assert!(err.to_string().contains("the vm was created for"), "{err}");
        let plain = request();
        let err = layer
            .run_job(&vm, &VmJob::Baseline { request: plain })
            .await
            .expect_err("no runner selected");
        assert!(
            err.to_string().contains("selects no in-guest runner"),
            "{err}"
        );
        assert!(hv.jobs().is_empty(), "nothing dispatched");

        let mut req = experiment_request(None);
        req.constraints
            .params
            .insert(proof_experiment::PARAM_PACK_DIGEST.into(), digest.clone());
        let job = VmJob::Evaluate {
            request: req.clone(),
            checklist_digest: token_for(&req).checklist_digest().to_owned(),
            rules_version: 1,
        };
        let out = layer.run_job(&vm, &job).await.expect("run");
        let att = out.sister.expect("attested");
        assert_eq!(att.mode, GuestMode::ExperimentVm);
        assert_eq!(att.sister_vm_id, "topic-a-x0001");
        assert_eq!(att.image_digest, pinned_template().image_digest);
        assert_eq!(att.network, "egress-allowlist");
        assert_eq!(att.flops_used, Some(5), "the guest's measurement, relayed");
        assert!(att.sandboxed);
        proof_vm_proto::bind_evidence(&job, &out.output, Some(&att)).expect("bound to the job");
        // The fake attests experiment VMs itself when asked; here the layer
        // wrote the attestation because the fake did not know the spec.
        let archived = layer
            .run_job(
                &vm,
                &VmJob::Archive {
                    topic_id: req.topic_id.clone(),
                },
            )
            .await
            .expect("archive");
        assert!(archived.sister.is_none(), "unpaid jobs are not attested");
        assert!(layer
            .teardown(&vm, RetainPolicy::Destroy)
            .await
            .expect("teardown"));
        assert_eq!(layer.live_experiments().await, 0);
        assert_eq!(hv.teardowns().len(), 1);

        // A topic VM (no experiment) passes straight through.
        let topic_spec = TopicVmSpec::for_topic("topic-a", pinned_template(), request().sandbox);
        let tvm = layer
            .boot("topic-a-0001", &topic_spec)
            .await
            .expect("topic vm");
        assert_eq!(layer.live_experiments().await, 0);
        let out = layer
            .run_job(&tvm, &VmJob::Baseline { request: request() })
            .await
            .expect("baseline on the topic vm");
        assert_eq!(
            out.sister.as_ref().map(|s| s.mode),
            Some(GuestMode::Sister),
            "the fake booted a sister for the topic vm"
        );
        assert_eq!(tar.len(), out.sister.map_or(tar.len(), |_| tar.len()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Staging that fails destroys the VM the layer just booted and fails
    /// the boot: no experiment VM without its pack.
    #[tokio::test]
    async fn a_failed_staging_destroys_the_vm_and_fails_the_boot() {
        let dir = pack_dir("stagefail");
        let (digest, _) = stage_pack(&dir, b"task content");
        let hv = FakeHypervisor::new(0.9);
        let layer = layer(&dir, hv.clone(), true);
        let err = layer
            .boot("topic-a-x0001", &spec_for(&digest, 4, 32_768))
            .await
            .expect_err("staging refused");
        assert!(matches!(err, HvError::Guest(_)), "{err}");
        assert_eq!(hv.boots().len(), 1, "the vm booted");
        assert_eq!(
            hv.teardowns(),
            vec![("topic-a-x0001".to_owned(), RetainPolicy::Destroy)],
            "and was destroyed"
        );
        assert_eq!(layer.live_experiments().await, 0);
        let mut bad = ExperimentHostConfig::default();
        bad.ceilings.max_vcpus = 0;
        assert!(ExperimentHypervisor::new(
            hv as Arc<dyn Hypervisor>,
            bad,
            Box::new(RecordingStager {
                staged: StdMutex::new(Vec::new()),
                fail: false,
            })
        )
        .is_err());
        assert_eq!(DEFAULT_PACK_DIR, "/var/lib/proof-vm/packs");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The production stager over Firecracker's vsock UDS: the CONNECT
    /// handshake, one `StagePack` frame, and a `PackStaged` answer that must
    /// echo the digest — a fake guest, no Firecracker.
    #[tokio::test]
    async fn the_vsock_stager_sends_one_frame_and_requires_the_echo() {
        use proof_vm_proto::guest::{read_frame, write_frame};
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        use tokio::net::UnixListener;

        let base = pack_dir("vsock");
        let mut host = HostConfig::defaults();
        host.chroot_base = base.join("jailer");
        host.kernel_digest = format!("sha256:{}", "aa".repeat(32));
        host.sister_image_digest = format!("sha256:{}", "bb".repeat(32));
        host.boot_timeout = Duration::from_secs(5);
        let root = host.jail_root("topic-a-x0007");
        std::fs::create_dir_all(&root).expect("root");
        let vm = BootedVm {
            vm_id: "topic-a-x0007".into(),
            topic_id: "topic-a".into(),
            image_digest: format!("sha256:{}", "cc".repeat(32)),
        };
        let tar = archive(&[member("pack/task.toml", b'0', b"x")]);
        let digest = format!("sha256:{}", hex::encode(Sha256::digest(&tar)));

        let serve = |listener: UnixListener, echo: Option<String>, want: Vec<u8>| async move {
            let (stream, _) = listener.accept().await.expect("connection");
            let mut stream = BufReader::new(stream);
            let mut line = String::new();
            stream.read_line(&mut line).await.expect("connect");
            assert_eq!(line, format!("CONNECT {RLM_JOB_PORT}\n"));
            stream
                .get_mut()
                .write_all(b"OK 1073741824\n")
                .await
                .expect("ok");
            let msg: HostToRlm = read_frame(&mut stream).await.expect("frame");
            let HostToRlm::StagePack { digest, pack_tar } = msg else {
                panic!("expected a stage_pack frame");
            };
            assert_eq!(pack_tar.name, "pack.tar");
            assert_eq!(pack_tar.bytes().expect("b64"), want);
            let answer = match echo {
                Some(d) => RlmToHost::PackStaged {
                    digest: d,
                    bytes: want.len() as u64,
                },
                None => RlmToHost::Failed {
                    error: format!("refusing {digest}: unpack failed"),
                },
            };
            write_frame(stream.get_mut(), &answer)
                .await
                .expect("answer");
        };

        let bind = || {
            let path = proof_fc_host::vsock::uds_path(&root);
            let _ = std::fs::remove_file(&path);
            UnixListener::bind(path).expect("bind")
        };
        let guest = tokio::spawn(serve(bind(), Some(digest.clone()), tar.clone()));
        let stager = VsockPackStager::new(host.clone(), Duration::from_secs(5));
        stager.stage(&vm, &digest, &tar).await.expect("staged");
        guest.await.expect("guest");

        let guest = tokio::spawn(serve(
            bind(),
            Some(format!("sha256:{}", "00".repeat(32))),
            tar.clone(),
        ));
        let err = stager
            .stage(&vm, &digest, &tar)
            .await
            .expect_err("other digest");
        assert!(err.to_string().contains("the host sent"), "{err}");
        guest.await.expect("guest");

        let guest = tokio::spawn(serve(bind(), None, tar.clone()));
        let err = stager
            .stage(&vm, &digest, &tar)
            .await
            .expect_err("guest failed");
        assert!(err.to_string().contains("unpack failed"), "{err}");
        guest.await.expect("guest");
        let _ = std::fs::remove_dir_all(&base);
    }
}
