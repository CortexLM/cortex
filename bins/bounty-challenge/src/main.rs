//! `bounty-challenge` — master-only Bounty service (port 8096).
//!
//! Internal ingest: pair hotkey ↔ Cortex Chat account, accept bug reports,
//! operator adjudicate. Scoring/weights **read** the CortexLM/backend public
//! feed (`BOUNTY_BACKEND_PUBLIC_URL`). This binary does not serve a public
//! leaderboard. Validators never evaluate reports; they verify sealed bundles.

#![forbid(unsafe_code)]

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use bounty_challenge::{
    backend_public_url, bounty_router, hash_admin_token, AppState, BountyStore, CHALLENGE_ID,
    SCORING_VERSION,
};
use challenge_keys::load_challenge_secret;
use clap::Parser;
use tokio::net::TcpListener;

/// Operator Bounty challenge service CLI.
#[derive(Debug, Parser)]
#[command(
    name = "bounty-challenge",
    about = "Bounty challenge service (port 8096, master-only)"
)]
struct Cli {
    /// Bind address (default 0.0.0.0:8096).
    #[arg(long, env = "BASE_CHALLENGE_BIND", default_value = "0.0.0.0:8096")]
    bind: SocketAddr,
    /// Challenge mini-secret file (leaf signatures).
    #[arg(long, env = "BASE_CHALLENGE_SK_FILE")]
    challenge_sk_file: Option<PathBuf>,
    /// Operator bearer tokens file (one per line). Empty → admin 503.
    #[arg(long, env = "BOUNTY_ADMIN_TOKENS_FILE")]
    admin_tokens_file: Option<PathBuf>,
    /// Session HMAC secret file. Random-at-boot when omitted (dev only).
    #[arg(long, env = "BOUNTY_SESSION_SECRET_FILE")]
    session_secret_file: Option<PathBuf>,
    /// CortexLM/backend public base URL. Empty → skip fetch (CI / sim).
    /// Never bake a host; operators set this on the host.
    #[arg(long, env = "BOUNTY_BACKEND_PUBLIC_URL")]
    backend_public_url: Option<String>,
}

fn main() -> ExitCode {
    let _ = telemetry::init_tracing();
    let cli = Cli::parse();
    match run(&cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!("{e}");
            ExitCode::from(1)
        }
    }
}

fn run(cli: &Cli) -> Result<(), String> {
    if let Some(p) = &cli.challenge_sk_file {
        let _sk = load_challenge_secret(p).map_err(|e| format!("challenge sk: {e}"))?;
    }
    let admin_hashes = load_admin_hashes(cli.admin_tokens_file.as_deref());
    let session_secret = load_session_secret(cli.session_secret_file.as_deref())?;
    if cli
        .backend_public_url
        .as_deref()
        .map(str::trim)
        .is_some_and(|s| !s.is_empty())
        || backend_public_url().is_some()
    {
        tracing::info!("bounty scoring reads CortexLM/backend public API");
    } else {
        tracing::info!("BOUNTY_BACKEND_PUBLIC_URL unset — skip backend public fetch (sim/CI)");
    }
    let state = AppState {
        store: BountyStore::new(),
        session_secret: Arc::new(session_secret),
        admin_hashes: Arc::new(admin_hashes),
    };
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    rt.block_on(serve(cli.bind, state))
}

fn load_admin_hashes(path: Option<&std::path::Path>) -> Vec<String> {
    let Some(p) = path else {
        return Vec::new();
    };
    let Ok(body) = std::fs::read_to_string(p) else {
        return Vec::new();
    };
    body.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(hash_admin_token)
        .collect()
}

fn load_session_secret(path: Option<&std::path::Path>) -> Result<Vec<u8>, String> {
    if let Some(p) = path {
        let bytes = std::fs::read(p).map_err(|e| format!("session secret: {e}"))?;
        if bytes.is_empty() {
            return Err("session secret file is empty".into());
        }
        return Ok(bytes);
    }
    // Dev / CI: ephemeral secret. Production should set BOUNTY_SESSION_SECRET_FILE.
    let mut out = vec![0u8; 32];
    getrandom_fill(&mut out)?;
    Ok(out)
}

fn getrandom_fill(buf: &mut [u8]) -> Result<(), String> {
    use std::fs::File;
    use std::io::Read;
    File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(buf))
        .map_err(|e| format!("urandom: {e}"))
}

async fn serve(bind: SocketAddr, state: AppState) -> Result<(), String> {
    let app = bounty_router(state);
    let listener = TcpListener::bind(bind)
        .await
        .map_err(|e| format!("bind {bind}: {e}"))?;
    tracing::info!(
        %bind,
        challenge_id = CHALLENGE_ID,
        scoring_version = SCORING_VERSION,
        "bounty-challenge listening"
    );
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .map_err(|e| e.to_string())
}
