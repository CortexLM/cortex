//! Operator configuration of the Firecracker host. Paths and pins only —
//! no secret ever lives here; owner key material is read from
//! `owner_key_dir` at boot time and staged over vsock.

use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use proof_vm_agent::HvError;

/// Locked sister (miner guest) shape: vCPUs.
pub const DEFAULT_SISTER_VCPUS: u32 = 2;
/// Locked sister (miner guest) shape: memory.
pub const DEFAULT_SISTER_MEM_MIB: u32 = 4_096;
/// RLM VM writable scratch drive.
pub const DEFAULT_SCRATCH_MIB: u32 = 8_192;
/// Sister scratch drive (outputs + unpacked artefact).
pub const DEFAULT_SISTER_SCRATCH_MIB: u32 = 2_048;
/// Largest artefact tarball the host relays into a sister.
pub const MAX_ARTIFACT_TAR_BYTES: usize = 64 * 1024 * 1024;

/// L4 protocol of one allowlist entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Proto {
    /// TCP only.
    Tcp,
    /// UDP only.
    Udp,
    /// Any protocol (no port).
    Any,
}

/// One egress allowlist entry: `CIDR[:port[/tcp|udp]]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EgressAllow {
    /// Destination network.
    pub net: Ipv4Addr,
    /// Prefix length.
    pub prefix: u8,
    /// Destination port (`None` = any).
    pub port: Option<u16>,
    /// Protocol.
    pub proto: Proto,
}

impl EgressAllow {
    /// Parse `1.2.3.4/32:443`, `10.0.0.0/8`, `1.1.1.1/32:53/udp`, `1.2.3.4:443`.
    ///
    /// # Errors
    ///
    /// [`HvError::Spec`] naming the entry.
    pub fn parse(raw: &str) -> Result<Self, HvError> {
        let bad = |why: &str| HvError::Spec(format!("egress allow {raw:?}: {why}"));
        let s = raw.trim();
        let (addr_part, rest) = match s.split_once(':') {
            Some((a, r)) => (a, Some(r)),
            None => (s, None),
        };
        let (net_s, prefix_s) = addr_part.split_once('/').unwrap_or((addr_part, "32"));
        let net: Ipv4Addr = net_s.parse().map_err(|_| bad("not an IPv4 address"))?;
        let prefix: u8 = prefix_s.parse().map_err(|_| bad("bad prefix"))?;
        if prefix > 32 {
            return Err(bad("prefix over 32"));
        }
        let (port, proto) = match rest {
            None => (None, Proto::Any),
            Some(r) => {
                let (p, proto) = match r.split_once('/') {
                    Some((p, "tcp")) => (p, Proto::Tcp),
                    Some((p, "udp")) => (p, Proto::Udp),
                    Some(_) => return Err(bad("protocol must be tcp or udp")),
                    None => (r, Proto::Tcp),
                };
                let port: u16 = p.parse().map_err(|_| bad("bad port"))?;
                if port == 0 {
                    return Err(bad("port 0"));
                }
                (Some(port), proto)
            }
        };
        Ok(Self {
            net,
            prefix,
            port,
            proto,
        })
    }

    /// `1.2.3.4/32`.
    #[must_use]
    pub fn cidr(&self) -> String {
        format!("{}/{}", self.net, self.prefix)
    }
}

/// Everything the backend needs. Built by the agent binary from flags / env.
#[derive(Debug, Clone)]
pub struct HostConfig {
    /// Statically linked `firecracker` binary the jailer execs.
    pub firecracker_bin: PathBuf,
    /// `jailer` binary (same release as `firecracker_bin`).
    pub jailer_bin: PathBuf,
    /// `--chroot-base-dir` (jails land under `<base>/firecracker/<vm_id>/root`).
    pub chroot_base: PathBuf,
    /// Root filesystem images named `sha256-<hex>.ext4`.
    pub image_dir: PathBuf,
    /// Guest kernel (`vmlinux`), pinned by `kernel_digest`.
    pub kernel: PathBuf,
    /// `sha256:<hex>` of `kernel`.
    pub kernel_digest: String,
    /// `sha256:<hex>` of the miner-guest (sister) rootfs in `image_dir`.
    pub sister_image_digest: String,
    /// uid / gid the jailer drops to.
    pub jail_uid: u32,
    /// See `jail_uid`.
    pub jail_gid: u32,
    /// RLM VM scratch drive size.
    pub scratch_mib: u32,
    /// Sister vCPUs (sized by the host, never by the RLM).
    pub sister_vcpus: u32,
    /// Sister memory.
    pub sister_mem_mib: u32,
    /// Sister scratch drive.
    pub sister_scratch_mib: u32,
    /// How long a guest agent may take to say `Ready`.
    pub boot_timeout: Duration,
    /// Slack added to a job's own deadline before the host kills it.
    pub deadline_grace: Duration,
    /// Budget for jobs that carry no deadline (`ProposeRules`, `Archive`).
    pub default_job_timeout: Duration,
    /// Directory whose files are staged into the RLM VM over vsock. The
    /// control plane never reads them.
    pub owner_key_dir: Option<PathBuf>,
    /// Uplink the RLM VMs are masqueraded through.
    pub uplink: String,
    /// First /30 of the host↔guest point-to-point pool.
    pub net_base: Ipv4Addr,
    /// What the RLM VM may reach. Empty = no egress at all.
    pub egress_allow: Vec<EgressAllow>,
    /// Where retained jails are moved on `Retain`.
    pub retain_dir: PathBuf,
}

impl HostConfig {
    /// Defaults for a host laid out like the runbook.
    #[must_use]
    pub fn defaults() -> Self {
        Self {
            firecracker_bin: PathBuf::from("/usr/local/bin/firecracker"),
            jailer_bin: PathBuf::from("/usr/local/bin/jailer"),
            chroot_base: PathBuf::from("/srv/jailer"),
            image_dir: PathBuf::from("/var/lib/proof-vm/images"),
            kernel: PathBuf::from("/var/lib/proof-vm/vmlinux"),
            kernel_digest: String::new(),
            sister_image_digest: String::new(),
            jail_uid: 65534,
            jail_gid: 65534,
            scratch_mib: DEFAULT_SCRATCH_MIB,
            sister_vcpus: DEFAULT_SISTER_VCPUS,
            sister_mem_mib: DEFAULT_SISTER_MEM_MIB,
            sister_scratch_mib: DEFAULT_SISTER_SCRATCH_MIB,
            boot_timeout: Duration::from_mins(2),
            deadline_grace: Duration::from_secs(30),
            default_job_timeout: Duration::from_hours(1),
            owner_key_dir: None,
            uplink: "eth0".into(),
            net_base: Ipv4Addr::new(172, 16, 0, 0),
            egress_allow: Vec::new(),
            retain_dir: PathBuf::from("/var/lib/proof-vm/retained"),
        }
    }

    /// Shape check of the pins and sizes (paths are checked by `ready()`).
    ///
    /// # Errors
    ///
    /// [`HvError::Spec`].
    pub fn validate(&self) -> Result<(), HvError> {
        if proof_fc_harvest::images::digest_hex(&self.kernel_digest).is_none() {
            return Err(HvError::Spec(
                "kernel_digest must be sha256:<64 hex> (do not invent one)".into(),
            ));
        }
        if proof_fc_harvest::images::digest_hex(&self.sister_image_digest).is_none() {
            return Err(HvError::Spec(
                "sister_image_digest must be sha256:<64 hex> (do not invent one)".into(),
            ));
        }
        if !(1..=64).contains(&self.sister_vcpus) || !(512..=131_072).contains(&self.sister_mem_mib)
        {
            return Err(HvError::Spec("sister vcpus / mem_mib out of range".into()));
        }
        if self.uplink.trim().is_empty() || self.uplink.len() > 15 {
            return Err(HvError::Spec("uplink must be an interface name".into()));
        }
        Ok(())
    }

    /// Jail root for `vm_id`: `<chroot_base>/<exec_file_name>/<vm_id>/root`.
    #[must_use]
    pub fn jail_root(&self, vm_id: &str) -> PathBuf {
        let exec_name = self.firecracker_bin.file_name().map_or_else(
            || "firecracker".into(),
            |n| n.to_string_lossy().into_owned(),
        );
        self.chroot_base.join(exec_name).join(vm_id).join("root")
    }

    /// `<chroot_base>/<exec_file_name>/<vm_id>` (what teardown removes or retains).
    #[must_use]
    pub fn jail_dir(&self, vm_id: &str) -> PathBuf {
        self.jail_root(vm_id)
            .parent()
            .map_or_else(|| self.chroot_base.join(vm_id), Path::to_path_buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn egress_entries_parse_cidr_port_and_proto() {
        let a = EgressAllow::parse("1.2.3.4/32:443").expect("parse");
        assert_eq!(a.cidr(), "1.2.3.4/32");
        assert_eq!((a.port, a.proto), (Some(443), Proto::Tcp));
        let b = EgressAllow::parse("10.0.0.0/8").expect("parse");
        assert_eq!((b.port, b.proto), (None, Proto::Any));
        let c = EgressAllow::parse(" 1.1.1.1:53/udp ").expect("parse");
        assert_eq!(
            (c.cidr(), c.port, c.proto),
            ("1.1.1.1/32".into(), Some(53), Proto::Udp)
        );
        for bad in [
            "nope",
            "1.2.3.4/33",
            "1.2.3.4:0",
            "1.2.3.4:443/sctp",
            "1.2.3.4:x",
        ] {
            assert!(EgressAllow::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn defaults_validate_only_with_real_pins_and_jail_paths_follow_the_jailer() {
        let mut cfg = HostConfig::defaults();
        assert!(cfg.validate().is_err(), "no invented digests");
        cfg.kernel_digest = format!("sha256:{}", "aa".repeat(32));
        assert!(cfg.validate().is_err());
        cfg.sister_image_digest = format!("sha256:{}", "bb".repeat(32));
        cfg.validate().expect("pinned");
        assert_eq!((cfg.sister_vcpus, cfg.sister_mem_mib), (2, 4_096));
        assert_eq!(
            cfg.jail_root("topic-a-0001"),
            PathBuf::from("/srv/jailer/firecracker/topic-a-0001/root")
        );
        assert_eq!(
            cfg.jail_dir("topic-a-0001"),
            PathBuf::from("/srv/jailer/firecracker/topic-a-0001")
        );
        cfg.uplink = "a-very-long-interface-name".into();
        assert!(cfg.validate().is_err());
    }
}
