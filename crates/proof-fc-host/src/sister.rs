//! The **sister** miner guest: a second Firecracker microVM the host boots
//! beside the RLM VM for one paid run, with no network interface, fed the
//! artefact bytes the RLM already inspected over vsock, held to the run's
//! deadline, then destroyed. The host — not the RLM — knows it happened,
//! which is what [`SisterAttestation`] records.

use std::sync::Arc;
use std::time::{Duration, Instant};

use proof_vm_agent::{BootedVm, HvError};
use proof_vm_proto::guest::{
    check_version, HostToMiner, MinerToHost, SisterRequest, SisterResult, MINER_PORT,
};
use proof_vm_proto::{SisterAttestation, API_VERSION};
use sha2::{Digest, Sha256};

use crate::config::{HostConfig, MAX_ARTIFACT_TAR_BYTES};
use crate::images::{image_path, ImageCache};
use crate::jail::{self, VmBoot};
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
/// # Errors
///
/// [`HvError::Spec`]: wrong topic, oversized or mis-hashed artefact.
pub fn check_request(parent: &BootedVm, req: &SisterRequest) -> Result<Vec<u8>, HvError> {
    if req.topic_id != parent.topic_id {
        return Err(HvError::Spec(format!(
            "sister request names topic {:?}, vm is bound to {:?}",
            req.topic_id, parent.topic_id
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
    let got = hex::encode(Sha256::digest(&tar));
    if !got.eq_ignore_ascii_case(req.artifact_digest.trim()) {
        return Err(HvError::Spec(format!(
            "artifact_tar hashes to {got}, request says {}",
            req.artifact_digest
        )));
    }
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

/// Boot a sister for `req`, run it, destroy it, and attest.
///
/// # Errors
///
/// [`HvError::Spec`] (refused before boot), [`HvError::Image`] (sister image
/// missing / mismatched), [`HvError::Backend`] / [`HvError::Guest`] (the
/// host could not run it at all). A run cut at the deadline is **not** an
/// error: it is a result with `timed_out: true`.
pub async fn run(
    ctx: &SisterCtx,
    parent: &BootedVm,
    seq: u64,
    req: &SisterRequest,
) -> Result<(SisterResult, SisterAttestation), HvError> {
    check_request(parent, req)?;
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
    let root = jail::prepare(cfg, ctx.shell.as_ref(), &boot).await?;
    let mut child = jail::spawn(cfg, &id)?;
    let started = Instant::now();
    tracing::info!(sister = %id, parent = %parent.vm_id, topic_id = %parent.topic_id, "sister guest booting (no network)");
    let budget = Duration::from_secs(req.deadline_s).saturating_add(cfg.deadline_grace);
    let outcome = async {
        let mut ch = GuestChannel::connect_within(&root, MINER_PORT, cfg.boot_timeout).await?;
        match ch.recv_within::<MinerToHost>(cfg.boot_timeout).await? {
            MinerToHost::Ready { api_version, .. } => {
                check_version(api_version).map_err(|e| HvError::Guest(e.to_string()))?
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
        ch.recv_within::<MinerToHost>(budget).await
    }
    .await;
    jail::kill(&mut child).await;
    let wall_ms = u64_ms(started.elapsed());
    if let Err(e) = jail::destroy(cfg, ctx.shell.as_ref(), &id).await {
        tracing::warn!(sister = %id, "sister jail cleanup: {e}");
    }
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
    use super::*;
    use proof_vm_proto::guest::StagedFile;

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

    #[test]
    fn sister_ids_fit_the_jailer_and_requests_are_bound_and_hashed() {
        assert_eq!(sister_id("topic-a-0001", 3), "topic-a-0001-s3");
        let long = "t".repeat(63);
        let id = sister_id(&long, 12);
        assert!(id.len() <= MAX_JAIL_ID, "{id}");
        assert!(id.ends_with("-s12"));
        let tar = b"tar bytes";
        check_request(&parent(), &request(tar)).expect("bound + hashed");
        let mut other = request(tar);
        other.topic_id = "topic-b".into();
        assert!(matches!(
            check_request(&parent(), &other),
            Err(HvError::Spec(_))
        ));
        let mut wrong = request(tar);
        wrong.artifact_digest = "00".repeat(32);
        let err = check_request(&parent(), &wrong).expect_err("hash");
        assert!(err.to_string().contains("hashes to"), "{err}");
        let mut empty = request(b"");
        empty.artifact_digest = hex::encode(Sha256::digest(b""));
        assert!(check_request(&parent(), &empty).is_err());
        let mut junk = request(tar);
        junk.artifact_tar.bytes_b64 = "!!".into();
        assert!(check_request(&parent(), &junk).is_err());
        let mut no_entry = request(tar);
        no_entry.entrypoint.clear();
        assert!(check_request(&parent(), &no_entry).is_err());
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
        let shell = Arc::new(crate::shell::RecordingShell::default());
        let ctx = SisterCtx {
            cfg: Arc::new(cfg),
            shell: shell.clone(),
            images: Arc::new(ImageCache::default()),
        };
        let err = run(&ctx, &parent(), 1, &request(b"tar"))
            .await
            .expect_err("no image");
        assert!(matches!(err, HvError::Image(_)), "{err}");
        assert!(
            shell.calls().is_empty(),
            "nothing prepared, nothing spawned"
        );
    }
}
