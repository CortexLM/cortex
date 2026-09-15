//! `proof-admin` — Proof operator CLI for dynamic topics.
//!
//! P0 skeleton of the dynamic-topics admin path. It wraps the topic
//! publication procedure that **already exists** in this repository:
//!
//! | Step | Existing path this CLI reuses |
//! |------|------------------------------|
//! | Sign a topic | `xtask proof-topic` (sr25519 under `base-proof-topic-v1`) |
//! | Acceptance checks | [`proof_task::TopicDocument::validate`] + `verify_signature` — the same pair `POST /v1/admin/proof/topics` runs |
//! | Publish | `POST /v1/admin/proof/topics` (operator bearer) |
//! | Persist | `proof_topic_version` (migration `0020`) via `RlmStore::put_topic_version` |
//! | Score a custom id | `PROOF_VM_RUNNER_CUSTOM_IDS` + the pack staged under `PROOF_VM_AGENT_EXPERIMENT_PACK_DIR` |
//!
//! There is deliberately **no new topic table and no new route**: a topic has
//! one home (the signed document in `proof_topic_version`) and one publish
//! path (the admin route). What this CLI adds is the *procedure* — a bundle
//! that names the signed document plus the host env that must agree with it,
//! `validate` that runs the same acceptance the route runs, and `install
//! --dry-run` that prints the exact publish call and env lines without
//! touching anything.
//!
//! What this binary does **not** do, deliberately:
//!
//! - It never writes a topic. `install` prints the publish call for an
//!   operator to run (the bearer stays on the host); a `--execute` path
//!   belongs to a later slice.
//! - It touches no route, no allocator, and no scoring path.
//! - It removes none of the compiled-in bindings the current live topic uses.
//!
//! Exit codes: `0` ok, `1` error, `2` usage or configuration.

#![forbid(unsafe_code)]
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use proof_task::ProofPin;
use proof_topic_bundle::{InstallEnvironment, TopicInstallBundle, TopicInstallPlan, PUBLISH_PATH};

mod install;
mod registry;

pub(crate) use registry::print_json;
use registry::{
    cmd_gate, cmd_list, cmd_show, dash_if_empty, database_url, open_pool, open_store, status_word,
};

use install::InstallArgs;

/// Successful run.
const EXIT_OK: u8 = 0;
/// A command failed (bad bundle, refused document, database error).
const EXIT_ERROR: u8 = 1;
/// Bad usage or missing configuration.
const EXIT_USAGE: u8 = 2;

/// Proof operator CLI.
#[derive(Debug, Parser)]
#[command(
    name = "proof-admin",
    version,
    about = "Proof operator CLI: validate and install a topic install bundle",
    long_about = "proof-admin wraps the existing Proof topic publish path (dynamic-topics P0).

Validate a bundle — runs the same acceptance checks POST /v1/admin/proof/topics runs:
  proof-admin topic validate --bundle tb4.json --pin config/proof-pin.toml

Resolve the publish call and host env without touching anything:
  proof-admin topic install --bundle tb4.json --env metal --dry-run

List the installed topics (a read-only view of proof_topic_version):
  proof-admin topic list

Every command is implemented. Nothing here writes a topic document, opens a
route, or changes how a score is computed: `install` publishes the document
the bundle carries and applies the bundle's RLM section, `disable` / `enable`
throw the operator gate the challenge reads on the submit path, and
`seal` records the operator's seal of the baseline the RLM measured and
publishes the open document."
)]
struct Cli {
    /// Postgres URL for the topic registry view. Falls back to `BASE_DATABASE_URL`.
    #[arg(long, global = true, env = "BASE_DATABASE_URL", value_name = "URL")]
    database_url: Option<String>,
    /// Read the Postgres URL from this file (mutually exclusive with the value).
    #[arg(
        long,
        global = true,
        env = "BASE_DATABASE_URL_FILE",
        value_name = "PATH"
    )]
    database_url_file: Option<PathBuf>,
    /// Print machine-readable JSON instead of a summary.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Debug, Subcommand)]
enum Cmd {
    /// Manage Proof topic installs.
    Topic {
        #[command(subcommand)]
        cmd: TopicCmd,
    },
}

#[derive(Debug, Subcommand)]
enum TopicCmd {
    /// Check a bundle: the shared acceptance checks plus the host cross-checks.
    Validate {
        /// Bundle JSON.
        #[arg(long, value_name = "PATH")]
        bundle: PathBuf,
        /// Pin the document is checked against. Defaults to `config/proof-pin.toml`.
        #[arg(long, value_name = "PATH", default_value = "config/proof-pin.toml")]
        pin: PathBuf,
    },
    /// Resolve the publish call and host env, or run the install for real.
    ///
    /// `--dry-run` prints the plan and touches nothing. Without it, the
    /// install runs: the signed document is published through the existing
    /// admin route, then the bundle's RLM section is applied (migrations
    /// under the deny-list, routes, rules, the executor binding) and the
    /// topic's RLM is asked to set itself up.
    Install {
        /// Bundle JSON.
        #[arg(long, value_name = "PATH")]
        bundle: PathBuf,
        /// Install target. Must match the bundle's own `environment`.
        #[arg(long, value_name = "staging|metal")]
        env: String,
        /// Pin the document is checked against. Defaults to `config/proof-pin.toml`.
        #[arg(long, value_name = "PATH", default_value = "config/proof-pin.toml")]
        pin: PathBuf,
        /// Resolve and print the plan without touching anything.
        #[arg(long)]
        dry_run: bool,
        /// Assert Owner authority and that staging passed first. Required for
        /// `--env metal`; refused (usage) without it. This is an operator
        /// assertion, not a verified precondition — the gate exists so a
        /// metal install cannot happen by accident or by copy-paste.
        #[arg(long)]
        owner_metal_ack: bool,
        /// Skip the RLM's baseline job. Rules, migrations, routes, and the
        /// executor binding are still applied; the topic simply has no
        /// measured baseline yet, so it cannot open until one is sealed.
        /// Intended for staging, where a baseline run is the expensive part.
        #[arg(long)]
        skip_baseline: bool,
        /// Master base URL for the admin publish call, e.g.
        /// `http://10.116.0.3:8080` (the gateway) or
        /// `http://127.0.0.1:8100` (the challenge service directly).
        #[arg(long, env = "PROOF_ADMIN_URL", value_name = "URL")]
        admin_url: Option<String>,
        /// File holding the operator bearer for `/v1/admin/*`. Never logged,
        /// never printed. Defaults to `PROOF_ADMIN_TOKENS_FILE`.
        #[arg(long, env = "PROOF_ADMIN_TOKEN_FILE", value_name = "PATH")]
        admin_token_file: Option<PathBuf>,
        /// Drive the RLM setup (provision, rules, baseline) over the
        /// topic-VM orchestrator. Without it the install applies the bundle's
        /// migrations, routes, rules, and binding, and stops there.
        #[arg(long)]
        drive_rlm: bool,
        /// Assert the Owner approved provisioning and spend. Required with
        /// `--drive-rlm`; refused (usage) without it, because the setup
        /// provisions a VM and runs a paid baseline.
        #[arg(long)]
        owner_approved: bool,
        /// Live RLM judge `InferenceOffer` JSON the baseline's paid run needs.
        #[arg(long, env = "PROOF_INFERENCE_OFFER_FILE", value_name = "PATH")]
        inference_offer_file: Option<PathBuf>,
        /// Owner inference key file the lifecycle's key probe checks.
        #[arg(long, env = "PROOF_RLM_OWNER_INFERENCE_KEY_FILE", value_name = "PATH")]
        owner_key_file: Option<PathBuf>,
    },
    /// List installed topics: a read-only view of `proof_topic_version`.
    List,
    /// Show one topic's newest install: the `proof_topic_install` journal.
    InstallLog {
        /// Topic slug.
        #[arg(long, value_name = "TOPIC_ID")]
        topic: String,
    },
    /// Show one installed topic. An alias resolves to its topic.
    Show {
        /// Topic slug, or an alias of one.
        topic_id: String,
    },
    /// Manage the temporary compatibility aliases a topic answers to.
    Alias {
        #[command(subcommand)]
        cmd: AliasCmd,
    },
    /// Stop a topic taking submissions, now.
    ///
    /// Appends a `disabled` row to `proof_topic_gate`; the challenge reads it
    /// on the next `POST /v1/submissions` and refuses with your reason. No
    /// re-sign, no restart, no redeploy — the document keeps its own `status`,
    /// in-flight evaluations finish, and rows already scored keep their
    /// verdicts. Use it when something is wrong with the topic, not to retire
    /// one: retiring is a signed `closed` document.
    Disable {
        /// Topic slug, or an alias of one.
        topic_id: String,
        /// Why, in your words. Shown to a miner in the 403, so no secrets.
        #[arg(long, value_name = "TEXT")]
        reason: Option<String>,
        /// Who is throwing the switch, for the audit trail. Never a token.
        #[arg(long, env = "PROOF_GATE_ACTOR", value_name = "LABEL")]
        actor: Option<String>,
    },
    /// Let a disabled topic take submissions again.
    ///
    /// Appends an `enabled` row — the only way back, so the history of who
    /// turned it off (and who turned it on) stays readable. The topic's own
    /// document is untouched.
    Enable {
        /// Topic slug, or an alias of one.
        topic_id: String,
        /// Why it is being re-enabled, for the audit trail.
        #[arg(long, value_name = "TEXT")]
        reason: Option<String>,
        /// Who is clearing the switch. Never a token.
        #[arg(long, env = "PROOF_GATE_ACTOR", value_name = "LABEL")]
        actor: Option<String>,
    },
    /// Show what the RLM measured, and the commitment an open document must
    /// seal. The read half of `topic seal`.
    Baseline {
        /// Topic slug, or an alias of one.
        topic_id: String,
        /// Pin the document is checked against. Defaults to `config/proof-pin.toml`.
        #[arg(long, value_name = "PATH", default_value = "config/proof-pin.toml")]
        pin: PathBuf,
    },
    /// Seal the RLM's measured baseline and open the topic.
    ///
    /// Takes the signed `status: open` document whose `baseline` carries the
    /// commitment `topic baseline` printed, checks it exactly the way the
    /// runtime does (`TopicSetup::mark_sealed`: open, valid on this host,
    /// operator-signed, and sealing the value the RLM measured), records the
    /// move to `open`, and — with `--publish` — publishes it through the
    /// admin route, which is what makes the topic reachable and scorable.
    Seal {
        /// Topic slug, or an alias of one.
        topic_id: String,
        /// The signed `status: open` document (JSON).
        #[arg(long, value_name = "PATH")]
        document: PathBuf,
        /// Pin the document is checked against. Defaults to `config/proof-pin.toml`.
        #[arg(long, value_name = "PATH", default_value = "config/proof-pin.toml")]
        pin: PathBuf,
        /// Publish the sealed document through the admin route.
        #[arg(long)]
        publish: bool,
        /// Master base URL for the publish call, e.g.
        /// `http://10.116.0.3:8080` (the gateway) or
        /// `http://127.0.0.1:8100` (the challenge service directly).
        #[arg(long, env = "PROOF_ADMIN_URL", value_name = "URL")]
        admin_url: Option<String>,
        /// File holding the operator bearer for `/v1/admin/*`. Never logged,
        /// never printed. Defaults to `PROOF_ADMIN_TOKENS_FILE`.
        #[arg(long, env = "PROOF_ADMIN_TOKEN_FILE", value_name = "PATH")]
        admin_token_file: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
enum AliasCmd {
    /// Point an alias at a topic. The topic must be published already.
    Set {
        /// The alias slug (e.g. `tbench`).
        alias: String,
        /// The canonical topic slug it resolves to (e.g. `tb4`).
        #[arg(long, value_name = "TOPIC_ID")]
        topic: String,
    },
    /// List the aliases of one topic.
    List {
        /// Canonical topic slug.
        #[arg(long, value_name = "TOPIC_ID")]
        topic: String,
    },
    /// Retire an alias. The topic itself is untouched.
    Rm {
        /// The alias slug to remove.
        alias: String,
    },
}

/// Global options, split out of [`Cli`] so the subcommand can be borrowed.
#[derive(Debug)]
struct Options {
    database_url: Option<String>,
    database_url_file: Option<PathBuf>,
    json: bool,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("proof-admin: tokio runtime: {e}");
            return ExitCode::from(EXIT_ERROR);
        }
    };
    match runtime.block_on(run(cli)) {
        Ok(()) => ExitCode::from(EXIT_OK),
        Err(Failure::Usage(msg)) => {
            eprintln!("proof-admin: {msg}");
            ExitCode::from(EXIT_USAGE)
        }
        Err(Failure::Error(msg)) => {
            eprintln!("proof-admin: {msg}");
            ExitCode::from(EXIT_ERROR)
        }
    }
}

/// How a command failed, which decides the process exit code.
#[derive(Debug)]
enum Failure {
    /// Bad usage or missing configuration.
    Usage(String),
    /// Anything else (bad bundle, refused document, database error).
    Error(String),
}

async fn run(cli: Cli) -> Result<(), Failure> {
    let opts = Options {
        database_url: cli.database_url,
        database_url_file: cli.database_url_file,
        json: cli.json,
    };
    match cli.cmd {
        Cmd::Topic { cmd } => run_topic(&opts, &cmd).await,
    }
}

async fn run_topic(opts: &Options, cmd: &TopicCmd) -> Result<(), Failure> {
    match cmd {
        TopicCmd::Validate { bundle, pin } => cmd_validate(opts, bundle, pin),
        TopicCmd::Install {
            bundle,
            env,
            pin,
            dry_run,
            owner_metal_ack,
            skip_baseline,
            admin_url,
            admin_token_file,
            drive_rlm,
            owner_approved,
            inference_offer_file,
            owner_key_file,
        } => {
            let request = InstallArgs {
                bundle,
                env,
                pin,
                gates: install::Gates {
                    dry_run: *dry_run,
                    owner_metal_ack: *owner_metal_ack,
                    owner_approved: *owner_approved,
                },
                skip_baseline: *skip_baseline,
                admin_url: admin_url.as_deref(),
                admin_token_file: admin_token_file.as_deref(),
                drive_rlm: *drive_rlm,
                inference_offer_file: inference_offer_file.as_deref(),
                owner_key_file: owner_key_file.as_deref(),
            };
            cmd_install(opts, &request).await
        }
        TopicCmd::List => cmd_list(opts).await,
        TopicCmd::InstallLog { topic } => cmd_install_log(opts, topic).await,
        TopicCmd::Show { topic_id } => cmd_show(opts, topic_id).await,
        TopicCmd::Alias { cmd } => run_alias(opts, cmd).await,
        TopicCmd::Disable {
            topic_id,
            reason,
            actor,
        } => {
            cmd_gate(
                opts,
                topic_id,
                proof_topic_install::GateState::Disabled,
                reason.as_deref(),
                actor.as_deref(),
            )
            .await
        }
        TopicCmd::Enable {
            topic_id,
            reason,
            actor,
        } => {
            cmd_gate(
                opts,
                topic_id,
                proof_topic_install::GateState::Enabled,
                reason.as_deref(),
                actor.as_deref(),
            )
            .await
        }
        TopicCmd::Seal {
            topic_id,
            document,
            pin,
            publish,
            admin_url,
            admin_token_file,
        } => {
            cmd_seal(
                opts,
                topic_id,
                document,
                pin,
                *publish,
                admin_url.as_deref(),
                admin_token_file.as_deref(),
            )
            .await
        }
        TopicCmd::Baseline { topic_id, pin } => cmd_baseline(opts, topic_id, pin).await,
    }
}

async fn run_alias(opts: &Options, cmd: &AliasCmd) -> Result<(), Failure> {
    let store = open_store(opts).await?;
    match cmd {
        AliasCmd::Set { alias, topic } => {
            store
                .put_alias(alias, topic)
                .await
                .map_err(|e| Failure::Error(e.to_string()))?;
            if opts.json {
                print_json(&serde_json::json!({
                    "ok": true,
                    "alias": alias,
                    "topic_id": topic,
                }))?;
                return Ok(());
            }
            println!("alias {alias} -> {topic}");
            println!();
            println!(
                "Temporary compatibility mapping. Retire it with \
                 `proof-admin topic alias rm {alias}` once links move to {topic}."
            );
            Ok(())
        }
        AliasCmd::List { topic } => {
            let aliases = store
                .aliases_for(topic)
                .await
                .map_err(|e| Failure::Error(format!("aliases for {topic}: {e}")))?;
            if opts.json {
                print_json(&serde_json::json!({ "topic_id": topic, "aliases": aliases }))?;
                return Ok(());
            }
            if aliases.is_empty() {
                println!("No aliases for {topic}.");
            } else {
                for alias in &aliases {
                    println!("{alias} -> {topic}");
                }
            }
            Ok(())
        }
        AliasCmd::Rm { alias } => {
            let removed = store
                .delete_alias(alias)
                .await
                .map_err(|e| Failure::Error(format!("remove alias {alias}: {e}")))?;
            if !removed {
                return Err(Failure::Error(format!("no alias {alias:?}")));
            }
            if opts.json {
                print_json(&serde_json::json!({ "ok": true, "removed": alias }))?;
                return Ok(());
            }
            println!("removed alias {alias}");
            Ok(())
        }
    }
}

/// Read a bundle file.
pub(crate) fn load_bundle(path: &Path) -> Result<TopicInstallBundle, Failure> {
    let body = std::fs::read_to_string(path)
        .map_err(|e| Failure::Error(format!("read {}: {e}", path.display())))?;
    TopicInstallBundle::from_json(&body)
        .map_err(|e| Failure::Error(format!("{}: {e}", path.display())))
}

/// Read the pin the document is checked against.
pub(crate) fn load_pin(path: &Path) -> Result<ProofPin, Failure> {
    let body = std::fs::read_to_string(path)
        .map_err(|e| Failure::Error(format!("read {}: {e}", path.display())))?;
    let pin = ProofPin::from_toml(&body).map_err(|e| Failure::Error(e.to_string()))?;
    pin.validate().map_err(|e| Failure::Error(e.to_string()))?;
    Ok(pin)
}

/// Custom ids this host registers for scoring (`PROOF_VM_RUNNER_CUSTOM_IDS`).
///
/// Read for the install's open-custom-topic gate: an open custom topic whose
/// id is not registered answers 503 when a miner submits, so an install
/// refuses rather than recording a binding that cannot score.
pub(crate) fn registered_custom_from_env() -> Vec<String> {
    std::env::var(proof_topic_bundle::ENV_CUSTOM_IDS)
        .ok()
        .map_or_else(Vec::new, |raw| proof_topic_bundle::parse_custom_ids(&raw))
}

/// The shared acceptance checks, in the order the admin route runs them.
///
/// This is the point of the CLI: an operator finds out here — not on the host
/// that matters — that a document would be refused, and why. It runs exactly
/// the two checks `POST /v1/admin/proof/topics` runs, against the same pin.
pub(crate) fn accept_document(bundle: &TopicInstallBundle, pin: &ProofPin) -> Result<(), Failure> {
    bundle
        .validate_shape()
        .map_err(|e| Failure::Error(e.to_string()))?;
    let registered = bundle.registered_custom();
    let registered: Vec<&str> = registered.iter().map(String::as_str).collect();
    bundle
        .topic
        .validate(pin, &registered)
        .map_err(|e| Failure::Error(format!("topic document: {e}")))?;
    bundle
        .topic
        .verify_signature(pin)
        .map_err(|e| Failure::Error(format!("topic signature: {e}")))?;
    Ok(())
}

/// Parse `--env` into an install target.
pub(crate) fn parse_env(raw: &str) -> Result<InstallEnvironment, Failure> {
    raw.parse::<InstallEnvironment>().map_err(Failure::Usage)
}

fn cmd_validate(opts: &Options, path: &Path, pin_path: &Path) -> Result<(), Failure> {
    let bundle = load_bundle(path)?;
    let pin = load_pin(pin_path)?;
    accept_document(&bundle, &pin)?;
    let digest = bundle.digest().map_err(|e| Failure::Error(e.to_string()))?;
    let binding = bundle
        .binding()
        .map_err(|e| Failure::Error(e.to_string()))?;
    if opts.json {
        let body = serde_json::json!({
            "ok": true,
            "bundle": path.display().to_string(),
            "topic_id": bundle.topic.id,
            "environment": bundle.environment.as_str(),
            "document_status": bundle.topic.status,
            "metric_family": bundle.topic.metric.family,
            "custom_id": bundle.topic.metric.custom_id,
            "runner_id": binding.as_ref().map(|b| b.runner.clone()),
            "bundle_digest": digest,
            "rlm_install": !bundle.rlm.is_empty(),
        });
        print_json(&body)?;
        return Ok(());
    }
    println!("bundle {} is valid", path.display());
    println!("  topic_id         {}", bundle.topic.id);
    println!("  environment      {}", bundle.environment);
    println!("  document_status  {}", status_word(bundle.topic.status));
    println!("  metric_family    {}", bundle.topic.metric.family.as_str());
    println!(
        "  custom_id        {}",
        dash_if_empty(&bundle.topic.metric.custom_id)
    );
    println!(
        "  runner_id        {}",
        binding
            .as_ref()
            .map_or_else(|| "-".to_owned(), |b| b.runner.clone())
    );
    println!("  bundle_digest    {digest}");
    println!(
        "  rlm_install      {}",
        if bundle.rlm.is_empty() {
            "-".to_owned()
        } else {
            "present (handed to the RLM verbatim)".to_owned()
        }
    );
    println!();
    println!("Checked against {}.", pin_path.display());
    println!(
        "Nothing was written. Resolve the publish call with `proof-admin topic install --dry-run`."
    );
    Ok(())
}

/// The real install (and the dry run) live in [`install`].
async fn cmd_install(opts: &Options, args: &InstallArgs<'_>) -> Result<(), Failure> {
    install::run(opts, args).await
}

/// The install journal for one topic.
async fn cmd_install_log(opts: &Options, topic_id: &str) -> Result<(), Failure> {
    install::install_log(opts, topic_id).await
}

/// `topic baseline`: read the measurement and print what to seal.
///
/// The procedure is [`proof_topic_ops::baseline`]; this is the printing.
async fn cmd_baseline(opts: &Options, topic_id: &str, pin_path: &Path) -> Result<(), Failure> {
    let pool = open_pool(opts).await?;
    let pin = load_pin(pin_path)?;
    let report = proof_topic_ops::baseline(&pool, &pin, topic_id)
        .await
        .map_err(ops_to_failure)?;
    if opts.json {
        return print_json(&serde_json::json!({
            "topic_id": report.topic_id,
            "rules_version": report.rules_version,
            "primary_value": report.primary_value,
            "metric_primary": report.metric_primary,
            "custom_id": report.custom_id,
            "holdout_commitment": report.holdout_commitment,
            "metrics_commitment": report.metrics_commitment,
            "document_status": report.document_status,
            "next": report.next_steps(),
        }));
    }
    println!("topic {} — measured baseline", report.topic_id);
    println!("  primary_value     {}", report.primary_value);
    println!("  metric_primary    {}", report.metric_primary);
    println!("  custom_id         {}", dash_if_empty(&report.custom_id));
    println!("  rules_version     {}", report.rules_version);
    println!("  holdout           {}", report.holdout_commitment);
    println!(
        "  document_status   {}",
        status_word(report.document_status)
    );
    println!();
    println!("An `open` document must seal this measurement. Its baseline block needs:");
    println!("  metrics_commitment  {}", report.metrics_commitment);
    println!();
    println!("{}", report.next_steps());
    Ok(())
}

/// `topic seal`: record the seal, then optionally publish.
///
/// The procedure is [`proof_topic_ops::seal`]; this is the printing.
async fn cmd_seal(
    opts: &Options,
    topic_id: &str,
    document: &Path,
    pin_path: &Path,
    publish: bool,
    admin_url: Option<&str>,
    admin_token_file: Option<&Path>,
) -> Result<(), Failure> {
    let pool = open_pool(opts).await?;
    let pin = load_pin(pin_path)?;
    let outcome = proof_topic_ops::seal(
        &pool,
        &proof_topic_ops::SealArgs {
            topic_id,
            document,
            pin: &pin,
            publish,
            admin_url,
            admin_token_file,
            registered_custom: registered_custom_from_env(),
        },
    )
    .await
    .map_err(ops_to_failure)?;
    if opts.json {
        return print_json(&serde_json::json!({
            "ok": true,
            "topic_id": outcome.topic_id,
            "state": "open",
            "document_version": outcome.document_version,
            "metrics_commitment": outcome.metrics_commitment,
            "primary_value": outcome.primary_value,
            "published": outcome.published,
            "already_sealed": outcome.already_sealed,
        }));
    }
    if outcome.already_sealed {
        println!(
            "topic {} was already sealed (document version {}); this run only published.",
            outcome.topic_id, outcome.document_version
        );
    } else {
        println!("topic {} sealed and opened.", outcome.topic_id);
        println!("  state             open");
        println!("  document_version  {}", outcome.document_version);
        println!("  primary_value     {}", outcome.primary_value);
        println!("  commitment        {}", outcome.metrics_commitment);
    }
    if outcome.published {
        println!("  published         yes (the topic's routes and document are live)");
    } else {
        println!("  published         no (--publish was not given)");
    }
    println!();
    println!("{}", outcome.after());
    Ok(())
}

/// An operator procedure's refusal, as the CLI's exit code and message.
pub(crate) fn ops_to_failure(e: proof_topic_ops::OpsError) -> Failure {
    match e {
        proof_topic_ops::OpsError::Usage(m) => Failure::Usage(m),
        proof_topic_ops::OpsError::Error(m) => Failure::Error(m),
    }
}

pub(crate) fn print_plan(plan: &TopicInstallPlan, bundle_path: &Path, pin_path: &Path) {
    println!("topic install plan");
    println!("  topic_id          {}", plan.topic_id);
    println!("  display_name      {}", plan.display_name);
    println!("  environment       {}", plan.environment);
    println!("  document_status   {}", status_word(plan.document_status));
    println!("  metric_family     {}", plan.metric_family.as_str());
    println!("  custom_id         {}", dash_if_empty(&plan.custom_id));
    println!(
        "  runner_id         {}",
        plan.runner_id.as_deref().unwrap_or("-")
    );
    println!(
        "  pack_digest       {}",
        plan.pack_digest.as_deref().unwrap_or("-")
    );
    println!("  bundle_digest     {}", plan.bundle_digest);
    println!("  pin               {}", pin_path.display());
    println!(
        "  aliases           {}",
        if plan.aliases.is_empty() {
            "-".to_owned()
        } else {
            plan.aliases.join(", ")
        }
    );
    if plan.environment == InstallEnvironment::Metal {
        println!("  owner_gate        acknowledged (Owner-only metal install)");
    } else {
        println!("  owner_gate        n/a (staging)");
    }
    println!();
    println!("1) Hand control to the topic's RLM (it installs and sets the topic up):");
    println!(
        "     # The RLM drives, in order: {}",
        plan.rlm_jobs.join(" -> ")
    );
    if plan.rlm_install.is_some() {
        println!("     # This bundle carries an RLM install section. A real install applies it");
        println!("     # under two closed gates (a SQL deny-list and a handler allow-list) and");
        println!("     # records what it did in `proof_topic_install`. The CLI itself does not");
        println!("     # read into the section: Rust never branches on a topic's rules, APIs,");
        println!("     # submit format, or scoring.");
    } else {
        println!("     # No RLM install section in this bundle: the RLM uses its defaults.");
    }
    println!();
    println!("2) Publish the signed document (one block; existing route, operator bearer):");
    println!("     # The route takes a TopicDocument, not the bundle envelope, so this");
    println!("     # extracts .topic into a private mktemp -d directory first.");
    for line in publish_block(bundle_path).lines() {
        println!("     {line}");
    }
    println!();
    if plan.host_env.is_empty() {
        println!("3) Host env: nothing extra is required for this topic.");
    } else {
        println!("3) Set these on the master before the topic can score:");
        for var in &plan.host_env {
            println!("     {}={}", var.name, var.value);
            println!("       # {}", var.why);
        }
    }
}

/// The runnable publish step, as one shell block.
///
/// The publish route takes a `TopicDocument`, **not** the bundle envelope, so
/// the procedure has to extract `topic` first.
///
/// Two things make this safe to paste:
///
/// - The extracted document goes into a **private directory created by
///   `mktemp -d`** (`mktemp` makes it 0700), and the file itself is `0600`. A
///   fixed shared path like `/tmp/document.json` would let any local process
///   replace the file between the checks and the publication, so the document
///   that gets published would not be the one that was validated.
/// - Extraction and publication are **one block**, so the path variable and
///   the file it names cannot drift apart or be swapped in between. An
///   operator pastes the whole thing once.
///
/// The document itself was already accepted by `validate` before this is
/// printed, so the block does not re-check it; re-running `proof-admin topic
/// validate` on the extracted file is a reasonable extra step for an operator
/// who wants it.
fn publish_block(bundle_path: &Path) -> String {
    let bundle = shell_single_quote(&bundle_path.display().to_string());
    format!(
        "PROOF_TOPIC_DIR=$(mktemp -d) \\\n  \
         && jq '.topic' {bundle} > \"$PROOF_TOPIC_DIR/document.json\" \\\n  \
         && chmod 600 \"$PROOF_TOPIC_DIR/document.json\" \\\n  \
         && curl -sS -X POST \\\n       \
         -H \"Authorization: Bearer $PROOF_ADMIN_TOKEN\" \\\n       \
         -H 'content-type: application/json' \\\n       \
         --data-binary @\"$PROOF_TOPIC_DIR/document.json\" \\\n       \
         <host>{PUBLISH_PATH} \\\n  \
         && rm -rf \"$PROOF_TOPIC_DIR\""
    )
}

/// Single-quote a path for `sh`, escaping any embedded quote.
///
/// A path with a space or a quote must not turn the printed procedure into a
/// different command than the operator read.
fn shell_single_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}
