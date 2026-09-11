//! `proof-vm-guest-agent` — the vsock agent baked into the Proof RLM /
//! experiment microVM image. Listens on guest vsock port 5000
//! (`proof_vm_proto::guest::RLM_JOB_PORT`) for the KVM host's
//! `proof-vm-orchestrator`, or speaks the same frames over stdin / stdout
//! (`--stdio`, for a `socat VSOCK-LISTEN` shim or a test harness).
//!
//! It is the generic half of an in-guest run: protocol, staging, and the
//! exec of an operator adaptor under `--runners-dir`. It never carries a
//! default value — a job without a selected, installed runner and a staged
//! pack fails. See `crates/proof-vm-guest`.

#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use clap::Parser;
use proof_vm_guest::{GuestAgent, GuestConfig, AGENT_NAME};
use proof_vm_proto::guest::RLM_JOB_PORT;
use proof_vm_proto::ProtoError;

/// Guest agent CLI. Every flag has a `PROOF_GUEST_*` env twin for the init
/// script baked into the image.
#[derive(Debug, Parser)]
#[command(
    name = "proof-vm-guest-agent",
    about = "Proof topic-VM guest agent (vsock :5000)"
)]
struct Cli {
    /// Guest vsock port to listen on.
    #[arg(long, env = "PROOF_GUEST_VSOCK_PORT", default_value_t = RLM_JOB_PORT)]
    vsock_port: u32,
    /// Speak frames over stdin/stdout instead of listening on vsock.
    #[arg(long, env = "PROOF_GUEST_STDIO", default_value_t = false)]
    stdio: bool,
    /// Owner key material staged by the host (tmpfs).
    #[arg(
        long,
        env = "PROOF_GUEST_SECRETS_DIR",
        default_value = "/run/proof/secrets"
    )]
    secrets_dir: PathBuf,
    /// Unpacked experiment packs (writable disk).
    #[arg(
        long,
        env = "PROOF_GUEST_PACK_ROOT",
        default_value = "/var/lib/proof/packs"
    )]
    pack_root: PathBuf,
    /// Per-job work directories (writable disk).
    #[arg(
        long,
        env = "PROOF_GUEST_WORK_ROOT",
        default_value = "/var/lib/proof/work"
    )]
    work_root: PathBuf,
    /// Operator adaptors: one directory per runner id holding the
    /// executables `run` / `inspect` / `propose_rules`.
    #[arg(
        long,
        env = "PROOF_GUEST_RUNNERS_DIR",
        default_value = "/opt/proof/runners"
    )]
    runners_dir: PathBuf,
    /// uid adaptors run as (rootless container runtimes want an unprivileged user).
    #[arg(long, env = "PROOF_GUEST_RUN_AS_UID")]
    run_as_uid: Option<u32>,
    /// gid adaptors run as (defaults to the uid).
    #[arg(long, env = "PROOF_GUEST_RUN_AS_GID")]
    run_as_gid: Option<u32>,
    /// Accept plain http:// artefact locators (staging artefact hosts only).
    #[arg(long, env = "PROOF_GUEST_ALLOW_PLAIN_HTTP", default_value_t = false)]
    allow_plain_http: bool,
}

fn config(cli: &Cli) -> GuestConfig {
    GuestConfig {
        secrets_dir: cli.secrets_dir.clone(),
        pack_root: cli.pack_root.clone(),
        work_root: cli.work_root.clone(),
        runners_dir: cli.runners_dir.clone(),
        run_as: cli
            .run_as_uid
            .map(|uid| (uid, cli.run_as_gid.unwrap_or(uid))),
        allow_plain_http: cli.allow_plain_http,
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let _ = telemetry::init_tracing();
    let cli = Cli::parse();
    let cfg = config(&cli);
    tracing::info!(
        agent = AGENT_NAME,
        runners_dir = %cfg.runners_dir.display(),
        pack_root = %cfg.pack_root.display(),
        work_root = %cfg.work_root.display(),
        run_as = ?cfg.run_as,
        allow_plain_http = cfg.allow_plain_http,
        "guest agent starting"
    );
    let agent = GuestAgent::new(cfg);
    if cli.stdio {
        let io = tokio::io::join(tokio::io::stdin(), tokio::io::stdout());
        return match agent.serve_connection(io).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                tracing::error!("stdio session: {e}");
                ExitCode::from(1)
            }
        };
    }
    match serve_vsock(agent, cli.vsock_port).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!("{e}");
            ExitCode::from(1)
        }
    }
}

/// Accept host connections on the guest vsock port until told to stop.
async fn serve_vsock(agent: Arc<GuestAgent>, port: u32) -> Result<(), String> {
    use tokio_vsock::{VsockAddr, VsockListener, VMADDR_CID_ANY};
    let listener = VsockListener::bind(VsockAddr::new(VMADDR_CID_ANY, port)).map_err(|e| {
        format!("bind vsock port {port}: {e} (is CONFIG_VIRTIO_VSOCKETS in the guest kernel?)")
    })?;
    tracing::info!(port, "listening on vsock");
    loop {
        let accepted = tokio::select! {
            r = listener.accept() => r,
            _ = tokio::signal::ctrl_c() => return Ok(()),
        };
        match accepted {
            Ok((stream, peer)) => {
                tracing::debug!(cid = peer.cid(), port = peer.port(), "host connected");
                let agent = agent.clone();
                tokio::spawn(async move {
                    if let Err(e) = agent
                        .serve_connection_resending(
                            stream,
                            || async move { reconnect_host(port).await },
                        )
                        .await
                    {
                        tracing::warn!("host connection ended with an error: {e}");
                    }
                });
            }
            Err(e) => {
                tracing::warn!("vsock accept: {e}");
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            }
        }
    }
}

/// Guest → host CID 2, same job port: rewrite Done/Failed after Broken pipe.
async fn reconnect_host(port: u32) -> Result<tokio_vsock::VsockStream, ProtoError> {
    use tokio_vsock::{VsockAddr, VsockStream, VMADDR_CID_HOST};
    VsockStream::connect(VsockAddr::new(VMADDR_CID_HOST, port))
        .await
        .map_err(|e| ProtoError::Io(format!("reconnect host vsock {port}: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_baked_image_layout() {
        let cli = Cli::try_parse_from(["proof-vm-guest-agent"]).expect("cli");
        let cfg = config(&cli);
        assert_eq!(cli.vsock_port, 5000);
        assert!(!cli.stdio);
        assert_eq!(cfg.secrets_dir, PathBuf::from("/run/proof/secrets"));
        assert_eq!(cfg.pack_root, PathBuf::from("/var/lib/proof/packs"));
        assert_eq!(cfg.work_root, PathBuf::from("/var/lib/proof/work"));
        assert_eq!(cfg.runners_dir, PathBuf::from("/opt/proof/runners"));
        assert_eq!(cfg.run_as, None);
        assert!(!cfg.allow_plain_http);
        let cli = Cli::try_parse_from([
            "proof-vm-guest-agent",
            "--stdio",
            "--run-as-uid",
            "1000",
            "--allow-plain-http",
        ])
        .expect("cli");
        let cfg = config(&cli);
        assert!(cli.stdio);
        assert_eq!(cfg.run_as, Some((1000, 1000)), "gid defaults to the uid");
        assert!(cfg.allow_plain_http);
    }
}
