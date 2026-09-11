//! `proof-challenge` — master-only Proof service (port 8100).
//!
//! Miner HTTP submit, digest freeze, per-topic holdout unseal, then RLM agent
//! and harness to lattice. Miners pay Lium. Topics are operator-published
//! signed documents, not a catalog in git.
//!
//! Without `PROOF_FORCE_SIM=1` the host needs a `sha256:` eval-image pin, a
//! wired scorer for the topic's family (the Lium harvest for `nll` /
//! `throughput`; the topic-VM orchestrator plus a registered custom id for
//! `custom` — each wired on its own), at least one `open` topic with a
//! verified holdout, and a sealed baseline. Sim is never a fallback.

#![forbid(unsafe_code)]

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use challenge_keys::load_challenge_secret;
use clap::Parser;
use prism_lium::LiumClient;
use proof_challenge::{
    executor_slot, hash_admin_token, parse_holdout_file, proof_router, AppState,
    BaselineMeasurement, EvalBackend, EvalExecutorOffer, GatewayClient, GatewayClientConfig,
    HarvestOverrides, InferenceOffer, LiveScorer, MemoryStore, MinerEnvVault, ProofEmitter,
    ProofPin, TopicDocument, VmAgentHealth, VmOrchestratorProbe, VmOrchestratorReport,
    CHALLENGE_ID, DEFAULT_EMIT_POLL_SECS, MINER_BYOK_DIR_ENV, SCORING_VERSION,
};
use proof_eval::{custom_ids_ref, registered_custom, FamilyMux};
use proof_harvest::{HarvestLimits, LiumProofHarvest};
use proof_rlm::{
    ExperimentPolicy, RunnerRegistry, TopicVmOrchestrator, UnwiredVmOrchestrator, VmBackedRunner,
    VmTemplate, RLM_VM_IMAGE_DIGEST_ENV, VM_ORCHESTRATOR_TOKEN_FILE_ENV, VM_ORCHESTRATOR_URL_ENV,
};
use proof_rlm_scorer::{max_zip_numeric_id, ArtefactStore, RlmScorer};
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
    #[arg(long, env = "PROOF_EMIT_POLL_SECS", default_value_t = DEFAULT_EMIT_POLL_SECS)]
    emit_poll_secs: u64,
    /// Seconds between queue-drain passes: `queued` rows of every open topic
    /// that no longer defers scoring are scored oldest first, one at a time,
    /// through the live path. `0` disables the loop — the queue then drains
    /// only through `POST /v1/admin/proof/queue/drain`.
    #[arg(
        long,
        env = "PROOF_QUEUE_DRAIN_POLL_SECS",
        default_value_t = DEFAULT_QUEUE_DRAIN_POLL_SECS
    )]
    queue_drain_poll_secs: u64,
    /// Persisted scored-epoch watermark (survives process restart).
    #[arg(
        long,
        env = "PROOF_SCORED_EPOCH_FILE",
        default_value = "/var/lib/proof/scored_epoch"
    )]
    scored_epoch_file: PathBuf,
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

/// Where this host keeps miner BYOK material, said out loud at boot.
///
/// It belongs in 0600 files rather than in this process: a topic that defers
/// scoring is drained long after the submit that carried the key, and a
/// restart in between must not reach the paid run without it.
fn miner_byok_vault() -> MinerEnvVault {
    let vault = MinerEnvVault::from_env();
    if let Some(dir) = vault.root() {
        tracing::info!(
            dir = %dir.display(),
            "miner byok vault (0600 per variable; contents never logged)"
        );
    } else {
        tracing::warn!(
            "{MINER_BYOK_DIR_ENV} is empty: miner byok material stays in this process and a restart loses it, so a deferred topic's queue will 503 on drain"
        );
    }
    vault
}

fn run(cli: &Cli) -> Result<(), String> {
    let sk = load_optional_sk(cli.challenge_sk_file.as_deref());
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
    let harvest = build_live_scorer(
        backend,
        cli.eval_timeout_secs,
        judge_api_key.clone(),
        cli.proxy_model_dir.clone(),
        cli.holdout_store.clone(),
    );
    // One topic-VM orchestrator per process: the runner registry drives it
    // and the admin probe reports on the same client (bearer file, pin, CA).
    let vm = Arc::new(topic_vm_orchestrator());
    let store = MemoryStore::new().with_miner_byok_vault(miner_byok_vault());
    rt.block_on(seed_pf_allocator(
        &store,
        rlm_store.as_ref(),
        &cli.artefact_root,
    ))?;
    let live_scorer = live_scorer(backend, harvest, rlm_store, &cli.artefact_root, &vm);
    log_live_wiring(backend, live_scorer.as_deref(), &cli.artefact_root);
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

    if let Some(emitter) = build_emitter(cli, store.clone(), sk)? {
        let poll = Duration::from_secs(cli.emit_poll_secs.max(1));
        rt.spawn(emitter.run(poll));
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
        vm_probe: Some(vm),
        epoch: 0,
    };
    if cli.queue_drain_poll_secs == 0 {
        tracing::info!(
            "queue drain loop disabled (PROOF_QUEUE_DRAIN_POLL_SECS=0); queued rows score only \
             through POST /v1/admin/proof/queue/drain"
        );
    } else {
        rt.spawn(run_queue_drainer(
            state.clone(),
            Duration::from_secs(cli.queue_drain_poll_secs),
        ));
    }
    rt.block_on(serve(cli.bind, state))
}

/// Default seconds between queue-drain passes.
const DEFAULT_QUEUE_DRAIN_POLL_SECS: u64 = 60;

/// Score the `queued` rows of every open topic that no longer defers scoring
/// (`constraints.params.defer_scoring` lifted by a re-publish), oldest first,
/// one at a time, through the same path a live submit takes. A topic that
/// still defers is never touched; a host that cannot score leaves the rows
/// queued and says why, once per pass. Lifting the flag is the operator's
/// "score now".
async fn run_queue_drainer(state: AppState, poll: Duration) {
    loop {
        tokio::time::sleep(poll).await;
        for report in state.drain_ready_queues().await {
            let scored: Vec<&str> = report.drained.iter().map(|r| r.id.as_str()).collect();
            if let Some(why) = &report.stopped {
                tracing::warn!(
                    topic_id = %report.topic_id,
                    scored = ?scored,
                    remaining = report.remaining,
                    "queue drain stopped: {why}; the remaining rows stay queued for the next pass"
                );
            } else {
                tracing::info!(
                    topic_id = %report.topic_id,
                    scored = ?scored,
                    remaining = report.remaining,
                    "queue drained"
                );
            }
        }
    }
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

/// Boot log for what [`live_scorer`] wired, and what answers 503 because of
/// what is missing. Nothing here is a boot error: a live host refuses until
/// the operator wires the piece it names. The harvest flag is the same
/// Lium-only answer `/v1/status` gives as `live_harvest_wired`.
fn log_live_wiring(backend: EvalBackend, live: Option<&dyn LiveScorer>, artefact_root: &Path) {
    let harvest_wired = live.is_some_and(LiveScorer::harvest_wired);
    match backend {
        EvalBackend::Lium if harvest_wired => {
            tracing::info!("live harvest wired: digest-pinned proof-eval image on Lium");
            tracing::info!(
                registered_custom = ?registered_custom(live),
                artefact_root = %artefact_root.display(),
                "custom-family topics route to the rlm scorer; an id with no registered runner \
                 answers 503 (no runner is compiled in)"
            );
        }
        EvalBackend::Lium if live.is_some() => tracing::info!(
            registered_custom = ?registered_custom(live),
            artefact_root = %artefact_root.display(),
            "live harvest not wired: custom-family topics route to the rlm scorer over the \
             topic-vm orchestrator; every nll / throughput topic answers 503 until the Lium \
             credentials and LIUM_SSH_PUBLIC_KEY_FILE are set"
        ),
        EvalBackend::Lium => tracing::warn!(
            "live harvest not wired; every submission will 503. Set the Lium credentials \
             and LIUM_SSH_PUBLIC_KEY_FILE (deploy/env/proof-challenge.env.example), or wire \
             the topic-vm orchestrator and {VM_RUNNER_CUSTOM_IDS_ENV} for custom topics"
        ),
        EvalBackend::Sim => {}
    }
}

/// The live scorer of this host, by metric family.
///
/// `nll` / `throughput` score on the digest-pinned Lium harvest; `custom`
/// scores on the RLM scorer over the runner registry [`custom_family`]
/// builds from the topic-VM orchestrator env. The two are wired
/// independently: with a harvest the mux routes both families; with none,
/// the custom family still stands on its own when the env selected the live
/// orchestrator and registered at least one custom id
/// ([`FamilyMux::custom_only`] — every `nll` / `throughput` topic then
/// answers 503 `LiveHarvestUnavailable`, no row, no rent). Neither wired →
/// `None`, and every submission answers 503. Sim scores in-process and
/// wires nothing live.
fn live_scorer(
    backend: EvalBackend,
    harvest: Option<Arc<dyn LiveScorer>>,
    rlm_store: Arc<dyn RlmStore>,
    artefact_root: &Path,
    vm: &TopicVm,
) -> Option<Arc<dyn LiveScorer>> {
    if backend != EvalBackend::Lium {
        return None;
    }
    let custom = custom_family(rlm_store, artefact_root, vm);
    match harvest {
        Some(harvest) => Some(Arc::new(
            FamilyMux::new(harvest).with_custom_family(custom.scorer),
        )),
        None if custom.standalone => Some(Arc::new(FamilyMux::custom_only(custom.scorer))),
        None => None,
    }
}

/// The custom-family scorer of this host.
struct CustomFamily {
    /// RLM scorer over the runner registry.
    scorer: Arc<dyn LiveScorer>,
    /// The env selected the live topic-VM orchestrator **and** at least one
    /// custom id is registered over it: enough to carry the custom family
    /// with no Lium harvest. False with the unwired orchestrator (env unset
    /// or refused) or an empty registry — a mux with nothing to route to is
    /// not wired.
    standalone: bool,
}

/// The RLM scorer over the runner registry, wired from the topic-VM
/// orchestrator env alone — never from the Lium harvest.
///
/// No benchmark, model, or repository is compiled in: the registry holds only
/// the generic `VmBackedRunner`, under the custom ids the operator lists in
/// `PROOF_VM_RUNNER_CUSTOM_IDS`, over the topic-VM orchestrator `vm`
/// ([`topic_vm_orchestrator`] resolved once per process). With no ids the
/// registry is empty and every custom topic answers 503 (`RunnerUnwired`);
/// with ids but an unwired or unpinned orchestrator, 503 naming the missing
/// env var. It never falls back to the digest-pinned harvest and never
/// spends.
fn custom_family(rlm_store: Arc<dyn RlmStore>, artefact_root: &Path, vm: &TopicVm) -> CustomFamily {
    let registry = runner_registry(vm);
    let standalone = vm.live && !registry.is_empty();
    let scorer = RlmScorer::new(Arc::new(registry), rlm_store)
        .with_artefacts(Some(ArtefactStore::new(artefact_root)));
    CustomFamily {
        scorer: Arc::new(scorer),
        standalone,
    }
}

/// What the topic-VM orchestrator env resolved to. Resolved once per process
/// and shared: the runner registry drives `orchestrator`, and
/// `GET /v1/admin/proof/vm-orchestrator` reports through the same client
/// ([`VmOrchestratorProbe`]) — same bearer file, same pin, same TLS roots.
struct TopicVm {
    orchestrator: Arc<dyn TopicVmOrchestrator>,
    /// RLM VM template the runner boots for topics without a VM.
    template: VmTemplate,
    /// Per-experiment VM policy (`PROOF_EXPERIMENT_VM_*`: ceilings, image)
    /// for topics whose signed params select an in-guest runner — one
    /// dedicated VM per paid job, destroyed after it.
    experiments: ExperimentPolicy,
    /// The env selected the live `FirecrackerOrchestrator` (URL + bearer
    /// file env, https). False = `UnwiredVmOrchestrator`, which refuses
    /// every call.
    live: bool,
    /// The live client, concretely, for the probe's agent health call. The
    /// same allocation as `orchestrator`; `None` when unwired.
    fc: Option<Arc<FirecrackerOrchestrator>>,
    /// Why nothing is wired (names the env vars). Empty when live.
    unwired_reason: String,
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
fn topic_vm_orchestrator() -> TopicVm {
    match FirecrackerOrchestrator::from_env() {
        Ok(Some(fc)) => {
            let template = fc.template().clone();
            let experiments = fc.experiments().clone();
            match fc.ready() {
                Ok(()) => tracing::info!(
                    url = %fc.url(), vcpus = template.vcpus, mem_mib = template.mem_mib,
                    image = %template.image_digest,
                    experiment_max_vcpus = experiments.ceilings.max_vcpus,
                    experiment_max_mem_mib = experiments.ceilings.max_mem_mib,
                    experiment_disk_mib = experiments.ceilings.default_disk_mib,
                    experiment_image = %experiments.image_for(&template.image_digest),
                    "firecracker topic-vm orchestrator wired (bearer file present, contents not logged)"
                ),
                Err(e) => tracing::warn!(
                    url = %fc.url(),
                    "firecracker topic-vm orchestrator configured but not ready ({e}); custom \
                     topics answer 503 until fixed"
                ),
            }
            let fc = Arc::new(fc);
            TopicVm {
                orchestrator: fc.clone(),
                template,
                experiments,
                live: true,
                fc: Some(fc),
                unwired_reason: String::new(),
            }
        }
        Ok(None) => {
            let reason = format!(
                "no topic-vm orchestrator ({VM_ORCHESTRATOR_URL_ENV} / \
                 {VM_ORCHESTRATOR_TOKEN_FILE_ENV} / {RLM_VM_IMAGE_DIGEST_ENV} unset)"
            );
            tracing::warn!(
                "{reason}; every custom topic answers 503 and nothing runs on this host"
            );
            TopicVm::unwired(reason)
        }
        Err(e) => {
            let reason = format!("topic-vm orchestrator refused ({e})");
            tracing::warn!("{reason}; staying unwired, custom topics 503");
            TopicVm::unwired(reason)
        }
    }
}

impl TopicVm {
    fn unwired(reason: String) -> Self {
        Self {
            orchestrator: Arc::new(UnwiredVmOrchestrator),
            template: VmTemplate::from_env(),
            experiments: ExperimentPolicy::default(),
            live: false,
            fc: None,
            unwired_reason: reason,
        }
    }
}

#[async_trait]
impl VmOrchestratorProbe for TopicVm {
    /// `ready()` (bearer file + pin, re-read now) and one agent health call
    /// through the very client the runner uses — the same TLS roots, the same
    /// bearer file — so a green answer here is the wire the runner will use.
    /// Unwired hosts report the boot-time reason. Never the bearer.
    async fn probe(&self) -> VmOrchestratorReport {
        let Some(fc) = &self.fc else {
            return VmOrchestratorReport::unwired(
                &self.unwired_reason,
                &self.template.image_digest,
            );
        };
        let template = fc.template();
        let ready = fc.ready();
        let health = fc.health().await;
        VmOrchestratorReport {
            orchestrator: "firecracker".into(),
            ready: ready.is_ok(),
            reason: ready.err().map(|e| e.to_string()).unwrap_or_default(),
            image_digest: template.image_digest.clone(),
            vcpus: template.vcpus,
            mem_mib: template.mem_mib,
            agent: health.as_ref().ok().map(|h| VmAgentHealth {
                api_version: h.api_version,
                ready: h.ready,
                reason: h.reason.clone(),
                hypervisor: h.hypervisor.clone(),
                vms: h.vms,
            }),
            agent_error: health.err().map(|e| e.to_string()),
            live_harvest_wired: false,
            custom_family_wired: false,
            registered_custom: Vec::new(),
        }
    }
}

/// `custom_id → VmBackedRunner` for every id in `PROOF_VM_RUNNER_CUSTOM_IDS`.
fn runner_registry(vm: &TopicVm) -> RunnerRegistry {
    let runner = Arc::new(
        VmBackedRunner::new(vm.orchestrator.clone(), vm.template.clone())
            .with_experiments(vm.experiments.clone()),
    );
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

/// Raise the in-memory `pf_…` allocator past every id already used as
/// artefact metadata (Postgres) or a zip basename under `PROOF_ARTEFACT_ROOT`.
/// A fresh store with neither still mints from 0. An unreadable artefact
/// tree is fatal: skipping it would under-seed and collide with a zip the
/// scan could not see.
async fn seed_pf_allocator(
    store: &MemoryStore,
    rlm: &dyn RlmStore,
    artefact_root: &Path,
) -> Result<(), String> {
    let from_pg = rlm
        .max_artefact_numeric_id()
        .await
        .map_err(|e| format!("seed pf ids from rlm store: {e}"))?;
    let from_disk = max_zip_numeric_id(artefact_root).map_err(|e| {
        format!(
            "seed pf ids from artefact root {}: {e}",
            artefact_root.display()
        )
    })?;
    let Some(used) = [from_pg, from_disk].into_iter().flatten().max() else {
        tracing::info!("pf id allocator starts at 0 (no existing artefacts)");
        return Ok(());
    };
    store.seed_next_id(used).map_err(|e| e.to_string())?;
    tracing::info!(
        used,
        from_pg,
        from_disk,
        "pf id allocator seeded past existing artefacts"
    );
    Ok(())
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

/// Wire the leaf emitter.
///
/// An empty store still gets one. It pays nobody — every leaf is
/// `NoScore(ChallengeInternal)`, so the share burns to uid 0 — but proof
/// holds a paid trust-root row, and a paid challenge with no leaves fails D24:
/// the seal would 409 for *every* challenge. Only a missing challenge key
/// stops emission, because a leaf the trust root rejects is not weight.
fn build_emitter(
    cli: &Cli,
    store: MemoryStore,
    sk: Option<[u8; 32]>,
) -> Result<Option<Arc<ProofEmitter<chain_live::LiveChainClient>>>, String> {
    let Some(sk) = sk else {
        tracing::warn!(
            "no BASE_CHALLENGE_SK_FILE: proof cannot sign leaves, so nothing will be emitted \
             and POST /v1/admin/seal will answer 409 while proof holds a paid trust-root row"
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
        scored_epoch_file = %cli.scored_epoch_file.display(),
        "proof emitter wired"
    );
    Ok(Some(Arc::new(
        ProofEmitter::new(chain, gateway, sk, cli.netuid, store)
            .with_scored_epoch_path(cli.scored_epoch_file.clone()),
    )))
}

fn load_optional_sk(path: Option<&Path>) -> Option<[u8; 32]> {
    let p = path?;
    match load_challenge_secret(p) {
        Ok(k) => Some(k),
        Err(e) => {
            // Compose always sets BASE_CHALLENGE_SK_FILE; remote-deploy may
            // materialize an empty placeholder so Docker does not create a
            // directory. Missing/invalid key must not take /health down.
            tracing::warn!(
                "challenge sk: {e}; leaf signing unavailable until BASE_CHALLENGE_SK_FILE is a \
                 32-byte mini-secret (or 64 hex chars)"
            );
            None
        }
    }
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

    fn cli() -> Cli {
        Cli::try_parse_from(["proof-challenge"]).expect("defaults parse")
    }

    /// A leaf the trust root would reject is not weight, so a missing
    /// challenge key is a refusal rather than an unsigned emit.
    #[test]
    fn no_challenge_key_wires_no_emitter() {
        assert!(build_emitter(&cli(), MemoryStore::new(), None)
            .expect("no emitter is not an error")
            .is_none());
    }

    #[test]
    fn a_challenge_key_wires_the_emitter() {
        let wired = build_emitter(&cli(), MemoryStore::new(), Some([3u8; 32]))
            .expect("wire")
            .expect("emitter");
        assert_eq!(wired.scored_epoch(), 0);
    }

    #[test]
    fn emit_poll_secs_defaults_to_the_bounty_cadence() {
        assert_eq!(cli().emit_poll_secs, DEFAULT_EMIT_POLL_SECS);
        assert_eq!(DEFAULT_EMIT_POLL_SECS, 120);
    }

    fn artefact_row(submission_id: &str) -> proof_rlm_store::ArtefactRow {
        proof_rlm_store::ArtefactRow {
            topic_id: "topic-a".into(),
            submission_id: submission_id.into(),
            submission_digest: "ab".repeat(32),
            path: format!("/artefacts/topic-a/{submission_id}.zip"),
            sha256: "aa".repeat(32),
            bytes: 1,
            primary_value: None,
            checklist_green: false,
            promoted: false,
        }
    }

    #[tokio::test]
    async fn seed_pf_allocator_starts_at_zero_without_existing_ids() {
        let store = MemoryStore::new();
        seed_pf_allocator(
            &store,
            &MemoryRlmStore::new(),
            Path::new("/no/such/artefacts"),
        )
        .await
        .expect("seed");
        let row = store
            .insert(proof_store::Submission {
                id: String::new(),
                topic_id: "t".into(),
                miner_hotkey: "aa".repeat(32),
                artifact_digest: "11".repeat(32),
                artifact_uri: None,
                claim: "c".into(),
                declared_flops: 1,
                architecture: String::new(),
                inference_offer_id: String::new(),
                config_commitment: String::new(),
                executor_offer_id: String::new(),
                executor_commitment: String::new(),
                manifest: proof_store::ArtifactManifest::default(),
                submission_digest: "dd".repeat(32),
                nonce: "n".into(),
                submit_nonce: "ee".repeat(32),
                state: proof_store::SubmissionState::Queued,
                receipt_json: None,
                verdict: None,
                detail: None,
            })
            .expect("mint");
        assert_eq!(row.id, "pf_0000000000000000");
    }

    #[tokio::test]
    async fn seed_pf_allocator_takes_the_max_of_store_and_disk() {
        let rlm = MemoryRlmStore::new();
        rlm.put_artefact(&artefact_row("pf_0000000000000001"))
            .await
            .expect("pg row");
        let root = std::env::temp_dir().join(format!(
            "proof-seed-disk-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("topic-b")).expect("dir");
        std::fs::write(root.join("topic-b").join("pf_000000000000000a.zip"), b"z").expect("zip");
        let store = MemoryStore::new();
        seed_pf_allocator(&store, &rlm, &root).await.expect("seed");
        let row = store
            .insert(proof_store::Submission {
                id: String::new(),
                topic_id: "t".into(),
                miner_hotkey: "aa".repeat(32),
                artifact_digest: "11".repeat(32),
                artifact_uri: None,
                claim: "c".into(),
                declared_flops: 1,
                architecture: String::new(),
                inference_offer_id: String::new(),
                config_commitment: String::new(),
                executor_offer_id: String::new(),
                executor_commitment: String::new(),
                manifest: proof_store::ArtifactManifest::default(),
                submission_digest: "dd".repeat(32),
                nonce: "n".into(),
                submit_nonce: "ee".repeat(32),
                state: proof_store::SubmissionState::Queued,
                receipt_json: None,
                verdict: None,
                detail: None,
            })
            .expect("mint");
        assert_eq!(row.id, "pf_000000000000000b");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn seed_pf_allocator_refuses_boot_when_the_artefact_scan_fails() {
        let root = std::env::temp_dir().join(format!(
            "proof-seed-notdir-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::write(&root, b"not a directory").expect("file");
        let err = seed_pf_allocator(&MemoryStore::new(), &MemoryRlmStore::new(), &root)
            .await
            .expect_err("incomplete scan must refuse boot");
        assert!(
            err.contains("artefact root"),
            "boot error must name the scan: {err}"
        );
        let _ = std::fs::remove_file(&root);
    }

    #[test]
    fn scored_epoch_file_defaults_to_the_artifacts_volume() {
        assert_eq!(
            cli().scored_epoch_file.as_os_str(),
            "/var/lib/proof/scored_epoch"
        );
    }

    /// Compose always sets `BASE_CHALLENGE_SK_FILE`. remote-deploy may leave an
    /// empty placeholder so Docker does not create a directory at that path.
    /// That must not exit 1 — `/health` has to come up so routing smoke works.
    #[test]
    fn an_empty_or_missing_challenge_sk_file_does_not_abort_boot() {
        let dir = std::env::temp_dir().join(format!(
            "proof-sk-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("dir");
        let empty = dir.join("sk");
        std::fs::write(&empty, []).expect("write");
        assert!(load_optional_sk(Some(&empty)).is_none());
        assert!(load_optional_sk(Some(&dir.join("missing"))).is_none());
        assert!(load_optional_sk(None).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

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
        let mux = live_scorer(
            EvalBackend::Lium,
            Some(harvest),
            Arc::new(MemoryRlmStore::new()),
            &root,
            &topic_vm_orchestrator(),
        )
        .expect("a wired harvest is the live scorer");
        assert!(mux.harvest_wired(), "the Lium harvest is the default route");
        assert!(registered_custom(Some(mux.as_ref())).is_empty());
        assert!(mux.ready_custom_ids().is_empty());
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
        let unwired = topic_vm_orchestrator();
        assert!(matches!(
            unwired.orchestrator.ready(),
            Err(proof_rlm::VmError::NotWired(_))
        ));
        assert!(unwired.template.image_digest.is_empty());
        assert!(!unwired.live);

        let dir = std::env::temp_dir().join(format!("proof-vm-wire-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let token = dir.join("vm_orchestrator_token");
        std::fs::write(&token, "vm-bearer-not-a-real-secret\n").expect("token");
        std::env::set_var(VM_ORCHESTRATOR_URL_ENV, "https://kvm.example.invalid:8200");
        std::env::set_var(VM_ORCHESTRATOR_TOKEN_FILE_ENV, &token);
        let pinned_less = topic_vm_orchestrator();
        let err = pinned_less.orchestrator.ready().expect_err("no image pin");
        assert!(matches!(err, proof_rlm::VmError::NotWired(_)), "{err}");
        assert!(err.to_string().contains(RLM_VM_IMAGE_DIGEST_ENV), "{err}");
        assert_eq!(
            (pinned_less.template.vcpus, pinned_less.template.mem_mib),
            (4, 8_192),
            "locked shape"
        );
        assert!(
            pinned_less.live,
            "selected by env; readiness is per request"
        );

        std::env::set_var(
            RLM_VM_IMAGE_DIGEST_ENV,
            format!("sha256:{}", "ab".repeat(32)),
        );
        let live = topic_vm_orchestrator();
        live.orchestrator
            .ready()
            .expect("url + token + digest = wired");
        live.template.validate().expect("pinned");
        assert!(live.live);
        std::env::set_var(VM_RUNNER_CUSTOM_IDS_ENV, "metric_a");
        let reg = runner_registry(&live);
        assert_eq!(reg.ids(), vec!["metric_a".to_owned()]);
        reg.resolve("metric_a")
            .expect("registered")
            .ready()
            .expect("runner over the live orchestrator is ready");

        std::fs::write(&token, "\n").expect("empty token");
        let live = topic_vm_orchestrator();
        let err = live.orchestrator.ready().expect_err("empty bearer file");
        assert!(
            err.to_string().contains(VM_ORCHESTRATOR_TOKEN_FILE_ENV),
            "{err}"
        );

        std::env::set_var(VM_ORCHESTRATOR_URL_ENV, "http://10.0.0.7:8200");
        let refused = topic_vm_orchestrator();
        let err = refused
            .orchestrator
            .ready()
            .expect_err("plain http off loopback is never wired");
        assert!(err.to_string().contains(VM_ORCHESTRATOR_URL_ENV), "{err}");
        assert!(!refused.live, "a refused config is the unwired stub");
        clear_vm_env();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The admin probe reports through the very client the runner drives: an
    /// unwired host names the env vars; a wired one shows `ready()` next to
    /// one agent health call, and a bad bearer, a dead agent, or an emptied
    /// bearer file each show up as data — never as a fallback, never as the
    /// bearer itself.
    #[test]
    fn vm_orchestrator_probe_reports_ready_agent_bearer_and_outage_as_data() {
        const TOKEN: &str = "probe-bearer-not-a-real-secret";
        let _guard = LIUM_ENV
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        clear_vm_env();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");

        let unwired = topic_vm_orchestrator();
        let report = rt.block_on(unwired.probe());
        assert_eq!(report.orchestrator, "unwired");
        assert!(!report.ready);
        assert!(
            report.reason.contains(VM_ORCHESTRATOR_URL_ENV),
            "{report:?}"
        );
        assert!(report.agent.is_none() && report.agent_error.is_none());

        // Two copies of the bearer, as on a real deployment: the agent's
        // /etc/proof-vm/token and the CP's PROOF_VM_ORCHESTRATOR_TOKEN_FILE.
        let agent_token = proof_vm_agent::fixtures::token_file("probe-agent", TOKEN);
        let token = proof_vm_agent::fixtures::token_file("probe-cp", TOKEN);
        let agent = rt.block_on(proof_vm_agent::fixtures::FakeAgent::serve(
            proof_vm_agent::fixtures::FakeHypervisor::new(0.8),
            &agent_token,
        ));
        std::env::set_var(VM_ORCHESTRATOR_URL_ENV, agent.url());
        std::env::set_var(VM_ORCHESTRATOR_TOKEN_FILE_ENV, &token);
        std::env::set_var(
            RLM_VM_IMAGE_DIGEST_ENV,
            format!("sha256:{}", "ab".repeat(32)),
        );
        let wired = topic_vm_orchestrator();
        assert!(wired.live && wired.fc.is_some());
        let report = rt.block_on(wired.probe());
        assert_eq!(report.orchestrator, "firecracker");
        assert!(report.ready, "{report:?}");
        assert_eq!((report.vcpus, report.mem_mib), (4, 8_192));
        let health = report.agent.as_ref().expect("agent answered");
        assert!(health.ready && health.hypervisor == "fake" && health.vms == 0);
        assert_eq!(report.agent_error, None);
        let dump = serde_json::to_string(&report).expect("json");
        assert!(!dump.contains(TOKEN), "bearer leaked: {dump}");

        std::fs::write(&token, "another-bearer-not-a-real-secret\n").expect("rotate one side");
        let report = rt.block_on(wired.probe());
        assert!(
            report.ready,
            "a non-empty bearer file is ready on the client"
        );
        assert!(report.agent.is_none());
        assert!(
            report
                .agent_error
                .as_deref()
                .is_some_and(|e| e.contains("refused the bearer")),
            "{report:?}"
        );

        agent.stop();
        std::fs::write(&token, format!("{TOKEN}\n")).expect("restore");
        let report = rt.block_on(wired.probe());
        assert!(
            report
                .agent_error
                .as_deref()
                .is_some_and(|e| e.contains("unreachable")),
            "{report:?}"
        );

        std::fs::write(&token, "\n").expect("empty");
        let report = rt.block_on(wired.probe());
        assert!(!report.ready);
        assert!(
            report.reason.contains(VM_ORCHESTRATOR_TOKEN_FILE_ENV),
            "{report:?}"
        );
        assert!(report.agent.is_none(), "no call without a bearer");
        clear_vm_env();
        let _ = std::fs::remove_file(&token);
        let _ = std::fs::remove_file(&agent_token);
    }

    /// Full topic-VM env (https URL, bearer file, image pin, one custom id),
    /// **no** Lium credentials: the custom family stands on its own. The
    /// registry is non-empty, the mux passes the host-wide gate and the
    /// registered custom topic is ready over the live orchestrator, while
    /// every `nll` / `throughput` topic refuses with
    /// `LiveHarvestUnavailable` and an unlisted custom id with
    /// `RunnerUnwired` — no placeholder Lium harvest is needed to open
    /// custom topics.
    #[test]
    fn custom_family_stands_without_a_lium_harvest() {
        let _guard = LIUM_ENV
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        clear_vm_env();
        std::env::remove_var("LIUM_API_KEY");
        std::env::remove_var("LIUM_SSH_PUBLIC_KEY_FILE");
        let dir = std::env::temp_dir().join(format!("proof-custom-only-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let token = dir.join("vm_orchestrator_token");
        std::fs::write(&token, "vm-bearer-not-a-real-secret\n").expect("token");
        std::env::set_var(VM_ORCHESTRATOR_URL_ENV, "https://kvm.example.invalid:8200");
        std::env::set_var(VM_ORCHESTRATOR_TOKEN_FILE_ENV, &token);
        std::env::set_var(
            RLM_VM_IMAGE_DIGEST_ENV,
            format!("sha256:{}", "ab".repeat(32)),
        );
        std::env::set_var(VM_RUNNER_CUSTOM_IDS_ENV, "metric_a");

        let harvest = build_live_scorer(EvalBackend::Lium, 900, None, None, None);
        assert!(harvest.is_none(), "no Lium credentials, no harvest");
        let root = dir.join("artefacts");
        let mux = live_scorer(
            EvalBackend::Lium,
            harvest,
            Arc::new(MemoryRlmStore::new()),
            &root,
            &topic_vm_orchestrator(),
        )
        .expect("the custom family is wired from the topic-vm env alone");
        assert!(
            !mux.harvest_wired(),
            "live_harvest_wired is Lium-only; a custom-only host reads false"
        );
        assert_eq!(
            registered_custom(Some(mux.as_ref())),
            vec!["metric_a".to_owned()]
        );
        assert_eq!(
            mux.ready_custom_ids(),
            vec!["metric_a".to_owned()],
            "custom readiness is reported on its own"
        );
        mux.ready().expect("no host-wide blocker");

        let mut custom = TopicDocument::default();
        custom.metric.family = proof_task::MetricFamily::Custom;
        custom.metric.custom_id = "metric_a".into();
        mux.ready_for_topic(&custom)
            .expect("registered runner over the live orchestrator is ready");
        custom.metric.custom_id = "metric_b".into();
        let err = mux.ready_for_topic(&custom).expect_err("unlisted id");
        assert!(
            matches!(err, proof_eval::EvalError::RunnerUnwired { .. }),
            "{err}"
        );
        let nll = TopicDocument::default();
        let err = mux.ready_for_topic(&nll).expect_err("no harvest");
        assert!(
            matches!(err, proof_eval::EvalError::LiveHarvestUnavailable),
            "{err}"
        );

        // Exactly the gate `/v1/status` and `POST /v1/submissions` apply
        // host-wide: it passes, so custom topics can score here.
        let pin = proof_rlm::fixtures::pin();
        let exec = executor_for(&pin);
        proof_eval::scoring_readiness(
            &pin,
            EvalBackend::Lium,
            Some(mux.as_ref()),
            true,
            Some(&proof_rlm::fixtures::offer()),
            Some(&exec),
            Some("test-judge-key"),
        )
        .expect("ready host-wide without Lium");

        // Registration and readiness are separate: an emptied bearer file
        // keeps the id registered but no longer ready (per-request 503).
        std::fs::write(&token, "\n").expect("empty token");
        assert_eq!(
            registered_custom(Some(mux.as_ref())),
            vec!["metric_a".to_owned()]
        );
        assert!(mux.ready_custom_ids().is_empty());
        assert!(!mux.harvest_wired());

        // Sim scores in-process and never wires anything live.
        assert!(live_scorer(
            EvalBackend::Sim,
            None,
            Arc::new(MemoryRlmStore::new()),
            &root,
            &topic_vm_orchestrator(),
        )
        .is_none());
        clear_vm_env();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Without Lium the custom family needs the live orchestrator **and** a
    /// listed id, or nothing is wired and the host stays fail-closed:
    /// orchestrator env unset (ids listed or not), env refused, or ids
    /// unset → `None` → `LiveHarvestUnavailable` host-wide (503). The
    /// `UnwiredVmOrchestrator` never carries a mux.
    #[test]
    fn without_lium_the_custom_family_needs_the_live_orchestrator_and_an_id() {
        let _guard = LIUM_ENV
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        clear_vm_env();
        std::env::remove_var("LIUM_API_KEY");
        std::env::remove_var("LIUM_SSH_PUBLIC_KEY_FILE");
        let dir = std::env::temp_dir().join(format!("proof-unwired-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let root = dir.join("artefacts");
        let store: Arc<dyn RlmStore> = Arc::new(MemoryRlmStore::new());
        let none = |label: &str| {
            assert!(
                live_scorer(
                    EvalBackend::Lium,
                    None,
                    store.clone(),
                    &root,
                    &topic_vm_orchestrator()
                )
                .is_none(),
                "{label}: nothing may be wired"
            );
        };

        none("no env at all");
        std::env::set_var(VM_RUNNER_CUSTOM_IDS_ENV, "metric_a");
        none("ids listed over the unwired orchestrator");

        let token = dir.join("vm_orchestrator_token");
        std::fs::write(&token, "vm-bearer-not-a-real-secret\n").expect("token");
        std::env::set_var(VM_ORCHESTRATOR_URL_ENV, "http://10.0.0.7:8200");
        std::env::set_var(VM_ORCHESTRATOR_TOKEN_FILE_ENV, &token);
        std::env::set_var(
            RLM_VM_IMAGE_DIGEST_ENV,
            format!("sha256:{}", "ab".repeat(32)),
        );
        none("refused orchestrator config (plain http off loopback)");

        std::env::set_var(VM_ORCHESTRATOR_URL_ENV, "https://kvm.example.invalid:8200");
        std::env::remove_var(VM_RUNNER_CUSTOM_IDS_ENV);
        none("live orchestrator but no custom id registered");
        std::env::set_var(VM_RUNNER_CUSTOM_IDS_ENV, " , Bad Id ");
        none("live orchestrator but no valid custom id");

        // What that `None` is on the wire: the host-wide gate refuses.
        let pin = proof_rlm::fixtures::pin();
        let err = proof_eval::scoring_readiness(
            &pin,
            EvalBackend::Lium,
            None,
            true,
            Some(&proof_rlm::fixtures::offer()),
            Some(&executor_for(&pin)),
            Some("test-judge-key"),
        )
        .expect_err("unwired host");
        assert!(
            matches!(err, proof_eval::EvalError::LiveHarvestUnavailable),
            "{err}"
        );
        clear_vm_env();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Open `1x` executor on `pin`'s digest-scoped template (host state the
    /// live gate requires; nothing here rents).
    fn executor_for(pin: &ProofPin) -> EvalExecutorOffer {
        let hex = pin.eval_image_digest.trim_start_matches("sha256:");
        let mut offer = EvalExecutorOffer {
            offer_id: "executor-placeholder".into(),
            lium_template_id: format!("proof-eval-{}", hex.get(..12).unwrap_or("unpinned")),
            machine_shape: "1x".into(),
            max_proof_deadline_s: 3_600,
            eval_image_digest: pin.eval_image_digest.clone(),
            config_commitment: String::new(),
            status: proof_challenge::OfferStatus::Open,
        };
        offer.config_commitment = offer.expected_commitment();
        offer
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
