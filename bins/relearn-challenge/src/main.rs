//! `relearn-challenge` — master-only Relearn service (port 8095).
//!
//! Miner HTTP submit → digest freeze → holdout unseal → sim/Lium eval →
//! operator-audited promote. No TDX / Phala CVM.

#![forbid(unsafe_code)]

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use challenge_keys::load_challenge_secret;
use clap::Parser;
use relearn_challenge::{
    hash_admin_token, relearn_router, AppState, MemoryStore, CHALLENGE_ID, SCORING_VERSION,
};
use relearn_eval::{base_champion_scores, RelearnPin};
use tokio::net::TcpListener;

/// Operator Relearn challenge service CLI.
#[derive(Debug, Parser)]
#[command(
    name = "relearn-challenge",
    about = "Relearn challenge service (port 8095, master→Lium/sim, no CVM)"
)]
struct Cli {
    /// Bind address (default 0.0.0.0:8095).
    #[arg(long, env = "BASE_CHALLENGE_BIND", default_value = "0.0.0.0:8095")]
    bind: SocketAddr,
    /// Challenge mini-secret file (leaf signatures).
    #[arg(long, env = "BASE_CHALLENGE_SK_FILE")]
    challenge_sk_file: Option<PathBuf>,
    /// Force sim eval (no Lium spend).
    #[arg(long, env = "RELEARN_FORCE_SIM", default_value_t = false)]
    force_sim: bool,
    /// Operator bearer tokens file (one per line). Empty → admin 503.
    #[arg(long, env = "RELEARN_ADMIN_TOKENS_FILE")]
    admin_tokens_file: Option<PathBuf>,
    /// Pin file (`config/relearn-pin.toml`).
    #[arg(long, env = "RELEARN_PIN_FILE")]
    pin_file: Option<PathBuf>,
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
    let pin = load_pin(cli.pin_file.as_deref());
    if cli.force_sim {
        tracing::info!("RELEARN_FORCE_SIM=1 — sim eval only");
    }
    let admin_hashes = load_admin_hashes(cli.admin_tokens_file.as_deref());
    let store = MemoryStore::new();
    store
        .set_base_champion(base_champion_scores())
        .map_err(|e| e.to_string())?;
    let state = AppState {
        store,
        pin,
        admin_hashes: Arc::new(admin_hashes),
    };
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    rt.block_on(serve(cli.bind, state))
}

fn load_pin(path: Option<&std::path::Path>) -> RelearnPin {
    path.and_then(|p| std::fs::read_to_string(p).ok())
        .map(|s| RelearnPin::from_toml(&s))
        .unwrap_or_default()
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

async fn serve(bind: SocketAddr, state: AppState) -> Result<(), String> {
    let app = relearn_router(state);
    let listener = TcpListener::bind(bind)
        .await
        .map_err(|e| format!("bind {bind}: {e}"))?;
    tracing::info!(
        %bind,
        challenge_id = CHALLENGE_ID,
        scoring_version = SCORING_VERSION,
        "relearn-challenge listening"
    );
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .map_err(|e| e.to_string())
}
