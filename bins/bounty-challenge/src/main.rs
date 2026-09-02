//! `bounty-challenge` — master-only Bounty service (port 8096).
//!
//! Internal ingest: pair hotkey ↔ Cortex Chat account, accept bug reports,
//! operator adjudicate. Scoring/weights **read** the CortexLM/backend public
//! feed (`BOUNTY_BACKEND_PUBLIC_URL`) and turn published rows into signed
//! leaves on the gateway. This binary does not serve a public leaderboard.
//! Validators never evaluate reports; they verify sealed bundles.
//!
//! The feed is the only scorer. Without it ingest answers 503 and the emitter
//! signs nothing, so the challenge share burns to uid 0 instead of paying on
//! numbers no validator could reproduce.

#![forbid(unsafe_code)]

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use bounty_challenge::{
    bounty_router, hash_admin_token, legacy_sim_opt_in_present, resolve_scoring_backend, AppState,
    BountyEmitter, BountyStore, GatewayClient, GatewayClientConfig, ScoringBackend, CHALLENGE_ID,
    DEFAULT_EMIT_POLL_SECS, SCORING_VERSION,
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
    /// CortexLM/backend public base URL. Empty → this host cannot score:
    /// ingest 503s and no leaf is signed. Never bake a host; operators set
    /// this on the host.
    #[arg(long, env = "BOUNTY_BACKEND_PUBLIC_URL")]
    backend_public_url: Option<String>,
    /// Netuid the expected set is derived from.
    #[arg(long, env = "BASE_NETUID", default_value_t = 1)]
    netuid: u16,
    /// Chain WS endpoint (`BASE_CHAIN_ENDPOINTS` wins when it carries a list).
    #[arg(
        long,
        env = "BASE_CHAIN_ENDPOINT",
        default_value = "wss://test.finney.opentensor.ai:443"
    )]
    chain_endpoint: String,
    /// Gateway base URL for `POST /v1/weights/raw`.
    #[arg(
        long,
        env = "BASE_CHALLENGE_GATEWAY_ENDPOINT",
        default_value = "http://gateway:8080"
    )]
    gateway_endpoint: String,
    /// Seconds between emitter ticks.
    #[arg(long, env = "BOUNTY_EMIT_POLL_SECS", default_value_t = DEFAULT_EMIT_POLL_SECS)]
    emit_poll_secs: u64,
}

fn main() -> ExitCode {
    let _ = telemetry::init_tracing();
    let mut cli = Cli::parse();
    // Ordered failover list wins over the single-endpoint flag/env.
    if let Ok(list) = std::env::var("BASE_CHAIN_ENDPOINTS") {
        if !list.trim().is_empty() {
            cli.chain_endpoint = list;
        }
    }
    match run(&cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!("{e}");
            ExitCode::from(1)
        }
    }
}

fn run(cli: &Cli) -> Result<(), String> {
    let sk = match &cli.challenge_sk_file {
        Some(p) => Some(load_challenge_secret(p).map_err(|e| format!("challenge sk: {e}"))?),
        None => None,
    };
    let admin_hashes = load_admin_hashes(cli.admin_tokens_file.as_deref());
    let session_secret = load_session_secret(cli.session_secret_file.as_deref())?;
    // The CLI flag and the env var are the same knob; `resolve_scoring_backend`
    // reads the env, so a flag-only invocation has to publish it first.
    if let Some(url) = cli
        .backend_public_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        std::env::set_var("BOUNTY_BACKEND_PUBLIC_URL", url);
    }
    if legacy_sim_opt_in_present() {
        tracing::warn!(
            "BOUNTY_FORCE_SIM is set and is ignored: adjudication happens in CortexLM/backend, \
             so there is no offline scorer to fall back to. Set BOUNTY_BACKEND_PUBLIC_URL"
        );
    }
    let scoring = resolve_scoring_backend();
    match scoring {
        ScoringBackend::BackendPublic => {
            tracing::info!("bounty scoring reads the CortexLM/backend public API");
        }
        // Reports would be real work this host could never pay for, so ingest
        // refuses rather than banking them, and the emitter signs nothing.
        ScoringBackend::Unconfigured => tracing::warn!(
            "no scoring backend: set BOUNTY_BACKEND_PUBLIC_URL. POST /v1/reports will answer \
             503 and no leaf will be emitted (the challenge share burns to uid 0) until then"
        ),
    }
    let state = AppState {
        store: BountyStore::new(),
        session_secret: Arc::new(session_secret),
        scoring,
        admin_hashes: Arc::new(admin_hashes),
    };
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    if let Some(emitter) = build_emitter(cli, scoring, sk)? {
        let poll = Duration::from_secs(cli.emit_poll_secs.max(1));
        rt.spawn(emitter.run(poll));
    }
    rt.block_on(serve(cli.bind, state))
}

/// Wire the leaf emitter, or explain why this host will not emit.
///
/// Both refusals are load-bearing: an unconfigured host has no adjudications
/// to pay on, and a host with no challenge key cannot sign a leaf the trust
/// root would accept. Neither is a reason to invent scores.
fn build_emitter(
    cli: &Cli,
    scoring: ScoringBackend,
    sk: Option<[u8; 32]>,
) -> Result<Option<Arc<BountyEmitter<chain_live::LiveChainClient>>>, String> {
    if scoring != ScoringBackend::BackendPublic {
        return Ok(None);
    }
    let Some(sk) = sk else {
        tracing::warn!(
            "no BASE_CHALLENGE_SK_FILE: bounty cannot sign leaves, so nothing will be emitted"
        );
        return Ok(None);
    };
    let gateway = Arc::new(
        GatewayClient::new(GatewayClientConfig {
            base_url: cli.gateway_endpoint.clone(),
            ..GatewayClientConfig::default()
        })
        .map_err(|e| format!("gateway client: {e}"))?,
    );
    let mut chain = chain_live::LiveChainClient::connect(&cli.chain_endpoint)
        .map_err(|e| format!("chain connect: {e}"))?;
    chain.set_netuid(cli.netuid);
    tracing::info!(
        netuid = cli.netuid,
        gateway = %cli.gateway_endpoint,
        poll_secs = cli.emit_poll_secs,
        "bounty emitter wired: backend public rows → signed leaves"
    );
    Ok(Some(Arc::new(BountyEmitter::new(
        chain,
        gateway,
        sk,
        cli.netuid,
        cli.backend_public_url.clone(),
    ))))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn cli() -> Cli {
        Cli::try_parse_from(["bounty-challenge"]).expect("defaults parse")
    }

    /// The boot path, not just the library: a host with no feed wires no
    /// emitter, so it cannot post a leaf set nobody adjudicated.
    #[test]
    fn an_unconfigured_host_wires_no_emitter() {
        let wired = build_emitter(&cli(), ScoringBackend::Unconfigured, Some([3u8; 32]))
            .expect("no emitter is not an error");
        assert!(wired.is_none());
    }

    /// A leaf the trust root would reject is not weight, so a missing
    /// challenge key is a refusal rather than an unsigned emit.
    #[test]
    fn no_challenge_key_wires_no_emitter() {
        assert!(build_emitter(&cli(), ScoringBackend::BackendPublic, None)
            .expect("no emitter is not an error")
            .is_none());
    }

    #[test]
    fn a_configured_feed_and_a_key_wire_the_emitter() {
        let wired = build_emitter(&cli(), ScoringBackend::BackendPublic, Some([3u8; 32]))
            .expect("wire")
            .expect("emitter");
        assert_eq!(wired.emitted_epoch(), 0);
    }

    /// `BOUNTY_FORCE_SIM` used to select an offline scorer. It is retired, and
    /// setting it must not resurrect one at boot.
    #[test]
    fn the_retired_sim_opt_in_does_not_select_a_scorer_at_boot() {
        std::env::set_var("BOUNTY_FORCE_SIM", "1");
        assert!(legacy_sim_opt_in_present());
        assert_eq!(resolve_scoring_backend(), ScoringBackend::Unconfigured);
        assert!(
            build_emitter(&cli(), resolve_scoring_backend(), Some([3u8; 32]))
                .expect("wire")
                .is_none()
        );
        std::env::remove_var("BOUNTY_FORCE_SIM");
    }
}
