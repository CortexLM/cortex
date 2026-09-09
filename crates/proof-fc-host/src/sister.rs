//! The **sister** miner guest: a second Firecracker microVM the host boots
//! beside the RLM VM for one paid run, with no network interface, fed the
//! artefact bytes the RLM already inspected over vsock, held to the run's
//! deadline, then destroyed. The host — not the RLM — knows it happened,
//! which is what [`SisterAttestation`] records — for the job's own topic,
//! submission, and artefact only: a request naming any other identity is
//! refused before a jail is built. Whether the run ends, times out, or is
//! cancelled because the job it served finished first, the sister jail and
//! its scratch are destroyed before the host moves on.

use std::sync::Arc;
use std::time::{Duration, Instant};

use proof_vm_agent::{BootedVm, HvError};
use proof_vm_proto::guest::{
    check_version, HostToMiner, MinerToHost, SisterRequest, SisterResult, MINER_PORT,
};
use proof_vm_proto::{EvidenceBinding, SisterAttestation, API_VERSION};
use tokio_util::sync::CancellationToken;

use crate::config::{HostConfig, MAX_ARTIFACT_TAR_BYTES};
use crate::images::{image_path, ImageCache};
use crate::jail::{JailGuard, VmBoot};
use crate::shell::Shell;
use crate::vsock::GuestChannel;

/// Longest jail id the jailer accepts.
const MAX_JAIL_ID: usize = 64;

/// What a sister run needs from the host.
pub struct SisterCtx {
    /// Host config (sister image pin, sizes, timeouts).
    pub cfg: Arc<HostConfig>,
    /// Command runner.
    pub shell: Arc<dyn Shell>,
    /// Verified-image cache.
    pub images: Arc<ImageCache>,
}

/// `<parent vm id>-s<seq>`, trimmed so the jailer accepts it.
#[must_use]
pub fn sister_id(parent_vm_id: &str, seq: u64) -> String {
    let suffix = format!("-s{seq}");
    let keep = MAX_JAIL_ID.saturating_sub(suffix.len());
    let mut prefix = parent_vm_id.to_owned();
    prefix.truncate(keep);
    format!("{prefix}{suffix}")
}

/// Refuse a request the host will not boot a sister for.
///
/// `job` is what the control plane asked the RLM to run; the sister must be
/// for exactly that topic, submission, and artefact, or its evidence would be
/// evidence for something else. The bytes must also *be* that artefact: the
/// file served at `artifact_uri`, forwarded verbatim — an uncompressed tar
/// with file content whose sha256 is the paid digest
/// ([`proof_vm_proto::tar::verify_artifact`], the same check the guest runs
/// on what it fetched). A re-tarred tree hashes differently and is refused;
/// a digest that matches an empty tree is a guest whose fetch failed, not a
/// miner's work.
///
/// # Errors
///
/// [`HvError::Spec`]: wrong topic / submission / artefact, oversized,
/// content-less, compressed, or mis-hashed artefact.
pub fn check_request(
    parent: &BootedVm,
    job: &EvidenceBinding,
    req: &SisterRequest,
) -> Result<Vec<u8>, HvError> {
    if req.topic_id != parent.topic_id {
        return Err(HvError::Spec(format!(
            "sister request names topic {:?}, vm is bound to {:?}",
            req.topic_id, parent.topic_id
        )));
    }
    let asked = EvidenceBinding::new(&req.topic_id, &req.submission_digest, &req.artifact_digest);
    if let Some((field, got, want)) = job.first_mismatch(&asked) {
        return Err(HvError::Spec(format!(
            "sister request names {field} {got:?}, the paid job names {want:?}"
        )));
    }
    let tar = req
        .artifact_tar
        .bytes()
        .map_err(|e| HvError::Spec(format!("artifact_tar: {e}")))?;
    if tar.is_empty() || tar.len() > MAX_ARTIFACT_TAR_BYTES {
        return Err(HvError::Spec(format!(
            "artifact_tar is {} bytes (1..={MAX_ARTIFACT_TAR_BYTES})",
            tar.len()
        )));
    }
    // Same check, same bytes, same digest as the guest ran on its fetch:
    // shape (uncompressed tar with content) then identity (sha256 of the
    // bytes as served).
    proof_vm_proto::tar::verify_artifact(&tar, &req.artifact_digest)
        .map_err(|e| HvError::Spec(e.to_string()))?;
    if req.entrypoint.is_empty() || req.deadline_s == 0 {
        return Err(HvError::Spec(
            "entrypoint and deadline_s are required".into(),
        ));
    }
    Ok(tar)
}

fn u64_ms(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

/// Wait for the sister's agent, hand it the run, wait for the answer.
async fn drive_guest(
    cfg: &HostConfig,
    root: &std::path::Path,
    req: &SisterRequest,
) -> Result<MinerToHost, HvError> {
    let mut ch = GuestChannel::connect_within(root, MINER_PORT, cfg.boot_timeout).await?;
    match ch.recv_within::<MinerToHost>(cfg.boot_timeout).await? {
        MinerToHost::Ready { api_version, .. } => {
            check_version(api_version).map_err(|e| HvError::Guest(e.to_string()))?;
        }
        other => {
            return Err(HvError::Guest(format!(
                "sister spoke before ready: {other:?}"
            )));
        }
    }
    ch.send(&HostToMiner::Run {
        api_version: API_VERSION,
        artifact_tar: req.artifact_tar.clone(),
        entrypoint: req.entrypoint.clone(),
        deadline_s: req.deadline_s,
        declared_flops: req.declared_flops,
        seed: req.seed,
        params: req.params.clone(),
    })
    .await?;
    let budget = Duration::from_secs(req.deadline_s).saturating_add(cfg.deadline_grace);
    ch.recv_within::<MinerToHost>(budget).await
}

/// Boot a sister for `req`, run it, destroy it, and attest.
///
/// `job` binds the sister to the paid job being served; `cancel` is the
/// job's own lifetime — when it fires (the RLM answered, or the job's
/// deadline passed) the run is cut, the guest killed, and the jail destroyed
/// before this returns. The jail is destroyed on **every** exit, including a
/// dropped future ([`JailGuard`]).
///
/// # Errors
///
/// [`HvError::Spec`] (refused before boot), [`HvError::Image`] (sister image
/// missing / mismatched), [`HvError::Backend`] / [`HvError::Guest`] (the
/// host could not run it at all), [`HvError::Cancelled`] (the job ended
/// first). A run cut at the deadline is **not** an error: it is a result
/// with `timed_out: true`.
pub async fn run(
    ctx: &SisterCtx,
    parent: &BootedVm,
    job: &EvidenceBinding,
    seq: u64,
    req: &SisterRequest,
    cancel: &CancellationToken,
) -> Result<(SisterResult, SisterAttestation), HvError> {
    check_request(parent, job, req)?;
    if cancel.is_cancelled() {
        return Err(HvError::Cancelled(
            "job ended before the sister booted".into(),
        ));
    }
    let cfg = &ctx.cfg;
    let image = image_path(&cfg.image_dir, &cfg.sister_image_digest)?;
    ctx.images.verify(&image, &cfg.sister_image_digest).await?;
    let id = sister_id(&parent.vm_id, seq);
    let boot = VmBoot {
        id: id.clone(),
        vcpus: cfg.sister_vcpus,
        mem_mib: cfg.sister_mem_mib,
        rootfs: image,
        scratch_mib: cfg.sister_scratch_mib,
        net: None,
    };
    let mut jail = JailGuard::prepare(cfg.clone(), ctx.shell.clone(), &boot).await?;
    if let Err(e) = jail.spawn() {
        jail.destroy().await;
        return Err(e);
    }
    let started = Instant::now();
    tracing::info!(sister = %id, parent = %parent.vm_id, topic_id = %parent.topic_id, "sister guest booting (no network)");
    let root = jail.root().to_path_buf();
    let outcome = tokio::select! {
        out = drive_guest(cfg, &root, req) => out,
        () = cancel.cancelled() => Err(HvError::Cancelled(
            "job ended while the sister was running".into(),
        )),
    };
    let wall_ms = u64_ms(started.elapsed());
    // Whatever happened, the sister is over: kill it and remove its jail now.
    jail.destroy().await;
    let (exit_code, timed_out, stdout_tail, flops_used, outputs) = match outcome {
        Ok(MinerToHost::Done {
            exit_code,
            timed_out,
            stdout_tail,
            flops_used,
            outputs,
        }) => (exit_code, timed_out, stdout_tail, flops_used, outputs),
        // Nothing ran: zero is a measurement here, so the RLM can write a
        // reject the control plane persists instead of a 503.
        Ok(MinerToHost::Failed { error }) => (
            None,
            false,
            format!("guest failed: {error}"),
            Some(0),
            vec![],
        ),
        Ok(MinerToHost::Ready { .. }) => {
            return Err(HvError::Guest("sister answered ready twice".into()));
        }
        Err(HvError::Deadline(_)) => (
            None,
            true,
            format!(
                "killed by the host {}s after the deadline",
                cfg.deadline_grace.as_secs()
            ),
            None,
            vec![],
        ),
        Err(e) => return Err(e),
    };
    let result = SisterResult {
        sister_vm_id: id.clone(),
        image_digest: cfg.sister_image_digest.clone(),
        exit_code,
        timed_out,
        stdout_tail,
        flops_used,
        wall_ms,
        outputs,
    };
    let attestation = SisterAttestation {
        sister_vm_id: id,
        image_digest: cfg.sister_image_digest.clone(),
        topic_id: job.topic_id.clone(),
        submission_digest: job.submission_digest.clone(),
        artifact_digest: job.artifact_digest.clone(),
        sandboxed: true,
        network: "none".into(),
        flops_used,
        wall_ms,
        exit_code,
    };
    Ok((result, attestation))
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;
    use crate::shell::RecordingShell;
    use proof_vm_proto::guest::StagedFile;
    use proof_vm_proto::tar::fixtures::{archive, empty_archive, member};
    use sha2::{Digest, Sha256};

    /// A real (uncompressed ustar) artefact whose one file holds `payload`.
    fn tarball(payload: &[u8]) -> Vec<u8> {
        archive(&[member("recipe/run.sh", b'0', payload)])
    }

    fn parent() -> BootedVm {
        BootedVm {
            vm_id: "topic-a-0001".into(),
            topic_id: "topic-a".into(),
            image_digest: format!("sha256:{}", "cc".repeat(32)),
        }
    }

    fn request(tar: &[u8]) -> SisterRequest {
        SisterRequest {
            topic_id: "topic-a".into(),
            submission_digest: "d".into(),
            artifact_digest: hex::encode(Sha256::digest(tar)),
            artifact_tar: StagedFile::new("artifact.tar", tar),
            entrypoint: vec!["./run.sh".into()],
            deadline_s: 60,
            declared_flops: 1,
            seed: 7,
            params: std::collections::BTreeMap::default(),
        }
    }

    /// The paid job the sister is served for (same identities as `request`).
    fn job(tar: &[u8]) -> EvidenceBinding {
        EvidenceBinding::new("topic-a", "d", &hex::encode(Sha256::digest(tar)))
    }

    #[test]
    fn sister_ids_fit_the_jailer_and_requests_are_bound_and_hashed() {
        assert_eq!(sister_id("topic-a-0001", 3), "topic-a-0001-s3");
        let long = "t".repeat(63);
        let id = sister_id(&long, 12);
        assert!(id.len() <= MAX_JAIL_ID, "{id}");
        assert!(id.ends_with("-s12"));
        let tar = &tarball(b"tar bytes");
        check_request(&parent(), &job(tar), &request(tar)).expect("bound + hashed");
        let mut other = request(tar);
        other.topic_id = "topic-b".into();
        assert!(matches!(
            check_request(&parent(), &job(tar), &other),
            Err(HvError::Spec(_))
        ));
        let mut wrong = request(tar);
        wrong.artifact_digest = "00".repeat(32);
        let err = check_request(&parent(), &job(tar), &wrong).expect_err("hash");
        assert!(err.to_string().contains("the paid job names"), "{err}");
        let mut empty = request(b"");
        empty.artifact_digest = hex::encode(Sha256::digest(b""));
        assert!(check_request(&parent(), &job(b""), &empty).is_err());
        // Consistent digest, but the bytes are an empty archive / a tree of
        // empty files: exactly the shape a guest whose fetch failed would
        // send. Refused by name before any hash or jail.
        for hollow in [
            empty_archive(),
            tarball(b""),
            archive(&[member("recipe/", b'5', b"")]),
        ] {
            let err =
                check_request(&parent(), &job(&hollow), &request(&hollow)).expect_err("no content");
            assert!(err.to_string().contains("no file content"), "{err}");
        }
        let mut gz = tarball(b"x");
        gz[0] = 0x1f;
        gz[1] = 0x8b;
        let err = check_request(&parent(), &job(&gz), &request(&gz)).expect_err("gzip");
        assert!(err.to_string().contains("gzip"), "{err}");
        let not_tar = b"not a tar at all, just bytes that hash consistently";
        let err = check_request(&parent(), &job(not_tar), &request(not_tar)).expect_err("not tar");
        assert!(err.to_string().contains("not a tar archive"), "{err}");
        let mut junk = request(tar);
        junk.artifact_tar.bytes_b64 = "!!".into();
        assert!(check_request(&parent(), &job(tar), &junk).is_err());
        let mut no_entry = request(tar);
        no_entry.entrypoint.clear();
        assert!(check_request(&parent(), &job(tar), &no_entry).is_err());
        // Consistent with its job on paper, but the bytes do not hash to the
        // digest: the re-hash refuses it.
        let mut mislabeled = request(tar);
        mislabeled.artifact_digest = "00".repeat(32);
        let err = check_request(
            &parent(),
            &EvidenceBinding::new("topic-a", "d", &"00".repeat(32)),
            &mislabeled,
        )
        .expect_err("hash");
        assert!(err.to_string().contains("hashes to"), "{err}");
        // The paid digest is the served file's. A guest that re-tars the same
        // tree (here: another member order) ships different bytes under that
        // digest and is refused with the reason — forward the fetch verbatim.
        let served = archive(&[
            member("recipe/run.sh", b'0', b"echo hi\n"),
            member("recipe/data.bin", b'0', &[1u8; 600]),
        ]);
        let retarred = archive(&[
            member("recipe/data.bin", b'0', &[1u8; 600]),
            member("recipe/run.sh", b'0', b"echo hi\n"),
        ]);
        let paid = job(&served);
        check_request(&parent(), &paid, &request(&served)).expect("verbatim bytes");
        let mut forwarded = request(&retarred);
        forwarded.artifact_digest = hex::encode(Sha256::digest(&served));
        let err = check_request(&parent(), &paid, &forwarded).expect_err("re-tar");
        let msg = err.to_string();
        assert!(msg.contains("re-tarred tree never matches"), "{msg}");
        assert!(msg.contains("verbatim"), "{msg}");
    }

    /// A sister request for another submission or artefact than the paid job
    /// is refused before any jail is built: replayed evidence cannot exist.
    #[tokio::test]
    async fn a_request_for_another_submission_or_artifact_never_boots() {
        let tar = &tarball(b"artifact b");
        let for_a = EvidenceBinding::new("topic-a", "submission-a", &"aa".repeat(32));
        let err = check_request(&parent(), &for_a, &request(tar)).expect_err("other job");
        assert!(matches!(err, HvError::Spec(_)), "{err}");
        assert!(err.to_string().contains("submission_digest"), "{err}");
        let same_submission = EvidenceBinding::new("topic-a", "d", &"aa".repeat(32));
        let err =
            check_request(&parent(), &same_submission, &request(tar)).expect_err("other artefact");
        assert!(err.to_string().contains("artifact_digest"), "{err}");
        let shell = Arc::new(RecordingShell::default());
        let mut cfg = HostConfig::defaults();
        cfg.sister_image_digest = format!("sha256:{}", "bb".repeat(32));
        let ctx = SisterCtx {
            cfg: Arc::new(cfg),
            shell: shell.clone(),
            images: Arc::new(ImageCache::default()),
        };
        let err = run(
            &ctx,
            &parent(),
            &for_a,
            1,
            &request(tar),
            &CancellationToken::new(),
        )
        .await
        .expect_err("refused");
        assert!(matches!(err, HvError::Spec(_)), "{err}");
        assert!(shell.calls().is_empty(), "no jail for a mismatched request");
    }

    /// The host never boots a sister whose image is not on disk at the
    /// pinned digest — and this test proves no process is spawned for it.
    #[tokio::test]
    async fn a_missing_sister_image_refuses_before_any_jail_or_process() {
        let mut cfg = HostConfig::defaults();
        cfg.image_dir =
            std::env::temp_dir().join(format!("proof-fc-sister-{}", std::process::id()));
        std::fs::create_dir_all(&cfg.image_dir).expect("dir");
        cfg.sister_image_digest = format!("sha256:{}", "bb".repeat(32));
        let shell = Arc::new(RecordingShell::default());
        let ctx = SisterCtx {
            cfg: Arc::new(cfg),
            shell: shell.clone(),
            images: Arc::new(ImageCache::default()),
        };
        let err = run(
            &ctx,
            &parent(),
            &job(&tarball(b"tar")),
            1,
            &request(&tarball(b"tar")),
            &CancellationToken::new(),
        )
        .await
        .expect_err("no image");
        assert!(matches!(err, HvError::Image(_)), "{err}");
        assert!(
            shell.calls().is_empty(),
            "nothing prepared, nothing spawned"
        );
    }

    /// A sister whose job ends first is cut and its jail destroyed before
    /// `run` returns — the storage a cancelled evaluation used is gone. The
    /// "jailer" here is a plain shell script that sleeps; no Firecracker, no
    /// KVM, no VM.
    #[tokio::test]
    async fn a_cancelled_sister_is_killed_and_its_jail_destroyed() {
        let base =
            std::env::temp_dir().join(format!("proof-fc-sister-cancel-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("images")).expect("dir");
        let mut cfg = HostConfig::defaults();
        cfg.image_dir = base.join("images");
        cfg.chroot_base = base.join("jailer-root");
        cfg.kernel = base.join("vmlinux");
        cfg.kernel_digest = format!("sha256:{}", "aa".repeat(32));
        cfg.jailer_bin = base.join("jailer");
        std::fs::write(&cfg.jailer_bin, b"#!/bin/sh\nexec sleep 30\n").expect("stand-in");
        std::fs::set_permissions(&cfg.jailer_bin, std::fs::Permissions::from_mode(0o755))
            .expect("chmod");
        let image_bytes = b"sister rootfs stand-in";
        let hex = hex::encode(Sha256::digest(image_bytes));
        std::fs::write(
            cfg.image_dir.join(format!("sha256-{hex}.ext4")),
            image_bytes,
        )
        .expect("image");
        cfg.sister_image_digest = format!("sha256:{hex}");
        cfg.boot_timeout = Duration::from_secs(20);
        let shell = Arc::new(RecordingShell::default());
        let ctx = SisterCtx {
            cfg: Arc::new(cfg.clone()),
            shell: shell.clone(),
            images: Arc::new(ImageCache::default()),
        };
        let cancel = CancellationToken::new();
        let tar = &tarball(b"artifact");
        let started = Instant::now();
        let sister = {
            let cancel = cancel.clone();
            async move { run(&ctx, &parent(), &job(tar), 7, &request(tar), &cancel).await }
        };
        let canceller = async {
            tokio::time::sleep(Duration::from_millis(300)).await;
            cancel.cancel();
        };
        let (outcome, ()) = tokio::join!(sister, canceller);
        let err = outcome.expect_err("cut by the job ending");
        assert!(matches!(err, HvError::Cancelled(_)), "{err}");
        assert!(
            started.elapsed() < cfg.boot_timeout,
            "did not wait for the guest handshake budget"
        );
        let lines: Vec<String> = shell.calls().iter().map(|c| c.join(" ")).collect();
        let jail_dir = cfg.jail_dir("topic-a-0001-s7").display().to_string();
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("mkdir -p ") && l.contains("topic-a-0001-s7")),
            "the sister jail was prepared: {lines:?}"
        );
        assert_eq!(
            lines.last().map(String::as_str),
            Some(format!("rm -rf {jail_dir}").as_str()),
            "destroyed before run returned: {lines:?}"
        );
        assert!(
            !cfg.jail_root("topic-a-0001-s7")
                .join(crate::jail::VSOCK_IN_JAIL)
                .exists(),
            "nothing but the stand-in ever ran"
        );
        let _ = std::fs::remove_dir_all(&base);
    }
}
