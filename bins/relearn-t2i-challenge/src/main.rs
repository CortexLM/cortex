//! `relearn-t2i-challenge` — master-only Relearn Image service (port 8097).
//!
//! Miner HTTP submit → digest freeze → holdout unseal → frozen-cell generation
//! → Q-Judger scoring → operator-audited promote. Miners pay Lium.
//!
//! The challenge id on the wire is `relearn-image`; the binary, env prefix, and
//! deployed paths keep the pre-launch `t2i` spelling (`docs/NAMING.md`).
//!
//! The holdout prompt records are loaded from an operator file and verified
//! against the commitment in `config/relearn-t2i-pin.toml`. If that file is
//! absent or does not match, the service still serves `/health` and
//! `/v1/status` but refuses submissions: scoring against the public split
//! instead would silently turn the anti-overfit gate off.
//!
//! The same rule applies to the scorer itself. Without `RELEARN_T2I_FORCE_SIM=1`
//! the host needs a `sha256:` eval-image pin, a wired harvest, and a champion
//! baseline measured by that image, and submissions answer 503 until it has
//! all three. Sim is never a fallback.

#![forbid(unsafe_code)]

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use challenge_keys::load_challenge_secret;
use clap::Parser;
use prism_lium::LiumClient;
use relearn_t2i_challenge::{
    boot_base_champion, hash_admin_token, parse_holdout_file, relearn_t2i_router, AppState,
    JudgeBackend, JudgeConfig, LiveJudge, MemoryStore, RelearnT2iPin, T2iBaselineMeasurement,
    CHALLENGE_ID, SCORING_VERSION,
};
use relearn_t2i_harvest::{HarvestLimits, LiumImageHarvest};
use relearn_t2i_task::FrozenPrompt;
use tokio::net::TcpListener;

/// Run id bound into the boot baseline measurement.
const BOOT_BASELINE_RUN: &str = "boot-baseline";

/// Operator Relearn Image challenge service CLI.
#[derive(Debug, Parser)]
#[command(
    name = "relearn-t2i-challenge",
    about = "Relearn Image challenge service (port 8097, master→Lium/sim)"
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
    /// accepts `1` / `true` / `yes`, so it is deliberately not bound here:
    /// clap would reject the documented `RELEARN_T2I_FORCE_SIM=1` spelling and
    /// the service would refuse to boot.
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
    /// Seconds the eval image gets to score one artifact on the pod.
    #[arg(long, env = "RELEARN_T2I_EVAL_TIMEOUT_SECS", default_value_t = 5400)]
    eval_timeout_secs: u64,
    /// Champion baseline measured by the pinned eval image on the base
    /// checkpoint.
    ///
    /// Required on a live host: every gate compares against this. Verified
    /// against the pin's `eval_image_digest` + `holdout_commitment` at boot.
    #[arg(long, env = "RELEARN_T2I_BASE_CHAMPION_FILE")]
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
    let judge = if cli.force_sim {
        tracing::info!("RELEARN_T2I_FORCE_SIM=1 — deterministic offline judge, not a real eval");
        JudgeConfig::sim()
    } else {
        JudgeConfig::from_env()
    };
    if judge.backend != JudgeBackend::Sim && !pin.can_rent() {
        tracing::warn!(
            "eval_image_digest not pinned; submissions will 503 until CortexLM/relearn CI \
             publishes a sha256 eval image (sim is opt-in via RELEARN_T2I_FORCE_SIM, never a \
             fallback)"
        );
    }

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

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;

    let live_judge = build_live_judge(&judge, cli.eval_timeout_secs);
    match judge.backend {
        JudgeBackend::Sim => {}
        _ if live_judge.is_some() => {
            tracing::info!("live harvest wired: digest-pinned eval image on Lium");
        }
        // The baseline file cannot stand in for this: it covers the champion
        // only, and every submission still needs its own measurement.
        _ => tracing::warn!(
            "live harvest not wired; every submission will 503. Set the Lium credentials \
             and LIUM_SSH_PUBLIC_KEY_FILE (deploy/env/relearn-t2i-challenge.env.example)"
        ),
    }

    // Record the baseline with the scorer this host will actually use. A sim
    // host gets sim numbers; a live host takes the operator's eval-image
    // measurement, else measures the base checkpoint through the harvest.
    // Without a baseline no gate can run, so a live host that skips this 503s
    // every submission rather than crowning against numbers nobody measured.
    let recorded = load_recorded_baseline(cli.base_champion_file.as_deref())?;
    match rt.block_on(record_base_champion(
        &store,
        &pin,
        &judge,
        recorded,
        live_judge.as_deref(),
    )) {
        Ok(n) => tracing::info!(
            judge_backend = ?judge.backend,
            holdout_prompts = n,
            "champion baseline recorded"
        ),
        Err(e) => tracing::warn!(
            judge_backend = ?judge.backend,
            "champion baseline not recorded ({e}); submissions will 503 until it is"
        ),
    }

    let state = AppState {
        store,
        pin,
        judge,
        live_judge,
        admin_hashes: Arc::new(load_admin_hashes(cli.admin_tokens_file.as_deref())),
    };
    rt.block_on(serve(cli.bind, state))
}

/// Wire the live harvest on the Q-Judger path.
///
/// Sim hosts never get one: sim is selected by `RELEARN_T2I_FORCE_SIM` and
/// scores in-process. On a live path the miner's `LIUM_API_KEY` pays for the
/// pod, so no key means no harvest and the host refuses rather than inventing
/// numbers.
fn build_live_judge(judge: &JudgeConfig, run_timeout_secs: u64) -> Option<Arc<dyn LiveJudge>> {
    if judge.backend == JudgeBackend::Sim {
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
    let pod = Arc::new(harvest_pod::LiumEvalPod::new(
        client,
        run_timeout_secs,
        relearn_t2i_harvest::PROGRAM,
    ));
    Some(Arc::new(LiumImageHarvest::new(
        pod,
        HarvestLimits::default(),
        vec![ssh_pub],
    )))
}

/// Master SSH public key for harvest pods. Same convention as the text
/// challenge: `LIUM_SSH_PUBLIC_KEY_FILE`, else the default path.
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

fn load_recorded_baseline(path: Option<&Path>) -> Result<Option<T2iBaselineMeasurement>, String> {
    let Some(p) = path else {
        return Ok(None);
    };
    let body = std::fs::read_to_string(p).map_err(|e| format!("read {}: {e}", p.display()))?;
    T2iBaselineMeasurement::from_json(&body)
        .map(Some)
        .map_err(|e| format!("{}: {e}", p.display()))
}

async fn record_base_champion(
    store: &MemoryStore,
    pin: &RelearnT2iPin,
    judge: &JudgeConfig,
    recorded: Option<T2iBaselineMeasurement>,
    live: Option<&dyn LiveJudge>,
) -> Result<usize, String> {
    let prompts: Vec<FrozenPrompt> = store
        .unseal_holdout(BOOT_BASELINE_RUN)
        .map_err(|e| e.to_string())?;
    let scores = boot_base_champion(pin, &prompts, judge, recorded, live)
        .await
        .map_err(|e| e.to_string())?;
    store.set_base_champion(scores).map_err(|e| e.to_string())?;
    Ok(prompts.len())
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
        "relearn-image challenge listening"
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
    use relearn_t2i_task::{frozen_prompt_commitment, PromptPin};

    use super::*;

    /// `RELEARN_T2I_FORCE_SIM=1` is what every doc and runbook says to set.
    /// Binding it to a clap `bool` would make clap reject `1` and the service
    /// would refuse to boot, leaving the documented sim opt-in unreachable.
    #[test]
    fn documented_force_sim_values_do_not_break_argument_parsing() {
        for value in ["1", "true", "yes", "false", ""] {
            std::env::set_var("RELEARN_T2I_FORCE_SIM", value);
            let cli = Cli::try_parse_from(["relearn-t2i-challenge"])
                .unwrap_or_else(|e| panic!("RELEARN_T2I_FORCE_SIM={value:?} broke parsing: {e}"));
            assert!(!cli.force_sim, "env must not set the flag");
        }
        std::env::set_var("RELEARN_T2I_FORCE_SIM", "1");
        assert_eq!(JudgeConfig::from_env().backend, JudgeBackend::Sim);
        std::env::remove_var("RELEARN_T2I_FORCE_SIM");
    }

    fn prompts() -> Vec<FrozenPrompt> {
        (900..=924)
            .map(|id| FrozenPrompt {
                id,
                text: format!("holdout prompt {id}"),
                upsampled_json: None,
            })
            .collect()
    }

    fn pin(digest: &str) -> RelearnT2iPin {
        RelearnT2iPin {
            prompts: PromptPin {
                pin_salt: "cortex-image-boot-test".into(),
                variations_per_prompt: 4,
                public_ids: (1..=25).collect(),
                holdout_commitment: frozen_prompt_commitment(&prompts()),
                holdout_size: prompts().len(),
            },
            eval_image_digest: digest.to_owned(),
            ..RelearnT2iPin::default()
        }
    }

    fn seeded(p: &RelearnT2iPin) -> MemoryStore {
        let store = MemoryStore::new();
        store
            .set_holdout_commitment(&p.prompts.holdout_commitment, p.prompts.holdout_size)
            .expect("commit");
        store
            .load_holdout(prompts(), &p.prompts.public_ids)
            .expect("load");
        store
    }

    /// The boot path, not just the library: a live host with no baseline
    /// source records nothing rather than inheriting sim numbers.
    #[tokio::test]
    async fn live_boot_without_a_baseline_source_records_nothing() {
        let p = pin(&format!("sha256:{}", "ab".repeat(32)));
        let store = seeded(&p);
        let live = JudgeConfig::http_api("http://judge.invalid/v1");
        assert!(record_base_champion(&store, &p, &live, None, None)
            .await
            .is_err());
        assert!(
            store.champion_scores().expect("read").is_none(),
            "a live host must not inherit sim numbers as its champion"
        );
    }

    #[tokio::test]
    async fn sim_boot_still_records_the_sim_baseline() {
        let p = pin("");
        let store = seeded(&p);
        record_base_champion(&store, &p, &JudgeConfig::sim(), None, None)
            .await
            .expect("sim baseline");
        assert!(store.champion_scores().expect("read").is_some());
    }

    #[test]
    fn missing_baseline_file_is_none_and_a_bad_one_is_an_error() {
        assert!(load_recorded_baseline(None).expect("no path").is_none());
        assert!(load_recorded_baseline(Some(Path::new("/nonexistent/baseline.json"))).is_err());
    }

    /// The harvest is built on the live judge path, not only under
    /// `RELEARN_T2I_FORCE_SIM`. Sim scores in-process and must never be handed
    /// a Lium harvest.
    #[test]
    fn live_harvest_is_wired_on_the_judge_path_not_on_sim() {
        let _guard = LIUM_ENV
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let pubkey = stub_ssh_pubkey("image-wired");
        std::env::set_var("LIUM_API_KEY", "test-key-not-a-real-secret");
        std::env::set_var("LIUM_SSH_PUBLIC_KEY_FILE", &pubkey);
        let live = JudgeConfig::http_api("http://judge.invalid/v1");

        assert!(
            build_live_judge(&live, 900).is_some(),
            "a live judge path must wire the digest-pinned harvest"
        );
        assert!(
            build_live_judge(&JudgeConfig::sim(), 900).is_none(),
            "sim scores in-process; a Lium harvest there would spend money"
        );

        // No miner key, no pod: refuse rather than invent numbers.
        std::env::remove_var("LIUM_API_KEY");
        assert!(build_live_judge(&live, 900).is_none());
        std::env::set_var("LIUM_API_KEY", "   ");
        assert!(
            build_live_judge(&live, 900).is_none(),
            "a blank key is not a key"
        );

        // A pod with no master key is unreachable, so it must not be rented.
        std::env::set_var("LIUM_API_KEY", "test-key-not-a-real-secret");
        std::env::set_var("LIUM_SSH_PUBLIC_KEY_FILE", "/nonexistent/id.pub");
        assert!(
            build_live_judge(&live, 900).is_none(),
            "no master SSH public key means no harvest"
        );
        std::env::remove_var("LIUM_API_KEY");
        std::env::remove_var("LIUM_SSH_PUBLIC_KEY_FILE");
    }

    fn stub_ssh_pubkey(tag: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("relearn-test-{tag}.pub"));
        std::fs::write(&path, "ssh-ed25519 AAAAtest relearn-image-test\n").expect("write pubkey");
        path
    }

    /// `LIUM_API_KEY` is process-wide, so the tests that set it must not race.
    static LIUM_ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());
}
