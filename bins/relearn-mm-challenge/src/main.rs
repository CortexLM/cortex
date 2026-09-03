//! `relearn-mm-challenge` — master-only Relearn Multimodal service (port 8098).
//!
//! Miner HTTP submit → digest freeze → text-intact rerun → vision holdout +
//! agentic traces (with the pixel-shuffle control) → operator-audited promote.
//! Miners pay Lium.
//!
//! `--champion-lm-hash` is the reference gate 1 measures against. Without it an
//! encoder-only submission cannot prove it left the language model alone, so
//! those submissions are rejected until an operator supplies it.

#![forbid(unsafe_code)]

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use challenge_keys::load_challenge_secret;
use clap::Parser;
use relearn_mm_challenge::{
    hash_admin_token, relearn_mm_router, AppState, EvalBackend, MemoryStore, RelearnMmPin,
    CHALLENGE_ID, SCORING_VERSION,
};
use relearn_mm_eval::{base_champion_scores, resolve_backend};
use tokio::net::TcpListener;

/// Operator Relearn Multimodal challenge service CLI.
#[derive(Debug, Parser)]
#[command(
    name = "relearn-mm-challenge",
    about = "Relearn Multimodal challenge service (port 8098, master→Lium/sim)"
)]
struct Cli {
    /// Bind address (default 0.0.0.0:8098).
    #[arg(long, env = "BASE_CHALLENGE_BIND", default_value = "0.0.0.0:8098")]
    bind: SocketAddr,
    /// Challenge mini-secret file (leaf signatures).
    #[arg(long, env = "BASE_CHALLENGE_SK_FILE")]
    challenge_sk_file: Option<PathBuf>,
    /// Force sim eval (no Lium spend).
    ///
    /// `RELEARN_MM_FORCE_SIM` is read by `resolve_backend`, which accepts
    /// `1` / `true` / `yes`, so it is deliberately not bound here.
    #[arg(long, default_value_t = false)]
    force_sim: bool,
    /// Operator bearer tokens file (one per line). Empty → admin 503.
    #[arg(long, env = "RELEARN_MM_ADMIN_TOKENS_FILE")]
    admin_tokens_file: Option<PathBuf>,
    /// Pin file (`config/relearn-mm-pin.toml`).
    #[arg(long, env = "RELEARN_MM_PIN_FILE")]
    pin_file: Option<PathBuf>,
    /// SHA-256 hex of the champion Relearn LLM weights (gate 1 reference).
    #[arg(long, env = "RELEARN_MM_CHAMPION_LM_HASH")]
    champion_lm_hash: Option<String>,
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
    let pin = load_pin(cli.pin_file.as_deref())?;
    let backend = if cli.force_sim {
        tracing::info!("RELEARN_MM_FORCE_SIM=1 — deterministic offline eval, not a real eval");
        EvalBackend::Sim
    } else {
        resolve_backend()
    };

    let champion_lm_hash = cli
        .champion_lm_hash
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or_default()
        .to_owned();
    if champion_lm_hash.is_empty() {
        tracing::warn!(
            "RELEARN_MM_CHAMPION_LM_HASH unset — encoder-only submissions cannot prove the \
             language model is unchanged and will be rejected"
        );
    }

    let store = MemoryStore::new();
    store
        .set_champion_lm_hash(&champion_lm_hash)
        .map_err(|e| e.to_string())?;
    store
        .set_base_champion(base_champion_scores(&pin, &champion_lm_hash))
        .map_err(|e| e.to_string())?;

    let state = AppState {
        store,
        pin,
        backend,
        admin_hashes: Arc::new(load_admin_hashes(cli.admin_tokens_file.as_deref())),
    };
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    rt.block_on(serve(cli.bind, state))
}

fn load_pin(path: Option<&Path>) -> Result<RelearnMmPin, String> {
    let Some(p) = path else {
        return Ok(RelearnMmPin::default());
    };
    let body = std::fs::read_to_string(p).map_err(|e| format!("read {}: {e}", p.display()))?;
    RelearnMmPin::from_toml(&body).map_err(|e| e.to_string())
}

fn load_admin_hashes(path: Option<&Path>) -> Vec<String> {
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
    let app = relearn_mm_router(state);
    let listener = TcpListener::bind(bind)
        .await
        .map_err(|e| format!("bind {bind}: {e}"))?;
    tracing::info!(
        %bind,
        challenge_id = CHALLENGE_ID,
        scoring_version = SCORING_VERSION,
        lm_base_model = relearn_mm_task::LM_BASE_MODEL_ID,
        encoder_model = relearn_mm_task::ENCODER_MODEL_ID,
        "relearn-mm-challenge listening"
    );
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .map_err(|e| e.to_string())
}
