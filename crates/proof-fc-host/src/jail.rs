//! Jail layout and process control for one Firecracker microVM.
//!
//! The jailer chroots Firecracker into `<chroot_base>/firecracker/<id>/root`;
//! everything the VM needs is placed there first (kernel copy, read-only
//! rootfs copy, fresh scratch drive, `vm-config.json`) and referenced by
//! jail-relative paths. Without `--daemonize` / `--new-pid-ns` the jailer
//! `exec`s into Firecracker, so the child handle we hold **is** the VM.
//!
//! From [`prepare`] until the VM is registered (or, for a sister, until its
//! run ends) the jail is owned by a [`JailGuard`]: every failure path, a
//! cancelled task, or a dropped request destroys the jail — process, TAP,
//! nftables table, directory — so nothing a boot started is left behind.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use proof_vm_agent::HvError;
use proof_vm_proto::guest::GUEST_CID;
use serde_json::{json, Value};

use crate::config::HostConfig;
use crate::net::NetPlan;
use crate::shell::{sh, Shell};

/// Jail-relative names Firecracker sees.
pub const KERNEL_IN_JAIL: &str = "vmlinux";
/// See [`KERNEL_IN_JAIL`].
pub const ROOTFS_IN_JAIL: &str = "rootfs.ext4";
/// See [`KERNEL_IN_JAIL`].
pub const SCRATCH_IN_JAIL: &str = "scratch.ext4";
/// See [`KERNEL_IN_JAIL`].
pub const CONFIG_IN_JAIL: &str = "vm-config.json";
/// Firecracker vsock UDS (host connects here with `CONNECT <port>`).
pub const VSOCK_IN_JAIL: &str = "v.sock";
/// Firecracker API socket (unused by the agent; kept for operators).
pub const API_SOCK_IN_JAIL: &str = "run/firecracker.socket";
/// Serial console capture beside the jail.
pub const CONSOLE_LOG: &str = "console.log";

/// What one microVM boots with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VmBoot {
    /// Jail id (also the VM id).
    pub id: String,
    /// vCPUs.
    pub vcpus: u32,
    /// Memory.
    pub mem_mib: u32,
    /// Verified rootfs on the host to copy in.
    pub rootfs: PathBuf,
    /// Scratch drive size.
    pub scratch_mib: u32,
    /// Network (RLM VM) or none (sister guest).
    pub net: Option<NetPlan>,
}

/// Firecracker `--config-file` document for `boot`.
#[must_use]
pub fn vm_config(boot: &VmBoot) -> Value {
    let mut boot_args = String::from("console=ttyS0 reboot=k panic=1 pci=off");
    let mut ifaces = Vec::new();
    if let Some(net) = &boot.net {
        boot_args.push(' ');
        boot_args.push_str(&net.boot_arg());
        ifaces.push(json!({
            "iface_id": "eth0",
            "guest_mac": net.guest_mac,
            "host_dev_name": net.tap,
        }));
    }
    json!({
        "boot-source": {
            "kernel_image_path": format!("/{KERNEL_IN_JAIL}"),
            "boot_args": boot_args,
            "initrd_path": null,
        },
        "drives": [
            {
                "drive_id": "rootfs",
                "path_on_host": format!("/{ROOTFS_IN_JAIL}"),
                "is_root_device": true,
                "is_read_only": true,
            },
            {
                "drive_id": "scratch",
                "path_on_host": format!("/{SCRATCH_IN_JAIL}"),
                "is_root_device": false,
                "is_read_only": false,
            }
        ],
        "machine-config": {
            "vcpu_count": boot.vcpus,
            "mem_size_mib": boot.mem_mib,
            "smt": false,
        },
        "vsock": {
            "guest_cid": GUEST_CID,
            "uds_path": format!("/{VSOCK_IN_JAIL}"),
        },
        "network-interfaces": ifaces,
    })
}

/// The jailer argv (program excluded).
#[must_use]
pub fn jailer_args(cfg: &HostConfig, id: &str) -> Vec<String> {
    vec![
        "--id".into(),
        id.into(),
        "--exec-file".into(),
        cfg.firecracker_bin.display().to_string(),
        "--uid".into(),
        cfg.jail_uid.to_string(),
        "--gid".into(),
        cfg.jail_gid.to_string(),
        "--chroot-base-dir".into(),
        cfg.chroot_base.display().to_string(),
        "--".into(),
        "--config-file".into(),
        format!("/{CONFIG_IN_JAIL}"),
        "--api-sock".into(),
        format!("/{API_SOCK_IN_JAIL}"),
    ]
}

/// Build the jail root for `boot`: copies (reflink when the filesystem can),
/// a fresh ext4 scratch drive, ownership for the jail uid, the config file,
/// and the per-VM nftables ruleset beside the root (never inside it). A step
/// that fails removes whatever was already built; a jail that already exists
/// is refused and left alone.
///
/// # Errors
///
/// [`HvError::Backend`] from the first failing step.
pub async fn prepare(
    cfg: &HostConfig,
    shell: &dyn Shell,
    boot: &VmBoot,
) -> Result<PathBuf, HvError> {
    let root = cfg.jail_root(&boot.id);
    if root.exists() {
        return Err(HvError::Backend(format!(
            "jail {} already exists",
            root.display()
        )));
    }
    match build(cfg, shell, boot, &root).await {
        Ok(()) => Ok(root),
        Err(e) => {
            if let Err(rm) = destroy(cfg, shell, &boot.id).await {
                tracing::warn!(jail = %boot.id, "half-built jail not removed: {rm}");
            }
            Err(e)
        }
    }
}

async fn build(
    cfg: &HostConfig,
    shell: &dyn Shell,
    boot: &VmBoot,
    root: &Path,
) -> Result<(), HvError> {
    let root_s = root.display().to_string();
    sh(shell, "mkdir", &["-p", &format!("{root_s}/run")]).await?;
    let kernel_src = cfg.kernel.display().to_string();
    sh(
        shell,
        "cp",
        &[
            "--reflink=auto",
            &kernel_src,
            &format!("{root_s}/{KERNEL_IN_JAIL}"),
        ],
    )
    .await?;
    let rootfs_src = boot.rootfs.display().to_string();
    sh(
        shell,
        "cp",
        &[
            "--reflink=auto",
            &rootfs_src,
            &format!("{root_s}/{ROOTFS_IN_JAIL}"),
        ],
    )
    .await?;
    let scratch = format!("{root_s}/{SCRATCH_IN_JAIL}");
    sh(
        shell,
        "truncate",
        &["-s", &format!("{}M", boot.scratch_mib), &scratch],
    )
    .await?;
    sh(shell, "mkfs.ext4", &["-q", "-F", &scratch]).await?;
    let config = serde_json::to_string_pretty(&vm_config(boot))
        .map_err(|e| HvError::Backend(format!("render vm config: {e}")))?;
    write(&root.join(CONFIG_IN_JAIL), config.as_bytes())?;
    if let Some(net) = &boot.net {
        write(
            &cfg.jail_dir(&boot.id).join("net.nft"),
            net.ruleset().as_bytes(),
        )?;
    }
    let owner = format!("{}:{}", cfg.jail_uid, cfg.jail_gid);
    sh(shell, "chown", &["-R", &owner, &root_s]).await?;
    Ok(())
}

fn write(path: &Path, bytes: &[u8]) -> Result<(), HvError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| HvError::Backend(format!("mkdir {}: {e}", parent.display())))?;
    }
    std::fs::write(path, bytes)
        .map_err(|e| HvError::Backend(format!("write {}: {e}", path.display())))
}

/// Spawn the jailer (which execs Firecracker). Console goes to
/// `<jail_dir>/console.log`.
///
/// # Errors
///
/// [`HvError::Backend`].
pub fn spawn(cfg: &HostConfig, id: &str) -> Result<tokio::process::Child, HvError> {
    let log_path = cfg.jail_dir(id).join(CONSOLE_LOG);
    let log = std::fs::File::create(&log_path)
        .map_err(|e| HvError::Backend(format!("console log {}: {e}", log_path.display())))?;
    let err = log
        .try_clone()
        .map_err(|e| HvError::Backend(format!("console log clone: {e}")))?;
    tokio::process::Command::new(&cfg.jailer_bin)
        .args(jailer_args(cfg, id))
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(err))
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| HvError::Backend(format!("spawn {}: {e}", cfg.jailer_bin.display())))
}

/// Kill the VM process and reap it.
pub async fn kill(child: &mut tokio::process::Child) {
    let _ = child.start_kill();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(10), child.wait()).await;
}

/// Remove the jail entirely.
///
/// # Errors
///
/// [`HvError::Backend`].
pub async fn destroy(cfg: &HostConfig, shell: &dyn Shell, id: &str) -> Result<(), HvError> {
    let dir = cfg.jail_dir(id).display().to_string();
    sh(shell, "rm", &["-rf", &dir]).await?;
    Ok(())
}

/// Move the jail under `retain_dir` for audit (scratch, console, config).
///
/// Destination is `<retain_dir>/<id>` when that path is free (the runbook
/// layout). If a prior retain already occupies it — a restarted agent reissues
/// deterministic ids — the jail lands at `<id>-<stamp>` instead of GNU `mv`
/// nesting it as `<id>/<id>`. Success is confirmed only when the source is
/// gone, that unique destination exists, and it is not a nested `<dest>/<id>`.
///
/// # Errors
///
/// [`HvError::Backend`]. An occupied destination is never treated as success.
pub async fn retain(cfg: &HostConfig, shell: &dyn Shell, id: &str) -> Result<PathBuf, HvError> {
    sh(
        shell,
        "mkdir",
        &["-p", &cfg.retain_dir.display().to_string()],
    )
    .await?;
    let dest = unique_retain_dest(&cfg.retain_dir, id)?;
    if dest.exists() {
        return Err(HvError::Backend(format!(
            "retain destination {} already exists; refusing to nest",
            dest.display()
        )));
    }
    let src = cfg.jail_dir(id);
    let src_s = src.display().to_string();
    let dest_s = dest.display().to_string();
    sh(shell, "mv", &[&src_s, &dest_s]).await?;
    // GNU `mv` into an existing directory "succeeds" by nesting. Confirm the
    // source is gone, the unique dest exists, and it is not `<dest>/<id>`.
    sh(shell, "test", &["-d", &dest_s]).await?;
    sh(shell, "test", &["!", "-e", &src_s]).await?;
    sh(
        shell,
        "test",
        &["!", "-e", &dest.join(id).display().to_string()],
    )
    .await?;
    Ok(dest)
}

/// `<retain_dir>/<id>` when free; otherwise an exclusive `<id>-<stamp>-<n>`
/// sibling. Never returns a path that already exists.
fn unique_retain_dest(retain_dir: &Path, id: &str) -> Result<PathBuf, HvError> {
    let primary = retain_dir.join(id);
    if !primary.exists() {
        return Ok(primary);
    }
    let stamp = retain_stamp();
    for n in 0u32..32 {
        let dest = retain_dir.join(format!("{id}-{stamp}-{n}"));
        if !dest.exists() {
            return Ok(dest);
        }
    }
    Err(HvError::Backend(format!(
        "retain destination for {id} already exists under {}; refusing to nest",
        retain_dir.display()
    )))
}

fn retain_stamp() -> String {
    let d = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}.{:09}", d.as_secs(), d.subsec_nanos())
}

/// Kill the process (if any), tear the network down (if any), remove the jail.
async fn release(
    cfg: Arc<HostConfig>,
    shell: Arc<dyn Shell>,
    id: String,
    net: Option<NetPlan>,
    child: Option<tokio::process::Child>,
) {
    if let Some(mut child) = child {
        kill(&mut child).await;
    }
    if let Some(net) = net {
        for e in net.down(shell.as_ref()).await {
            tracing::debug!(jail = %id, "network teardown on release: {e}");
        }
    }
    if let Err(e) = destroy(&cfg, shell.as_ref(), &id).await {
        tracing::warn!(jail = %id, "jail not removed on release: {e}");
    } else {
        tracing::info!(jail = %id, "jail released");
    }
}

/// Owns a prepared jail — and, once spawned, its VM process — until it is
/// either handed over ([`keep`](Self::keep)) or torn down
/// ([`destroy`](Self::destroy)). Dropping an armed guard (an error before
/// the guest handshake, a cancelled sister task, a request the client gave
/// up on) releases everything on the runtime instead, so a boot that did not
/// finish never leaves a jail directory, a scratch drive, a TAP, or an
/// nftables table behind.
pub struct JailGuard {
    cfg: Arc<HostConfig>,
    shell: Arc<dyn Shell>,
    id: String,
    root: PathBuf,
    net: Option<NetPlan>,
    child: Option<tokio::process::Child>,
    armed: bool,
}

impl JailGuard {
    /// [`prepare`] the jail for `boot` and take ownership of it.
    ///
    /// # Errors
    ///
    /// [`HvError::Backend`]; nothing is left behind.
    pub async fn prepare(
        cfg: Arc<HostConfig>,
        shell: Arc<dyn Shell>,
        boot: &VmBoot,
    ) -> Result<Self, HvError> {
        let root = prepare(&cfg, shell.as_ref(), boot).await?;
        Ok(Self {
            cfg,
            shell,
            id: boot.id.clone(),
            root,
            net: boot.net.clone(),
            child: None,
            armed: true,
        })
    }

    /// Jail id.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Jail root (`<chroot_base>/firecracker/<id>/root`).
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// [`spawn`] the VM process into this jail. A guard holds at most one.
    ///
    /// # Errors
    ///
    /// [`HvError::Backend`]; the guard stays armed, so the jail is still released.
    pub fn spawn(&mut self) -> Result<(), HvError> {
        if self.child.is_some() {
            return Err(HvError::Backend(format!(
                "jail {} already has a process",
                self.id
            )));
        }
        self.child = Some(spawn(&self.cfg, &self.id)?);
        Ok(())
    }

    /// Hand the jail and its process over: the caller now owns both and the
    /// guard does nothing more. Without a spawned process there is nothing
    /// to hand over, so the guard comes back still owning the jail.
    ///
    /// # Errors
    ///
    /// The guard itself, still armed, when no process was spawned.
    pub fn keep(mut self) -> Result<tokio::process::Child, Box<Self>> {
        match self.child.take() {
            Some(child) => {
                self.armed = false;
                Ok(child)
            }
            None => Err(Box::new(self)),
        }
    }

    /// Kill the process, tear the network down, remove the jail — now, and
    /// to completion.
    pub async fn destroy(mut self) {
        self.armed = false;
        release(
            self.cfg.clone(),
            self.shell.clone(),
            std::mem::take(&mut self.id),
            self.net.take(),
            self.child.take(),
        )
        .await;
    }
}

impl Drop for JailGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let id = std::mem::take(&mut self.id);
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            tracing::warn!(jail = %id, "jail dropped before hand-over; releasing");
            handle.spawn(release(
                self.cfg.clone(),
                self.shell.clone(),
                id,
                self.net.take(),
                self.child.take(),
            ));
        } else {
            tracing::error!(jail = %id, "jail dropped outside a runtime; not released");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;
    use crate::shell::RecordingShell;

    fn cfg(tag: &str) -> HostConfig {
        let mut c = HostConfig::defaults();
        c.chroot_base =
            std::env::temp_dir().join(format!("proof-fc-jail-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&c.chroot_base);
        c.retain_dir = c.chroot_base.join("retained");
        c.kernel = PathBuf::from("/var/lib/proof-vm/vmlinux");
        c.uplink = "eno1".into();
        c
    }

    fn boot(net: Option<NetPlan>) -> VmBoot {
        VmBoot {
            id: "topic-a-0001".into(),
            vcpus: 4,
            mem_mib: 8_192,
            rootfs: PathBuf::from("/var/lib/proof-vm/images/sha256-aa.ext4"),
            scratch_mib: 8_192,
            net,
        }
    }

    #[test]
    fn the_config_file_references_jail_relative_paths_and_a_read_only_root() {
        let c = cfg("config");
        let plan = NetPlan::for_index(&c, 3);
        let v = vm_config(&boot(Some(plan.clone())));
        assert_eq!(v["boot-source"]["kernel_image_path"], "/vmlinux");
        let args = v["boot-source"]["boot_args"].as_str().expect("args");
        assert!(
            args.starts_with("console=ttyS0 reboot=k panic=1 pci=off ip=172.16.0.14::172.16.0.13:"),
            "{args}"
        );
        assert_eq!(v["drives"][0]["path_on_host"], "/rootfs.ext4");
        assert_eq!(v["drives"][0]["is_read_only"], true);
        assert_eq!(v["drives"][0]["is_root_device"], true);
        assert_eq!(v["drives"][1]["path_on_host"], "/scratch.ext4");
        assert_eq!(v["drives"][1]["is_read_only"], false);
        assert_eq!(v["machine-config"]["vcpu_count"], 4);
        assert_eq!(v["machine-config"]["mem_size_mib"], 8_192);
        assert_eq!(v["vsock"]["guest_cid"], 3);
        assert_eq!(v["vsock"]["uds_path"], "/v.sock");
        assert_eq!(v["network-interfaces"][0]["host_dev_name"], "pfc3");
        assert_eq!(v["network-interfaces"][0]["guest_mac"], plan.guest_mac);
        let sister = vm_config(&boot(None));
        assert_eq!(
            sister["network-interfaces"]
                .as_array()
                .expect("array")
                .len(),
            0,
            "sister has no nic"
        );
        assert!(!sister["boot-source"]["boot_args"]
            .as_str()
            .expect("args")
            .contains("ip="));
    }

    #[test]
    fn jailer_argv_pins_id_uid_gid_chroot_and_the_config_file() {
        let c = cfg("argv");
        let args = jailer_args(&c, "topic-a-0001");
        let joined = args.join(" ");
        assert!(joined.starts_with("--id topic-a-0001 --exec-file /usr/local/bin/firecracker --uid 65534 --gid 65534 --chroot-base-dir "), "{joined}");
        assert!(
            joined.ends_with("-- --config-file /vm-config.json --api-sock /run/firecracker.socket"),
            "{joined}"
        );
        assert!(
            !joined.contains("--daemonize") && !joined.contains("--new-pid-ns"),
            "the child handle must be the vm"
        );
    }

    #[tokio::test]
    async fn prepare_renders_copy_scratch_chown_and_writes_config_plus_rules() {
        let c = cfg("prepare");
        let shell = RecordingShell::default();
        let plan = NetPlan::for_index(&c, 0);
        let root = prepare(&c, &shell, &boot(Some(plan)))
            .await
            .expect("prepare");
        assert_eq!(root, c.jail_root("topic-a-0001"));
        let flat: Vec<String> = shell.calls().iter().map(|a| a.join(" ")).collect();
        let r = root.display().to_string();
        assert_eq!(flat[0], format!("mkdir -p {r}/run"));
        assert_eq!(
            flat[1],
            format!("cp --reflink=auto /var/lib/proof-vm/vmlinux {r}/vmlinux")
        );
        assert_eq!(
            flat[2],
            format!("cp --reflink=auto /var/lib/proof-vm/images/sha256-aa.ext4 {r}/rootfs.ext4")
        );
        assert_eq!(flat[3], format!("truncate -s 8192M {r}/scratch.ext4"));
        assert_eq!(flat[4], format!("mkfs.ext4 -q -F {r}/scratch.ext4"));
        assert_eq!(flat[5], format!("chown -R 65534:65534 {r}"));
        let config = std::fs::read_to_string(root.join(CONFIG_IN_JAIL)).expect("config written");
        assert!(config.contains("\"vcpu_count\": 4"));
        let rules = std::fs::read_to_string(c.jail_dir("topic-a-0001").join("net.nft"))
            .expect("rules written");
        assert!(rules.starts_with("table inet proof_vm_pfc0 {"));
        assert!(
            !root.join("net.nft").exists(),
            "rules stay outside the chroot"
        );
        let err = prepare(&c, &shell, &boot(None))
            .await
            .expect_err("second jail with the same id");
        assert!(err.to_string().contains("already exists"), "{err}");
        destroy(&c, &shell, "topic-a-0001").await.expect("destroy");
        let dest = retain(&c, &shell, "topic-a-0001").await.expect("retain");
        assert_eq!(dest, c.retain_dir.join("topic-a-0001"));
        let flat: Vec<String> = shell.calls().iter().map(|a| a.join(" ")).collect();
        assert!(flat
            .iter()
            .any(|l| l == &format!("rm -rf {}", c.jail_dir("topic-a-0001").display())));
        let src = c.jail_dir("topic-a-0001").display().to_string();
        let dest_s = dest.display().to_string();
        assert!(
            flat.iter().any(|l| l == &format!("mv {src} {dest_s}")),
            "{flat:?}"
        );
        assert!(
            flat.iter().any(|l| l == &format!("test -d {dest_s}")),
            "confirm dest exists: {flat:?}"
        );
        assert!(
            flat.iter().any(|l| l == &format!("test ! -e {src}")),
            "confirm source gone: {flat:?}"
        );
        assert!(
            flat.iter()
                .any(|l| l == &format!("test ! -e {dest_s}/topic-a-0001")),
            "confirm not nested: {flat:?}"
        );
        let _ = std::fs::remove_dir_all(&c.chroot_base);
    }

    /// GNU `mv` into an occupied `<retain>/<id>` would nest the new jail as
    /// `<retain>/<id>/<id>` and still report success. After an agent restart
    /// `mint_vm_id` reissues the same id, so that dest is often occupied.
    /// Retain must land at a unique sibling and leave the old dest intact.
    #[tokio::test]
    async fn retain_does_not_nest_into_an_existing_destination() {
        let c = cfg("retain-collide");
        let id = "topic-a-0001";
        let jail = c.jail_dir(id);
        std::fs::create_dir_all(jail.join("root")).expect("jail");
        std::fs::write(jail.join("console.log"), b"new jail").expect("console");
        let old = c.retain_dir.join(id);
        std::fs::create_dir_all(&old).expect("old retain");
        std::fs::write(old.join("console.log"), b"old jail").expect("old console");

        let dest = retain(&c, &crate::shell::SystemShell, id)
            .await
            .expect("unique sibling");
        assert_ne!(dest, old, "must not reuse the occupied name");
        assert!(
            dest.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("topic-a-0001-")),
            "sibling of the occupied id: {}",
            dest.display()
        );
        assert_eq!(
            std::fs::read_to_string(old.join("console.log")).expect("old"),
            "old jail"
        );
        assert!(
            !old.join(id).exists(),
            "GNU mv must not nest the new jail under the old dest"
        );
        assert!(!jail.exists(), "source gone");
        assert_eq!(
            std::fs::read_to_string(dest.join("console.log")).expect("new"),
            "new jail"
        );
        assert!(dest.join("root").is_dir(), "new jail contents at dest");
        let _ = std::fs::remove_dir_all(&c.chroot_base);
    }

    /// A step of `prepare` that fails removes what was already built; a jail
    /// that already exists is refused without touching it.
    #[tokio::test]
    async fn a_half_built_jail_is_removed_and_an_existing_one_is_left_alone() {
        let c = cfg("half");
        let shell = crate::shell::FailingShell::failing_on("mkfs.ext4");
        let err = prepare(&c, &shell, &boot(None))
            .await
            .expect_err("mkfs failed");
        assert!(err.to_string().contains("mkfs.ext4"), "{err}");
        let lines = shell.lines();
        assert_eq!(
            lines.last().map(String::as_str),
            Some(format!("rm -rf {}", c.jail_dir("topic-a-0001").display()).as_str()),
            "{lines:?}"
        );
        assert!(
            !lines.iter().any(|l| l.starts_with("chown")),
            "stopped at mkfs"
        );

        std::fs::create_dir_all(c.jail_root("topic-a-0001")).expect("pre-existing jail");
        let fresh = RecordingShell::default();
        let err = prepare(&c, &fresh, &boot(None)).await.expect_err("exists");
        assert!(err.to_string().contains("already exists"), "{err}");
        assert!(fresh.calls().is_empty(), "not ours to remove");
        let _ = std::fs::remove_dir_all(&c.chroot_base);
    }

    /// The guard is the no-leak contract: dropped armed (an aborted task, a
    /// request the client gave up on) it releases the jail on the runtime;
    /// handed over with `keep` it does nothing; `destroy` releases inline.
    #[tokio::test]
    async fn a_dropped_guard_releases_the_jail_and_a_kept_one_does_not() {
        let c = Arc::new(cfg("guard"));
        let shell = Arc::new(RecordingShell::default());
        let plan = NetPlan::for_index(&c, 5);
        let guard = JailGuard::prepare(c.clone(), shell.clone(), &boot(Some(plan)))
            .await
            .expect("prepare");
        assert_eq!(guard.id(), "topic-a-0001");
        assert_eq!(guard.root(), c.jail_root("topic-a-0001"));
        let before = shell.calls().len();
        let aborted = tokio::spawn(async move {
            let _held = guard;
            std::future::pending::<()>().await;
        });
        tokio::task::yield_now().await;
        aborted.abort();
        let _ = aborted.await;
        for _ in 0..50 {
            if shell.calls().len() > before {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let after: Vec<String> = shell.calls()[before..]
            .iter()
            .map(|c| c.join(" "))
            .collect();
        assert!(
            after.contains(&"nft delete table inet proof_vm_pfc5".to_owned()),
            "{after:?}"
        );
        assert!(after.contains(&"ip link del pfc5".to_owned()), "{after:?}");
        assert_eq!(
            after.last().map(String::as_str),
            Some(format!("rm -rf {}", c.jail_dir("topic-a-0001").display()).as_str()),
            "{after:?}"
        );
        let _ = std::fs::remove_dir_all(&c.chroot_base);

        // Without a process there is nothing to hand over: `keep` gives the
        // guard back, still owning the jail, and `destroy` releases inline.
        let c2 = Arc::new(cfg("kept"));
        let shell2 = Arc::new(RecordingShell::default());
        let guard = JailGuard::prepare(c2.clone(), shell2.clone(), &boot(None))
            .await
            .expect("prepare");
        let Err(guard) = guard.keep() else {
            panic!("nothing was spawned, nothing to keep");
        };
        let Err(err) = JailGuard::prepare(c2.clone(), shell2.clone(), &boot(None)).await else {
            panic!("root still exists on disk");
        };
        assert!(err.to_string().contains("already exists"), "{err}");
        let before = shell2.calls().len();
        guard.destroy().await;
        let lines: Vec<String> = shell2.calls()[before..]
            .iter()
            .map(|c| c.join(" "))
            .collect();
        assert_eq!(
            lines,
            vec![format!("rm -rf {}", c2.jail_dir("topic-a-0001").display())]
        );

        // With a process (a sleeping stand-in, not Firecracker) `keep` hands
        // it over and the guard does nothing more.
        let _ = std::fs::remove_dir_all(c2.jail_root("topic-a-0001"));
        let mut c3 = cfg("kept-process");
        c3.jailer_bin = c3.chroot_base.join("jailer");
        std::fs::create_dir_all(&c3.chroot_base).expect("base");
        std::fs::write(&c3.jailer_bin, b"#!/bin/sh\nexec sleep 30\n").expect("stand-in");
        std::fs::set_permissions(&c3.jailer_bin, std::fs::Permissions::from_mode(0o755))
            .expect("chmod");
        let c3 = Arc::new(c3);
        let shell3 = Arc::new(RecordingShell::default());
        let mut guard = JailGuard::prepare(c3.clone(), shell3.clone(), &boot(None))
            .await
            .expect("prepare");
        guard.spawn().expect("stand-in spawned");
        let before = shell3.calls().len();
        let Ok(mut child) = guard.keep() else {
            panic!("a process to keep");
        };
        tokio::task::yield_now().await;
        assert_eq!(shell3.calls().len(), before, "a kept jail is not removed");
        assert!(
            matches!(child.try_wait(), Ok(None)),
            "the process is ours now"
        );
        kill(&mut child).await;
        let _ = std::fs::remove_dir_all(&c3.chroot_base);
        let _ = std::fs::remove_dir_all(&c2.chroot_base);
    }
}
