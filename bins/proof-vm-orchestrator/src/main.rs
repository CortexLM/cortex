//! `proof-vm-orchestrator` — Firecracker topic-VM agent for a **KVM host**
//! (HTTPS `:8200`): in production a **dedicated** DO droplet on the VPC
//! (`g-8vcpu-32gb`, nested `/dev/kvm`), never the control-plane droplet; on
//! staging the control-plane droplet itself with nested `/dev/kvm` is an
//! allowed, proven exception.
//!
//! The Proof control plane (`proof-challenge`, `FirecrackerOrchestrator`)
//! is its only client. It boots one jailed RLM microVM per topic from the
//! digest the control plane pins, runs every miner artefact in a sister
//! microVM with no network, and stamps what it saw onto the report. It
//! never runs on a Lium pod, never without `/dev/kvm`, and never receives a
//! key from the control plane — owner key material is read from
//! `--owner-key-dir` on this host and staged over vsock.
//!
//! Fail-closed at boot: malformed kernel / sister image pins exit 1, a
//! non-loopback bind without a TLS certificate + key exits 1, a certificate
//! without a SAN for every host the control plane's
//! `PROOF_VM_ORCHESTRATOR_URL` may name (`--tls-sans`, or the bind address)
//! exits 1 — the CP's rustls client would refuse it anyway, and a wildcard
//! bind must say which names it serves. A missing bearer file does not stop
//! the process — every request is refused until it exists (the file is
//! re-read per request, so rotation needs no restart).
//!
//! TLS boots the same way however the binary was built: the rustls
//! [`CryptoProvider`](rustls::crypto::CryptoProvider) is installed explicitly
//! ([`install_crypto_provider`]) before any TLS config exists, so a
//! workspace-wide build that unified rustls's `ring` and `aws-lc-rs` features
//! no longer panics at the first HTTPS listener.

#![forbid(unsafe_code)]

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
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
    /// Hosts the control plane's orchestrator URL may name for this agent —
    /// the dedicated droplet's VPC IP, its hostname, … (repeat or
    /// comma-separate). The certificate must carry a SAN for each, or boot
    /// exits 1. Default: the bind address; required with a wildcard bind.
    #[arg(long, env = "PROOF_VM_AGENT_TLS_SANS", value_delimiter = ',')]
    tls_sans: Vec<String>,
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

/// Operator tool that mints a certificate covering the right names.
const TLS_HELPER: &str = "deploy/scripts/proof-vm-agent-tls.sh";

/// The server names the certificate must be valid for: every `--tls-sans`
/// entry (DNS name or IP literal), else the bind address when it is a
/// specific one. A wildcard bind (`0.0.0.0` / `[::]`) says nothing about the
/// host clients use, so it requires the list — no default, no guess (a
/// docker gateway address is a colocated-staging artefact, never a prod
/// default).
///
/// # Errors
///
/// A name that is neither a DNS name nor an IP, or a wildcard bind without
/// `--tls-sans`.
fn expected_server_names(
    bind: SocketAddr,
    sans: &[String],
) -> Result<Vec<rustls::pki_types::ServerName<'static>>, String> {
    use rustls::pki_types::ServerName;
    let mut names = Vec::new();
    for raw in sans.iter().map(|s| s.trim()).filter(|s| !s.is_empty()) {
        let name = ServerName::try_from(raw.to_ascii_lowercase()).map_err(|e| {
            format!("--tls-sans entry {raw:?} is neither a DNS name nor an IP address: {e}")
        })?;
        if !names.contains(&name) {
            names.push(name);
        }
    }
    if names.is_empty() {
        if bind.ip().is_unspecified() {
            return Err(format!(
                "bind {bind} is a wildcard: --tls-sans (PROOF_VM_AGENT_TLS_SANS) must list the host(s) \
                 the control plane's PROOF_VM_ORCHESTRATOR_URL uses for this agent (VPC IP, hostname); \
                 the certificate is checked against them at boot"
            ));
        }
        names.push(ServerName::IpAddress(bind.ip().into()));
    }
    Ok(names)
}

/// Refuse a certificate the control plane's rustls client would refuse:
/// the leaf must be valid for every expected name (SAN — a CN alone is not
/// a name). The names it does present are reported so the operator can see
/// what to regenerate.
///
/// # Errors
///
/// Unparseable PEM / DER, or a name the leaf does not cover.
fn check_cert_covers(
    cert_pem: &[u8],
    cert_path: &Path,
    names: &[rustls::pki_types::ServerName<'static>],
) -> Result<(), String> {
    use rustls::pki_types::pem::PemObject;
    use rustls::pki_types::CertificateDer;
    let leaf = CertificateDer::from_pem_slice(cert_pem)
        .map_err(|e| format!("tls cert {}: {e:?}", cert_path.display()))?;
    let cert = webpki::EndEntityCert::try_from(&leaf)
        .map_err(|e| format!("tls cert {}: {e}", cert_path.display()))?;
    for name in names {
        match cert.verify_is_valid_for_subject_name(name) {
            Ok(()) => {}
            Err(webpki::Error::CertNotValidForName(ctx)) => {
                return Err(format!(
                    "tls cert {} has no SAN for {} (it presents {:?}); the control plane's rustls client \
                     refuses it — regenerate with {TLS_HELPER} --san {} (CN alone is never a name)",
                    cert_path.display(),
                    name.to_str(),
                    ctx.presented,
                    name.to_str(),
                ));
            }
            Err(e) => {
                return Err(format!(
                    "tls cert {} cannot be checked for {}: {e}",
                    cert_path.display(),
                    name.to_str()
                ));
            }
        }
    }
    Ok(())
}

/// Name of the rustls provider this binary runs on.
const CRYPTO_PROVIDER: &str = "ring";

/// Select the process-level rustls [`CryptoProvider`](rustls::crypto::CryptoProvider)
/// **once**, before any TLS config is built.
///
/// rustls only picks a provider on its own when exactly one of its `ring` /
/// `aws-lc-rs` features is enabled. Cargo unifies features across every
/// package selected for a build, so `cargo build --workspace` or `cargo build
/// --bin proof-vm-orchestrator` from the workspace root used to enable both
/// (reqwest → `ring`, axum-server's `tls-rustls` → `aws-lc-rs`) and
/// `RustlsConfig::from_pem_file` panicked at boot with "Could not
/// automatically determine the process-level `CryptoProvider`". Installing
/// `ring` here makes boot independent of the build shape; the manifest also
/// drops `aws-lc-rs` from this binary's own graph so a package-scoped build
/// needs no cmake / C toolchain beyond what `ring` wants.
///
/// Returns `true` when this call installed the provider, `false` when one was
/// already in place (a second call, or a test harness) — both are fine, and
/// neither panics.
fn install_crypto_provider() -> bool {
    rustls::crypto::ring::default_provider()
        .install_default()
        .is_ok()
}

/// Load the certificate chain + private key into a server config. Requires
/// [`install_crypto_provider`] to have run (or exactly one provider feature).
async fn tls_config(cert: &Path, key: &Path) -> Result<RustlsConfig, String> {
    RustlsConfig::from_pem_file(cert, key)
        .await
        .map_err(|e| format!("tls {} / {}: {e}", cert.display(), key.display()))
}

fn main() -> ExitCode {
    let _ = telemetry::init_tracing();
    let installed = install_crypto_provider();
    tracing::info!(
        provider = CRYPTO_PROVIDER,
        installed_here = installed,
        "rustls crypto provider selected"
    );
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
        let names = expected_server_names(cli.bind, &cli.tls_sans)?;
        let pem = std::fs::read(&cert).map_err(|e| format!("tls cert {}: {e}", cert.display()))?;
        check_cert_covers(&pem, &cert, &names)?;
        let config = tls_config(&cert, &key).await?;
        let served: Vec<String> = names.iter().map(|n| n.to_str().into_owned()).collect();
        tracing::info!(
            bind = %cli.bind,
            tls_sans = ?served,
            "proof-vm-orchestrator listening (https); certificate covers every listed name"
        );
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

    /// Boot smoke: the provider is installed exactly once (a repeat is a
    /// no-op, never a panic) and a certificate + key load into a server
    /// config afterwards — the step that panicked on the workspace-built
    /// binary. The certificate is self-signed at test time; nothing on disk.
    #[tokio::test]
    async fn crypto_provider_installs_once_and_tls_boots_from_pem() {
        let first = install_crypto_provider();
        let second = install_crypto_provider();
        assert!(!second, "a second install is a no-op");
        // `first` may be false when another test in this process got there
        // earlier; what matters is that a provider is now in force.
        let _ = first;
        assert!(
            rustls::crypto::CryptoProvider::get_default().is_some(),
            "a process-level provider is installed"
        );
        // ServerConfig::builder() is where an undecidable provider panics.
        let _builder = rustls::ServerConfig::builder();

        let key = rcgen::KeyPair::generate().expect("keypair");
        let cert = rcgen::CertificateParams::new(vec!["kvm.example.invalid".to_owned()])
            .expect("params")
            .self_signed(&key)
            .expect("self-signed");
        let dir = std::env::temp_dir().join(format!("proof-vm-tls-smoke-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let cert_path = dir.join("tls.crt");
        let key_path = dir.join("tls.key");
        std::fs::write(&cert_path, cert.pem()).expect("cert");
        std::fs::write(&key_path, key.serialize_pem()).expect("key");
        tls_config(&cert_path, &key_path)
            .await
            .expect("pem cert + key load into a rustls server config");
        let err = tls_config(&dir.join("missing.crt"), &key_path)
            .await
            .expect_err("missing cert");
        assert!(err.contains("missing.crt"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The names the certificate is held to come from the operator (every
    /// host the CP's URL may use), else the specific bind address. A wildcard
    /// bind has no default: it must say which names it serves.
    #[test]
    fn expected_names_come_from_the_sans_list_or_a_specific_bind() {
        let vpc: SocketAddr = "10.116.0.7:8200".parse().expect("addr");
        let any: SocketAddr = "0.0.0.0:8200".parse().expect("addr");
        let any6: SocketAddr = "[::]:8200".parse().expect("addr");
        let names = expected_server_names(vpc, &[]).expect("bind ip is the default");
        assert_eq!(names.len(), 1);
        assert_eq!(names[0].to_str(), "10.116.0.7");
        for wildcard in [any, any6] {
            let err = expected_server_names(wildcard, &[]).expect_err("wildcard needs the list");
            assert!(err.contains("PROOF_VM_AGENT_TLS_SANS"), "{err}");
            assert!(err.contains("PROOF_VM_ORCHESTRATOR_URL"), "{err}");
        }
        let listed = expected_server_names(
            any,
            &[
                " 10.116.0.7 ".into(),
                "KVM-1.internal".into(),
                "10.116.0.7".into(),
                String::new(),
            ],
        )
        .expect("list");
        let shown: Vec<String> = listed.iter().map(|n| n.to_str().into_owned()).collect();
        assert_eq!(
            shown,
            ["10.116.0.7", "kvm-1.internal"],
            "trimmed, lower-cased, deduplicated"
        );
        let cli = cli(&["--tls-sans", "10.116.0.7,kvm-1.internal"]);
        assert_eq!(cli.tls_sans, ["10.116.0.7", "kvm-1.internal"]);
        let err = expected_server_names(vpc, &["not a name!".into()]).expect_err("junk");
        assert!(err.contains("neither a DNS name nor an IP"), "{err}");
    }

    /// The boot check is the CP's own rule: a SAN for every name, CN never
    /// counts. A certificate minted for the docker gateway only is refused
    /// for the droplet's VPC address, and the message names what it presents
    /// and the helper that fixes it.
    #[test]
    fn certificate_must_carry_a_san_for_every_expected_name() {
        let names = |list: &[&str]| {
            expected_server_names(
                "0.0.0.0:8200".parse().expect("addr"),
                &list.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>(),
            )
            .expect("names")
        };
        let path = Path::new("/etc/proof-vm/tls.crt");
        let key = rcgen::KeyPair::generate().expect("keypair");
        let droplet = rcgen::CertificateParams::new(vec![
            "10.116.0.7".to_owned(),
            "kvm-1.internal".to_owned(),
        ])
        .expect("params")
        .self_signed(&key)
        .expect("cert");
        let pem = droplet.pem();
        check_cert_covers(
            pem.as_bytes(),
            path,
            &names(&["10.116.0.7", "kvm-1.internal"]),
        )
        .expect("vpc ip + hostname covered");
        check_cert_covers(pem.as_bytes(), path, &names(&["KVM-1.internal"]))
            .expect("dns names compare case-insensitively");
        let err = check_cert_covers(pem.as_bytes(), path, &names(&["10.116.0.7", "172.18.0.1"]))
            .expect_err("docker gateway is not on this cert");
        assert!(err.contains("no SAN for 172.18.0.1"), "{err}");
        assert!(
            err.contains("10.116.0.7") && err.contains("kvm-1.internal"),
            "presented names: {err}"
        );
        assert!(
            err.contains(TLS_HELPER) && err.contains("--san 172.18.0.1"),
            "{err}"
        );

        let gateway_only = rcgen::CertificateParams::new(vec!["172.18.0.1".to_owned()])
            .expect("params")
            .self_signed(&key)
            .expect("cert");
        let err = check_cert_covers(gateway_only.pem().as_bytes(), path, &names(&["10.116.0.7"]))
            .expect_err("staging colo cert does not serve the dedicated droplet");
        assert!(err.contains("no SAN for 10.116.0.7"), "{err}");

        let mut cn_only = rcgen::CertificateParams::new(Vec::<String>::new()).expect("params");
        cn_only
            .distinguished_name
            .push(rcgen::DnType::CommonName, "10.116.0.7");
        let cn_only = cn_only.self_signed(&key).expect("cert");
        let err = check_cert_covers(cn_only.pem().as_bytes(), path, &names(&["10.116.0.7"]))
            .expect_err("CN is not a SAN");
        assert!(err.contains("no SAN for 10.116.0.7"), "{err}");
        assert!(
            check_cert_covers(b"not pem", path, &names(&["10.116.0.7"])).is_err(),
            "garbage is refused, not skipped"
        );
    }

    /// The operator helper mints what the boot check wants: run it for the
    /// names a dedicated droplet needs, then hold its output to the same
    /// rule the agent applies at boot and load the pair as the listener
    /// would. `--write-env` records exactly the minted names; an empty list
    /// is refused.
    #[tokio::test]
    async fn tls_helper_mints_a_certificate_the_boot_check_accepts() {
        use std::process::Command;
        if Command::new("openssl").arg("version").output().is_err() {
            eprintln!("openssl not installed; skipping the helper run");
            return;
        }
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../{TLS_HELPER}"));
        let dir = std::env::temp_dir().join(format!("proof-vm-tls-helper-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        let env_file = dir.join("orchestrator.env");
        std::fs::write(&env_file, "PROOF_VM_AGENT_BIND=0.0.0.0:8200\n").expect("env");
        let out = Command::new("bash")
            .arg(&script)
            .args([
                "--out-dir",
                &dir.display().to_string(),
                "--no-hostname",
                "--no-vpc",
            ])
            .args([
                "--san",
                "10.116.0.7",
                "--san",
                "KVM-1.internal",
                "--write-env",
            ])
            .output()
            .expect("run helper");
        assert!(
            out.status.success(),
            "helper failed: {}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        let env = std::fs::read_to_string(&env_file).expect("env");
        assert!(
            env.contains("PROOF_VM_AGENT_TLS_SANS=10.116.0.7,kvm-1.internal"),
            "{env}"
        );
        let cert_path = dir.join("tls.crt");
        let pem = std::fs::read(&cert_path).expect("cert");
        let names = |list: &[&str]| {
            expected_server_names(
                "0.0.0.0:8200".parse().expect("addr"),
                &list.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>(),
            )
            .expect("names")
        };
        check_cert_covers(&pem, &cert_path, &names(&["10.116.0.7", "kvm-1.internal"]))
            .expect("the minted certificate covers the minted names");
        let err = check_cert_covers(&pem, &cert_path, &names(&["172.18.0.1"]))
            .expect_err("not minted for the docker gateway");
        assert!(err.contains("no SAN for 172.18.0.1"), "{err}");
        install_crypto_provider();
        tls_config(&cert_path, &dir.join("tls.key"))
            .await
            .expect("the pair loads as the listener would");
        let ca = std::fs::read(dir.join("ca.pem")).expect("ca");
        assert!(
            ca.starts_with(b"-----BEGIN CERTIFICATE-----"),
            "ca.pem is PEM"
        );

        let refused = Command::new("bash")
            .arg(&script)
            .args([
                "--out-dir",
                &dir.display().to_string(),
                "--no-hostname",
                "--no-vpc",
            ])
            .args(["--env-file", "/nonexistent/orchestrator.env", "--dry-run"])
            .output()
            .expect("run helper");
        assert!(!refused.status.success(), "no SAN at all is refused");
        assert!(
            String::from_utf8_lossy(&refused.stderr).contains("no SAN"),
            "{}",
            String::from_utf8_lossy(&refused.stderr)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
