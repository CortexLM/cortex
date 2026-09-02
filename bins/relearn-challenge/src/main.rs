//! `relearn-challenge` — master-only Relearn service (port 8095).
//!
//! Miner HTTP submit → digest freeze → holdout unseal → sim/Lium eval →
//! operator-audited promote. Miners pay Lium.
//!
//! Holdout records are loaded from an operator file and verified against the
//! commitment in `config/relearn-pin.toml`. If that file is absent or does not
//! match, the service still serves `/health` and `/v1/status` but refuses
//! submissions: scoring a reconstructable seed or the public split would
//! silently turn the anti-overfit gates off.
//!
//! The same rule applies to the scorer itself. Without `RELEARN_FORCE_SIM=1`
//! the host needs a `sha256:` eval-image pin, and submissions answer 503 until
//! it has one. Sim is never a fallback.

#![forbid(unsafe_code)]

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use challenge_keys::load_challenge_secret;
use clap::Parser;
use prism_lium::LiumClient;
use relearn_challenge::{
    hash_admin_token, parse_holdout_file, relearn_router, resolve_eval_backend, AppState,
    BaselineMeasurement, EvalBackend, LiveScorer, MemoryStore, RelearnPin, BASE_CHAMPION_RUN,
    CHALLENGE_ID, SCORING_VERSION,
};
#[cfg(test)]
use relearn_challenge::{holdout_commitment, HoldoutItem, HoldoutTask};
use relearn_eval::boot_base_champion;
use relearn_lium_harvest::{HarvestLimits, LiumEvalPod, LiumHarvest, TeacherEnv};
use tokio::net::TcpListener;

/// Operator Relearn challenge service CLI.
#[derive(Debug, Parser)]
#[command(
    name = "relearn-challenge",
    about = "Relearn challenge service (port 8095, master→Lium/sim)"
)]
struct Cli {
    /// Bind address (default 0.0.0.0:8095).
    #[arg(long, env = "BASE_CHALLENGE_BIND", default_value = "0.0.0.0:8095")]
    bind: SocketAddr,
    /// Challenge mini-secret file (leaf signatures).
    #[arg(long, env = "BASE_CHALLENGE_SK_FILE")]
    challenge_sk_file: Option<PathBuf>,
    /// Force sim eval (no Lium spend). CI / local only — never a live scorer.
    ///
    /// `RELEARN_FORCE_SIM` is read by `resolve_eval_backend`, which accepts
    /// `1` / `true` / `yes`, so it is deliberately not bound here: clap would
    /// reject the documented `RELEARN_FORCE_SIM=1` and refuse to boot.
    #[arg(long, default_value_t = false)]
    force_sim: bool,
    /// Operator bearer tokens file (one per line). Empty → admin 503.
    #[arg(long, env = "RELEARN_ADMIN_TOKENS_FILE")]
    admin_tokens_file: Option<PathBuf>,
    /// Pin file (`config/relearn-pin.toml`).
    #[arg(long, env = "RELEARN_PIN_FILE")]
    pin_file: Option<PathBuf>,
    /// Operator holdout records (JSON array). Never in git.
    #[arg(long, env = "RELEARN_HOLDOUT_FILE")]
    holdout_file: Option<PathBuf>,
    /// Seconds the eval image gets to score one artifact on the pod.
    #[arg(long, env = "RELEARN_EVAL_TIMEOUT_SECS", default_value_t = 3600)]
    eval_timeout_secs: u64,
    /// Champion baseline measured by the pinned eval image on the base model.
    ///
    /// Required on a live host: the gates compare against this. Verified
    /// against the pin's `eval_image_digest` + `holdout_commitment` at boot.
    #[arg(long, env = "RELEARN_BASE_CHAMPION_FILE")]
    base_champion_file: Option<PathBuf>,
    /// Persisted submissions, evaluation results, and champion. Restored at
    /// boot; a corrupt file refuses to serve rather than empty-scoring.
    #[arg(long, env = "RELEARN_STATE_FILE")]
    state_file: Option<PathBuf>,
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
    let mut pin = load_pin(cli.pin_file.as_deref())?;
    let backend = if cli.force_sim {
        tracing::info!("RELEARN_FORCE_SIM=1 — deterministic offline eval, not a real eval");
        EvalBackend::Sim
    } else {
        resolve_eval_backend()
    };
    if backend != EvalBackend::Sim {
        match pin.bind_live_holdout_from_env() {
            Ok(()) => tracing::info!("live holdout commitment bound from secret store"),
            Err(e) => tracing::warn!(
                "live holdout not bound ({e}); submissions will 503 until a private commitment is supplied"
            ),
        }
    }
    if backend == EvalBackend::Lium && !pin.can_rent() {
        tracing::warn!(
            "eval_image_digest not pinned; submissions will 503 until CortexLM/relearn CI \
             publishes a sha256 eval image (sim is opt-in via RELEARN_FORCE_SIM, never a fallback)"
        );
    }
    let admin_hashes = load_admin_hashes(cli.admin_tokens_file.as_deref());
    let store = MemoryStore::open(cli.state_file.as_deref()).map_err(|e| e.to_string())?;
    store
        .set_holdout_commitment(&pin.holdout_commitment, pin.holdout_size)
        .map_err(|e| e.to_string())?;
    match load_holdout(&store, &pin, cli.holdout_file.as_deref()) {
        Ok(n) => tracing::info!(holdout_items = n, "holdout verified against pin commitment"),
        Err(e) => tracing::warn!("holdout unavailable ({e}); submissions will 503 until fixed"),
    }
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;

    let live_scorer = build_live_scorer(backend, cli.eval_timeout_secs);
    match backend {
        EvalBackend::Lium if live_scorer.is_some() => {
            tracing::info!("live harvest wired: digest-pinned eval image on Lium");
        }
        // The baseline file cannot stand in for this: it covers the champion
        // only, and every submission still needs its own measurement.
        EvalBackend::Lium => tracing::warn!(
            "live harvest not wired; every submission will 503. Set the Lium credentials \
             and LIUM_SSH_PUBLIC_KEY_FILE (deploy/env/relearn-challenge.env.example)"
        ),
        EvalBackend::Sim => {}
    }

    // Record the baseline with the scorer this host will actually use. A sim
    // host gets sim numbers; a live host takes the operator's eval-image
    // measurement, else measures the base model through the harvest. Without a
    // baseline no gate can run — contamination, public-holdout gap, and
    // pixel-shuffle all need a champion to compare against — so a live host
    // that skips this 503s every submission.
    let recorded = load_recorded_baseline(cli.base_champion_file.as_deref())?;
    match rt.block_on(record_base_champion(
        &store,
        &pin,
        backend,
        recorded,
        live_scorer.as_deref(),
    )) {
        Ok(n) => tracing::info!(
            eval_backend = ?backend,
            holdout_items = n,
            "champion baseline recorded"
        ),
        Err(e) => tracing::warn!(
            eval_backend = ?backend,
            "champion baseline not recorded ({e}); submissions will 503 until it is"
        ),
    }
    let state = AppState {
        store,
        pin,
        backend,
        live_scorer,
        admin_hashes: Arc::new(admin_hashes),
    };
    rt.block_on(serve(cli.bind, state))
}

/// Wire the live harvest on the Lium path.
///
/// Sim hosts never get one: sim is selected by `RELEARN_FORCE_SIM` and scores
/// in-process. On the Lium path the miner's `LIUM_API_KEY` pays for the pod, so
/// no key means no harvest and the host refuses rather than inventing numbers.
fn build_live_scorer(backend: EvalBackend, run_timeout_secs: u64) -> Option<Arc<dyn LiveScorer>> {
    if backend != EvalBackend::Lium {
        return None;
    }
    let key = std::env::var("LIUM_API_KEY")
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())?;
    // Never logged, never echoed on /v1/status. `LIUM_API_BASE_URL` lets a
    // staging host point at a stand-in provider instead of spending real money.
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
    // Without the master public key the pod boots unreachable: the request
    // could not be delivered and no metrics could be read back.
    let Some(ssh_pub) = load_ssh_public_key() else {
        tracing::warn!(
            "live harvest not wired: no SSH public key (LIUM_SSH_PUBLIC_KEY_FILE or default)"
        );
        return None;
    };
    // A Lium InstanceSpec has no env field, so the pod inherits nothing from
    // this host. The image refuses to score without the judge URL, which looks
    // like a pod that boots, runs, and never prints its OK marker.
    let teacher = TeacherEnv::from_host_env();
    if teacher.has_judge() {
        tracing::info!(
            forwarded = ?teacher.present_names(),
            base_weights_primed = teacher.has_base_weights(),
            base_weights_via = ?teacher.base_weights_via(),
            "teacher config will be forwarded into the eval pod"
        );
    } else {
        tracing::warn!(
            "RELEARN_TEACHER_API_URL unset; the eval image has no judge and every submission \
             will 503 before a pod is rented"
        );
    }
    if teacher.has_judge() && !teacher.has_base_weights() {
        tracing::warn!(
            "RELEARN_BASE_MODEL_DIR unset and RELEARN_ALLOW_MODEL_DOWNLOAD is not 1; \
             every submission will 503 before a pod is rented"
        );
    }
    let pod = Arc::new(LiumEvalPod::new(
        client,
        run_timeout_secs,
        relearn_lium_harvest::PROGRAM,
    ));
    Some(Arc::new(LiumHarvest::new(
        pod,
        HarvestLimits::default(),
        vec![ssh_pub],
        teacher,
    )))
}

/// Master SSH public key for harvest pods. Same convention as
/// `prism-challenge`: `LIUM_SSH_PUBLIC_KEY_FILE`, else the default path.
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

fn load_pin(path: Option<&Path>) -> Result<RelearnPin, String> {
    let Some(p) = path else {
        return Ok(RelearnPin::default());
    };
    let body = std::fs::read_to_string(p).map_err(|e| format!("read {}: {e}", p.display()))?;
    let pin = RelearnPin::from_toml(&body).map_err(|e| e.to_string())?;
    pin.validate().map_err(|e| e.to_string())?;
    Ok(pin)
}

fn load_holdout(
    store: &MemoryStore,
    pin: &RelearnPin,
    path: Option<&Path>,
) -> Result<usize, String> {
    let p = path.ok_or("RELEARN_HOLDOUT_FILE not set")?;
    let body = std::fs::read_to_string(p).map_err(|e| format!("read {}: {e}", p.display()))?;
    let records = parse_holdout_file(&body)?;
    let n = records.len();
    store
        .load_holdout(records, &[], &pin.public_ids)
        .map_err(|e| e.to_string())?;
    Ok(n)
}

fn load_recorded_baseline(path: Option<&Path>) -> Result<Option<BaselineMeasurement>, String> {
    let Some(p) = path else {
        return Ok(None);
    };
    let body = std::fs::read_to_string(p).map_err(|e| format!("read {}: {e}", p.display()))?;
    BaselineMeasurement::from_json(&body)
        .map(Some)
        .map_err(|e| format!("{}: {e}", p.display()))
}

async fn record_base_champion(
    store: &MemoryStore,
    pin: &RelearnPin,
    backend: EvalBackend,
    recorded: Option<BaselineMeasurement>,
    live: Option<&dyn LiveScorer>,
) -> Result<usize, String> {
    let recs = store
        .unseal_holdout(BASE_CHAMPION_RUN)
        .map_err(|e| e.to_string())?;
    let scores = boot_base_champion(pin, &recs, backend, recorded, live)
        .await
        .map_err(|e| e.to_string())?;
    store.set_base_champion(scores).map_err(|e| e.to_string())?;
    Ok(recs.len())
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

#[cfg(test)]
mod tests {
    use super::*;

    /// `RELEARN_FORCE_SIM=1` is what every doc and runbook says to set. Binding
    /// it to a clap `bool` made clap reject `1` and the service refused to
    /// boot, so the documented sim opt-in was unreachable.
    #[test]
    fn documented_force_sim_values_do_not_break_argument_parsing() {
        for value in ["1", "true", "yes", "false", ""] {
            std::env::set_var("RELEARN_FORCE_SIM", value);
            let cli = Cli::try_parse_from(["relearn-challenge"])
                .unwrap_or_else(|e| panic!("RELEARN_FORCE_SIM={value:?} broke parsing: {e}"));
            assert!(!cli.force_sim, "env must not set the flag");
        }
        std::env::set_var("RELEARN_FORCE_SIM", "1");
        assert_eq!(resolve_eval_backend(), EvalBackend::Sim);
        std::env::set_var("RELEARN_FORCE_SIM", "false");
        assert_eq!(resolve_eval_backend(), EvalBackend::Lium);
        std::env::remove_var("RELEARN_FORCE_SIM");
    }

    fn holdout() -> Vec<HoldoutItem> {
        (1..=120)
            .map(|id| HoldoutItem {
                id: 800 + id,
                prompt: format!("holdout item {id} with enough words for a trigram"),
                dataset_id: "dev".into(),
                task: HoldoutTask::Text,
                image_hash: String::new(),
            })
            .collect()
    }

    fn seeded(pin: &RelearnPin, recs: &[HoldoutItem]) -> MemoryStore {
        let store = MemoryStore::new();
        store
            .set_holdout_commitment(&pin.holdout_commitment, pin.holdout_size)
            .expect("commit");
        store
            .load_holdout(recs.to_vec(), &[], &pin.public_ids)
            .expect("load");
        store
    }

    /// The boot path, not just the library: a live host must come up with a
    /// champion baseline, otherwise every submission 503s before the gates.
    #[tokio::test]
    async fn live_boot_records_the_champion_baseline_from_the_operator_file() {
        let recs = holdout();
        let pin = RelearnPin {
            holdout_commitment: holdout_commitment(&recs),
            holdout_size: recs.len(),
            eval_image_digest: format!("sha256:{}", "ab".repeat(32)),
            ..RelearnPin::default()
        };
        let store = seeded(&pin, &recs);
        assert!(store.champion_scores().expect("read").is_none());

        let measured = relearn_eval::sim_slice_scores_at_skill(
            relearn_eval::BASE_CHAMPION_ARTIFACT,
            &recs,
            relearn_eval::BASE_CHAMPION_SKILL,
        );
        let file = BaselineMeasurement {
            eval_image_digest: pin.eval_image_digest.clone(),
            holdout_commitment: pin.holdout_commitment.clone(),
            holdout: measured.holdout.by_cluster,
            public: measured.public.by_cluster,
            perturbed: measured.perturbed.by_cluster,
            canaries: measured.canaries.by_cluster,
            general_canary: measured.general_canary.by_cluster,
            agent_trace: measured.agent_trace,
            vision_shuffle: measured.vision_shuffle,
        };

        let n = record_base_champion(&store, &pin, EvalBackend::Lium, Some(file), None)
            .await
            .expect("live baseline recorded");
        assert_eq!(n, recs.len());
        assert!(store.champion_scores().expect("read").is_some());
    }

    #[tokio::test]
    async fn live_boot_without_a_baseline_source_records_nothing() {
        let recs = holdout();
        let pin = RelearnPin {
            holdout_commitment: holdout_commitment(&recs),
            holdout_size: recs.len(),
            eval_image_digest: format!("sha256:{}", "ab".repeat(32)),
            ..RelearnPin::default()
        };
        let store = seeded(&pin, &recs);
        assert!(
            record_base_champion(&store, &pin, EvalBackend::Lium, None, None)
                .await
                .is_err()
        );
        assert!(
            store.champion_scores().expect("read").is_none(),
            "a live host must not inherit sim numbers as its champion"
        );
    }

    #[tokio::test]
    async fn sim_boot_still_records_the_sim_baseline() {
        let recs = holdout();
        let pin = RelearnPin {
            holdout_commitment: holdout_commitment(&recs),
            holdout_size: recs.len(),
            ..RelearnPin::default()
        };
        let store = seeded(&pin, &recs);
        record_base_champion(&store, &pin, EvalBackend::Sim, None, None)
            .await
            .expect("sim baseline");
        assert!(store.champion_scores().expect("read").is_some());
    }

    #[test]
    fn missing_baseline_file_is_none_and_a_bad_one_is_an_error() {
        assert!(load_recorded_baseline(None).expect("no path").is_none());
        assert!(load_recorded_baseline(Some(Path::new("/nonexistent/baseline.json"))).is_err());
    }

    #[test]
    fn corrupt_state_file_fails_closed_instead_of_empty_scoring() {
        let dir = std::env::temp_dir().join(format!("relearn-bin-state-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("state.json");
        std::fs::write(&path, "not-a-store").expect("write");
        let Err(err) = MemoryStore::open(Some(&path)) else {
            panic!("corrupt restore must fail closed")
        };
        assert!(err.to_string().contains("restore"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The wiring the Subnet Owner asked for: the harvest is built on the LIVE
    /// Lium path, not only under `FORCE_SIM`. Sim scores in-process and must
    /// never be handed a Lium harvest.
    #[test]
    fn live_harvest_is_wired_on_the_lium_path_not_on_sim() {
        let _guard = LIUM_ENV
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let pubkey = stub_ssh_pubkey("wired");
        std::env::set_var("LIUM_API_KEY", "test-key-not-a-real-secret");
        std::env::set_var("LIUM_SSH_PUBLIC_KEY_FILE", &pubkey);

        let live = build_live_scorer(EvalBackend::Lium, 900);
        assert!(
            live.is_some(),
            "Lium boot must wire the digest-pinned harvest"
        );
        assert!(
            build_live_scorer(EvalBackend::Sim, 900).is_none(),
            "sim scores in-process; a Lium harvest there would spend money"
        );

        // No miner key, no pod: refuse rather than invent numbers.
        std::env::remove_var("LIUM_API_KEY");
        assert!(build_live_scorer(EvalBackend::Lium, 900).is_none());
        std::env::set_var("LIUM_API_KEY", "   ");
        assert!(
            build_live_scorer(EvalBackend::Lium, 900).is_none(),
            "a blank key is not a key"
        );

        // A pod with no master key is unreachable, so it must not be rented.
        std::env::set_var("LIUM_API_KEY", "test-key-not-a-real-secret");
        std::env::set_var("LIUM_SSH_PUBLIC_KEY_FILE", "/nonexistent/id.pub");
        assert!(
            build_live_scorer(EvalBackend::Lium, 900).is_none(),
            "no master SSH public key means no harvest"
        );
        std::env::remove_var("LIUM_API_KEY");
        std::env::remove_var("LIUM_SSH_PUBLIC_KEY_FILE");
    }

    fn stub_ssh_pubkey(tag: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("relearn-test-{tag}.pub"));
        std::fs::write(&path, "ssh-ed25519 AAAAtest relearn-test\n").expect("write pubkey");
        path
    }

    /// A wired harvest is what turns `live_harvest_wired` on, and the readiness
    /// check is what a submission consults before spending anything.
    #[test]
    fn a_wired_harvest_makes_a_pinned_host_ready_to_score() {
        let _guard = LIUM_ENV
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let pubkey = stub_ssh_pubkey("ready");
        std::env::set_var("LIUM_API_KEY", "test-key-not-a-real-secret");
        std::env::set_var("LIUM_SSH_PUBLIC_KEY_FILE", &pubkey);

        // A harvest with no judge is wired but not ready: the eval image would
        // exit without scoring, so that must surface before a pod is rented.
        std::env::remove_var("RELEARN_TEACHER_API_URL");
        let judgeless = build_live_scorer(EvalBackend::Lium, 900).expect("wired");
        let pinned = RelearnPin {
            eval_image_digest: format!("sha256:{}", "ab".repeat(32)),
            ..RelearnPin::default()
        };
        let err =
            relearn_eval::scoring_readiness(&pinned, EvalBackend::Lium, Some(judgeless.as_ref()))
                .expect_err("no judge configured");
        assert!(err.to_string().contains("RELEARN_TEACHER_API_URL"), "{err}");

        std::env::set_var("RELEARN_TEACHER_API_URL", "http://teacher.invalid/v1");
        std::env::remove_var("RELEARN_ALLOW_MODEL_DOWNLOAD");
        std::env::remove_var("RELEARN_BASE_MODEL_DIR");
        let unprimed = build_live_scorer(EvalBackend::Lium, 900).expect("wired");
        let weights_err =
            relearn_eval::scoring_readiness(&pinned, EvalBackend::Lium, Some(unprimed.as_ref()))
                .expect_err("no backbone");
        assert!(
            weights_err.to_string().contains("RELEARN_BASE_MODEL_DIR"),
            "{weights_err}"
        );

        std::env::set_var("RELEARN_ALLOW_MODEL_DOWNLOAD", "1");
        let live = build_live_scorer(EvalBackend::Lium, 900).expect("wired");
        std::env::remove_var("LIUM_API_KEY");
        std::env::remove_var("LIUM_SSH_PUBLIC_KEY_FILE");
        std::env::remove_var("RELEARN_TEACHER_API_URL");
        std::env::remove_var("RELEARN_ALLOW_MODEL_DOWNLOAD");

        let unpinned = RelearnPin::default();
        assert!(
            relearn_eval::scoring_readiness(&unpinned, EvalBackend::Lium, Some(live.as_ref()))
                .is_err(),
            "a wired harvest still needs a digest pin"
        );

        assert!(
            relearn_eval::scoring_readiness(&pinned, EvalBackend::Lium, None).is_err(),
            "a digest pin alone is not enough"
        );
        relearn_eval::scoring_readiness(&pinned, EvalBackend::Lium, Some(live.as_ref()))
            .expect("pinned digest + wired harvest can score");
    }

    /// `LIUM_API_KEY` is process-wide, so the tests that set it must not race.
    static LIUM_ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());
}
