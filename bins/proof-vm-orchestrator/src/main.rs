//! `proof-vm-orchestrator` — Firecracker topic-VM agent for the **dedicated
//! KVM host** (HTTPS `:8200`).
//!
//! The Proof control plane (`proof-challenge`, `FirecrackerOrchestrator`)
//! is its only client. It boots one jailed RLM microVM per topic from the
//! digest the control plane pins, runs every miner artefact in a sister
//! microVM with no network, and stamps what it saw onto the report. It
//! never runs on the control-plane droplet, never on a Lium pod, and never
//! receives a key from the control plane — owner key material is read from
//! `--owner-key-dir` on this host and staged over vsock.
//!
//! Fail-closed at boot: malformed kernel / sister image pins exit 1, a
//! non-loopback bind without a TLS certificate + key exits 1. A missing bearer
//! file does not stop the process — every request is refused until it exists
//! (the file is re-read per request, so rotation needs no restart).

#![forbid(unsafe_code)]

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use axum_server::tls_rustls::RustlsConfig;
use clap::Parser;
use proof_fc_host::{EgressAllow, FirecrackerHypervisor, HostConfig};
use proof_vm_agent::{agent_router, AgentState, BearerAuth, Hypervisor};
use proof_vm_proto::DEFAULT_AGENT_PORT;

/// KVM-host agent CLI. Every flag has a `PROOF_VM_AGENT_*` env twin for the
/// systemd `EnvironmentFile`.
#[derive(Debug, Parser)]
#[command(
    name = "proof-vm-orchestrator",
    about = "Firecracker topic-VM agent (KVM host, HTTPS :8200)"
)]
struct Cli {
    /// Bind address. Non-loopback requires --tls-cert and --tls-key.
    #[arg(long, env = "PROOF_VM_AGENT_BIND", default_value_t = SocketAddr::from(([0, 0, 0, 0], DEFAULT_AGENT_PORT)))]
    bind: SocketAddr,
    /// Bearer token file (re-read per request; never logged).
    #[arg(long, env = "PROOF_VM_AGENT_TOKEN_FILE")]
    token_file: PathBuf,
    /// TLS certificate chain (PEM).
    #[arg(long, env = "PROOF_VM_AGENT_TLS_CERT")]
    tls_cert: Option<PathBuf>,
    /// TLS private key (PEM).
    #[arg(long, env = "PROOF_VM_AGENT_TLS_KEY")]
    tls_key: Option<PathBuf>,
    /// Statically linked firecracker binary.
    #[arg(
        long,
        env = "PROOF_VM_AGENT_FIRECRACKER_BIN",
        default_value = "/usr/local/bin/firecracker"
    )]
    firecracker_bin: PathBuf,
    /// jailer binary (same release as firecracker).
    #[arg(
        long,
        env = "PROOF_VM_AGENT_JAILER_BIN",
        default_value = "/usr/local/bin/jailer"
    )]
    jailer_bin: PathBuf,
    /// jailer --chroot-base-dir.
    #[arg(
        long,
        env = "PROOF_VM_AGENT_CHROOT_BASE",
        default_value = "/srv/jailer"
    )]
    chroot_base: PathBuf,
    /// Root filesystem images named sha256-<hex>.ext4.
    #[arg(
        long,
        env = "PROOF_VM_AGENT_IMAGE_DIR",
        default_value = "/var/lib/proof-vm/images"
    )]
    image_dir: PathBuf,
    /// Guest kernel (vmlinux).
    #[arg(
        long,
        env = "PROOF_VM_AGENT_KERNEL",
        default_value = "/var/lib/proof-vm/vmlinux"
    )]
    kernel: PathBuf,
    /// sha256:<hex> pin of the kernel. Required; never invent one.
    #[arg(long, env = "PROOF_VM_AGENT_KERNEL_DIGEST")]
    kernel_digest: String,
    /// sha256:<hex> pin of the miner (sister) guest rootfs in --image-dir. Required.
    #[arg(long, env = "PROOF_VM_AGENT_SISTER_IMAGE_DIGEST")]
    sister_image_digest: String,
    /// uid the jailer drops Firecracker to.
    #[arg(long, env = "PROOF_VM_AGENT_JAIL_UID", default_value_t = 65534)]
    jail_uid: u32,
    /// gid the jailer drops Firecracker to.
    #[arg(long, env = "PROOF_VM_AGENT_JAIL_GID", default_value_t = 65534)]
    jail_gid: u32,
    /// RLM VM scratch drive (MiB).
    #[arg(long, env = "PROOF_VM_AGENT_SCRATCH_MIB", default_value_t = proof_fc_host::config::DEFAULT_SCRATCH_MIB)]
    scratch_mib: u32,
    /// Sister guest vCPUs (host-sized; the RLM never picks).
    #[arg(long, env = "PROOF_VM_AGENT_SISTER_VCPUS", default_value_t = proof_fc_host::config::DEFAULT_SISTER_VCPUS)]
    sister_vcpus: u32,
    /// Sister guest memory (MiB).
    #[arg(long, env = "PROOF_VM_AGENT_SISTER_MEM_MIB", default_value_t = proof_fc_host::config::DEFAULT_SISTER_MEM_MIB)]
    sister_mem_mib: u32,
    /// Sister scratch drive (MiB).
    #[arg(long, env = "PROOF_VM_AGENT_SISTER_SCRATCH_MIB", default_value_t = proof_fc_host::config::DEFAULT_SISTER_SCRATCH_MIB)]
    sister_scratch_mib: u32,
    /// Seconds a guest agent may take to come up.
    #[arg(long, env = "PROOF_VM_AGENT_BOOT_TIMEOUT_SECS", default_value_t = 120)]
    boot_timeout_secs: u64,
    /// Seconds past a job's deadline before the host kills it.
    #[arg(long, env = "PROOF_VM_AGENT_DEADLINE_GRACE_SECS", default_value_t = 30)]
    deadline_grace_secs: u64,
    /// Seconds for jobs with no deadline of their own.
    #[arg(
        long,
        env = "PROOF_VM_AGENT_DEFAULT_JOB_TIMEOUT_SECS",
        default_value_t = 3_600
    )]
    default_job_timeout_secs: u64,
    /// Directory of owner key files staged into the RLM VM over vsock.
    #[arg(long, env = "PROOF_VM_AGENT_OWNER_KEY_DIR")]
    owner_key_dir: Option<PathBuf>,
    /// Uplink interface RLM VMs are masqueraded through.
    #[arg(long, env = "PROOF_VM_AGENT_UPLINK", default_value = "eth0")]
    uplink: String,
    /// First /30 of the host<->guest pool.
    #[arg(long, env = "PROOF_VM_AGENT_NET_BASE", default_value = "172.16.0.0")]
    net_base: std::net::Ipv4Addr,
    /// Egress allowlist entries `CIDR[:port[/tcp|udp]]` (repeat or comma-separate).
    /// Empty = RLM VMs get no egress.
    #[arg(long, env = "PROOF_VM_AGENT_EGRESS_ALLOW", value_delimiter = ',')]
    egress_allow: Vec<String>,
    /// Where retained jails are moved.
    #[arg(
        long,
        env = "PROOF_VM_AGENT_RETAIN_DIR",
        default_value = "/var/lib/proof-vm/retained"
    )]
    retain_dir: PathBuf,
}

fn host_config(cli: &Cli) -> Result<HostConfig, String> {
    let mut allow = Vec::new();
    for raw in cli
        .egress_allow
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        allow.push(EgressAllow::parse(raw).map_err(|e| e.to_string())?);
    }
    let cfg = HostConfig {
        firecracker_bin: cli.firecracker_bin.clone(),
        jailer_bin: cli.jailer_bin.clone(),
        chroot_base: cli.chroot_base.clone(),
        image_dir: cli.image_dir.clone(),
        kernel: cli.kernel.clone(),
        kernel_digest: cli.kernel_digest.trim().to_owned(),
        sister_image_digest: cli.sister_image_digest.trim().to_owned(),
        jail_uid: cli.jail_uid,
        jail_gid: cli.jail_gid,
        scratch_mib: cli.scratch_mib,
        sister_vcpus: cli.sister_vcpus,
        sister_mem_mib: cli.sister_mem_mib,
        sister_scratch_mib: cli.sister_scratch_mib,
        boot_timeout: Duration::from_secs(cli.boot_timeout_secs),
        deadline_grace: Duration::from_secs(cli.deadline_grace_secs),
        default_job_timeout: Duration::from_secs(cli.default_job_timeout_secs),
        owner_key_dir: cli.owner_key_dir.clone(),
        uplink: cli.uplink.trim().to_owned(),
        net_base: cli.net_base,
        egress_allow: allow,
        retain_dir: cli.retain_dir.clone(),
    };
    cfg.validate().map_err(|e| e.to_string())?;
    Ok(cfg)
}

/// TLS is mandatory off loopback: the bearer must never cross a network in clear.
fn tls_required(
    bind: SocketAddr,
    cert: Option<&PathBuf>,
    key: Option<&PathBuf>,
) -> Result<Option<(PathBuf, PathBuf)>, String> {
    match (cert, key) {
        (Some(c), Some(k)) => Ok(Some((c.clone(), k.clone()))),
        (None, None) if bind.ip().is_loopback() => Ok(None),
        (None, None) => Err(format!(
            "bind {bind} is not loopback: --tls-cert and --tls-key are required (the bearer never travels in clear)"
        )),
        _ => Err("--tls-cert and --tls-key go together".into()),
    }
}

fn main() -> ExitCode {
    let _ = telemetry::init_tracing();
    let cli = Cli::parse();
    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            tracing::error!("runtime: {e}");
            return ExitCode::from(1);
        }
    };
    match rt.block_on(run(&cli)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!("{e}");
            ExitCode::from(1)
        }
    }
}

async fn run(cli: &Cli) -> Result<(), String> {
    let cfg = host_config(cli)?;
    let tls = tls_required(cli.bind, cli.tls_cert.as_ref(), cli.tls_key.as_ref())?;
    let hypervisor = Arc::new(FirecrackerHypervisor::new(cfg).map_err(|e| e.to_string())?);
    match hypervisor.ready() {
        Ok(()) => tracing::info!("firecracker + jailer + /dev/kvm present; agent ready"),
        Err(e) => tracing::warn!("{e}; every create will answer 503 until fixed"),
    }
    let auth = Arc::new(BearerAuth::from_file(&cli.token_file));
    if auth.configured() {
        tracing::info!(token_file = %cli.token_file.display(), "bearer token file present (contents not logged)");
    } else {
        tracing::warn!(
            token_file = %cli.token_file.display(),
            "bearer token file missing or empty; every request is refused until it exists"
        );
    }
    tracing::info!(
        egress_allow = hypervisor.config().egress_allow.len(),
        owner_key_dir = ?hypervisor.config().owner_key_dir,
        sister_vcpus = hypervisor.config().sister_vcpus,
        sister_mem_mib = hypervisor.config().sister_mem_mib,
        "host config"
    );
    let state = AgentState::new(hypervisor as Arc<dyn Hypervisor>, auth);
    let app = agent_router(state);
    let handle = axum_server::Handle::new();
    let shutdown = handle.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        shutdown.graceful_shutdown(Some(Duration::from_secs(10)));
    });
    if let Some((cert, key)) = tls {
        let config = RustlsConfig::from_pem_file(&cert, &key)
            .await
            .map_err(|e| format!("tls {} / {}: {e}", cert.display(), key.display()))?;
        tracing::info!(bind = %cli.bind, "proof-vm-orchestrator listening (https)");
        return axum_server::bind_rustls(cli.bind, config)
            .handle(handle)
            .serve(app.into_make_service())
            .await
            .map_err(|e| e.to_string());
    }
    tracing::warn!(bind = %cli.bind, "proof-vm-orchestrator listening in clear on loopback (tests / local TLS terminator only)");
    axum_server::bind(cli.bind)
        .handle(handle)
        .serve(app.into_make_service())
        .await
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cli(extra: &[&str]) -> Cli {
        let mut argv = vec![
            "proof-vm-orchestrator",
            "--token-file",
            "/etc/proof-vm/token",
            "--kernel-digest",
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "--sister-image-digest",
            "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        ];
        argv.extend_from_slice(extra);
        Cli::try_parse_from(argv).expect("cli")
    }

    #[test]
    fn config_needs_real_pins_and_parses_the_allowlist() {
        let c = cli(&["--egress-allow", "203.0.113.10/32:443,1.1.1.1:53/udp"]);
        let cfg = host_config(&c).expect("config");
        assert_eq!(cfg.egress_allow.len(), 2);
        assert_eq!(cfg.sister_vcpus, 2);
        assert_eq!(cfg.sister_mem_mib, 4_096);
        assert_eq!(cfg.boot_timeout, Duration::from_mins(2));
        let bad = cli(&["--egress-allow", "not-an-address"]);
        assert!(host_config(&bad).is_err());
        let mut invented = cli(&[]);
        invented.kernel_digest = "latest".into();
        assert!(host_config(&invented).is_err(), "no invented pins");
        assert!(
            Cli::try_parse_from(["proof-vm-orchestrator"]).is_err(),
            "pins and token file are required"
        );
    }

    #[test]
    fn tls_is_required_off_loopback() {
        let public: SocketAddr = "0.0.0.0:8200".parse().expect("addr");
        let local: SocketAddr = "127.0.0.1:8200".parse().expect("addr");
        assert!(tls_required(public, None, None).is_err());
        assert!(tls_required(local, None, None)
            .expect("loopback clear")
            .is_none());
        let cert = PathBuf::from("/etc/proof-vm/tls.crt");
        let key = PathBuf::from("/etc/proof-vm/tls.key");
        assert!(tls_required(public, Some(&cert), Some(&key))
            .expect("tls")
            .is_some());
        assert!(tls_required(public, Some(&cert), None).is_err());
        assert_eq!(cli(&[]).bind, public);
    }
}
