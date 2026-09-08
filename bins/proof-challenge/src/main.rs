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
use proof_eval::{custom_ids_ref, registered_custom, FamilyMux};
use proof_harvest::{HarvestLimits, LiumProofHarvest};
use proof_rlm::{
    RunnerRegistry, TopicVmOrchestrator, UnwiredVmOrchestrator, VmBackedRunner, VmTemplate,
    RLM_VM_IMAGE_DIGEST_ENV, VM_ORCHESTRATOR_TOKEN_FILE_ENV, VM_ORCHESTRATOR_URL_ENV,
};
use proof_rlm_scorer::{ArtefactStore, RlmScorer};
use proof_rlm_store::{MemoryRlmStore, PgRlmStore, RlmStore};
use proof_vm_fc::{parse_custom_ids, FirecrackerOrchestrator, VM_RUNNER_CUSTOM_IDS_ENV};
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
    /// Fallback seconds the eval image gets when no executor deadline was
    /// resolved. A resolved `max_proof_deadline_s` is the pod timeout and is
    /// never clamped by this value; the default equals the pin ceiling.
    #[arg(
        long,
        env = "PROOF_EVAL_TIMEOUT_SECS",
        default_value_t = proof_task::MAX_PROOF_DEADLINE_S_CEILING
    )]
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
    /// Root for per-submission artefact zips (`{root}/{topic_id}/{submission_id}.zip`).
    #[arg(long, env = "PROOF_ARTEFACT_ROOT", default_value = "/artefacts")]
    artefact_root: PathBuf,
    /// Postgres URL for the RLM store (topic versions, rule versions,
    /// checklists, lifecycle, artefact metadata, promotions). Unset → in-memory.
    #[arg(long, env = "BASE_DATABASE_URL")]
    database_url: Option<String>,
    /// File holding the Postgres URL (preferred on a droplet).
    #[arg(long, env = "BASE_DATABASE_URL_FILE")]
    database_url_file: Option<PathBuf>,
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
    let executor = boot_executor(&pin, backend, cli.eval_executor_offer_file.as_deref());

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;

    let rlm_store = rt.block_on(resolve_rlm_store(cli))?;
    let live_scorer = build_live_scorer(
        backend,
        cli.eval_timeout_secs,
        judge_api_key.clone(),
        cli.proxy_model_dir.clone(),
        cli.holdout_store.clone(),
    )
    .map(|harvest| with_custom_family(harvest, rlm_store, &cli.artefact_root));
    match backend {
        EvalBackend::Lium if live_scorer.is_some() => {
            tracing::info!("live harvest wired: digest-pinned proof-eval image on Lium");
            tracing::info!(
                registered_custom = ?registered_custom(live_scorer.as_deref()),
                artefact_root = %cli.artefact_root.display(),
                "custom-family topics route to the rlm scorer; an id with no registered runner \
                 answers 503 (no runner is compiled in)"
            );
        }
        EvalBackend::Lium => tracing::warn!(
            "live harvest not wired; every submission will 503. Set the Lium credentials \
             and LIUM_SSH_PUBLIC_KEY_FILE (deploy/env/proof-challenge.env.example)"
        ),
        EvalBackend::Sim => {}
    }

    let store = MemoryStore::new();
    let registered = registered_custom(live_scorer.as_deref());
    match load_topics(&store, &pin, cli.topics_file.as_deref(), &registered) {
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

/// Route the `custom` metric family to the RLM scorer over the default harvest.
///
/// No benchmark, model, or repository is compiled in: the registry holds only
/// the generic `VmBackedRunner`, under the custom ids the operator lists in
/// `PROOF_VM_RUNNER_CUSTOM_IDS`, over the topic-VM orchestrator
/// [`topic_vm_orchestrator`] resolved. With no ids the registry is empty and
/// every custom topic answers 503 (`RunnerUnwired`); with ids but an unwired
/// or unpinned orchestrator, 503 naming the missing env var. It never falls
/// back to the digest-pinned harvest and never spends.
fn with_custom_family(
    harvest: Arc<dyn LiveScorer>,
    rlm_store: Arc<dyn RlmStore>,
    artefact_root: &Path,
) -> Arc<dyn LiveScorer> {
    let scorer = RlmScorer::new(Arc::new(runner_registry()), rlm_store)
        .with_artefacts(Some(ArtefactStore::new(artefact_root)));
    Arc::new(FamilyMux::new(harvest).with_custom_family(Arc::new(scorer)))
}

/// The topic-VM orchestrator this host talks to, plus the RLM VM template.
///
/// `PROOF_VM_ORCHESTRATOR_URL` + `PROOF_VM_ORCHESTRATOR_TOKEN_FILE` +
/// `PROOF_RLM_VM_IMAGE_DIGEST` select the live `FirecrackerOrchestrator`
/// (HTTPS to the agent on the dedicated KVM host, 4 vCPU / 8192 MiB by
/// default). URL unset → `UnwiredVmOrchestrator` (503, names the env vars).
/// URL set but malformed (not https, no token file env) → also unwired, with
/// the error logged: a half-configured orchestrator never becomes a host
/// fallback. Token / digest are checked at `ready()` so they can be fixed
/// without a restart.
fn topic_vm_orchestrator() -> (Arc<dyn TopicVmOrchestrator>, VmTemplate) {
    match FirecrackerOrchestrator::from_env() {
        Ok(Some(fc)) => {
            let template = fc.template().clone();
            match fc.ready() {
                Ok(()) => tracing::info!(
                    url = %fc.url(), vcpus = template.vcpus, mem_mib = template.mem_mib,
                    image = %template.image_digest,
                    "firecracker topic-vm orchestrator wired (bearer file present, contents not logged)"
                ),
                Err(e) => tracing::warn!(
                    url = %fc.url(),
                    "firecracker topic-vm orchestrator configured but not ready ({e}); custom \
                     topics answer 503 until fixed"
                ),
            }
            (Arc::new(fc), template)
        }
        Ok(None) => {
            tracing::warn!(
                "no topic-vm orchestrator ({VM_ORCHESTRATOR_URL_ENV} / \
                 {VM_ORCHESTRATOR_TOKEN_FILE_ENV} / {RLM_VM_IMAGE_DIGEST_ENV} unset); every custom \
                 topic answers 503 and nothing runs on this host"
            );
            (Arc::new(UnwiredVmOrchestrator), VmTemplate::from_env())
        }
        Err(e) => {
            tracing::warn!(
                "topic-vm orchestrator refused ({e}); staying unwired, custom topics 503"
            );
            (Arc::new(UnwiredVmOrchestrator), VmTemplate::from_env())
        }
    }
}

/// `custom_id → VmBackedRunner` for every id in `PROOF_VM_RUNNER_CUSTOM_IDS`.
fn runner_registry() -> RunnerRegistry {
    let (orchestrator, template) = topic_vm_orchestrator();
    let runner = Arc::new(VmBackedRunner::new(orchestrator, template));
    let raw = std::env::var(VM_RUNNER_CUSTOM_IDS_ENV).unwrap_or_default();
    registry_for(&raw, &runner)
}

fn registry_for(raw_ids: &str, runner: &Arc<VmBackedRunner>) -> RunnerRegistry {
    let mut registry = RunnerRegistry::new();
    for id in parse_custom_ids(raw_ids) {
        match registry.register(&id, runner.clone()) {
            Ok(()) => tracing::info!(custom_id = %id, "vm-backed runner registered"),
            Err(e) => tracing::warn!("{VM_RUNNER_CUSTOM_IDS_ENV}: {e}; skipped"),
        }
    }
    if registry.is_empty() {
        tracing::warn!(
            "{VM_RUNNER_CUSTOM_IDS_ENV} names no custom id; the runner registry is empty and \
             every custom topic answers 503 (registration is an operator action)"
        );
    }
    registry
}

fn database_url(cli: &Cli) -> Result<Option<String>, String> {
    if let Some(url) = cli.database_url.as_deref().map(str::trim) {
        if !url.is_empty() {
            return Ok(Some(url.to_owned()));
        }
    }
    let Some(path) = cli.database_url_file.as_deref() else {
        return Ok(None);
    };
    let raw = std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("BASE_DATABASE_URL_FILE is empty".into());
    }
    Ok(Some(trimmed.to_owned()))
}

/// Postgres RLM store when a database is configured, in-memory otherwise.
///
/// A configured but unreachable database is fatal: falling back to memory
/// would silently drop every rule version, checklist, and promotion on
/// restart.
async fn resolve_rlm_store(cli: &Cli) -> Result<Arc<dyn RlmStore>, String> {
    let Some(url) = database_url(cli)? else {
        tracing::warn!(
            "no database configured; rlm rules, checklists, lifecycle, and promotions are not \
             persisted across restarts"
        );
        return Ok(Arc::new(MemoryRlmStore::new()));
    };
    let pool = db::connect(&url)
        .await
        .map_err(|e| format!("database connect failed: {e}"))?;
    db::migrate(&pool)
        .await
        .map_err(|e| format!("database migrate failed: {e}"))?;
    tracing::info!("rlm store persists to postgres");
    Ok(Arc::new(PgRlmStore::new(pool)))
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

fn load_topics(
    store: &MemoryStore,
    pin: &ProofPin,
    path: Option<&Path>,
    registered_custom: &[String],
) -> Result<usize, String> {
    let p = path.ok_or("PROOF_TOPICS_FILE not set")?;
    let body = std::fs::read_to_string(p).map_err(|e| format!("read {}: {e}", p.display()))?;
    let docs = TopicDocument::many_from_json(&body).map_err(|e| e.to_string())?;
    let n = docs.len();
    for doc in docs {
        doc.validate(pin, &custom_ids_ref(registered_custom))
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

/// Load the live executor offer and log the harvest hot-swap state. Neither
/// is a boot error: the Lium path answers 503 until an open `1x` offer is on
/// the host (file or `POST /v1/admin/proof/executor`).
fn boot_executor(
    pin: &ProofPin,
    backend: EvalBackend,
    path: Option<&Path>,
) -> Option<EvalExecutorOffer> {
    match HarvestOverrides::from_env() {
        Ok(o) if !o.is_empty() => {
            tracing::info!(?o, "PROOF_HARVEST_* override set; pin ceilings still bind");
        }
        Ok(_) => {}
        Err(e) => tracing::warn!("{e}; every live harvest will refuse until it is fixed"),
    }
    match load_executor(pin, path) {
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
    }
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

    /// No runner is compiled in: with no orchestrator env and no listed ids,
    /// every custom id refuses through the mux (`RunnerUnwired`, the 503 root
    /// cause), the harvest still owns the nll / throughput route, and nothing
    /// is registered.
    #[test]
    fn live_scorer_registers_no_custom_runner_and_refuses_every_custom_id() {
        let _guard = LIUM_ENV
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        clear_vm_env();
        let pubkey = stub_ssh_pubkey("proof-families");
        std::env::set_var("LIUM_API_KEY", "test-key-not-a-real-secret");
        std::env::set_var("LIUM_SSH_PUBLIC_KEY_FILE", &pubkey);
        let harvest =
            build_live_scorer(EvalBackend::Lium, 900, None, None, None).expect("harvest wired");
        std::env::remove_var("LIUM_API_KEY");
        std::env::remove_var("LIUM_SSH_PUBLIC_KEY_FILE");

        let root = std::env::temp_dir().join("proof-families-artefacts");
        let mux = with_custom_family(harvest, Arc::new(MemoryRlmStore::new()), &root);
        assert!(registered_custom(Some(mux.as_ref())).is_empty());
        for id in ["any_metric", "another_metric"] {
            let mut custom = TopicDocument::default();
            custom.metric.family = proof_task::MetricFamily::Custom;
            custom.metric.custom_id = id.into();
            let err = mux.ready_for_topic(&custom).expect_err("unregistered");
            assert!(
                matches!(err, proof_eval::EvalError::RunnerUnwired { .. }),
                "{err}"
            );
            assert!(err.to_string().contains("no registered runner"), "{err}");
        }
        // The default route is the harvest itself, whose readiness is about
        // proxy weights and holdout shards, not the runner registry.
        let nll = TopicDocument::default();
        let err = mux.ready_for_topic(&nll).expect_err("no proxy dir staged");
        assert!(
            matches!(err, proof_eval::EvalError::ProxyModelMissing),
            "{err}"
        );
    }

    #[test]
    fn database_url_comes_from_the_value_or_the_file_or_nowhere() {
        let mut cli = Cli::try_parse_from(["proof-challenge"]).expect("cli");
        assert_eq!(database_url(&cli).expect("none"), None);
        cli.database_url = Some("  ".into());
        assert_eq!(database_url(&cli).expect("blank is none"), None);
        let dir = std::env::temp_dir().join(format!("proof-db-url-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let file = dir.join("url");
        std::fs::write(&file, "postgres://placeholder/db\n").expect("write");
        cli.database_url_file = Some(file.clone());
        assert_eq!(
            database_url(&cli).expect("file"),
            Some("postgres://placeholder/db".into())
        );
        std::fs::write(&file, "\n").expect("write");
        assert!(
            database_url(&cli).is_err(),
            "an empty file is a config error"
        );
        let _ = std::fs::remove_dir_all(&dir);
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

    /// Every env var the topic-vm wiring reads. Cleared under `LIUM_ENV`.
    fn clear_vm_env() {
        for name in [
            VM_ORCHESTRATOR_URL_ENV,
            VM_ORCHESTRATOR_TOKEN_FILE_ENV,
            RLM_VM_IMAGE_DIGEST_ENV,
            VM_RUNNER_CUSTOM_IDS_ENV,
            proof_vm_fc::RLM_VM_VCPUS_ENV,
            proof_vm_fc::RLM_VM_MEM_MIB_ENV,
            proof_vm_fc::VM_ORCHESTRATOR_CA_FILE_ENV,
        ] {
            std::env::remove_var(name);
        }
    }

    /// Registration is an operator action: no ids → empty registry; listed
    /// ids bind the one generic `VmBackedRunner`; a malformed id is skipped,
    /// never a boot error.
    #[test]
    fn runner_registry_binds_the_vm_runner_only_to_listed_custom_ids() {
        let runner = Arc::new(VmBackedRunner::unwired());
        assert!(registry_for("", &runner).is_empty());
        assert!(registry_for(" , ", &runner).is_empty());
        let reg = registry_for("metric_a, Bad Id ,metric-b,metric_a", &runner);
        assert_eq!(
            reg.ids(),
            vec!["metric-b".to_owned(), "metric_a".to_owned()]
        );
        let resolved = reg.resolve("metric_a").expect("registered");
        let err = resolved.ready().expect_err("unwired orchestrator");
        assert!(matches!(err, proof_rlm::RunnerError::NotWired(_)), "{err}");
        assert!(err.to_string().contains(VM_ORCHESTRATOR_URL_ENV), "{err}");
        assert!(
            reg.resolve("metric_c").is_err(),
            "unlisted ids stay unregistered"
        );
    }

    /// URL + token file + digest select the live `FirecrackerOrchestrator`
    /// with the locked 4 vCPU / 8192 MiB shape; anything less keeps the
    /// unwired orchestrator (503), including a half-configured plain-http URL.
    #[test]
    fn firecracker_orchestrator_is_preferred_only_when_fully_configured() {
        let _guard = LIUM_ENV
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        clear_vm_env();
        let (unwired, template) = topic_vm_orchestrator();
        assert!(matches!(
            unwired.ready(),
            Err(proof_rlm::VmError::NotWired(_))
        ));
        assert!(template.image_digest.is_empty());

        let dir = std::env::temp_dir().join(format!("proof-vm-wire-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let token = dir.join("vm_orchestrator_token");
        std::fs::write(&token, "vm-bearer-not-a-real-secret\n").expect("token");
        std::env::set_var(VM_ORCHESTRATOR_URL_ENV, "https://kvm.example.invalid:8200");
        std::env::set_var(VM_ORCHESTRATOR_TOKEN_FILE_ENV, &token);
        let (pinned_less, template) = topic_vm_orchestrator();
        let err = pinned_less.ready().expect_err("no image pin");
        assert!(matches!(err, proof_rlm::VmError::NotWired(_)), "{err}");
        assert!(err.to_string().contains(RLM_VM_IMAGE_DIGEST_ENV), "{err}");
        assert_eq!(
            (template.vcpus, template.mem_mib),
            (4, 8_192),
            "locked shape"
        );

        std::env::set_var(
            RLM_VM_IMAGE_DIGEST_ENV,
            format!("sha256:{}", "ab".repeat(32)),
        );
        let (live, template) = topic_vm_orchestrator();
        live.ready().expect("url + token + digest = wired");
        template.validate().expect("pinned");
        std::env::set_var(VM_RUNNER_CUSTOM_IDS_ENV, "metric_a");
        let reg = runner_registry();
        assert_eq!(reg.ids(), vec!["metric_a".to_owned()]);
        reg.resolve("metric_a")
            .expect("registered")
            .ready()
            .expect("runner over the live orchestrator is ready");

        std::fs::write(&token, "\n").expect("empty token");
        let (live, _) = topic_vm_orchestrator();
        let err = live.ready().expect_err("empty bearer file");
        assert!(
            err.to_string().contains(VM_ORCHESTRATOR_TOKEN_FILE_ENV),
            "{err}"
        );

        std::env::set_var(VM_ORCHESTRATOR_URL_ENV, "http://10.0.0.7:8200");
        let (refused, _) = topic_vm_orchestrator();
        let err = refused
            .ready()
            .expect_err("plain http off loopback is never wired");
        assert!(err.to_string().contains(VM_ORCHESTRATOR_URL_ENV), "{err}");
        clear_vm_env();
        let _ = std::fs::remove_dir_all(&dir);
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
