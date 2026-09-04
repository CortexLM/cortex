//! `proof-challenge` — master-only Proof service (port 8100).
//!
//! Miner HTTP submit, digest freeze, per-topic holdout unseal, then RLM agent
//! and harness to lattice. Miners pay Lium. Topics are operator-published
//! signed documents, not a catalog in git.
//!
//! Without `PROOF_FORCE_SIM=1` the host needs a `sha256:` eval-image pin, a
//! wired harvest, at least one `open` topic with a verified holdout, and a
//! sealed baseline. Sim is never a fallback.

#![forbid(unsafe_code)]

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use challenge_keys::load_challenge_secret;
use clap::Parser;
use prism_lium::LiumClient;
use proof_challenge::{
    hash_admin_token, parse_holdout_file, proof_router, AppState, BaselineMeasurement, EvalBackend,
    InferenceOffer, LiveScorer, MemoryStore, ProofPin, TopicDocument, CHALLENGE_ID,
    SCORING_VERSION,
};
use proof_eval::supported_custom;
use proof_harvest::{HarvestLimits, LiumProofHarvest};
use tokio::net::TcpListener;

/// Operator Proof challenge service CLI.
#[derive(Debug, Parser)]
#[command(
    name = "proof-challenge",
    about = "Proof challenge service (port 8100, master→Lium/sim)"
)]
struct Cli {
    /// Bind address (default 0.0.0.0:8100).
    #[arg(long, env = "BASE_CHALLENGE_BIND", default_value = "0.0.0.0:8100")]
    bind: SocketAddr,
    /// Challenge mini-secret file (leaf signatures).
    #[arg(long, env = "BASE_CHALLENGE_SK_FILE")]
    challenge_sk_file: Option<PathBuf>,
    /// Force sim eval (no Lium spend). CI / local only — never a live scorer.
    ///
    /// `PROOF_FORCE_SIM` is read by `resolve_eval_backend`, which accepts
    /// `1` / `true` / `yes`, so it is deliberately not bound here: clap would
    /// reject the documented spelling and refuse to boot.
    #[arg(long, default_value_t = false)]
    force_sim: bool,
    /// Operator bearer tokens file (one per line). Empty → admin 503.
    #[arg(long, env = "PROOF_ADMIN_TOKENS_FILE")]
    admin_tokens_file: Option<PathBuf>,
    /// Pin file (`config/proof-pin.toml`).
    #[arg(long, env = "PROOF_PIN_FILE")]
    pin_file: Option<PathBuf>,
    /// Signed topic documents (JSON array). Never a holdout catalog.
    #[arg(long, env = "PROOF_TOPICS_FILE")]
    topics_file: Option<PathBuf>,
    /// Operator holdout records (JSON array or map keyed by topic id). Never in git.
    #[arg(long, env = "PROOF_HOLDOUT_FILE")]
    holdout_file: Option<PathBuf>,
    /// Seconds the eval image gets to score one artifact on the pod.
    #[arg(long, env = "PROOF_EVAL_TIMEOUT_SECS", default_value_t = 5400)]
    eval_timeout_secs: u64,
    /// Sealed baseline measurements (JSON map keyed by topic id).
    #[arg(long, env = "PROOF_BASELINE_FILE")]
    baseline_file: Option<PathBuf>,
    /// Live `InferenceOffer` JSON. Operator state; never a git pin. Missing/closed → 503.
    #[arg(long, env = "PROOF_INFERENCE_OFFER_FILE")]
    inference_offer_file: Option<PathBuf>,
    /// Provider API key file. Never logged, never on `/v1/status`.
    #[arg(long, env = "PROOF_INFERENCE_API_KEY_FILE")]
    inference_api_key_file: Option<PathBuf>,
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
        tracing::info!("PROOF_FORCE_SIM=1 — deterministic offline eval, not a real eval");
        EvalBackend::Sim
    } else {
        proof_challenge::resolve_eval_backend()
    };
    if backend == EvalBackend::Lium && !pin.can_rent() {
        tracing::warn!(
            "eval_image_digest not pinned; submissions will 503 until CortexLM CI \
             publishes a sha256 proof-eval image (sim is opt-in via PROOF_FORCE_SIM, \
             never a fallback)"
        );
    }

    let store = MemoryStore::new();
    match load_topics(&store, &pin, cli.topics_file.as_deref()) {
        Ok(n) => tracing::info!(topics = n, "signed topics loaded"),
        Err(e) => tracing::warn!("topics unavailable ({e}); submissions will 400/503 until fixed"),
    }
    match load_holdouts(&store, cli.holdout_file.as_deref()) {
        Ok(n) => tracing::info!(topics = n, "holdouts verified against topic commitments"),
        Err(e) => tracing::warn!("holdouts unavailable ({e}); submissions will 503 until fixed"),
    }
    match load_baselines(&store, &pin, cli.baseline_file.as_deref()) {
        Ok(n) => tracing::info!(topics = n, "sealed baselines recorded"),
        Err(e) => tracing::warn!("baselines unavailable ({e}); submissions will 503 until fixed"),
    }
    let offer = match load_offer(&pin, cli.inference_offer_file.as_deref()) {
        Ok(o) => {
            tracing::info!(offer_id = %o.offer_id, status = ?o.status, "inference offer loaded");
            Some(o)
        }
        Err(e) => {
            tracing::warn!("inference offer unavailable ({e}); submissions will 503 until fixed");
            None
        }
    };
    if let Some(p) = cli.inference_api_key_file.as_deref() {
        match std::fs::read_to_string(p) {
            Ok(s) if !s.trim().is_empty() => {
                tracing::info!("inference api key file present (contents not logged)");
            }
            _ => tracing::warn!(
                "PROOF_INFERENCE_API_KEY_FILE set but unreadable; eval image auth will fail closed"
            ),
        }
    }

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;

    let live_scorer = build_live_scorer(backend, cli.eval_timeout_secs);
    match backend {
        EvalBackend::Lium if live_scorer.is_some() => {
            tracing::info!("live harvest wired: digest-pinned proof-eval image on Lium");
        }
        EvalBackend::Lium => tracing::warn!(
            "live harvest not wired; every submission will 503. Set the Lium credentials \
             and LIUM_SSH_PUBLIC_KEY_FILE (deploy/env/proof-challenge.env.example)"
        ),
        EvalBackend::Sim => {}
    }

    let state = AppState {
        store,
        pin,
        backend,
        live_scorer,
        offer,
        admin_hashes: Arc::new(load_admin_hashes(cli.admin_tokens_file.as_deref())),
        epoch: 0,
    };
    rt.block_on(serve(cli.bind, state))
}

fn build_live_scorer(backend: EvalBackend, run_timeout_secs: u64) -> Option<Arc<dyn LiveScorer>> {
    if backend != EvalBackend::Lium {
        return None;
    }
    let key = std::env::var("LIUM_API_KEY")
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())?;
    let base_url = std::env::var("LIUM_API_BASE_URL")
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty());
    let built = match base_url {
        Some(url) => LiumClient::with_base_url(key, url),
        None => LiumClient::new(key),
    };
    let client = match built {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("lium client unavailable ({e}); live harvest not wired");
            return None;
        }
    };
    let Some(ssh_pub) = load_ssh_public_key() else {
        tracing::warn!(
            "live harvest not wired: no SSH public key (LIUM_SSH_PUBLIC_KEY_FILE or default)"
        );
        return None;
    };
    let pod = Arc::new(harvest_pod::LiumEvalPod::new(
        client,
        run_timeout_secs,
        proof_harvest::PROGRAM,
    ));
    Some(Arc::new(LiumProofHarvest::new(
        pod,
        HarvestLimits::default(),
        vec![ssh_pub],
    )))
}

fn load_ssh_public_key() -> Option<String> {
    let p = std::env::var("LIUM_SSH_PUBLIC_KEY_FILE").map_or_else(
        |_| PathBuf::from("/root/.config/prism-mission/lium_ssh_ed25519.pub"),
        PathBuf::from,
    );
    std::fs::read_to_string(p)
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

fn load_pin(path: Option<&Path>) -> Result<ProofPin, String> {
    let Some(p) = path else {
        return Ok(ProofPin::default());
    };
    let body = std::fs::read_to_string(p).map_err(|e| format!("read {}: {e}", p.display()))?;
    let pin = ProofPin::from_toml(&body).map_err(|e| e.to_string())?;
    pin.validate().map_err(|e| e.to_string())?;
    Ok(pin)
}

fn load_topics(store: &MemoryStore, pin: &ProofPin, path: Option<&Path>) -> Result<usize, String> {
    let p = path.ok_or("PROOF_TOPICS_FILE not set")?;
    let body = std::fs::read_to_string(p).map_err(|e| format!("read {}: {e}", p.display()))?;
    let docs = TopicDocument::many_from_json(&body).map_err(|e| e.to_string())?;
    let n = docs.len();
    for doc in docs {
        doc.validate(pin, &supported_custom())
            .map_err(|e| format!("topic {}: {e}", doc.id))?;
        doc.verify_signature(pin)
            .map_err(|e| format!("topic {}: {e}", doc.id))?;
        store.put_topic(doc).map_err(|e| e.to_string())?;
    }
    Ok(n)
}

fn load_holdouts(store: &MemoryStore, path: Option<&Path>) -> Result<usize, String> {
    let p = path.ok_or("PROOF_HOLDOUT_FILE not set")?;
    let body = std::fs::read_to_string(p).map_err(|e| format!("read {}: {e}", p.display()))?;
    let topics = store.topics().map_err(|e| e.to_string())?;
    if topics.is_empty() {
        return Err("no topics loaded; cannot attach a holdout".into());
    }
    let single = topics.len() == 1;
    let mut n = 0usize;
    for topic in topics {
        match parse_holdout_file(&body, &topic.id) {
            Ok(recs) => {
                store
                    .load_holdout(&topic.id, recs)
                    .map_err(|e| format!("holdout {}: {e}", topic.id))?;
                n = n.saturating_add(1);
            }
            Err(_) if single => {
                let recs: Vec<proof_task::HoldoutRecord> =
                    serde_json::from_str(&body).map_err(|e| format!("parse holdout array: {e}"))?;
                store
                    .load_holdout(&topic.id, recs)
                    .map_err(|e| format!("holdout {}: {e}", topic.id))?;
                return Ok(1);
            }
            Err(e) => tracing::warn!("no holdout for {}: {e}", topic.id),
        }
    }
    Ok(n)
}

fn load_baselines(
    store: &MemoryStore,
    pin: &ProofPin,
    path: Option<&Path>,
) -> Result<usize, String> {
    let p = path.ok_or("PROOF_BASELINE_FILE not set")?;
    let body = std::fs::read_to_string(p).map_err(|e| format!("read {}: {e}", p.display()))?;
    let value: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| format!("parse baseline: {e}"))?;
    let mut n = 0usize;
    if value.get("topic_id").is_some() {
        let meas = BaselineMeasurement::from_json(&body).map_err(|e| e.to_string())?;
        record_one_baseline(store, pin, meas)?;
        return Ok(1);
    }
    let map = value
        .as_object()
        .ok_or("baseline file must be one measurement or a map keyed by topic id")?;
    for (id, v) in map {
        let meas: BaselineMeasurement =
            serde_json::from_value(v.clone()).map_err(|e| format!("baseline {id}: {e}"))?;
        record_one_baseline(store, pin, meas)?;
        n = n.saturating_add(1);
    }
    Ok(n)
}

fn record_one_baseline(
    store: &MemoryStore,
    pin: &ProofPin,
    meas: BaselineMeasurement,
) -> Result<(), String> {
    let topic = store.topic(&meas.topic_id).map_err(|e| e.to_string())?;
    meas.verify(pin, &topic).map_err(|e| e.to_string())?;
    store
        .set_baseline(&topic.id, meas.into_sealed())
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn load_offer(pin: &ProofPin, path: Option<&Path>) -> Result<InferenceOffer, String> {
    let p = path.ok_or("PROOF_INFERENCE_OFFER_FILE not set")?;
    let body = std::fs::read_to_string(p).map_err(|e| format!("read {}: {e}", p.display()))?;
    let offer = InferenceOffer::from_json(&body).map_err(|e| e.to_string())?;
    offer.validate(pin).map_err(|e| e.to_string())?;
    Ok(offer)
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
    let app = proof_router(state);
    let listener = TcpListener::bind(bind)
        .await
        .map_err(|e| format!("bind {bind}: {e}"))?;
    tracing::info!(
        %bind,
        challenge_id = CHALLENGE_ID,
        scoring_version = SCORING_VERSION,
        "proof-challenge listening"
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

    #[test]
    fn documented_force_sim_values_do_not_break_argument_parsing() {
        for value in ["1", "true", "yes", "false", ""] {
            std::env::set_var("PROOF_FORCE_SIM", value);
            let cli = Cli::try_parse_from(["proof-challenge"])
                .unwrap_or_else(|e| panic!("PROOF_FORCE_SIM={value:?} broke parsing: {e}"));
            assert!(!cli.force_sim, "env must not set the flag");
        }
        std::env::set_var("PROOF_FORCE_SIM", "1");
        assert_eq!(proof_challenge::resolve_eval_backend(), EvalBackend::Sim);
        std::env::set_var("PROOF_FORCE_SIM", "false");
        assert_eq!(proof_challenge::resolve_eval_backend(), EvalBackend::Lium);
        std::env::remove_var("PROOF_FORCE_SIM");
    }

    #[test]
    fn live_harvest_is_wired_on_the_lium_path_not_on_sim() {
        let _guard = LIUM_ENV
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let pubkey = stub_ssh_pubkey("proof-wired");
        std::env::set_var("LIUM_API_KEY", "test-key-not-a-real-secret");
        std::env::set_var("LIUM_SSH_PUBLIC_KEY_FILE", &pubkey);

        assert!(
            build_live_scorer(EvalBackend::Lium, 900).is_some(),
            "Lium boot must wire the digest-pinned harvest"
        );
        assert!(
            build_live_scorer(EvalBackend::Sim, 900).is_none(),
            "sim scores in-process; a Lium harvest there would spend money"
        );

        std::env::remove_var("LIUM_API_KEY");
        assert!(build_live_scorer(EvalBackend::Lium, 900).is_none());
        std::env::set_var("LIUM_API_KEY", "   ");
        assert!(build_live_scorer(EvalBackend::Lium, 900).is_none());

        std::env::set_var("LIUM_API_KEY", "test-key-not-a-real-secret");
        std::env::set_var("LIUM_SSH_PUBLIC_KEY_FILE", "/nonexistent/id.pub");
        assert!(build_live_scorer(EvalBackend::Lium, 900).is_none());
        std::env::remove_var("LIUM_API_KEY");
        std::env::remove_var("LIUM_SSH_PUBLIC_KEY_FILE");
    }

    fn stub_ssh_pubkey(tag: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("proof-test-{tag}.pub"));
        std::fs::write(&path, "ssh-ed25519 AAAAtest proof-test\n").expect("write pubkey");
        path
    }

    static LIUM_ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());
}
