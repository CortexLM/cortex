//! In-guest experiment binding for Proof topic VMs.
//!
//! Proof is a generalist challenge: the control plane runs whatever a signed
//! topic describes and never compiles a benchmark, a dataset, a model, or a
//! task list in. Some topics want their paid runs (`Baseline`, `Evaluate`)
//! executed **inside a dedicated Firecracker microVM per experiment** by an
//! operator-installed backend (a container runtime + a benchmark harness
//! baked into the guest image). This crate is the generic contract for that:
//!
//! - the well-known `constraints.params` keys a topic uses to **select** such
//!   a backend ([`PARAM_RUNNER`]), **pin** the experiment pack it runs against
//!   ([`PARAM_PACK_DIGEST`], optional [`PARAM_PACK_PATH`]), and **size** the
//!   VM ([`PARAM_VCPUS`], [`PARAM_MEM_MIB`], [`PARAM_DISK_MIB`]);
//! - the operator **defaults and ceilings** every experiment VM is held to
//!   ([`ExperimentCeilings`]; Architecte lock: **16 vCPU / 32 GiB RAM** —
//!   what a silent topic gets, the most a topic may ask for, **and a hard
//!   maximum no operator ceiling may exceed** ([`LOCK_MAX_EXPERIMENT_VCPUS`],
//!   [`LOCK_MAX_EXPERIMENT_MEM_MIB`]: an operator may lower a ceiling for a
//!   smaller host, never raise it above the lock) — writable disk
//!   **≥ 16 GiB**, 32 GiB by default) and the control-plane policy env
//!   ([`ExperimentPolicy`]);
//! - the public wire shape the orchestrator carries for one experiment VM
//!   ([`ExperimentSpec`]).
//!
//! Everything here is a **shape** or a **ceiling**. The runner id, the pack
//! digest, the model pin, and the sizes are topic data read from the signed
//! document; a topic that names none of them selects no in-guest backend and
//! keeps whatever runner its `custom_id` is registered with. A topic that
//! selects a runner without pinning a pack digest fails closed (no run,
//! **503**) — a pack is never defaulted, and a digest is never invented.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions, clippy::must_use_candidate)]

use std::collections::BTreeMap;

use proof_canon::{is_custom_id, is_hex64};
use serde::{Deserialize, Serialize};

/// `constraints.params` key selecting the in-guest runner (an operator
/// adaptor id installed in the guest image, `[a-z0-9][a-z0-9_-]{1,63}`).
pub const PARAM_RUNNER: &str = "in_guest_benchmark_runner";
/// Accepted alias of [`PARAM_RUNNER`] (the key the first topics used).
pub const PARAM_RUNNER_ALIAS: &str = "baseline_runner";
/// `constraints.params` key: relative locator of the pack file under the KVM
/// host's pack directory. Optional; default `sha256-<hex>.tar`.
pub const PARAM_PACK_PATH: &str = "experiment_pack_path";
/// `constraints.params` key: `sha256:<hex>` of the experiment pack — the
/// exact bytes of an uncompressed tar the host stages into the VM. Required
/// whenever a runner is selected.
pub const PARAM_PACK_DIGEST: &str = "experiment_pack_digest";
/// `constraints.params` key: vCPUs the experiment VM asks for (≤ ceiling).
pub const PARAM_VCPUS: &str = "experiment_vcpus";
/// `constraints.params` key: guest memory in MiB (≤ ceiling).
pub const PARAM_MEM_MIB: &str = "experiment_mem_mib";
/// `constraints.params` key: writable disk in MiB (≤ ceiling).
pub const PARAM_DISK_MIB: &str = "experiment_disk_mib";

/// Architecte lock: the **hard** vCPU maximum of one experiment VM. Not an
/// operator knob — [`ExperimentCeilings::validate`] refuses any ceiling
/// above it, on the control plane and on the KVM host alike.
pub const LOCK_MAX_EXPERIMENT_VCPUS: u32 = 16;
/// Architecte lock: the **hard** memory maximum of one experiment VM in MiB
/// (32 GiB). Not an operator knob; see [`LOCK_MAX_EXPERIMENT_VCPUS`].
pub const LOCK_MAX_EXPERIMENT_MEM_MIB: u32 = 32_768;
/// Architecte lock: vCPUs an experiment VM gets when the topic does not ask.
pub const DEFAULT_EXPERIMENT_VCPUS: u32 = LOCK_MAX_EXPERIMENT_VCPUS;
/// Architecte lock: guest memory when the topic does not ask (32 GiB).
pub const DEFAULT_EXPERIMENT_MEM_MIB: u32 = LOCK_MAX_EXPERIMENT_MEM_MIB;
/// Architecte lock: most vCPUs a topic may ask for (the default ceiling; an
/// operator may set a lower one for a smaller host, never a higher one).
pub const DEFAULT_MAX_EXPERIMENT_VCPUS: u32 = LOCK_MAX_EXPERIMENT_VCPUS;
/// Architecte lock: most guest memory a topic may ask for (32 GiB; the
/// default ceiling — lower is an operator choice, higher is refused).
pub const DEFAULT_MAX_EXPERIMENT_MEM_MIB: u32 = LOCK_MAX_EXPERIMENT_MEM_MIB;
/// Writable disk an experiment VM gets when the topic does not ask (32 GiB,
/// the preferred size when the metal allows: container image pulls land
/// here, never on the read-only rootfs).
pub const DEFAULT_EXPERIMENT_DISK_MIB: u32 = 32_768;
/// Largest writable disk a topic may ask for by default (32 GiB; the
/// operator raises it when the metal has more).
pub const DEFAULT_MAX_EXPERIMENT_DISK_MIB: u32 = 32_768;
/// Architecte lock: smallest writable disk an experiment VM may have (16 GiB).
pub const MIN_EXPERIMENT_DISK_MIB: u32 = 16_384;

/// Control-plane env: `sha256:` digest of the guest image experiment VMs
/// boot. Unset = the RLM image pin (`PROOF_RLM_VM_IMAGE_DIGEST`).
pub const EXPERIMENT_VM_IMAGE_DIGEST_ENV: &str = "PROOF_EXPERIMENT_VM_IMAGE_DIGEST";
/// Control-plane env: vCPUs when the topic does not ask (default [`DEFAULT_EXPERIMENT_VCPUS`]).
pub const EXPERIMENT_VM_VCPUS_ENV: &str = "PROOF_EXPERIMENT_VM_VCPUS";
/// Control-plane env: memory in MiB when the topic does not ask (default [`DEFAULT_EXPERIMENT_MEM_MIB`]).
pub const EXPERIMENT_VM_MEM_MIB_ENV: &str = "PROOF_EXPERIMENT_VM_MEM_MIB";
/// Control-plane env: vCPU ceiling (default [`DEFAULT_MAX_EXPERIMENT_VCPUS`]).
pub const EXPERIMENT_VM_MAX_VCPUS_ENV: &str = "PROOF_EXPERIMENT_VM_MAX_VCPUS";
/// Control-plane env: memory ceiling in MiB (default [`DEFAULT_MAX_EXPERIMENT_MEM_MIB`]).
pub const EXPERIMENT_VM_MAX_MEM_MIB_ENV: &str = "PROOF_EXPERIMENT_VM_MAX_MEM_MIB";
/// Control-plane env: writable disk in MiB when the topic does not ask
/// (default [`DEFAULT_EXPERIMENT_DISK_MIB`]).
pub const EXPERIMENT_VM_DISK_MIB_ENV: &str = "PROOF_EXPERIMENT_VM_DISK_MIB";
/// Control-plane env: writable disk ceiling in MiB (default [`DEFAULT_MAX_EXPERIMENT_DISK_MIB`]).
pub const EXPERIMENT_VM_MAX_DISK_MIB_ENV: &str = "PROOF_EXPERIMENT_VM_MAX_DISK_MIB";

/// What an operator may do about an ask over a ceiling (both sides enforce
/// their own copy; neither may sit above the lock).
const CEILING_HINT: &str = "re-sign the topic with a smaller ask; an operator ceiling (PROOF_EXPERIMENT_VM_MAX_* on the control plane, PROOF_VM_AGENT_EXPERIMENT_MAX_* on the KVM host) may sit below the lock of 16 vCPU / 32768 MiB, never above it";

/// Why a binding, spec, or ceiling is not usable.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ExperimentError {
    /// The runner param is not an identifier.
    #[error("constraints.params.{key} {value:?} is not a runner id ([a-z0-9][a-z0-9_-]{{1,63}})")]
    BadRunner {
        /// Which key carried it.
        key: String,
        /// What it said.
        value: String,
    },
    /// A runner is selected but no pack digest pins what it runs against.
    #[error("constraints.params.{PARAM_PACK_DIGEST} is required when constraints.params.{PARAM_RUNNER} selects an in-guest runner (sha256:<64 hex> of the pack tar staged on the KVM host; never invented, never defaulted)")]
    PackDigestMissing,
    /// The pack digest is not `sha256:<64 hex>`.
    #[error("constraints.params.{PARAM_PACK_DIGEST} {0:?} is not sha256:<64 hex>")]
    BadPackDigest(String),
    /// The pack path escapes the host pack directory or is not a plain path.
    #[error("constraints.params.{PARAM_PACK_PATH} {0:?} must be a relative path of plain segments (no leading '/', no '..', no empty segment)")]
    BadPackPath(String),
    /// A size param did not parse.
    #[error("constraints.params.{key} {value:?} is not a positive integer")]
    BadNumber {
        /// Which key.
        key: String,
        /// What it said.
        value: String,
    },
    /// The topic asks for more than the operator allows.
    #[error("experiment {field} {asked} exceeds the ceiling {ceiling}; {CEILING_HINT}")]
    OverCeiling {
        /// `vcpus`, `mem_mib`, or `disk_mib`.
        field: &'static str,
        /// What the topic asked for.
        asked: u32,
        /// The operator ceiling in force.
        ceiling: u32,
    },
    /// A size is below what a VM can boot with.
    #[error("experiment {field} {asked} is below the minimum {min}")]
    UnderFloor {
        /// Which size.
        field: &'static str,
        /// What was asked.
        asked: u32,
        /// The floor.
        min: u32,
    },
    /// An operator env value is not a number in range.
    #[error("{env} is not a number in range: {value:?}")]
    BadEnv {
        /// Which variable.
        env: &'static str,
        /// What it said.
        value: String,
    },
    /// A wire spec field is malformed.
    #[error("experiment spec: {0} is missing or out of range")]
    Spec(&'static str),
    /// An operator ceiling (or a VM shape) sits above the hard lock.
    #[error("experiment {field} {value} is above the lock {lock} (16 vCPU / 32768 MiB per experiment VM; the lock is not an operator knob — lower the ceiling or the ask)")]
    AboveLock {
        /// `max_vcpus`, `max_mem_mib`, `vcpus`, or `mem_mib`.
        field: &'static str,
        /// The value refused.
        value: u32,
        /// The lock it exceeds.
        lock: u32,
    },
}

/// The experiment pack one topic pins: an uncompressed tar the KVM host
/// stages into the experiment VM, identified by the sha256 of its bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackRef {
    /// Relative locator under the host's pack directory. `None` = the
    /// digest-named file `sha256-<hex>.tar`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// `sha256:<hex>` (lower-case) of the pack tar.
    pub digest: String,
}

/// A relative path of plain segments: no leading `/`, no `.` / `..`, no
/// empty segment, no control characters.
fn is_plain_relative_path(p: &str) -> bool {
    let p = p.trim();
    !p.is_empty()
        && !p.starts_with('/')
        && !p.chars().any(|c| c.is_control() || c == '\\')
        && p.split('/')
            .all(|seg| !seg.is_empty() && seg != "." && seg != "..")
}

impl PackRef {
    /// Build from the topic's params: the digest is required and normalised,
    /// the path optional and shape-checked.
    ///
    /// # Errors
    ///
    /// [`ExperimentError::PackDigestMissing`] / [`ExperimentError::BadPackDigest`] /
    /// [`ExperimentError::BadPackPath`].
    pub fn from_params(params: &BTreeMap<String, String>) -> Result<Self, ExperimentError> {
        let raw = params
            .get(PARAM_PACK_DIGEST)
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .ok_or(ExperimentError::PackDigestMissing)?;
        let hex = raw
            .strip_prefix("sha256:")
            .filter(|h| is_hex64(h))
            .ok_or_else(|| ExperimentError::BadPackDigest(raw.to_owned()))?;
        let path = match params.get(PARAM_PACK_PATH).map(|s| s.trim()) {
            None | Some("") => None,
            Some(p) if is_plain_relative_path(p) => Some(p.to_owned()),
            Some(p) => return Err(ExperimentError::BadPackPath(p.to_owned())),
        };
        Ok(Self {
            path,
            digest: format!("sha256:{}", hex.trim().to_ascii_lowercase()),
        })
    }

    /// The 64 hex chars of the digest, if well-formed.
    pub fn hex(&self) -> Option<&str> {
        self.digest
            .trim()
            .strip_prefix("sha256:")
            .filter(|h| is_hex64(h))
            .map(str::trim)
    }

    /// File name / relative path under the host pack directory.
    pub fn locator(&self) -> String {
        match &self.path {
            Some(p) => p.clone(),
            None => format!("sha256-{}.tar", self.hex().unwrap_or("")),
        }
    }

    /// Shape check.
    ///
    /// # Errors
    ///
    /// [`ExperimentError::BadPackDigest`] / [`ExperimentError::BadPackPath`].
    pub fn validate(&self) -> Result<(), ExperimentError> {
        if self.hex().is_none() {
            return Err(ExperimentError::BadPackDigest(self.digest.clone()));
        }
        if let Some(p) = &self.path {
            if !is_plain_relative_path(p) {
                return Err(ExperimentError::BadPackPath(p.clone()));
            }
        }
        Ok(())
    }
}

/// What a signed topic says about running its paid jobs in a dedicated VM.
/// Built from `constraints.params` only; `None` when the topic selects no
/// runner (it keeps its registered custom runner's ordinary path).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExperimentBinding {
    /// Operator adaptor id the guest resolves (never interpreted here).
    pub runner: String,
    /// Pinned experiment pack.
    pub pack: PackRef,
    /// vCPUs the topic asks for (`None` = the operator default).
    pub vcpus: Option<u32>,
    /// Memory the topic asks for (`None` = the operator default).
    pub mem_mib: Option<u32>,
    /// Writable disk the topic asks for (`None` = the operator default).
    pub disk_mib: Option<u32>,
}

fn number(params: &BTreeMap<String, String>, key: &str) -> Result<Option<u32>, ExperimentError> {
    match params.get(key).map(|s| s.trim()) {
        None | Some("") => Ok(None),
        Some(raw) => raw
            .parse::<u32>()
            .ok()
            .filter(|n| *n > 0)
            .map(Some)
            .ok_or_else(|| ExperimentError::BadNumber {
                key: key.to_owned(),
                value: raw.to_owned(),
            }),
    }
}

impl ExperimentBinding {
    /// Read the binding from a topic's `constraints.params`.
    ///
    /// # Errors
    ///
    /// [`ExperimentError`] naming the first bad knob. A malformed binding is
    /// refused, never ignored: a topic that half-selects a backend does not
    /// silently run on another path.
    pub fn from_params(params: &BTreeMap<String, String>) -> Result<Option<Self>, ExperimentError> {
        let selected = [PARAM_RUNNER, PARAM_RUNNER_ALIAS]
            .into_iter()
            .find_map(|key| params.get(key).map(|v| (key, v.trim())));
        let Some((key, runner)) = selected else {
            return Ok(None);
        };
        if !is_custom_id(runner) {
            return Err(ExperimentError::BadRunner {
                key: key.to_owned(),
                value: runner.to_owned(),
            });
        }
        Ok(Some(Self {
            runner: runner.to_owned(),
            pack: PackRef::from_params(params)?,
            vcpus: number(params, PARAM_VCPUS)?,
            mem_mib: number(params, PARAM_MEM_MIB)?,
            disk_mib: number(params, PARAM_DISK_MIB)?,
        }))
    }
}

/// Public wire shape of one experiment VM (carried by the topic-VM spec):
/// what the host stages and how big the writable disk is. Sizes for CPU and
/// memory travel in the VM template beside it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExperimentSpec {
    /// Operator adaptor id the guest resolves.
    pub runner: String,
    /// Pack the host verifies and stages before the first job.
    pub pack: PackRef,
    /// Writable scratch disk in MiB.
    pub disk_mib: u32,
}

impl ExperimentSpec {
    /// Shape check (runner id, pack, disk floor).
    ///
    /// # Errors
    ///
    /// [`ExperimentError::Spec`] naming the field.
    pub fn validate(&self) -> Result<(), ExperimentError> {
        if !is_custom_id(self.runner.trim()) {
            return Err(ExperimentError::Spec("runner"));
        }
        self.pack
            .validate()
            .map_err(|_| ExperimentError::Spec("pack"))?;
        if self.disk_mib < MIN_EXPERIMENT_DISK_MIB {
            return Err(ExperimentError::Spec("disk_mib"));
        }
        Ok(())
    }
}

/// The resolved size of one experiment VM.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VmShape {
    /// vCPUs.
    pub vcpus: u32,
    /// Guest memory in MiB.
    pub mem_mib: u32,
    /// Writable disk in MiB.
    pub disk_mib: u32,
}

/// Operator defaults and ceilings every experiment VM is held to. The
/// control plane sizes a VM under its own copy (a silent topic gets the
/// defaults); the KVM host refuses a spec over its copy. A topic may ask for
/// less than a ceiling or more than a default, never more than a ceiling —
/// and no ceiling may sit above the lock ([`LOCK_MAX_EXPERIMENT_VCPUS`] /
/// [`LOCK_MAX_EXPERIMENT_MEM_MIB`]): `validate` refuses one, so a process
/// configured above the lock does not boot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExperimentCeilings {
    /// vCPUs when the topic does not ask.
    pub default_vcpus: u32,
    /// Most vCPUs a topic may ask for (≤ [`LOCK_MAX_EXPERIMENT_VCPUS`]).
    pub max_vcpus: u32,
    /// Memory (MiB) when the topic does not ask.
    pub default_mem_mib: u32,
    /// Most memory (MiB) a topic may ask for (≤ [`LOCK_MAX_EXPERIMENT_MEM_MIB`]).
    pub max_mem_mib: u32,
    /// Writable disk (MiB) when the topic does not ask.
    pub default_disk_mib: u32,
    /// Most writable disk (MiB).
    pub max_disk_mib: u32,
}

impl Default for ExperimentCeilings {
    fn default() -> Self {
        Self {
            default_vcpus: DEFAULT_EXPERIMENT_VCPUS,
            max_vcpus: DEFAULT_MAX_EXPERIMENT_VCPUS,
            default_mem_mib: DEFAULT_EXPERIMENT_MEM_MIB,
            max_mem_mib: DEFAULT_MAX_EXPERIMENT_MEM_MIB,
            default_disk_mib: DEFAULT_EXPERIMENT_DISK_MIB,
            max_disk_mib: DEFAULT_MAX_EXPERIMENT_DISK_MIB,
        }
    }
}

fn under(field: &'static str, asked: u32, ceiling: u32) -> Result<u32, ExperimentError> {
    if asked > ceiling {
        Err(ExperimentError::OverCeiling {
            field,
            asked,
            ceiling,
        })
    } else {
        Ok(asked)
    }
}

fn over(field: &'static str, asked: u32, min: u32) -> Result<u32, ExperimentError> {
    if asked < min {
        Err(ExperimentError::UnderFloor { field, asked, min })
    } else {
        Ok(asked)
    }
}

fn locked(field: &'static str, value: u32, lock: u32) -> Result<u32, ExperimentError> {
    if value > lock {
        Err(ExperimentError::AboveLock { field, value, lock })
    } else {
        Ok(value)
    }
}

impl ExperimentCeilings {
    /// Ranges a Firecracker guest can boot with; no ceiling above the lock;
    /// every default under its ceiling; the disk floor.
    ///
    /// # Errors
    ///
    /// [`ExperimentError::AboveLock`] for a vCPU / memory ceiling above the
    /// lock, [`ExperimentError::Spec`] naming any other bad field.
    pub fn validate(&self) -> Result<(), ExperimentError> {
        locked("max_vcpus", self.max_vcpus, LOCK_MAX_EXPERIMENT_VCPUS)?;
        locked("max_mem_mib", self.max_mem_mib, LOCK_MAX_EXPERIMENT_MEM_MIB)?;
        if self.max_vcpus == 0 {
            return Err(ExperimentError::Spec("max_vcpus"));
        }
        if !(1..=self.max_vcpus).contains(&self.default_vcpus) {
            return Err(ExperimentError::Spec("default_vcpus"));
        }
        if self.max_mem_mib < 512 {
            return Err(ExperimentError::Spec("max_mem_mib"));
        }
        if !(512..=self.max_mem_mib).contains(&self.default_mem_mib) {
            return Err(ExperimentError::Spec("default_mem_mib"));
        }
        if self.max_disk_mib < MIN_EXPERIMENT_DISK_MIB {
            return Err(ExperimentError::Spec("max_disk_mib"));
        }
        if !(MIN_EXPERIMENT_DISK_MIB..=self.max_disk_mib).contains(&self.default_disk_mib) {
            return Err(ExperimentError::Spec("default_disk_mib"));
        }
        Ok(())
    }

    /// Size a VM for `binding`: what the topic asks for, held under the
    /// ceilings (which sit under the lock); a topic that does not ask gets
    /// the operator defaults (lock: 16 vCPU / 32 GiB / 32 GiB disk).
    ///
    /// # Errors
    ///
    /// [`ExperimentError::OverCeiling`] / [`ExperimentError::UnderFloor`] —
    /// never a silent clamp: a topic that asks for more than the host allows
    /// does not run on a smaller machine than it was signed for.
    pub fn shape(&self, binding: &ExperimentBinding) -> Result<VmShape, ExperimentError> {
        self.validate()?;
        let shape = VmShape {
            vcpus: under(
                "vcpus",
                binding.vcpus.unwrap_or(self.default_vcpus),
                self.max_vcpus,
            )?,
            mem_mib: over(
                "mem_mib",
                under(
                    "mem_mib",
                    binding.mem_mib.unwrap_or(self.default_mem_mib),
                    self.max_mem_mib,
                )?,
                512,
            )?,
            disk_mib: over(
                "disk_mib",
                under(
                    "disk_mib",
                    binding.disk_mib.unwrap_or(self.default_disk_mib),
                    self.max_disk_mib,
                )?,
                MIN_EXPERIMENT_DISK_MIB,
            )?,
        };
        shape.locked()?;
        Ok(shape)
    }

    /// Host-side gate: refuse a spec sized over this host's ceilings (or,
    /// whatever the ceilings say, over the lock).
    ///
    /// # Errors
    ///
    /// [`ExperimentError::OverCeiling`] / [`ExperimentError::UnderFloor`] /
    /// [`ExperimentError::AboveLock`].
    pub fn admit(&self, shape: VmShape) -> Result<(), ExperimentError> {
        self.validate()?;
        shape.locked()?;
        under("vcpus", shape.vcpus, self.max_vcpus)?;
        under("mem_mib", shape.mem_mib, self.max_mem_mib)?;
        under("disk_mib", shape.disk_mib, self.max_disk_mib)?;
        over("disk_mib", shape.disk_mib, MIN_EXPERIMENT_DISK_MIB)?;
        Ok(())
    }
}

impl VmShape {
    /// The lock, independent of any ceiling: no experiment VM shape above
    /// 16 vCPU / 32768 MiB is ever admitted or created.
    ///
    /// # Errors
    ///
    /// [`ExperimentError::AboveLock`].
    pub fn locked(&self) -> Result<(), ExperimentError> {
        locked("vcpus", self.vcpus, LOCK_MAX_EXPERIMENT_VCPUS)?;
        locked("mem_mib", self.mem_mib, LOCK_MAX_EXPERIMENT_MEM_MIB)?;
        Ok(())
    }
}

/// Control-plane policy for experiment VMs: ceilings plus the image they boot.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ExperimentPolicy {
    /// Ceilings the control plane sizes under.
    pub ceilings: ExperimentCeilings,
    /// `sha256:` digest of the experiment guest image. `None` = the RLM
    /// image pin (one enlarged image serves topic VMs and experiment VMs).
    pub image_digest: Option<String>,
}

fn env_trimmed(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

fn env_u32(name: &'static str, default: u32) -> Result<u32, ExperimentError> {
    match env_trimmed(name) {
        None => Ok(default),
        Some(raw) => raw.parse::<u32>().map_err(|_| ExperimentError::BadEnv {
            env: name,
            value: raw,
        }),
    }
}

impl ExperimentPolicy {
    /// Read `PROOF_EXPERIMENT_VM_*`; unset knobs take the lock defaults.
    ///
    /// # Errors
    ///
    /// [`ExperimentError::BadEnv`] for a value that is not a number,
    /// [`ExperimentError::Spec`] for ceilings out of range.
    pub fn from_env() -> Result<Self, ExperimentError> {
        let ceilings = ExperimentCeilings {
            default_vcpus: env_u32(EXPERIMENT_VM_VCPUS_ENV, DEFAULT_EXPERIMENT_VCPUS)?,
            max_vcpus: env_u32(EXPERIMENT_VM_MAX_VCPUS_ENV, DEFAULT_MAX_EXPERIMENT_VCPUS)?,
            default_mem_mib: env_u32(EXPERIMENT_VM_MEM_MIB_ENV, DEFAULT_EXPERIMENT_MEM_MIB)?,
            max_mem_mib: env_u32(
                EXPERIMENT_VM_MAX_MEM_MIB_ENV,
                DEFAULT_MAX_EXPERIMENT_MEM_MIB,
            )?,
            default_disk_mib: env_u32(EXPERIMENT_VM_DISK_MIB_ENV, DEFAULT_EXPERIMENT_DISK_MIB)?,
            max_disk_mib: env_u32(
                EXPERIMENT_VM_MAX_DISK_MIB_ENV,
                DEFAULT_MAX_EXPERIMENT_DISK_MIB,
            )?,
        };
        ceilings.validate()?;
        let image_digest = env_trimmed(EXPERIMENT_VM_IMAGE_DIGEST_ENV);
        if let Some(d) = &image_digest {
            if !d.strip_prefix("sha256:").is_some_and(is_hex64) {
                return Err(ExperimentError::BadEnv {
                    env: EXPERIMENT_VM_IMAGE_DIGEST_ENV,
                    value: d.clone(),
                });
            }
        }
        Ok(Self {
            ceilings,
            image_digest,
        })
    }

    /// The image digest experiment VMs boot: this policy's, else `rlm_image`.
    pub fn image_for<'a>(&'a self, rlm_image: &'a str) -> &'a str {
        self.image_digest.as_deref().unwrap_or(rlm_image)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    const HEX: &str = "0d00287acf3febe107cf938a57a18ef24928a0690a673c8b53ff6ba2ea59eda4";

    /// A topic that names no runner selects nothing; one that names a runner
    /// must pin a pack digest, and every knob is shape-checked, never guessed.
    #[test]
    #[allow(clippy::too_many_lines)]
    fn the_binding_is_topic_data_and_fails_closed_on_a_half_selection() {
        assert_eq!(
            ExperimentBinding::from_params(&params(&[("param_a", "value-a")])),
            Ok(None),
            "no runner param = no in-guest backend"
        );
        let full = params(&[
            (PARAM_RUNNER, "operator_adaptor_v0"),
            (
                PARAM_PACK_DIGEST,
                &format!(" sha256:{} ", HEX.to_ascii_uppercase()),
            ),
            (PARAM_PACK_PATH, "packs/first-slice.tar"),
            (PARAM_VCPUS, "8"),
            (PARAM_MEM_MIB, "16384"),
        ]);
        let b = ExperimentBinding::from_params(&full)
            .expect("well-formed")
            .expect("selected");
        assert_eq!(b.runner, "operator_adaptor_v0");
        assert_eq!(b.pack.digest, format!("sha256:{HEX}"), "normalised");
        assert_eq!(b.pack.hex(), Some(HEX));
        assert_eq!(b.pack.path.as_deref(), Some("packs/first-slice.tar"));
        assert_eq!(b.pack.locator(), "packs/first-slice.tar");
        assert_eq!(
            (b.vcpus, b.mem_mib, b.disk_mib),
            (Some(8), Some(16_384), None)
        );

        let alias = params(&[
            (PARAM_RUNNER_ALIAS, "operator_adaptor_v0"),
            (PARAM_PACK_DIGEST, &format!("sha256:{HEX}")),
        ]);
        let b = ExperimentBinding::from_params(&alias)
            .expect("alias key")
            .expect("selected");
        assert_eq!(b.pack.locator(), format!("sha256-{HEX}.tar"));
        assert_eq!(b.pack.path, None);

        let no_pack = params(&[(PARAM_RUNNER, "operator_adaptor_v0")]);
        assert_eq!(
            ExperimentBinding::from_params(&no_pack),
            Err(ExperimentError::PackDigestMissing)
        );
        let msg = ExperimentError::PackDigestMissing.to_string();
        assert!(
            msg.contains(PARAM_PACK_DIGEST) && msg.contains("never invented"),
            "{msg}"
        );

        let bad_runner = params(&[
            (PARAM_RUNNER, "Not An Id"),
            (PARAM_PACK_DIGEST, &format!("sha256:{HEX}")),
        ]);
        assert!(matches!(
            ExperimentBinding::from_params(&bad_runner),
            Err(ExperimentError::BadRunner { .. })
        ));
        for bad_digest in ["abc", HEX, "sha256:zz", ""] {
            let p = params(&[
                (PARAM_RUNNER, "operator_adaptor_v0"),
                (PARAM_PACK_DIGEST, bad_digest),
            ]);
            let err = ExperimentBinding::from_params(&p).expect_err(bad_digest);
            assert!(
                matches!(
                    err,
                    ExperimentError::BadPackDigest(_) | ExperimentError::PackDigestMissing
                ),
                "{bad_digest:?}: {err}"
            );
        }
        for bad_path in [
            "/abs/pack.tar",
            "../escape.tar",
            "a//b",
            "a/./b",
            "back\\slash",
        ] {
            let p = params(&[
                (PARAM_RUNNER, "operator_adaptor_v0"),
                (PARAM_PACK_DIGEST, &format!("sha256:{HEX}")),
                (PARAM_PACK_PATH, bad_path),
            ]);
            assert!(
                matches!(
                    ExperimentBinding::from_params(&p),
                    Err(ExperimentError::BadPackPath(_))
                ),
                "{bad_path}"
            );
        }
        let bad_num = params(&[
            (PARAM_RUNNER, "operator_adaptor_v0"),
            (PARAM_PACK_DIGEST, &format!("sha256:{HEX}")),
            (PARAM_VCPUS, "many"),
        ]);
        assert!(matches!(
            ExperimentBinding::from_params(&bad_num),
            Err(ExperimentError::BadNumber { .. })
        ));
        let zero = params(&[
            (PARAM_RUNNER, "operator_adaptor_v0"),
            (PARAM_PACK_DIGEST, &format!("sha256:{HEX}")),
            (PARAM_DISK_MIB, "0"),
        ]);
        assert!(ExperimentBinding::from_params(&zero).is_err());
    }

    /// The lock: a silent topic gets 16 vCPU / 32 GiB / 32 GiB disk, which
    /// is also the most a topic may ask for; disk never under 16 GiB. An ask
    /// over a ceiling is refused with the ceiling named, never clamped.
    #[test]
    #[allow(clippy::too_many_lines)]
    fn ceilings_are_the_lock_and_asks_are_held_under_them() {
        let c = ExperimentCeilings::default();
        c.validate().expect("lock validates");
        assert_eq!((c.default_vcpus, c.default_mem_mib), (16, 32_768));
        assert_eq!((c.max_vcpus, c.max_mem_mib), (16, 32_768));
        assert_eq!((c.default_disk_mib, c.max_disk_mib), (32_768, 32_768));
        assert_eq!(MIN_EXPERIMENT_DISK_MIB, 16_384);
        let silent = ExperimentBinding {
            runner: "r_0".into(),
            pack: PackRef {
                path: None,
                digest: format!("sha256:{HEX}"),
            },
            vcpus: None,
            mem_mib: None,
            disk_mib: None,
        };
        assert_eq!(
            c.shape(&silent).expect("shape"),
            VmShape {
                vcpus: 16,
                mem_mib: 32_768,
                disk_mib: 32_768
            },
            "silent = the defaults = the lock"
        );
        let smaller = ExperimentBinding {
            vcpus: Some(8),
            mem_mib: Some(16_384),
            disk_mib: Some(16_384),
            ..silent.clone()
        };
        assert_eq!(
            c.shape(&smaller).expect("the topic may ask for less"),
            VmShape {
                vcpus: 8,
                mem_mib: 16_384,
                disk_mib: 16_384
            }
        );
        let greedy = ExperimentBinding {
            vcpus: Some(32),
            ..silent.clone()
        };
        let err = c.shape(&greedy).expect_err("over");
        assert_eq!(
            err,
            ExperimentError::OverCeiling {
                field: "vcpus",
                asked: 32,
                ceiling: 16
            }
        );
        let greedy_mem = ExperimentBinding {
            mem_mib: Some(65_536),
            ..silent.clone()
        };
        assert!(matches!(
            c.shape(&greedy_mem),
            Err(ExperimentError::OverCeiling {
                field: "mem_mib",
                asked: 65_536,
                ceiling: 32_768
            })
        ));
        let thin_disk = ExperimentBinding {
            disk_mib: Some(8_192),
            ..silent.clone()
        };
        assert!(matches!(
            c.shape(&thin_disk),
            Err(ExperimentError::UnderFloor {
                field: "disk_mib",
                asked: 8_192,
                min: 16_384
            })
        ));
        let roomy = ExperimentCeilings {
            max_disk_mib: 131_072,
            ..ExperimentCeilings::default()
        };
        assert_eq!(
            roomy
                .shape(&ExperimentBinding {
                    disk_mib: Some(65_536),
                    ..silent.clone()
                })
                .expect("the operator raised the disk ceiling")
                .disk_mib,
            65_536
        );
        assert!(
            err.to_string().contains("PROOF_VM_AGENT_EXPERIMENT_MAX_"),
            "{err}"
        );
        let tiny = ExperimentBinding {
            mem_mib: Some(64),
            ..silent.clone()
        };
        assert!(matches!(
            c.shape(&tiny),
            Err(ExperimentError::UnderFloor {
                field: "mem_mib",
                ..
            })
        ));
        let huge_disk = ExperimentBinding {
            disk_mib: Some(1 << 20),
            ..silent
        };
        assert!(matches!(
            c.shape(&huge_disk),
            Err(ExperimentError::OverCeiling {
                field: "disk_mib",
                ..
            })
        ));
        c.admit(VmShape {
            vcpus: 16,
            mem_mib: 32_768,
            disk_mib: 32_768,
        })
        .expect("at the ceiling is admitted");
        assert!(c
            .admit(VmShape {
                vcpus: 17,
                mem_mib: 1_024,
                disk_mib: 32_768,
            })
            .is_err());
        assert!(c
            .admit(VmShape {
                vcpus: 1,
                mem_mib: 1_024,
                disk_mib: 8_192,
            })
            .is_err());
        let mut bad = ExperimentCeilings::default();
        bad.default_disk_mib = bad.max_disk_mib + 1;
        assert_eq!(
            bad.validate(),
            Err(ExperimentError::Spec("default_disk_mib"))
        );
        bad = ExperimentCeilings::default();
        bad.max_vcpus = 0;
        assert!(bad.validate().is_err());
        bad = ExperimentCeilings::default();
        bad.default_vcpus = bad.max_vcpus + 1;
        assert_eq!(bad.validate(), Err(ExperimentError::Spec("default_vcpus")));
        bad = ExperimentCeilings::default();
        bad.default_mem_mib = bad.max_mem_mib + 1;
        assert_eq!(
            bad.validate(),
            Err(ExperimentError::Spec("default_mem_mib"))
        );
    }

    /// The lock is a **hard** maximum, not a default: an operator ceiling
    /// above 16 vCPU / 32768 MiB does not validate (so the process does not
    /// boot), a matching shape is not admitted, and a topic's ask against
    /// such a ceiling is refused — on both sides. Lower ceilings (a smaller
    /// host) are fine; the disk ceiling is not locked.
    #[test]
    #[allow(clippy::too_many_lines)]
    fn the_lock_is_a_hard_maximum_no_ceiling_or_shape_may_exceed() {
        assert_eq!(LOCK_MAX_EXPERIMENT_VCPUS, 16);
        assert_eq!(LOCK_MAX_EXPERIMENT_MEM_MIB, 32_768);
        assert_eq!(DEFAULT_MAX_EXPERIMENT_VCPUS, LOCK_MAX_EXPERIMENT_VCPUS);
        assert_eq!(DEFAULT_MAX_EXPERIMENT_MEM_MIB, LOCK_MAX_EXPERIMENT_MEM_MIB);
        assert_eq!(DEFAULT_EXPERIMENT_VCPUS, LOCK_MAX_EXPERIMENT_VCPUS);
        assert_eq!(DEFAULT_EXPERIMENT_MEM_MIB, LOCK_MAX_EXPERIMENT_MEM_MIB);
        let binding = ExperimentBinding {
            runner: "r_0".into(),
            pack: PackRef {
                path: None,
                digest: format!("sha256:{HEX}"),
            },
            vcpus: Some(64),
            mem_mib: Some(131_072),
            disk_mib: None,
        };
        let oversized = ExperimentCeilings {
            max_vcpus: 64,
            max_mem_mib: 131_072,
            ..ExperimentCeilings::default()
        };
        assert_eq!(
            oversized.validate(),
            Err(ExperimentError::AboveLock {
                field: "max_vcpus",
                value: 64,
                lock: 16
            }),
            "an operator ceiling above the lock does not validate"
        );
        assert!(
            oversized.shape(&binding).is_err(),
            "nor does it size a vm for a matching ask"
        );
        assert!(oversized
            .admit(VmShape {
                vcpus: 64,
                mem_mib: 131_072,
                disk_mib: 32_768,
            })
            .is_err());
        let msg = oversized.validate().expect_err("above").to_string();
        assert!(msg.contains("above the lock 16"), "{msg}");
        assert!(msg.contains("not an operator knob"), "{msg}");
        let just_mem = ExperimentCeilings {
            max_mem_mib: 32_769,
            ..ExperimentCeilings::default()
        };
        assert_eq!(
            just_mem.validate(),
            Err(ExperimentError::AboveLock {
                field: "max_mem_mib",
                value: 32_769,
                lock: 32_768
            })
        );
        let just_vcpus = ExperimentCeilings {
            max_vcpus: 17,
            ..ExperimentCeilings::default()
        };
        assert!(matches!(
            just_vcpus.validate(),
            Err(ExperimentError::AboveLock {
                field: "max_vcpus",
                ..
            })
        ));
        // The shape check is independent of the ceilings that produced it.
        assert!(matches!(
            VmShape {
                vcpus: 17,
                mem_mib: 1_024,
                disk_mib: 32_768
            }
            .locked(),
            Err(ExperimentError::AboveLock { field: "vcpus", .. })
        ));
        assert!(matches!(
            VmShape {
                vcpus: 1,
                mem_mib: 32_769,
                disk_mib: 32_768
            }
            .locked(),
            Err(ExperimentError::AboveLock {
                field: "mem_mib",
                ..
            })
        ));
        VmShape {
            vcpus: 16,
            mem_mib: 32_768,
            disk_mib: 1 << 20,
        }
        .locked()
        .expect("at the lock; disk is not locked");
        // A smaller host lowers its ceilings — allowed — and asks are held
        // under those, with the ceiling named.
        let small = ExperimentCeilings {
            default_vcpus: 4,
            max_vcpus: 8,
            default_mem_mib: 8_192,
            max_mem_mib: 16_384,
            ..ExperimentCeilings::default()
        };
        small
            .validate()
            .expect("below the lock is an operator choice");
        assert!(matches!(
            small.shape(&binding),
            Err(ExperimentError::OverCeiling {
                field: "vcpus",
                asked: 64,
                ceiling: 8
            })
        ));
        let fits = ExperimentBinding {
            vcpus: Some(8),
            mem_mib: Some(16_384),
            ..binding
        };
        assert_eq!(
            small.shape(&fits).expect("under the lowered ceiling"),
            VmShape {
                vcpus: 8,
                mem_mib: 16_384,
                disk_mib: 32_768
            }
        );
        // The env reader builds ceilings the same validator refuses.
        let msg = ExperimentError::OverCeiling {
            field: "vcpus",
            asked: 32,
            ceiling: 16,
        }
        .to_string();
        assert!(msg.contains("never above it"), "{msg}");
        assert!(!msg.contains("raise the operator ceiling"), "{msg}");
    }

    #[test]
    fn specs_round_trip_as_public_data_and_validate_their_shape() {
        let spec = ExperimentSpec {
            runner: "operator_adaptor_v0".into(),
            pack: PackRef {
                path: Some("first-slice/pack.tar".into()),
                digest: format!("sha256:{HEX}"),
            },
            disk_mib: 32_768,
        };
        spec.validate().expect("valid");
        let json = serde_json::to_string(&spec).expect("json");
        for forbidden in ["/var/lib", "api_key", "127.0.0.1"] {
            assert!(!json.contains(forbidden), "{json}");
        }
        let back: ExperimentSpec = serde_json::from_str(&json).expect("round trip");
        assert_eq!(back, spec);
        let bare: ExperimentSpec = serde_json::from_str(&format!(
            r#"{{"runner":"r_0","pack":{{"digest":"sha256:{HEX}"}},"disk_mib":16384}}"#
        ))
        .expect("path is optional on the wire");
        assert_eq!(bare.pack.path, None);
        let mut bad = spec.clone();
        bad.runner = "Bad Runner".into();
        assert_eq!(bad.validate(), Err(ExperimentError::Spec("runner")));
        let mut bad = spec.clone();
        bad.pack.digest = "latest".into();
        assert_eq!(bad.validate(), Err(ExperimentError::Spec("pack")));
        let mut bad = spec;
        bad.disk_mib = 8_192;
        assert_eq!(
            bad.validate(),
            Err(ExperimentError::Spec("disk_mib")),
            "under the 16 GiB floor"
        );
    }

    #[test]
    fn env_names_are_stable_and_the_policy_picks_its_image() {
        assert_eq!(PARAM_RUNNER, "in_guest_benchmark_runner");
        assert_eq!(PARAM_RUNNER_ALIAS, "baseline_runner");
        assert_eq!(PARAM_PACK_PATH, "experiment_pack_path");
        assert_eq!(PARAM_PACK_DIGEST, "experiment_pack_digest");
        assert_eq!(
            EXPERIMENT_VM_IMAGE_DIGEST_ENV,
            "PROOF_EXPERIMENT_VM_IMAGE_DIGEST"
        );
        assert_eq!(EXPERIMENT_VM_MAX_VCPUS_ENV, "PROOF_EXPERIMENT_VM_MAX_VCPUS");
        assert_eq!(
            EXPERIMENT_VM_MAX_MEM_MIB_ENV,
            "PROOF_EXPERIMENT_VM_MAX_MEM_MIB"
        );
        assert_eq!(EXPERIMENT_VM_DISK_MIB_ENV, "PROOF_EXPERIMENT_VM_DISK_MIB");
        assert_eq!(
            EXPERIMENT_VM_MAX_DISK_MIB_ENV,
            "PROOF_EXPERIMENT_VM_MAX_DISK_MIB"
        );
        let policy = ExperimentPolicy::default();
        assert_eq!(policy.image_for("sha256:rlm"), "sha256:rlm");
        let own = ExperimentPolicy {
            image_digest: Some("sha256:exp".into()),
            ..ExperimentPolicy::default()
        };
        assert_eq!(own.image_for("sha256:rlm"), "sha256:exp");
        // The process env is shared across tests: only assert the parse
        // helper's behaviour on names no other test sets.
        assert_eq!(env_u32("PROOF_EXPERIMENT_TEST_UNSET_KNOB", 7), Ok(7));
    }
}
