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
use relearn_challenge::{
    hash_admin_token, parse_holdout_file, relearn_router, resolve_eval_backend, AppState,
    BaselineMeasurement, EvalBackend, LiveScorer, MemoryStore, RelearnPin, BASE_CHAMPION_RUN,
    CHALLENGE_ID, SCORING_VERSION,
};
#[cfg(test)]
use relearn_challenge::{holdout_commitment, HoldoutItem, HoldoutTask};
use relearn_eval::boot_base_champion;
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
    /// Champion baseline measured by the pinned eval image on the base model.
    ///
    /// Required on a live host: the gates compare against this. Verified
    /// against the pin's `eval_image_digest` + `holdout_commitment` at boot.
    #[arg(long, env = "RELEARN_BASE_CHAMPION_FILE")]
    base_champion_file: Option<PathBuf>,
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
        tracing::info!("RELEARN_FORCE_SIM=1 — deterministic offline eval, not a real eval");
        EvalBackend::Sim
    } else {
        resolve_eval_backend()
    };
    if backend == EvalBackend::Lium && !pin.can_rent() {
        tracing::warn!(
            "eval_image_digest not pinned; submissions will 503 until CortexLM/relearn CI \
             publishes a sha256 eval image (sim is opt-in via RELEARN_FORCE_SIM, never a fallback)"
        );
    }
    let admin_hashes = load_admin_hashes(cli.admin_tokens_file.as_deref());
    let store = MemoryStore::new();
    store
        .set_holdout_commitment(&pin.holdout_commitment, pin.holdout_size)
        .map_err(|e| e.to_string())?;
    match load_holdout(&store, &pin, cli.holdout_file.as_deref()) {
        Ok(n) => tracing::info!(holdout_items = n, "holdout verified against pin commitment"),
        Err(e) => tracing::warn!("holdout unavailable ({e}); submissions will 503 until fixed"),
    }
    // Record the baseline with the scorer this host will actually use. A sim
    // host gets sim numbers; a live host takes the operator's eval-image
    // measurement. Without a baseline no gate can run — contamination,
    // public-holdout gap, and pixel-shuffle all need a champion to compare
    // against — so a live host that skips this 503s every submission.
    let recorded = load_recorded_baseline(cli.base_champion_file.as_deref())?;
    let live_scorer: Option<Arc<dyn LiveScorer>> = None;
    match record_base_champion(&store, &pin, backend, recorded, live_scorer.as_deref()) {
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
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    rt.block_on(serve(cli.bind, state))
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

fn record_base_champion(
    store: &MemoryStore,
    pin: &RelearnPin,
    backend: EvalBackend,
    recorded: Option<BaselineMeasurement>,
    live: Option<&dyn LiveScorer>,
) -> Result<usize, String> {
    let recs = store
        .unseal_holdout(BASE_CHAMPION_RUN)
        .map_err(|e| e.to_string())?;
    let scores =
        boot_base_champion(pin, &recs, backend, recorded, live).map_err(|e| e.to_string())?;
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
    #[test]
    fn live_boot_records_the_champion_baseline_from_the_operator_file() {
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
            .expect("live baseline recorded");
        assert_eq!(n, recs.len());
        assert!(store.champion_scores().expect("read").is_some());
    }

    #[test]
    fn live_boot_without_a_baseline_source_records_nothing() {
        let recs = holdout();
        let pin = RelearnPin {
            holdout_commitment: holdout_commitment(&recs),
            holdout_size: recs.len(),
            eval_image_digest: format!("sha256:{}", "ab".repeat(32)),
            ..RelearnPin::default()
        };
        let store = seeded(&pin, &recs);
        assert!(record_base_champion(&store, &pin, EvalBackend::Lium, None, None).is_err());
        assert!(
            store.champion_scores().expect("read").is_none(),
            "a live host must not inherit sim numbers as its champion"
        );
    }

    #[test]
    fn sim_boot_still_records_the_sim_baseline() {
        let recs = holdout();
        let pin = RelearnPin {
            holdout_commitment: holdout_commitment(&recs),
            holdout_size: recs.len(),
            ..RelearnPin::default()
        };
        let store = seeded(&pin, &recs);
        record_base_champion(&store, &pin, EvalBackend::Sim, None, None).expect("sim baseline");
        assert!(store.champion_scores().expect("read").is_some());
    }

    #[test]
    fn missing_baseline_file_is_none_and_a_bad_one_is_an_error() {
        assert!(load_recorded_baseline(None).expect("no path").is_none());
        assert!(load_recorded_baseline(Some(Path::new("/nonexistent/baseline.json"))).is_err());
    }
}
