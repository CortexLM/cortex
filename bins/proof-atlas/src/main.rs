//! Separate opt-in service. No migrations, rentals, sealer calls or chain writes.
#![forbid(unsafe_code)]

mod agent;
mod private;

use clap::Parser;
use private::{private_bytes, private_file};
use proof_atlas_worker::{AtlasAgent, AtlasConfig, AtlasStore, AtlasWorker};
use proof_autonomy::commitment;
use proof_research::ResearchStore;
use proof_rounds::{GatewayRoundPublisher, RoundConfig};
use proof_task::ProofPin;
use proof_worker::{HeadlessConfig, HeadlessProcess};
use serde::Deserialize;
use std::{path::PathBuf, process::ExitCode, sync::Arc};
use tokio::sync::watch;

#[derive(Parser)]
struct Cli {
    /// Explicit operator-private configuration; no environment credential fallback.
    #[arg(long, env = "PROOF_ATLAS_CONFIG_FILE")]
    config: PathBuf,
    /// Validate files, signer, policy and restricted migrated DB; do not start agents.
    #[arg(long)]
    check: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    schema_version: u32,
    database_url_file: PathBuf,
    proof_secret_file: PathBuf,
    proof_pin_file: PathBuf,
    chain_endpoints: String,
    gateway_url: String,
    netuid: u16,
    anchor_block: u64,
    policy: String,
    headless: HeadlessConfig,
}

#[tokio::main]
async fn main() -> ExitCode {
    match run(Cli::parse()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> Result<(), &'static str> {
    let config: Config = serde_json::from_str(&private_file(&cli.config, 256 * 1024)?)
        .map_err(|_| "invalid Atlas configuration")?;
    if config.schema_version != 1
        || config.policy.trim().is_empty()
        || config.policy.len() > 64 * 1024
        || config.chain_endpoints.trim().is_empty()
    {
        return Err("invalid Atlas configuration");
    }
    let secret =
        challenge_keys::parse_challenge_secret(&private_bytes(&config.proof_secret_file, 256)?)
            .map_err(|_| "invalid private Proof signing key")?;
    let public =
        challenge_common::public_key_from_secret(&secret).map_err(|_| "invalid Proof signer")?;
    let pin = ProofPin::from_toml(&private_file(&config.proof_pin_file, 128 * 1024)?)
        .map_err(|_| "invalid Proof pin")?;
    pin.validate().map_err(|_| "invalid Proof pin")?;
    if pin.topic_pubkey != hex::encode(public) {
        return Err("Proof signer differs from topic trust pin");
    }
    let agent = Arc::new(agent::Agent {
        process: HeadlessProcess::new(config.headless)
            .map_err(|_| "invalid private headless configuration")?,
        policy: config.policy,
    });
    let rounds = RoundConfig {
        netuid: config.netuid,
        anchor_block: config.anchor_block,
        policy_digest: commitment(&agent.policy).map_err(|_| "invalid Atlas policy")?,
        runtime_digest: agent.binding().map_err(|_| "invalid runtime commitment")?,
        proof_public_key: hex::encode(public),
    };
    let pool = db::connect(&private_file(&config.database_url_file, 16 * 1024)?)
        .await
        .map_err(|_| "Atlas database unavailable")?;
    let store = AtlasStore::new(pool.clone(), ResearchStore::new(pool, pin), rounds);
    store.ready().await.map_err(|_| {
        "Atlas database requires owner-applied migrations and restricted privileges"
    })?;
    let publisher = GatewayRoundPublisher::new(&config.gateway_url, public)
        .map_err(|_| "invalid strict gateway endpoint")?;
    // The existing chain client owns a blocking HTTP runtime.
    let worker = tokio::task::spawn_blocking(move || {
        let mut chain = chain_live::LiveChainClient::connect(&config.chain_endpoints)
            .map_err(|_| "invalid finalized-chain endpoint")?;
        chain.set_netuid(config.netuid);
        AtlasWorker::new(
            store,
            Arc::new(chain),
            agent,
            Arc::new(publisher),
            secret,
            AtlasConfig::default(),
        )
        .map_err(|_| "invalid Atlas worker binding")
    })
    .await
    .map_err(|_| "Atlas chain client initialization failed")??;
    let result = supervise(&worker, cli.check).await;
    tokio::task::spawn_blocking(move || drop(worker))
        .await
        .map_err(|_| "Atlas chain client shutdown failed")?;
    result
}

async fn supervise(worker: &AtlasWorker, check: bool) -> Result<(), &'static str> {
    if check {
        println!("Atlas configuration and restricted database validated; no worker started");
        return Ok(());
    }
    let (stop, signal) = watch::channel(false);
    let work = worker.run(signal);
    tokio::pin!(work);
    tokio::select! {
        result = &mut work => result.map_err(|_| "Atlas controller stopped"),
        () = shutdown() => { let _ = stop.send(true); work.await.map_err(|_| "Atlas shutdown failed") }
    }
}

async fn shutdown() {
    let Ok(mut terminate) =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
    else {
        return;
    };
    tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = terminate.recv() => {} }
}
