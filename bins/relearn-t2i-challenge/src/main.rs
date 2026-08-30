//! `relearn-t2i-challenge` — master-only Relearn T2I service (port 8097).
//!
//! Miner HTTP submit → digest freeze → holdout unseal → frozen-cell generation
//! → Q-Judger scoring → operator-audited promote. Miners pay Lium.
//!
//! The holdout prompt records are loaded from an operator file and verified
//! against the commitment in `config/relearn-t2i-pin.toml`. If that file is
//! absent or does not match, the service still serves `/health` and
//! `/v1/status` but refuses submissions: scoring against the public split
//! instead would silently turn the anti-overfit gate off.

#![forbid(unsafe_code)]

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use challenge_keys::load_challenge_secret;
use clap::Parser;
use relearn_t2i_challenge::{
    hash_admin_token, parse_holdout_file, relearn_t2i_router, AppState, JudgeConfig, MemoryStore,
    RelearnT2iPin, CHALLENGE_ID, SCORING_VERSION,
};
use relearn_t2i_eval::base_champion_scores;
use tokio::net::TcpListener;

/// Operator Relearn T2I challenge service CLI.
#[derive(Debug, Parser)]
#[command(
    name = "relearn-t2i-challenge",
    about = "Relearn T2I challenge service (port 8097, master→Lium/sim)"
)]
struct Cli {
    /// Bind address (default 0.0.0.0:8097).
    #[arg(long, env = "BASE_CHALLENGE_BIND", default_value = "0.0.0.0:8097")]
    bind: SocketAddr,
    /// Challenge mini-secret file (leaf signatures).
    #[arg(long, env = "BASE_CHALLENGE_SK_FILE")]
    challenge_sk_file: Option<PathBuf>,
    /// Force sim eval (no Lium spend, no Q-Judger endpoint).
    ///
    /// `RELEARN_T2I_FORCE_SIM` is read by [`JudgeConfig::from_env`], which
    /// accepts `1` / `true` / `yes`, so it is deliberately not bound here.
    #[arg(long, default_value_t = false)]
    force_sim: bool,
    /// Operator bearer tokens file (one per line). Empty → admin 503.
    #[arg(long, env = "RELEARN_T2I_ADMIN_TOKENS_FILE")]
    admin_tokens_file: Option<PathBuf>,
    /// Pin file (`config/relearn-t2i-pin.toml`).
    #[arg(long, env = "RELEARN_T2I_PIN_FILE")]
    pin_file: Option<PathBuf>,
    /// Operator holdout prompt records (JSON array). Never in git.
    #[arg(long, env = "RELEARN_T2I_HOLDOUT_FILE")]
    holdout_file: Option<PathBuf>,
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
    let judge = if cli.force_sim {
        tracing::info!("RELEARN_T2I_FORCE_SIM=1 — deterministic offline judge, not a real eval");
        JudgeConfig::sim()
    } else {
        JudgeConfig::from_env()
    };

    let store = MemoryStore::new();
    store
        .set_holdout_commitment(&pin.prompts.holdout_commitment, pin.prompts.holdout_size)
        .map_err(|e| e.to_string())?;
    match load_holdout(&store, &pin, cli.holdout_file.as_deref()) {
        Ok(n) => tracing::info!(
            holdout_prompts = n,
            "holdout verified against pin commitment"
        ),
        Err(e) => tracing::warn!("holdout unavailable ({e}); submissions will 503 until fixed"),
    }
    let holdout_ids = store
        .unseal_holdout("boot-baseline")
        .map(|recs| recs.iter().map(|p| p.id).collect::<Vec<u32>>())
        .unwrap_or_default();
    if !holdout_ids.is_empty() {
        let base = base_champion_scores(&pin, &holdout_ids).map_err(|e| e.to_string())?;
        store.set_base_champion(base).map_err(|e| e.to_string())?;
    }

    let state = AppState {
        store,
        pin,
        judge,
        admin_hashes: Arc::new(load_admin_hashes(cli.admin_tokens_file.as_deref())),
    };
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    rt.block_on(serve(cli.bind, state))
}

fn load_pin(path: Option<&Path>) -> Result<RelearnT2iPin, String> {
    let Some(p) = path else {
        return Ok(RelearnT2iPin::default());
    };
    let body = std::fs::read_to_string(p).map_err(|e| format!("read {}: {e}", p.display()))?;
    RelearnT2iPin::from_toml(&body).map_err(|e| e.to_string())
}

fn load_holdout(
    store: &MemoryStore,
    pin: &RelearnT2iPin,
    path: Option<&Path>,
) -> Result<usize, String> {
    let p = path.ok_or("RELEARN_T2I_HOLDOUT_FILE not set")?;
    let body = std::fs::read_to_string(p).map_err(|e| format!("read {}: {e}", p.display()))?;
    let records = parse_holdout_file(&body)?;
    let n = records.len();
    store
        .load_holdout(records, &pin.prompts.public_ids)
        .map_err(|e| e.to_string())?;
    Ok(n)
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
    let app = relearn_t2i_router(state);
    let listener = TcpListener::bind(bind)
        .await
        .map_err(|e| format!("bind {bind}: {e}"))?;
    tracing::info!(
        %bind,
        challenge_id = CHALLENGE_ID,
        scoring_version = SCORING_VERSION,
        base_model = relearn_t2i_task::BASE_MODEL_ID,
        judge_model = relearn_t2i_task::JUDGE_MODEL_ID,
        "relearn-t2i-challenge listening"
    );
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .map_err(|e| e.to_string())
}
