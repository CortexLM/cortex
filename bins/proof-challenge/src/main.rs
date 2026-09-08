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
    executor_slot, hash_admin_token, parse_holdout_file, proof_router, AppState,
    BaselineMeasurement, EvalBackend, EvalExecutorOffer, HarvestOverrides, InferenceOffer,
    LiveScorer, MemoryStore, ProofPin, TopicDocument, CHALLENGE_ID, SCORING_VERSION,
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
    /// Live RLM judge `InferenceOffer` JSON. Operator state; never a git pin. Missing/closed → 503.
    #[arg(long, env = "PROOF_INFERENCE_OFFER_FILE")]
    inference_offer_file: Option<PathBuf>,
    /// Provider API key file. Never logged, never on `/v1/status`.
    #[arg(long, env = "PROOF_INFERENCE_API_KEY_FILE")]
    inference_api_key_file: Option<PathBuf>,
    /// Live `1x` `EvalExecutorOffer` JSON (Lium template + proof deadline).
    /// Operator state; never a git pin. Rotated at runtime via
    /// `POST /v1/admin/proof/executor`. Missing/closed/shape ≠ 1x → 503.
    #[arg(long, env = "PROOF_EVAL_EXECUTOR_OFFER_FILE")]
    eval_executor_offer_file: Option<PathBuf>,
    /// Local measurement weights staged onto the eval pod (no HF bake).
    #[arg(long, env = "PROOF_PROXY_MODEL_DIR")]
    proxy_model_dir: Option<PathBuf>,
    /// Holdout shard bytes (`<content_sha256>` files). Not the record catalog.
    #[arg(long, env = "PROOF_HOLDOUT_STORE")]
    holdout_store: Option<PathBuf>,
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
        if let Err(e) = load_challenge_secret(p) {
            // Compose always sets BASE_CHALLENGE_SK_FILE; remote-deploy may
            // materialize an empty placeholder so Docker does not create a
            // directory. Missing/invalid key must not take /health down.
            tracing::warn!(
                "challenge sk: {e}; leaf signing unavailable until BASE_CHALLENGE_SK_FILE is a \
                 32-byte mini-secret (or 64 hex chars)"
            );
        }
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
    let judge_api_key = load_inference_api_key(cli.inference_api_key_file.as_deref());
    match (&offer, &judge_api_key) {
        (Some(o), Some(_)) if o.is_open() => {
            tracing::info!("inference api key file present (contents not logged)");
        }
        (Some(o), None) if o.is_open() => tracing::warn!(
            "open InferenceOffer needs PROOF_INFERENCE_API_KEY_FILE; live submits will 503"
        ),
        _ => {}
    }
    let executor = match load_executor(&pin, cli.eval_executor_offer_file.as_deref()) {
        Ok(x) => {
            tracing::info!(
                offer_id = %x.offer_id,
                lium_template_id = %x.lium_template_id,
                machine_shape = %x.machine_shape,
                max_proof_deadline_s = x.max_proof_deadline_s,
                status = ?x.status,
                "eval executor offer loaded"
            );
            Some(x)
        }
        Err(e) => {
            if backend == EvalBackend::Lium {
                tracing::warn!(
                    "eval executor offer unavailable ({e}); live submits will 503 until \
                     PROOF_EVAL_EXECUTOR_OFFER_FILE holds an open 1x offer or one is posted \
                     to /v1/admin/proof/executor"
                );
            }
            None
        }
    };
    match HarvestOverrides::from_env() {
        Ok(o) if !o.is_empty() => {
            tracing::info!(?o, "PROOF_HARVEST_* override set; pin ceilings still bind");
        }
        Ok(_) => {}
        Err(e) => tracing::warn!("{e}; every live harvest will refuse until it is fixed"),
    }

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;

    let live_scorer = build_live_scorer(
        backend,
        cli.eval_timeout_secs,
        judge_api_key.clone(),
        cli.proxy_model_dir.clone(),
        cli.holdout_store.clone(),
    );
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
        executor: executor_slot(executor),
        judge_api_key,
        admin_hashes: Arc::new(load_admin_hashes(cli.admin_tokens_file.as_deref())),
        epoch: 0,
    };
    rt.block_on(serve(cli.bind, state))
}

fn build_live_scorer(
    backend: EvalBackend,
    run_timeout_secs: u64,
    judge_api_key: Option<String>,
    proxy_model_dir: Option<PathBuf>,
    holdout_store: Option<PathBuf>,
) -> Option<Arc<dyn LiveScorer>> {
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
    Some(Arc::new(
        LiumProofHarvest::new(pod, HarvestLimits::default(), vec![ssh_pub])
            .with_judge_api_key(judge_api_key)
            .with_proxy_model_dir(proxy_model_dir)
            .with_holdout_store(holdout_store),
    ))
}

fn load_inference_api_key(path: Option<&Path>) -> Option<String> {
    let p = path?;
    std::fs::read_to_string(p)
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
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

fn load_executor(pin: &ProofPin, path: Option<&Path>) -> Result<EvalExecutorOffer, String> {
    let p = path.ok_or("PROOF_EVAL_EXECUTOR_OFFER_FILE not set")?;
    let body = std::fs::read_to_string(p).map_err(|e| format!("read {}: {e}", p.display()))?;
    let offer = EvalExecutorOffer::from_json(&body).map_err(|e| e.to_string())?;
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
            build_live_scorer(EvalBackend::Lium, 900, None, None, None).is_some(),
            "Lium boot must wire the digest-pinned harvest"
        );
        assert!(
            build_live_scorer(EvalBackend::Sim, 900, None, None, None).is_none(),
            "sim scores in-process; a Lium harvest there would spend money"
        );

        std::env::remove_var("LIUM_API_KEY");
        assert!(build_live_scorer(EvalBackend::Lium, 900, None, None, None).is_none());
        std::env::set_var("LIUM_API_KEY", "   ");
        assert!(build_live_scorer(EvalBackend::Lium, 900, None, None, None).is_none());

        std::env::set_var("LIUM_API_KEY", "test-key-not-a-real-secret");
        std::env::set_var("LIUM_SSH_PUBLIC_KEY_FILE", "/nonexistent/id.pub");
        assert!(build_live_scorer(EvalBackend::Lium, 900, None, None, None).is_none());
        std::env::remove_var("LIUM_API_KEY");
        std::env::remove_var("LIUM_SSH_PUBLIC_KEY_FILE");
    }

    #[test]
    fn inference_api_key_file_is_read_not_existence_only() {
        let dir = std::env::temp_dir().join(format!(
            "proof-key-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("dir");
        let missing = dir.join("nope");
        assert!(load_inference_api_key(Some(&missing)).is_none());
        let empty = dir.join("empty");
        std::fs::write(&empty, "  \n").expect("write");
        assert!(load_inference_api_key(Some(&empty)).is_none());
        let present = dir.join("key");
        std::fs::write(&present, " sk-live-not-a-real-secret \n").expect("write");
        assert_eq!(
            load_inference_api_key(Some(&present)).as_deref(),
            Some("sk-live-not-a-real-secret")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn stub_ssh_pubkey(tag: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("proof-test-{tag}.pub"));
        std::fs::write(&path, "ssh-ed25519 AAAAtest proof-test\n").expect("write pubkey");
        path
    }

    static LIUM_ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Compose always points `PROOF_PIN_FILE` at the committed pin. Empty
    /// `[inference].model` / `base_url` is pre-launch fail-closed (503), not a
    /// boot reject — same as an empty eval digest.
    #[test]
    fn committed_pin_boots_with_empty_inference_model() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/proof-pin.toml");
        let pin = load_pin(Some(&path)).expect("committed pin must boot");
        assert!(
            pin.inference.model.trim().is_empty(),
            "empty model is pre-launch 503"
        );
        assert!(
            pin.inference.base_url.trim().is_empty(),
            "url is secret-backed"
        );
        assert!(pin.proxy_model.trim().is_empty());
    }

    /// Compose sets `PROOF_INFERENCE_OFFER_FILE`. A missing/closed offer is
    /// `can_score=false` / submit 503 — it must not `exit 1`.
    #[test]
    fn compose_inference_offer_env_parses_and_a_missing_file_is_not_a_boot_error() {
        let _guard = OFFER_ENV
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        std::env::set_var(
            "PROOF_INFERENCE_OFFER_FILE",
            "/run/base/proof/inference_offer.json",
        );
        let cli = Cli::try_parse_from(["proof-challenge"])
            .unwrap_or_else(|e| panic!("PROOF_INFERENCE_OFFER_FILE broke parsing: {e}"));
        assert_eq!(
            cli.inference_offer_file.as_deref(),
            Some(Path::new("/run/base/proof/inference_offer.json"))
        );
        let pin = load_pin(None).expect("default pin");
        let err = load_offer(&pin, cli.inference_offer_file.as_deref())
            .expect_err("missing offer is unavailable, not a panic");
        assert!(
            err.contains("read") || err.contains("PROOF_INFERENCE_OFFER_FILE"),
            "{err}"
        );
        std::env::remove_var("PROOF_INFERENCE_OFFER_FILE");
    }

    static OFFER_ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Compose sets `PROOF_EVAL_EXECUTOR_OFFER_FILE`. Missing / unparseable /
    /// non-`1x` is `can_score=false` / submit 503 — never `exit 1`. A valid
    /// `1x` offer on the committed pin's digest-scoped template loads.
    #[test]
    fn compose_executor_offer_env_parses_and_bad_files_are_not_boot_errors() {
        let _guard = OFFER_ENV
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        std::env::set_var(
            "PROOF_EVAL_EXECUTOR_OFFER_FILE",
            "/run/base/proof/eval_executor_offer.json",
        );
        let cli = Cli::try_parse_from(["proof-challenge"])
            .unwrap_or_else(|e| panic!("PROOF_EVAL_EXECUTOR_OFFER_FILE broke parsing: {e}"));
        assert_eq!(
            cli.eval_executor_offer_file.as_deref(),
            Some(Path::new("/run/base/proof/eval_executor_offer.json"))
        );
        std::env::remove_var("PROOF_EVAL_EXECUTOR_OFFER_FILE");

        let pin_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/proof-pin.toml");
        let pin = load_pin(Some(&pin_path)).expect("committed pin");
        let missing = load_executor(&pin, Some(Path::new("/nonexistent/executor.json")))
            .expect_err("missing file is unavailable, not a panic");
        assert!(missing.contains("read"), "{missing}");
        assert!(load_executor(&pin, None)
            .expect_err("unset")
            .contains("PROOF_EVAL_EXECUTOR_OFFER_FILE"));

        let dir = std::env::temp_dir().join(format!(
            "proof-executor-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("dir");
        let hex = pin.eval_image_digest.trim_start_matches("sha256:");
        let mut good = EvalExecutorOffer {
            offer_id: "lium-1x-v0".into(),
            lium_template_id: format!("proof-eval-{}", &hex[..12]),
            machine_shape: "1x".into(),
            max_proof_deadline_s: 7_200,
            eval_image_digest: pin.eval_image_digest.clone(),
            config_commitment: String::new(),
            status: proof_challenge::OfferStatus::Open,
        };
        good.config_commitment = good.expected_commitment();
        let good_path = dir.join("good.json");
        std::fs::write(&good_path, serde_json::to_vec(&good).expect("json")).expect("write");
        let loaded = load_executor(&pin, Some(&good_path)).expect("valid 1x offer loads");
        assert_eq!(loaded, good);

        let mut wide = good.clone();
        wide.machine_shape = "8x".into();
        wide.config_commitment = wide.expected_commitment();
        let wide_path = dir.join("wide.json");
        std::fs::write(&wide_path, serde_json::to_vec(&wide).expect("json")).expect("write");
        let err = load_executor(&pin, Some(&wide_path)).expect_err("8x is refused");
        assert!(err.contains("machine_shape"), "{err}");

        let junk_path = dir.join("junk.json");
        std::fs::write(
            &junk_path,
            b"{\"offer_id\":\"x\",\"lium_api_key\":\"nope\"}",
        )
        .expect("write");
        let err = load_executor(&pin, Some(&junk_path)).expect_err("unknown key");
        assert!(err.contains("lium_api_key"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
