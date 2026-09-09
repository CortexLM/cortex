//! Agent ↔ guest protocol over Firecracker vsock.
//!
//! Every message is one **frame**: a 4-byte big-endian length followed by
//! that many bytes of JSON ([`write_frame`] / [`read_frame`]). Three
//! channels exist, each on its own vsock port:
//!
//! | Port | Direction | Purpose |
//! |------|-----------|---------|
//! | [`RLM_JOB_PORT`] | host → RLM guest | [`HostToRlm`] / [`RlmToHost`]: hello, secret staging, jobs |
//! | [`SISTER_PORT`] | RLM guest → host | [`SisterRequest`] / [`SisterAnswer`]: "run this artefact in a sister guest" |
//! | [`MINER_PORT`] | host → miner guest | [`HostToMiner`] / [`MinerToHost`]: the run itself |
//!
//! The RLM guest never talks to the miner guest; the host relays the
//! artefact bytes it already inspected and relays the result back. The miner
//! guest has no network interface at all. Guest agents live in the pinned
//! images (`PROOF_RLM_VM_IMAGE_DIGEST`, the agent's miner image), not in this
//! repository; this module is the contract they implement.

use std::collections::BTreeMap;

use base64::Engine;
use proof_rlm::{VmJob, VmJobOutput};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::{ProtoError, API_VERSION};

/// vsock port the RLM guest agent listens on for host jobs.
pub const RLM_JOB_PORT: u32 = 5000;
/// Host-side vsock port the RLM guest connects to for a sister run.
pub const SISTER_PORT: u32 = 5001;
/// vsock port the miner (sister) guest agent listens on.
pub const MINER_PORT: u32 = 5002;
/// Firecracker's guest CID for every guest (the host is always CID 2).
pub const GUEST_CID: u32 = 3;
/// Largest frame either side accepts (artefact tarballs travel inside one).
pub const MAX_FRAME_BYTES: u32 = 256 * 1024 * 1024;

/// One file staged into a guest (owner key material, artefact members, outputs).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StagedFile {
    /// Single path segment or relative path inside the guest's staging dir.
    pub name: String,
    /// Standard base64 of the bytes.
    pub bytes_b64: String,
}

impl StagedFile {
    /// Encode `bytes` under `name`.
    #[must_use]
    pub fn new(name: &str, bytes: &[u8]) -> Self {
        Self {
            name: name.to_owned(),
            bytes_b64: base64::engine::general_purpose::STANDARD.encode(bytes),
        }
    }

    /// Decode the bytes.
    ///
    /// # Errors
    ///
    /// [`ProtoError::Decode`] on bad base64.
    pub fn bytes(&self) -> Result<Vec<u8>, ProtoError> {
        base64::engine::general_purpose::STANDARD
            .decode(&self.bytes_b64)
            .map_err(|e| ProtoError::Decode(format!("{}: {e}", self.name)))
    }
}

/// Host → RLM guest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HostToRlm {
    /// First message after boot: version + the VM's binding.
    Hello {
        /// [`API_VERSION`].
        api_version: u32,
        /// Topic this VM is bound to.
        topic_id: String,
        /// Host VM id.
        vm_id: String,
    },
    /// Owner key material the control plane only ever probed for presence.
    /// The guest keeps it in memory / tmpfs; it never appears in any output.
    StageSecrets {
        /// Files by name.
        files: Vec<StagedFile>,
    },
    /// Run one job. Answered by [`RlmToHost::Done`] or [`RlmToHost::Failed`].
    Run {
        /// The work (public data only).
        job: Box<VmJob>,
    },
}

/// RLM guest → host (answers on [`RLM_JOB_PORT`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RlmToHost {
    /// Answer to `Hello`.
    Ready {
        /// Guest agent name/version (informational).
        agent: String,
        /// [`API_VERSION`] the guest speaks.
        api_version: u32,
    },
    /// Answer to `StageSecrets`.
    Staged {
        /// Files accepted.
        count: usize,
    },
    /// Job finished.
    Done {
        /// The document. Paid outputs are re-stamped by the host.
        output: VmJobOutput,
    },
    /// Job failed inside the guest.
    Failed {
        /// Why (never a secret).
        error: String,
    },
}

/// RLM guest → host on [`SISTER_PORT`]: run a miner artefact in a sister
/// Firecracker guest. The RLM already fetched and inspected the artefact; it
/// ships the bytes so the sister needs no network.
///
/// **Artefact identity is the served file, verbatim** ([`crate::tar`]). The
/// guest fetched `artifact_uri`, ran [`crate::tar::verify_artifact`] on the
/// bytes as received against the job's `artifact_digest`, inspected a copy,
/// and puts **those exact bytes** in `artifact_tar`. It must not re-tar the
/// tree: tar metadata and member order change under re-encoding even when
/// every file is identical, so the host's re-hash would refuse it. A fetch
/// that fails or does not verify is a failed job (`RlmToHost::Failed`),
/// never a substitute artefact: the host refuses a `SisterRequest` whose
/// tar does not hash to the paid digest, is compressed, is not a tar, or
/// carries no file content — a digest that matches an empty tree is not
/// evidence of anything.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SisterRequest {
    /// Must equal the RLM VM's bound topic.
    pub topic_id: String,
    /// Frozen submission digest.
    pub submission_digest: String,
    /// The job's `artifact_digest`: sha256 hex of the file served at
    /// `artifact_uri` — and therefore of `artifact_tar`, which is that file.
    /// The host re-hashes before boot.
    pub artifact_digest: String,
    /// The bytes fetched from `artifact_uri`, **unchanged** (`StagedFile`
    /// named `artifact.tar`): an **uncompressed** tar (ustar / GNU / pax)
    /// with at least one regular file that has content. gzip, non-tar bytes,
    /// an empty archive, a tree of empty files, or bytes that do not hash to
    /// `artifact_digest` (a re-tarred tree) are refused by the host before
    /// any sister jail is built.
    pub artifact_tar: StagedFile,
    /// Command run inside the sister (relative to the unpacked tree).
    pub entrypoint: Vec<String>,
    /// Wall-clock the run is held to.
    pub deadline_s: u64,
    /// Cap the guest enforces on measured FLOPs (the miner's declaration).
    pub declared_flops: u64,
    /// Seed every run uses.
    pub seed: u64,
    /// Opaque topic params (`constraints.params`), exported to the run.
    pub params: BTreeMap<String, String>,
}

/// Host → RLM guest on [`SISTER_PORT`]: the answer to a [`SisterRequest`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SisterAnswer {
    /// The sister booted, ran, and was destroyed.
    Result {
        /// What happened.
        result: SisterResult,
    },
    /// The host refused to boot a sister (bad bind, digest mismatch, no
    /// image, a second sister in the same job, host not ready).
    Refused {
        /// Why (never a secret).
        error: String,
    },
}

/// Host → RLM guest: what happened in the sister.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SisterResult {
    /// Host id of the sister VM.
    pub sister_vm_id: String,
    /// `sha256:` digest of the miner-guest image the host booted.
    pub image_digest: String,
    /// Exit code (`None` = killed at the deadline).
    pub exit_code: Option<i32>,
    /// The deadline cut the run.
    pub timed_out: bool,
    /// Last bytes of stdout/stderr (bounded).
    pub stdout_tail: String,
    /// FLOPs the guest measured (`None` = the guest measured nothing).
    pub flops_used: Option<u64>,
    /// Wall-clock of the run.
    pub wall_ms: u64,
    /// Output files the run left in its output dir (bounded).
    pub outputs: Vec<StagedFile>,
}

/// Host → miner guest on [`MINER_PORT`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HostToMiner {
    /// The run. Answered by [`MinerToHost::Done`] or [`MinerToHost::Failed`].
    Run {
        /// [`API_VERSION`].
        api_version: u32,
        /// The artefact exactly as the host verified it: the file served at
        /// `artifact_uri` (uncompressed tar), same bytes as
        /// [`SisterRequest::artifact_tar`].
        artifact_tar: StagedFile,
        /// Command.
        entrypoint: Vec<String>,
        /// Deadline the guest itself also enforces.
        deadline_s: u64,
        /// FLOP cap.
        declared_flops: u64,
        /// Seed.
        seed: u64,
        /// Opaque params exported to the run's environment.
        params: BTreeMap<String, String>,
    },
}

/// Miner guest → host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MinerToHost {
    /// Sent once after boot.
    Ready {
        /// Guest agent name/version.
        agent: String,
        /// [`API_VERSION`].
        api_version: u32,
    },
    /// The run finished (or was cut by the guest's own deadline).
    Done {
        /// Exit code.
        exit_code: Option<i32>,
        /// Guest deadline hit.
        timed_out: bool,
        /// Bounded tail.
        stdout_tail: String,
        /// Measured FLOPs.
        flops_used: Option<u64>,
        /// Output files (bounded).
        outputs: Vec<StagedFile>,
    },
    /// The guest could not run at all.
    Failed {
        /// Why.
        error: String,
    },
}

/// Refuse a peer that speaks another version.
///
/// # Errors
///
/// [`ProtoError::WrongVersion`].
pub fn check_version(got: u32) -> Result<(), ProtoError> {
    if got == API_VERSION {
        Ok(())
    } else {
        Err(ProtoError::WrongVersion { got })
    }
}

/// Encode one frame.
///
/// # Errors
///
/// [`ProtoError::Decode`] when the value does not serialise,
/// [`ProtoError::FrameTooLarge`] over the cap.
pub fn encode_frame<T: Serialize>(value: &T) -> Result<Vec<u8>, ProtoError> {
    let body = serde_json::to_vec(value).map_err(|e| ProtoError::Decode(e.to_string()))?;
    let len = u32::try_from(body.len()).map_err(|_| ProtoError::FrameTooLarge(u32::MAX))?;
    if len > MAX_FRAME_BYTES {
        return Err(ProtoError::FrameTooLarge(len));
    }
    let mut out = Vec::with_capacity(body.len().saturating_add(4));
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

/// Write one frame.
///
/// # Errors
///
/// See [`encode_frame`]; [`ProtoError::Io`] on the channel.
pub async fn write_frame<W: AsyncWrite + Unpin, T: Serialize>(
    w: &mut W,
    value: &T,
) -> Result<(), ProtoError> {
    let bytes = encode_frame(value)?;
    w.write_all(&bytes)
        .await
        .map_err(|e| ProtoError::Io(e.to_string()))?;
    w.flush().await.map_err(|e| ProtoError::Io(e.to_string()))
}

/// Read one frame.
///
/// # Errors
///
/// [`ProtoError::FrameTooLarge`], [`ProtoError::Decode`], [`ProtoError::Io`].
pub async fn read_frame<R: AsyncRead + Unpin, T: for<'de> Deserialize<'de>>(
    r: &mut R,
) -> Result<T, ProtoError> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)
        .await
        .map_err(|e| ProtoError::Io(e.to_string()))?;
    let len = u32::from_be_bytes(len);
    if len > MAX_FRAME_BYTES {
        return Err(ProtoError::FrameTooLarge(len));
    }
    let mut body = vec![0u8; len as usize];
    r.read_exact(&mut body)
        .await
        .map_err(|e| ProtoError::Io(e.to_string()))?;
    serde_json::from_slice(&body).map_err(|e| ProtoError::Decode(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proof_rlm::fixtures::request;

    #[tokio::test]
    async fn frames_round_trip_and_oversized_frames_refuse() {
        let msg = HostToRlm::Hello {
            api_version: API_VERSION,
            topic_id: "topic-a".into(),
            vm_id: "vm-0".into(),
        };
        let bytes = encode_frame(&msg).expect("encode");
        let body_len = u32::try_from(bytes.len() - 4).expect("fits");
        assert_eq!(&bytes[..4], &body_len.to_be_bytes());
        let mut cursor = std::io::Cursor::new(bytes);
        let back: HostToRlm = read_frame(&mut cursor).await.expect("decode");
        assert_eq!(back, msg);

        let (mut a, mut b) = tokio::io::duplex(1 << 16);
        let job = HostToRlm::Run {
            job: Box::new(VmJob::Archive {
                topic_id: "topic-a".into(),
            }),
        };
        write_frame(&mut a, &job).await.expect("write");
        let back: HostToRlm = read_frame(&mut b).await.expect("read");
        assert_eq!(back, job);

        let mut huge = std::io::Cursor::new((MAX_FRAME_BYTES + 1).to_be_bytes().to_vec());
        let err = read_frame::<_, HostToRlm>(&mut huge)
            .await
            .expect_err("too large");
        assert!(matches!(err, ProtoError::FrameTooLarge(_)), "{err}");
        assert!(check_version(API_VERSION).is_ok());
        assert_eq!(check_version(2), Err(ProtoError::WrongVersion { got: 2 }));
    }

    #[test]
    fn staged_files_round_trip_and_sister_documents_are_public() {
        let f = StagedFile::new("artifact.tar", b"\x00\x01binary");
        assert_eq!(f.bytes().expect("decode"), b"\x00\x01binary");
        let mut bad = f.clone();
        bad.bytes_b64 = "!!".into();
        assert!(bad.bytes().is_err());
        let req = request();
        let sister = SisterRequest {
            topic_id: req.topic_id.clone(),
            submission_digest: req.submission_digest.clone(),
            artifact_digest: req.artifact_digest.clone(),
            artifact_tar: f,
            entrypoint: vec!["./run.sh".into()],
            deadline_s: req.sandbox.deadline_s,
            declared_flops: req.declared_flops,
            seed: req.seed,
            params: req.constraints.params.clone(),
        };
        let json = serde_json::to_string(&sister).expect("json");
        for forbidden in ["/run/base", "api_key", "127.0.0.1", "base_url"] {
            assert!(!json.contains(forbidden), "{json}");
        }
        let back: SisterRequest = serde_json::from_str(&json).expect("round trip");
        assert_eq!(back, sister);
        let done = MinerToHost::Done {
            exit_code: Some(0),
            timed_out: false,
            stdout_tail: "ok".into(),
            flops_used: Some(7),
            outputs: vec![],
        };
        assert!(serde_json::to_string(&done)
            .expect("json")
            .contains("\"type\":\"done\""));
        assert_eq!(RLM_JOB_PORT, 5000);
        assert_eq!(SISTER_PORT, 5001);
        assert_eq!(MINER_PORT, 5002);
    }
}
