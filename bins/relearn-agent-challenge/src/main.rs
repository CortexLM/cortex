//! `relearn-agent-challenge` — master-only Relearn Agent service (port 8099).
//!
//! Miner HTTP submit → digest freeze → episode unseal → replayed tool traces →
//! operator-audited promote. Miners pay Lium.
//!
//! Episodes are loaded from an operator file and verified against the
//! commitment in `config/relearn-agent-pin.toml`. If that file is absent or
//! does not match, the service still serves `/health` and `/v1/status` but
//! refuses submissions: scoring a reconstructable set or the published split
//! would silently turn the anti-overfit gates off.
//!
//! The same rule applies to the scorer. Without `RELEARN_AGENT_FORCE_SIM=1`
//! the host needs a `sha256:` eval-image pin, a wired harvest, and a champion
//! baseline measured by that image. Sim is never a fallback.

#![forbid(unsafe_code)]

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use challenge_keys::load_challenge_secret;
use clap::Parser;
use prism_lium::LiumClient;
use relearn_agent_challenge::{
    boot_base_champion, hash_admin_token, parse_episode_file, relearn_agent_router, AppState,
    BaselineMeasurement, EvalBackend, LiveScorer, MemoryStore, RelearnAgentPin, BASE_CHAMPION_RUN,
    CHALLENGE_ID, SCORING_VERSION,
};
use relearn_agent_harvest::{HarvestLimits, LiumAgentHarvest, TeacherEnv};
use tokio::net::TcpListener;

/// Operator Relearn Agent challenge service CLI.
#[derive(Debug, Parser)]
#[command(
    name = "relearn-agent-challenge",
    about = "Relearn Agent challenge service (port 8099, master→Lium/sim)"
)]
struct Cli {
    /// Bind address (default 0.0.0.0:8099).
    #[arg(long, env = "BASE_CHALLENGE_BIND", default_value = "0.0.0.0:8099")]
    bind: SocketAddr,
    /// Challenge mini-secret file (leaf signatures).
    #[arg(long, env = "BASE_CHALLENGE_SK_FILE")]
    challenge_sk_file: Option<PathBuf>,
    /// Force sim eval (no Lium spend). CI / local only — never a live scorer.
    ///
    /// `RELEARN_AGENT_FORCE_SIM` is read by `resolve_eval_backend`, which
    /// accepts `1` / `true` / `yes`, so it is deliberately not bound here:
    /// clap would reject the documented spelling and refuse to boot.
    #[arg(long, default_value_t = false)]
    force_sim: bool,
    /// Operator bearer tokens file (one per line). Empty → admin 503.
    #[arg(long, env = "RELEARN_AGENT_ADMIN_TOKENS_FILE")]
    admin_tokens_file: Option<PathBuf>,
    /// Pin file (`config/relearn-agent-pin.toml`).
    #[arg(long, env = "RELEARN_AGENT_PIN_FILE")]
    pin_file: Option<PathBuf>,
    /// Operator episode records (JSON array). Never in git.
    #[arg(long, env = "RELEARN_AGENT_HOLDOUT_FILE")]
    holdout_file: Option<PathBuf>,
    /// Seconds the eval image gets to replay one artifact on the pod.
    #[arg(long, env = "RELEARN_AGENT_EVAL_TIMEOUT_SECS", default_value_t = 5400)]
    eval_timeout_secs: u64,
    /// Champion baseline measured by the pinned eval image on the base model.
    ///
    /// Required on a live host: every gate compares against this. Verified
    /// against the pin's `eval_image_digest` + `holdout_commitment` at boot.
    #[arg(long, env = "RELEARN_AGENT_BASE_CHAMPION_FILE")]
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
        tracing::info!("RELEARN_AGENT_FORCE_SIM=1 — deterministic offline eval, not a real eval");
        EvalBackend::Sim
    } else {
        relearn_agent_challenge::resolve_eval_backend()
    };
    if backend == EvalBackend::Lium && !pin.can_rent() {
        tracing::warn!(
            "eval_image_digest not pinned; submissions will 503 until CortexLM/relearn CI \
             publishes a sha256 agent eval image (sim is opt-in via RELEARN_AGENT_FORCE_SIM, \
             never a fallback)"
        );
    }

    let store = MemoryStore::new();
    store
        .set_holdout_commitment(&pin.holdout_commitment, pin.holdout_size)
        .map_err(|e| e.to_string())?;
    match load_episodes(&store, &pin, cli.holdout_file.as_deref()) {
        Ok(n) => tracing::info!(episodes = n, "episodes verified against pin commitment"),
        Err(e) => tracing::warn!("episodes unavailable ({e}); submissions will 503 until fixed"),
    }

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;

    let live_scorer = build_live_scorer(backend, cli.eval_timeout_secs);
    match backend {
        EvalBackend::Lium if live_scorer.is_some() => {
            tracing::info!("live harvest wired: digest-pinned agent eval image on Lium");
        }
        EvalBackend::Lium => tracing::warn!(
            "live harvest not wired; every submission will 503. Set the Lium credentials \
             and LIUM_SSH_PUBLIC_KEY_FILE (deploy/env/relearn-agent-challenge.env.example)"
        ),
        EvalBackend::Sim => {}
    }

    // Record the baseline with the scorer this host will actually use. Every
    // gate — trace replay, tool ablation, observation shuffle, the canary — is
    // a comparison against the champion, so a live host that skips this 503s
    // every submission rather than crowning against numbers nobody measured.
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
            episodes = n,
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
        admin_hashes: Arc::new(load_admin_hashes(cli.admin_tokens_file.as_deref())),
    };
    rt.block_on(serve(cli.bind, state))
}

/// Wire the live harvest on the Lium path.
///
/// Sim hosts never get one: sim is selected by `RELEARN_AGENT_FORCE_SIM` and
/// scores in-process. On the Lium path the miner's `LIUM_API_KEY` pays for the
/// pod, so no key means no harvest and the host refuses.
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
    let pod = Arc::new(harvest_pod::LiumEvalPod::new(
        client,
        run_timeout_secs,
        relearn_agent_harvest::PROGRAM,
    ));
    Some(Arc::new(LiumAgentHarvest::new(
        pod,
        HarvestLimits::default(),
        vec![ssh_pub],
        teacher,
    )))
}

/// Master SSH public key for harvest pods.
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

fn load_pin(path: Option<&Path>) -> Result<RelearnAgentPin, String> {
    let Some(p) = path else {
        return Ok(RelearnAgentPin::default());
    };
    let body = std::fs::read_to_string(p).map_err(|e| format!("read {}: {e}", p.display()))?;
    let pin = RelearnAgentPin::from_toml(&body).map_err(|e| e.to_string())?;
    pin.validate().map_err(|e| e.to_string())?;
    Ok(pin)
}

fn load_episodes(
    store: &MemoryStore,
    pin: &RelearnAgentPin,
    path: Option<&Path>,
) -> Result<usize, String> {
    let p = path.ok_or("RELEARN_AGENT_HOLDOUT_FILE not set")?;
    let body = std::fs::read_to_string(p).map_err(|e| format!("read {}: {e}", p.display()))?;
    let records = parse_episode_file(&body)?;
    let n = records.len();
    store
        .load_episodes(records, &[], &pin.public_ids)
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
    pin: &RelearnAgentPin,
    backend: EvalBackend,
    recorded: Option<BaselineMeasurement>,
    live: Option<&dyn LiveScorer>,
) -> Result<usize, String> {
    let episodes = store
        .unseal_episodes(BASE_CHAMPION_RUN)
        .map_err(|e| e.to_string())?;
    let scores = boot_base_champion(pin, &episodes, backend, recorded, live)
        .await
        .map_err(|e| e.to_string())?;
    store.set_base_champion(scores).map_err(|e| e.to_string())?;
    Ok(episodes.len())
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
    let app = relearn_agent_router(state);
    let listener = TcpListener::bind(bind)
        .await
        .map_err(|e| format!("bind {bind}: {e}"))?;
    tracing::info!(
        %bind,
        challenge_id = CHALLENGE_ID,
        scoring_version = SCORING_VERSION,
        base_model = relearn_agent_challenge::BASE_MODEL_ID,
        "relearn-agent-challenge listening"
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
    use relearn_agent_challenge::{episode_commitment, AgentEpisode};

    use super::*;

    #[test]
    fn documented_force_sim_values_do_not_break_argument_parsing() {
        for value in ["1", "true", "yes", "false", ""] {
            std::env::set_var("RELEARN_AGENT_FORCE_SIM", value);
            let cli = Cli::try_parse_from(["relearn-agent-challenge"])
                .unwrap_or_else(|e| panic!("RELEARN_AGENT_FORCE_SIM={value:?} broke parsing: {e}"));
            assert!(!cli.force_sim, "env must not set the flag");
        }
        std::env::set_var("RELEARN_AGENT_FORCE_SIM", "1");
        assert_eq!(
            relearn_agent_challenge::resolve_eval_backend(),
            EvalBackend::Sim
        );
        std::env::set_var("RELEARN_AGENT_FORCE_SIM", "false");
        assert_eq!(
            relearn_agent_challenge::resolve_eval_backend(),
            EvalBackend::Lium
        );
        std::env::remove_var("RELEARN_AGENT_FORCE_SIM");
    }

    fn episodes() -> Vec<AgentEpisode> {
        (1..=120)
            .map(|i| {
                AgentEpisode::synthetic(
                    800 + i,
                    format!("episode {i} asks for a figure buried in the ledger"),
                )
            })
            .collect()
    }

    fn pin(digest: &str) -> RelearnAgentPin {
        let eps = episodes();
        RelearnAgentPin {
            holdout_commitment: episode_commitment(&eps),
            holdout_size: eps.len(),
            eval_image_digest: digest.to_owned(),
            ..RelearnAgentPin::default()
        }
    }

    fn seeded(p: &RelearnAgentPin) -> MemoryStore {
        let store = MemoryStore::new();
        store
            .set_holdout_commitment(&p.holdout_commitment, p.holdout_size)
            .expect("commit");
        store
            .load_episodes(episodes(), &[], &p.public_ids)
            .expect("load");
        store
    }

    /// The boot path, not just the library: a live host with no baseline
    /// source records nothing rather than inheriting sim numbers.
    #[tokio::test]
    async fn live_boot_without_a_baseline_source_records_nothing() {
        let p = pin(&format!("sha256:{}", "ab".repeat(32)));
        let store = seeded(&p);
        assert!(
            record_base_champion(&store, &p, EvalBackend::Lium, None, None)
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
        let p = pin("");
        let store = seeded(&p);
        record_base_champion(&store, &p, EvalBackend::Sim, None, None)
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
    fn live_harvest_is_wired_on_the_lium_path_not_on_sim() {
        let _guard = LIUM_ENV
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let pubkey = stub_ssh_pubkey("agent-wired");
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
        assert!(
            build_live_scorer(EvalBackend::Lium, 900).is_none(),
            "a blank key is not a key"
        );

        std::env::set_var("LIUM_API_KEY", "test-key-not-a-real-secret");
        std::env::set_var("LIUM_SSH_PUBLIC_KEY_FILE", "/nonexistent/id.pub");
        assert!(
            build_live_scorer(EvalBackend::Lium, 900).is_none(),
            "no master SSH public key means no harvest"
        );
        std::env::remove_var("LIUM_API_KEY");
        std::env::remove_var("LIUM_SSH_PUBLIC_KEY_FILE");
    }

    #[test]
    fn a_wired_harvest_without_a_teacher_url_is_not_ready() {
        let _guard = LIUM_ENV
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let pubkey = stub_ssh_pubkey("agent-ready");
        std::env::set_var("LIUM_API_KEY", "test-key-not-a-real-secret");
        std::env::set_var("LIUM_SSH_PUBLIC_KEY_FILE", &pubkey);
        std::env::remove_var("RELEARN_TEACHER_API_URL");
        let judgeless = build_live_scorer(EvalBackend::Lium, 900).expect("wired");
        let pinned = RelearnAgentPin {
            eval_image_digest: format!("sha256:{}", "ab".repeat(32)),
            ..RelearnAgentPin::default()
        };
        let err = relearn_agent_challenge::scoring_readiness(
            &pinned,
            EvalBackend::Lium,
            Some(judgeless.as_ref()),
            true,
        )
        .expect_err("no judge configured");
        assert!(err.to_string().contains("RELEARN_TEACHER_API_URL"), "{err}");

        std::env::set_var("RELEARN_TEACHER_API_URL", "http://teacher.invalid/v1");
        let live = build_live_scorer(EvalBackend::Lium, 900).expect("wired");
        relearn_agent_challenge::scoring_readiness(
            &pinned,
            EvalBackend::Lium,
            Some(live.as_ref()),
            true,
        )
        .expect("pinned digest + teacher URL can score");
        std::env::remove_var("LIUM_API_KEY");
        std::env::remove_var("LIUM_SSH_PUBLIC_KEY_FILE");
        std::env::remove_var("RELEARN_TEACHER_API_URL");
    }

    fn stub_ssh_pubkey(tag: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("relearn-test-{tag}.pub"));
        std::fs::write(&path, "ssh-ed25519 AAAAtest relearn-agent-test\n").expect("write pubkey");
        path
    }

    /// `LIUM_API_KEY` is process-wide, so the tests that set it must not race.
    static LIUM_ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());
}
