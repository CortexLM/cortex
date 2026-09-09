//! Firecracker + jailer backend of the `proof-vm-orchestrator` agent.
//!
//! One **dedicated KVM host** runs this. For every topic the control plane
//! asks about, the host:
//!
//! 1. resolves the pinned RLM image (`<image_dir>/sha256-<hex>.ext4`) and
//!    re-verifies its digest ([`images`]);
//! 2. builds a jail (kernel copy, read-only rootfs copy, fresh scratch
//!    drive, `vm-config.json`) and a TAP on its own /30 with an nftables
//!    table that allows **only** the operator's egress list ([`net`]);
//! 3. execs Firecracker through the jailer ([`jail`]), waits for the RLM
//!    guest agent on vsock, and stages the owner key material from the
//!    host's own directory — the control plane never sees it ([`vsock`]);
//! 4. runs jobs over vsock; while a **paid** job runs it listens for the
//!    RLM's sister request and boots a second microVM with **no network**
//!    for the miner artefact — for that job's topic, submission, and
//!    artefact only — then attests that run ([`sister`]);
//! 5. tears the VM down: `Destroy` removes the jail, `Retain` moves it under
//!    `retain_dir` for audit.
//!
//! Nothing a boot started outlives its failure: from the moment a jail is
//! prepared it is owned by a [`jail::JailGuard`] until the VM is registered
//! (or, for a sister, until its run ends), and every error, cancellation, or
//! dropped request releases the process, the TAP, the nftables table, and
//! the directory. A sister whose job ends first is cancelled cooperatively —
//! killed and destroyed before the job answers — never abandoned mid-flight.
//!
//! Every host command goes through [`Shell`], so the tests in this crate
//! assert the exact argv without spawning anything. Nothing in CI boots a
//! VM: [`FirecrackerHypervisor::ready`] refuses on a host without
//! `firecracker`, `jailer`, and `/dev/kvm`, and the tests check that refusal.

#![forbid(unsafe_code)]
#![allow(clippy::missing_errors_doc, clippy::module_name_repetitions)]

pub mod config;
pub mod images;
pub mod jail;
pub mod net;
pub mod shell;
pub mod sister;
pub mod vsock;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use proof_rlm::{RetainPolicy, TopicVmSpec, VmJob};
use proof_vm_agent::{BootedVm, HvError, Hypervisor, JobOutcome};
use proof_vm_proto::guest::{
    check_version, HostToRlm, RlmToHost, SisterAnswer, SisterRequest, StagedFile, RLM_JOB_PORT,
    SISTER_PORT,
};
use proof_vm_proto::{EvidenceBinding, SisterAttestation, API_VERSION};
use tokio::net::UnixListener;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

pub use config::{EgressAllow, HostConfig, Proto};
pub use images::ImageCache;
pub use jail::JailGuard;
pub use net::NetPlan;
pub use shell::{RecordingShell, Shell, SystemShell};
pub use sister::SisterCtx;

/// How long a job waits for its sister task to finish killing and destroying
/// a sister after the job ended. Past this the task keeps cleaning up on its
/// own; it is never aborted mid-cleanup.
const SISTER_STOP_BUDGET: Duration = Duration::from_mins(1);

struct LiveVm {
    child: tokio::process::Child,
    root: PathBuf,
    net: NetPlan,
}

/// The production [`Hypervisor`].
pub struct FirecrackerHypervisor {
    ctx: SisterCtx,
    kernel_verified: AtomicBool,
    vms: Mutex<HashMap<String, LiveVm>>,
    net_index: AtomicU32,
    sister_seq: AtomicU64,
}

fn executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

impl FirecrackerHypervisor {
    /// Over the real shell.
    ///
    /// # Errors
    ///
    /// [`HvError::Spec`] when the config's pins / sizes are malformed.
    pub fn new(cfg: HostConfig) -> Result<Self, HvError> {
        Self::with_shell(cfg, Arc::new(SystemShell))
    }

    /// Over any [`Shell`] (tests record instead of running).
    ///
    /// # Errors
    ///
    /// [`HvError::Spec`].
    pub fn with_shell(cfg: HostConfig, shell: Arc<dyn Shell>) -> Result<Self, HvError> {
        cfg.validate()?;
        Ok(Self {
            ctx: SisterCtx {
                cfg: Arc::new(cfg),
                shell,
                images: Arc::new(ImageCache::default()),
            },
            kernel_verified: AtomicBool::new(false),
            vms: Mutex::new(HashMap::new()),
            net_index: AtomicU32::new(0),
            sister_seq: AtomicU64::new(1),
        })
    }

    /// The config in force.
    #[must_use]
    pub fn config(&self) -> &HostConfig {
        &self.ctx.cfg
    }

    /// Owner key material from `owner_key_dir`, to stage over vsock. Read
    /// here, sent to the guest, never logged, never returned to the control
    /// plane. Absent dir = nothing to stage.
    fn owner_files(&self) -> Result<Vec<StagedFile>, HvError> {
        let Some(dir) = &self.ctx.cfg.owner_key_dir else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        let entries = std::fs::read_dir(dir)
            .map_err(|e| HvError::Backend(format!("owner key dir {}: {e}", dir.display())))?;
        for entry in entries {
            let entry = entry.map_err(|e| HvError::Backend(format!("owner key dir: {e}")))?;
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let bytes = std::fs::read(&path)
                .map_err(|e| HvError::Backend(format!("owner key {name}: {e}")))?;
            out.push(StagedFile::new(&name, &bytes));
        }
        Ok(out)
    }

    async fn verify_kernel(&self) -> Result<(), HvError> {
        let cfg = &self.ctx.cfg;
        self.ctx
            .images
            .verify(&cfg.kernel, &cfg.kernel_digest)
            .await?;
        self.kernel_verified.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn job_budget(&self, job: &VmJob) -> Duration {
        job.deadline_s()
            .map_or(self.ctx.cfg.default_job_timeout, |d| {
                Duration::from_secs(d).saturating_add(self.ctx.cfg.deadline_grace)
            })
    }

    /// Accept sister requests from the RLM guest for one job; at most one
    /// sister per job, none for jobs that run no miner code (`paid` is
    /// `None`), and only for the paid job's own topic / submission /
    /// artefact. Returns once `cancel` fires — after any sister in flight has
    /// been killed and its jail destroyed — so the job never outruns its
    /// sister's cleanup.
    async fn serve_sisters(
        ctx: Arc<SisterCtx>,
        listener: UnixListener,
        vm: BootedVm,
        seq: Arc<AtomicU64>,
        slot: Arc<Mutex<Option<SisterAttestation>>>,
        paid: Option<EvidenceBinding>,
        cancel: CancellationToken,
    ) {
        loop {
            let accepted = tokio::select! {
                () = cancel.cancelled() => return,
                accepted = listener.accept() => accepted,
            };
            let Ok((stream, _)) = accepted else {
                return;
            };
            let mut ch = vsock::GuestChannel::from_stream(stream);
            let received = tokio::select! {
                () = cancel.cancelled() => return,
                r = ch.recv_within::<SisterRequest>(Duration::from_mins(1)) => r,
            };
            let answer = match (received, &paid) {
                (Err(e), _) => SisterAnswer::Refused {
                    error: e.to_string(),
                },
                (Ok(_), None) => SisterAnswer::Refused {
                    error: "this job runs no miner code; no sister".into(),
                },
                (Ok(req), Some(job)) => {
                    let mut taken = slot.lock().await;
                    if taken.is_some() {
                        SisterAnswer::Refused {
                            error: "one sister per job".into(),
                        }
                    } else {
                        let n = seq.fetch_add(1, Ordering::SeqCst);
                        match sister::run(&ctx, &vm, job, n, &req, &cancel).await {
                            Ok((result, attestation)) => {
                                *taken = Some(attestation);
                                SisterAnswer::Result { result }
                            }
                            Err(e) => {
                                // Operator evidence: a refused artefact (empty
                                // tree, gzip, wrong digest) is named here, and
                                // the run comes back without an attestation.
                                tracing::warn!(vm_id = %vm.vm_id, topic_id = %vm.topic_id, "sister request refused: {e}");
                                SisterAnswer::Refused {
                                    error: e.to_string(),
                                }
                            }
                        }
                    }
                }
            };
            if let Err(e) = ch.send(&answer).await {
                tracing::warn!(vm_id = %vm.vm_id, "sister answer not delivered: {e}");
            }
            if cancel.is_cancelled() {
                return;
            }
        }
    }

    /// Boot once the host is ready and the kernel + `image` verified: jail,
    /// network, process, guest handshake, staging, registration. Any failure
    /// after the jail exists releases the process, the TAP + nftables table,
    /// and the jail directory before the error is returned; a dropped future
    /// releases them through the guard.
    async fn boot_verified(
        &self,
        vm_id: &str,
        spec: &TopicVmSpec,
        image: PathBuf,
    ) -> Result<BootedVm, HvError> {
        let cfg = self.ctx.cfg.clone();
        let owner_files = self.owner_files()?;
        let net = NetPlan::for_index(&cfg, self.net_index.fetch_add(1, Ordering::SeqCst));
        let boot = jail::VmBoot {
            id: vm_id.to_owned(),
            vcpus: spec.template.vcpus,
            mem_mib: spec.template.mem_mib,
            rootfs: image,
            scratch_mib: cfg.scratch_mib,
            net: Some(net.clone()),
        };
        let mut jail = JailGuard::prepare(cfg.clone(), self.ctx.shell.clone(), &boot).await?;
        let up = Self::bring_up(
            &cfg,
            self.ctx.shell.as_ref(),
            &mut jail,
            &net,
            spec,
            owner_files,
        )
        .await;
        if let Err(e) = up {
            tracing::warn!(%vm_id, "topic vm boot failed; releasing its jail: {e}");
            jail.destroy().await;
            return Err(e);
        }
        let root = jail.root().to_path_buf();
        // Take the registry lock while the guard still owns the jail: a request
        // cancelled here releases everything. Nothing awaits between the
        // hand-over and the insert.
        let mut vms = self.vms.lock().await;
        let child = match jail.keep() {
            Ok(child) => child,
            Err(jail) => {
                drop(vms);
                jail.destroy().await;
                return Err(HvError::Backend(format!(
                    "vm {vm_id} has no process after boot"
                )));
            }
        };
        vms.insert(vm_id.to_owned(), LiveVm { child, root, net });
        drop(vms);
        Ok(BootedVm {
            vm_id: vm_id.to_owned(),
            topic_id: spec.topic_id.clone(),
            image_digest: spec.template.image_digest.clone(),
        })
    }

    /// Everything after the jail exists, up to a guest that answered hello
    /// and took its owner key material. On any error the guard the caller
    /// holds still owns the jail, so the caller releases it.
    async fn bring_up(
        cfg: &HostConfig,
        shell: &dyn Shell,
        jail: &mut JailGuard,
        net: &NetPlan,
        spec: &TopicVmSpec,
        owner_files: Vec<StagedFile>,
    ) -> Result<(), HvError> {
        let vm_id = jail.id().to_owned();
        net.up(shell, cfg.jail_uid).await?;
        let rules = cfg.jail_dir(&vm_id).join("net.nft").display().to_string();
        net.load_rules(shell, &rules).await?;
        // Advisory: a ufw / Docker forward chain that drops by default
        // silently kills the guest's egress (judge origin, artefact host) no
        // matter what the per-VM table allows. Name it so the operator adds
        // the TAP accept there (runbook § Egress); the check reads the rules,
        // so it goes quiet once that accept exists. Never fail the boot on it.
        match NetPlan::foreign_forward_drops(shell).await {
            Ok(drops) if !drops.is_empty() => tracing::warn!(
                %vm_id,
                tap = %net.tap,
                chains = ?drops,
                "advisory: host forward chains drop by default and accept no pfc* tap: guest egress \
                 (judge, artefact host) is blocked until an `iifname \"pfc*\" … accept` exists in those \
                 tables (runbook: proof-vm-orchestrator.md § Egress); this line clears on the next vm boot \
                 once it does"
            ),
            Ok(_) => {}
            Err(e) => tracing::debug!(%vm_id, "forward-policy preflight skipped: {e}"),
        }
        jail.spawn()?;
        let root = jail.root();
        let mut ch =
            vsock::GuestChannel::connect_within(root, RLM_JOB_PORT, cfg.boot_timeout).await?;
        ch.send(&HostToRlm::Hello {
            api_version: API_VERSION,
            topic_id: spec.topic_id.clone(),
            vm_id: vm_id.clone(),
        })
        .await?;
        match ch.recv_within::<RlmToHost>(cfg.boot_timeout).await? {
            RlmToHost::Ready { api_version, agent } => {
                check_version(api_version).map_err(|e| HvError::Guest(e.to_string()))?;
                tracing::info!(%vm_id, %agent, "rlm guest ready");
            }
            other => {
                return Err(HvError::Guest(format!(
                    "rlm guest answered {other:?} to hello"
                )))
            }
        }
        if !owner_files.is_empty() {
            let count = owner_files.len();
            ch.send(&HostToRlm::StageSecrets { files: owner_files })
                .await?;
            match ch.recv_within::<RlmToHost>(cfg.boot_timeout).await? {
                RlmToHost::Staged { count: got } if got == count => {
                    tracing::info!(%vm_id, count, "owner key material staged (contents not logged)");
                }
                other => {
                    return Err(HvError::Guest(format!("staging answered {other:?}")));
                }
            }
        }
        Ok(())
    }
}

#[async_trait]
impl Hypervisor for FirecrackerHypervisor {
    fn name(&self) -> &'static str {
        "firecracker"
    }

    fn ready(&self) -> Result<(), HvError> {
        let cfg = &self.ctx.cfg;
        let checks: [(&str, bool); 5] = [
            ("firecracker binary", executable(&cfg.firecracker_bin)),
            ("jailer binary", executable(&cfg.jailer_bin)),
            ("/dev/kvm", Path::new("/dev/kvm").exists()),
            ("image dir", cfg.image_dir.is_dir()),
            ("kernel", cfg.kernel.is_file()),
        ];
        if let Some((what, _)) = checks.iter().find(|(_, ok)| !ok) {
            return Err(HvError::NotReady(format!("{what} missing on this host")));
        }
        if cfg.egress_allow.is_empty() {
            tracing::debug!("egress allowlist empty: topic vms get no egress");
        }
        Ok(())
    }

    async fn boot(&self, vm_id: &str, spec: &TopicVmSpec) -> Result<BootedVm, HvError> {
        self.ready()?;
        spec.validate().map_err(|e| HvError::Spec(e.to_string()))?;
        let cfg = self.ctx.cfg.clone();
        if !self.kernel_verified.load(Ordering::SeqCst) {
            self.verify_kernel().await?;
        }
        let image = images::image_path(&cfg.image_dir, &spec.template.image_digest)?;
        self.ctx
            .images
            .verify(&image, &spec.template.image_digest)
            .await?;
        self.boot_verified(vm_id, spec, image).await
    }

    async fn alive(&self, vm: &BootedVm) -> bool {
        let mut vms = self.vms.lock().await;
        vms.get_mut(&vm.vm_id)
            .is_some_and(|live| matches!(live.child.try_wait(), Ok(None)))
    }

    async fn run_job(&self, vm: &BootedVm, job: &VmJob) -> Result<JobOutcome, HvError> {
        let root = {
            let vms = self.vms.lock().await;
            let live = vms
                .get(&vm.vm_id)
                .ok_or_else(|| HvError::Backend(format!("vm {} is not running here", vm.vm_id)))?;
            live.root.clone()
        };
        // Only a paid job may ask for a sister, and only for its own identities.
        let paid = EvidenceBinding::of_job(job);
        let listener = vsock::listen(&root, SISTER_PORT)?;
        let slot = Arc::new(Mutex::new(None));
        let cancel = CancellationToken::new();
        // Should this job be dropped mid-flight (the client gave up), the
        // guard still fires the token and the sister task cleans up after itself.
        let _stop_sisters = cancel.clone().drop_guard();
        let sisters = tokio::spawn(Self::serve_sisters(
            Arc::new(SisterCtx {
                cfg: self.ctx.cfg.clone(),
                shell: self.ctx.shell.clone(),
                images: self.ctx.images.clone(),
            }),
            listener,
            vm.clone(),
            Arc::new(AtomicU64::new(
                self.sister_seq.fetch_add(1_000, Ordering::SeqCst),
            )),
            slot.clone(),
            paid,
            cancel.clone(),
        ));
        let budget = self.job_budget(job);
        let answer = async {
            let mut ch = vsock::GuestChannel::connect(&root, RLM_JOB_PORT).await?;
            ch.send(&HostToRlm::Run {
                job: Box::new(job.clone()),
            })
            .await?;
            ch.recv_within::<RlmToHost>(budget).await
        }
        .await;
        // The job is over: stop serving sisters, and wait for a sister still
        // in flight to be killed and its jail destroyed. Never abort the task
        // — an aborted sister would leave its jail and scratch on disk.
        cancel.cancel();
        if tokio::time::timeout(SISTER_STOP_BUDGET, sisters)
            .await
            .is_err()
        {
            tracing::warn!(vm_id = %vm.vm_id, "sister cleanup still running after the job; it finishes on its own");
        }
        let _ = std::fs::remove_file(vsock::listener_path(&root, SISTER_PORT));
        let sister = slot.lock().await.take();
        match answer? {
            RlmToHost::Done { output } => Ok(JobOutcome { output, sister }),
            RlmToHost::Failed { error } => Err(HvError::Guest(error)),
            other => Err(HvError::Guest(format!(
                "rlm guest answered {other:?} to a job"
            ))),
        }
    }

    async fn teardown(&self, vm: &BootedVm, policy: RetainPolicy) -> Result<bool, HvError> {
        let Some(mut live) = self.vms.lock().await.remove(&vm.vm_id) else {
            return Err(HvError::Backend(format!(
                "vm {} is not running here",
                vm.vm_id
            )));
        };
        jail::kill(&mut live.child).await;
        let shell = self.ctx.shell.as_ref();
        for e in live.net.down(shell).await {
            tracing::warn!(vm_id = %vm.vm_id, "network teardown: {e}");
        }
        match policy {
            RetainPolicy::Destroy => jail::destroy(&self.ctx.cfg, shell, &vm.vm_id).await?,
            RetainPolicy::Retain => {
                let dest = jail::retain(&self.ctx.cfg, shell, &vm.vm_id).await?;
                tracing::info!(vm_id = %vm.vm_id, retained = %dest.display(), "topic vm retained");
            }
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;
    use proof_rlm::fixtures::{pinned_template, request};

    fn cfg(tag: &str) -> HostConfig {
        let mut c = HostConfig::defaults();
        let base = std::env::temp_dir().join(format!("proof-fc-host-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("images")).expect("dir");
        c.firecracker_bin = base.join("firecracker");
        c.jailer_bin = base.join("jailer");
        c.chroot_base = base.join("jailer-root");
        c.image_dir = base.join("images");
        c.kernel = base.join("vmlinux");
        c.kernel_digest = format!("sha256:{}", "aa".repeat(32));
        c.sister_image_digest = format!("sha256:{}", "bb".repeat(32));
        c
    }

    /// CI has no Firecracker: `ready()` names what is missing and nothing
    /// is ever spawned. This is the "zero live FC in GitHub runners" gate.
    #[tokio::test]
    async fn without_firecracker_the_backend_refuses_and_never_spawns() {
        let shell = Arc::new(RecordingShell::default());
        let hv = FirecrackerHypervisor::with_shell(cfg("unready"), shell.clone()).expect("config");
        assert_eq!(hv.name(), "firecracker");
        let err = hv.ready().expect_err("no binaries");
        assert!(matches!(err, HvError::NotReady(_)), "{err}");
        assert!(err.to_string().contains("firecracker binary"), "{err}");
        let req = request();
        let spec = TopicVmSpec::for_topic(&req.topic_id, pinned_template(), req.sandbox.clone());
        let err = hv
            .boot("topic-a-0001", &spec)
            .await
            .expect_err("boot refused");
        assert!(matches!(err, HvError::NotReady(_)), "{err}");
        let vm = BootedVm {
            vm_id: "topic-a-0001".into(),
            topic_id: req.topic_id.clone(),
            image_digest: pinned_template().image_digest,
        };
        let err = hv
            .run_job(
                &vm,
                &VmJob::Archive {
                    topic_id: req.topic_id.clone(),
                },
            )
            .await
            .expect_err("unknown vm");
        assert!(err.to_string().contains("not running here"), "{err}");
        assert!(hv.teardown(&vm, RetainPolicy::Destroy).await.is_err());
        assert!(shell.calls().is_empty(), "no host command ran");
        let mut bad = cfg("badpin");
        bad.kernel_digest = "latest".into();
        assert!(FirecrackerHypervisor::with_shell(bad, shell).is_err());
    }

    /// With fake binaries present, the pin gate runs next: a kernel whose
    /// bytes do not hash to the pin, or an RLM image absent from the image
    /// dir, refuses before any jail is prepared.
    #[tokio::test]
    async fn pins_gate_the_boot_before_any_jail_is_built() {
        let mut c = cfg("pins");
        for bin in [&c.firecracker_bin, &c.jailer_bin] {
            std::fs::write(bin, b"#!/bin/sh\nexit 0\n").expect("write");
            std::fs::set_permissions(bin, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        }
        std::fs::write(&c.kernel, b"not the pinned kernel").expect("kernel");
        c.kernel_digest = format!("sha256:{}", images::sha256_file(&c.kernel).expect("hash"));
        let shell = Arc::new(RecordingShell::default());
        let hv = FirecrackerHypervisor::with_shell(c.clone(), shell.clone()).expect("config");
        if !Path::new("/dev/kvm").exists() {
            let err = hv.ready().expect_err("no kvm on this box");
            assert!(err.to_string().contains("/dev/kvm"), "{err}");
            return;
        }
        hv.ready().expect("binaries + kvm present");
        let req = request();
        let spec = TopicVmSpec::for_topic(&req.topic_id, pinned_template(), req.sandbox.clone());
        let err = hv
            .boot("topic-a-0001", &spec)
            .await
            .expect_err("rlm image absent");
        assert!(matches!(err, HvError::Image(_)), "{err}");
        assert!(shell.calls().is_empty(), "refused before the jail");
        let owner = hv.owner_files().expect("no dir = nothing");
        assert!(owner.is_empty());
    }

    /// A boot that fails after the jail is prepared — TAP setup, rules load,
    /// or a guest that never says hello — releases everything it built: the
    /// nftables table, the TAP, and the jail directory (with the rules file
    /// and scratch inside it). Nothing is registered, nothing is alive.
    #[tokio::test]
    async fn a_boot_that_fails_before_the_handshake_releases_its_jail_and_network() {
        let req = request();
        let spec = TopicVmSpec::for_topic(&req.topic_id, pinned_template(), req.sandbox.clone());
        for (tag, fail_on) in [("tap", "ip tuntap"), ("rules", "nft -f")] {
            let c = cfg(tag);
            let image = c.image_dir.join("rlm.ext4");
            std::fs::write(&image, b"rlm rootfs stand-in").expect("image");
            let shell = Arc::new(shell::FailingShell::failing_on(fail_on));
            let hv = FirecrackerHypervisor::with_shell(c.clone(), shell.clone()).expect("config");
            let err = hv
                .boot_verified("topic-a-0001", &spec, image)
                .await
                .expect_err("injected host failure");
            assert!(err.to_string().contains("injected failure"), "{tag}: {err}");
            let lines = shell.lines();
            let jail_dir = c.jail_dir("topic-a-0001").display().to_string();
            assert!(
                lines.iter().any(|l| l.starts_with(fail_on)),
                "{tag}: the failing step ran: {lines:?}"
            );
            let failed_at = lines
                .iter()
                .position(|l| l.starts_with(fail_on))
                .expect("position");
            let after = &lines[failed_at + 1..];
            assert!(
                after.contains(&"nft delete table inet proof_vm_pfc0".to_owned()),
                "{tag}: table released: {after:?}"
            );
            assert!(
                after.contains(&"ip link del pfc0".to_owned()),
                "{tag}: tap released: {after:?}"
            );
            assert_eq!(
                after.last().map(String::as_str),
                Some(format!("rm -rf {jail_dir}").as_str()),
                "{tag}: jail removed last: {after:?}"
            );
            assert!(hv.vms.lock().await.is_empty(), "{tag}: nothing registered");
            let vm = BootedVm {
                vm_id: "topic-a-0001".into(),
                topic_id: req.topic_id.clone(),
                image_digest: pinned_template().image_digest,
            };
            assert!(!hv.alive(&vm).await, "{tag}: never alive");
            let _ = std::fs::remove_dir_all(c.chroot_base.parent().unwrap_or(&c.chroot_base));
        }

        // The process side: a stand-in "jailer" (a sleeping shell script, no
        // Firecracker) that never brings a guest up. The handshake budget
        // runs out, the process is killed, and the jail is released.
        let c = {
            let mut c = cfg("hello");
            c.boot_timeout = Duration::from_millis(400);
            std::fs::write(&c.jailer_bin, b"#!/bin/sh\nexec sleep 30\n").expect("stand-in");
            std::fs::set_permissions(&c.jailer_bin, std::fs::Permissions::from_mode(0o755))
                .expect("chmod");
            c
        };
        let image = c.image_dir.join("rlm.ext4");
        std::fs::write(&image, b"rlm rootfs stand-in").expect("image");
        let shell = Arc::new(RecordingShell::default());
        let hv = FirecrackerHypervisor::with_shell(c.clone(), shell.clone()).expect("config");
        let err = hv
            .boot_verified("topic-a-0002", &spec, image)
            .await
            .expect_err("no guest ever answered");
        assert!(matches!(err, HvError::Guest(_)), "{err}");
        let lines: Vec<String> = shell.calls().iter().map(|l| l.join(" ")).collect();
        assert!(lines.contains(&"ip link del pfc0".to_owned()), "{lines:?}");
        assert_eq!(
            lines.last().map(String::as_str),
            Some(format!("rm -rf {}", c.jail_dir("topic-a-0002").display()).as_str()),
            "{lines:?}"
        );
        assert!(hv.vms.lock().await.is_empty());
        let _ = std::fs::remove_dir_all(c.chroot_base.parent().unwrap_or(&c.chroot_base));
    }

    /// Stand in for Firecracker's vsock UDS + the RLM guest agent: answer the
    /// `CONNECT` handshake, then `Hello` with `Ready`. Bound by the caller
    /// once the jail root exists.
    async fn fake_rlm_guest(listener: UnixListener) {
        use proof_vm_proto::guest::{read_frame, write_frame};
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let Ok((stream, _)) = listener.accept().await else {
            return;
        };
        let mut stream = BufReader::new(stream);
        let mut line = String::new();
        let _ = stream.read_line(&mut line).await;
        assert_eq!(line, format!("CONNECT {RLM_JOB_PORT}\n"));
        stream
            .get_mut()
            .write_all(b"OK 1073741824\n")
            .await
            .expect("ok");
        let hello: HostToRlm = read_frame(&mut stream).await.expect("hello");
        assert!(matches!(hello, HostToRlm::Hello { .. }));
        write_frame(
            stream.get_mut(),
            &RlmToHost::Ready {
                agent: "fake-rlm-guest".into(),
                api_version: API_VERSION,
            },
        )
        .await
        .expect("ready");
    }

    /// Serve the fake guest as soon as the boot has prepared `root`.
    fn serve_fake_guest_when_ready(root: PathBuf) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            for _ in 0..100 {
                if root.is_dir() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            let listener = UnixListener::bind(vsock::uds_path(&root)).expect("bind fake vsock");
            fake_rlm_guest(listener).await;
        })
    }

    fn stand_in_host(tag: &str) -> HostConfig {
        let mut c = cfg(tag);
        c.boot_timeout = Duration::from_secs(10);
        std::fs::write(&c.jailer_bin, b"#!/bin/sh\nexec sleep 30\n").expect("stand-in");
        std::fs::set_permissions(&c.jailer_bin, std::fs::Permissions::from_mode(0o755))
            .expect("chmod");
        std::fs::write(c.image_dir.join("rlm.ext4"), b"rlm rootfs stand-in").expect("image");
        c
    }

    /// The guest is ready and the boot is waiting for the VM registry when
    /// the request is cancelled: the guard still owns the jail, so the
    /// process, the network, and the directory are released — nothing is
    /// registered. With the registry free the same boot completes, is alive,
    /// and tears down cleanly. Stand-in process + fake guest, no Firecracker.
    #[tokio::test]
    async fn a_boot_cancelled_at_the_registry_still_releases_everything() {
        let c = stand_in_host("registry");
        let req = request();
        let spec = TopicVmSpec::for_topic(&req.topic_id, pinned_template(), req.sandbox.clone());
        let shell = Arc::new(RecordingShell::default());
        let hv =
            Arc::new(FirecrackerHypervisor::with_shell(c.clone(), shell.clone()).expect("config"));
        let held = hv.vms.lock().await;
        let guest = serve_fake_guest_when_ready(c.jail_root("topic-a-0001"));
        let boot = {
            let hv = hv.clone();
            let spec = spec.clone();
            let image = c.image_dir.join("rlm.ext4");
            tokio::spawn(async move { hv.boot_verified("topic-a-0001", &spec, image).await })
        };
        guest.await.expect("guest answered hello");
        // The boot is now parked on the registry lock we hold.
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(!boot.is_finished(), "blocked on the registry");
        boot.abort();
        let _ = boot.await;
        drop(held);
        let jail_dir = c.jail_dir("topic-a-0001").display().to_string();
        for _ in 0..100 {
            if shell
                .calls()
                .iter()
                .any(|l| l.join(" ") == format!("rm -rf {jail_dir}"))
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let lines: Vec<String> = shell.calls().iter().map(|l| l.join(" ")).collect();
        assert!(
            lines.contains(&"nft delete table inet proof_vm_pfc0".to_owned()),
            "{lines:?}"
        );
        assert!(lines.contains(&"ip link del pfc0".to_owned()), "{lines:?}");
        assert_eq!(
            lines.last().map(String::as_str),
            Some(format!("rm -rf {jail_dir}").as_str()),
            "{lines:?}"
        );
        assert!(hv.vms.lock().await.is_empty(), "nothing registered");
        let _ = std::fs::remove_dir_all(c.jail_dir("topic-a-0001"));

        // Registry free: the boot completes and the VM is alive until torn down.
        let guest = serve_fake_guest_when_ready(c.jail_root("topic-a-0002"));
        let vm = hv
            .boot_verified("topic-a-0002", &spec, c.image_dir.join("rlm.ext4"))
            .await
            .expect("boot completes");
        guest.await.expect("guest");
        assert_eq!(vm.topic_id, req.topic_id);
        assert!(hv.alive(&vm).await, "stand-in process is running");
        assert_eq!(hv.vms.lock().await.len(), 1);
        assert!(hv
            .teardown(&vm, RetainPolicy::Destroy)
            .await
            .expect("teardown"));
        assert!(!hv.alive(&vm).await);
        assert!(hv.vms.lock().await.is_empty());
        let lines: Vec<String> = shell.calls().iter().map(|l| l.join(" ")).collect();
        assert_eq!(
            lines.last().map(String::as_str),
            Some(format!("rm -rf {}", c.jail_dir("topic-a-0002").display()).as_str()),
            "{lines:?}"
        );
        let _ = std::fs::remove_dir_all(c.chroot_base.parent().unwrap_or(&c.chroot_base));
    }

    #[tokio::test]
    async fn owner_key_material_is_read_from_the_host_dir_only() {
        let mut c = cfg("owner");
        let dir = c.image_dir.join("../owner-keys");
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(dir.join("inference_key"), b"owner-key-not-a-real-secret").expect("write");
        std::fs::create_dir_all(dir.join("subdir")).expect("subdir ignored");
        c.owner_key_dir = Some(dir);
        let hv = FirecrackerHypervisor::with_shell(c, Arc::new(RecordingShell::default()))
            .expect("config");
        let files = hv.owner_files().expect("read");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].name, "inference_key");
        assert_eq!(
            files[0].bytes().expect("decode"),
            b"owner-key-not-a-real-secret"
        );
        let budget = hv.job_budget(&VmJob::Archive {
            topic_id: "t".into(),
        });
        assert_eq!(budget, hv.config().default_job_timeout);
        let req = request();
        let paid = hv.job_budget(&VmJob::Baseline {
            request: req.clone(),
        });
        assert_eq!(
            paid,
            Duration::from_secs(req.sandbox.deadline_s) + hv.config().deadline_grace
        );
    }
}
