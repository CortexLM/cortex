//! Jail layout and process control for one Firecracker microVM.
//!
//! The jailer chroots Firecracker into `<chroot_base>/firecracker/<id>/root`;
//! everything the VM needs is placed there first (kernel copy, read-only
//! rootfs copy, fresh scratch drive, `vm-config.json`) and referenced by
//! jail-relative paths. Without `--daemonize` / `--new-pid-ns` the jailer
//! `exec`s into Firecracker, so the child handle we hold **is** the VM.

use std::path::{Path, PathBuf};
use std::process::Stdio;

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
/// and the per-VM nftables ruleset beside the root (never inside it).
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
    let root_s = root.display().to_string();
    if root.exists() {
        return Err(HvError::Backend(format!("jail {root_s} already exists")));
    }
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
    Ok(root)
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
/// # Errors
///
/// [`HvError::Backend`].
pub async fn retain(cfg: &HostConfig, shell: &dyn Shell, id: &str) -> Result<PathBuf, HvError> {
    let dest = cfg.retain_dir.join(id);
    sh(
        shell,
        "mkdir",
        &["-p", &cfg.retain_dir.display().to_string()],
    )
    .await?;
    let src = cfg.jail_dir(id).display().to_string();
    sh(shell, "mv", &[&src, &dest.display().to_string()]).await?;
    Ok(dest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::RecordingShell;

    fn cfg(tag: &str) -> HostConfig {
        let mut c = HostConfig::defaults();
        c.chroot_base =
            std::env::temp_dir().join(format!("proof-fc-jail-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&c.chroot_base);
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
        assert!(flat.iter().any(|l| l.starts_with("mv ")));
        let _ = std::fs::remove_dir_all(&c.chroot_base);
    }
}
