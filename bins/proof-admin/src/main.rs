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
//! Exit codes: `0` ok, `1` error, `2` usage or configuration, `3` not
//! implemented in this slice.

#![forbid(unsafe_code)]
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use proof_rlm_store::{MemoryRlmStore, PgRlmStore, RlmStore, TopicVersionRow};
use proof_task::ProofPin;
use proof_topic_bundle::{InstallEnvironment, TopicInstallBundle, TopicInstallPlan, PUBLISH_PATH};

mod drive;
mod install;

use install::InstallArgs;

/// Successful run.
const EXIT_OK: u8 = 0;
/// A command failed (bad bundle, refused document, database error).
const EXIT_ERROR: u8 = 1;
/// Bad usage or missing configuration.
const EXIT_USAGE: u8 = 2;
/// The command exists but its behaviour belongs to a later slice.
const EXIT_NOT_IMPLEMENTED: u8 = 3;

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

Nothing here writes a topic, opens a route, or changes how a score is
computed. `install` prints the publish call and the host env for an operator
to run; `disable` / `enable` throw the operator gate the challenge reads on
the submit path; `topic seal` exits 3 as not-implemented."
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
    /// Not implemented in this slice.
    Seal {
        /// Topic slug.
        topic_id: String,
        /// Measured baseline primary.
        #[arg(long, value_name = "VALUE")]
        value: f64,
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
        Err(Failure::NotImplemented(msg)) => {
            eprintln!("proof-admin: {msg}");
            ExitCode::from(EXIT_NOT_IMPLEMENTED)
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
    /// A later slice owns this behaviour.
    NotImplemented(String),
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
        TopicCmd::Seal { topic_id, value } => Err(not_implemented(
            &format!("topic seal (value {value})"),
            topic_id,
        )),
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

/// A stub that names what is missing instead of guessing.
fn not_implemented(command: &str, topic_id: &str) -> Failure {
    Failure::NotImplemented(format!(
        "`{command}` for topic {topic_id:?} is not implemented in this slice (P0: bundle + \
         admin CLI skeleton). Nothing was changed. A topic's lifecycle is the signed document's \
         `status`; re-sign and re-publish through POST /v1/admin/proof/topics instead."
    ))
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

async fn cmd_list(opts: &Options) -> Result<(), Failure> {
    let store = open_store(opts).await?;
    let rows = store
        .latest_topics()
        .await
        .map_err(|e| Failure::Error(format!("list topics: {e}")))?;
    if opts.json {
        let body: Vec<serde_json::Value> = rows.iter().map(topic_json).collect();
        print_json(&body)?;
        return Ok(());
    }
    if rows.is_empty() {
        println!("No topics installed.");
        return Ok(());
    }
    println!("{} topic(s) installed:", rows.len());
    for row in &rows {
        println!("  {}", summarize(row));
    }
    println!();
    println!("Read from proof_topic_version; the signed document is the source of truth.");
    Ok(())
}

async fn cmd_show(opts: &Options, topic_id: &str) -> Result<(), Failure> {
    let pool = open_pool(opts).await?;
    let store = PgRlmStore::new(pool.clone());
    // An alias resolves to its canonical slug first, so `show tbench` finds
    // `tb4`. Resolution is fail-closed in the store: an alias whose topic has
    // no published version resolves to nothing rather than to an empty row.
    let resolved = store
        .resolve_alias(topic_id)
        .await
        .map_err(|e| Failure::Error(format!("resolve {topic_id}: {e}")))?;
    let canonical = resolved.as_deref().unwrap_or(topic_id);
    let row = store
        .latest_topic(canonical)
        .await
        .map_err(|e| Failure::Error(format!("show {canonical}: {e}")))?;
    let Some((version, document)) = row else {
        return Err(Failure::Error(format!(
            "no installed topic {topic_id:?}{}. Use `proof-admin topic list` to see the exact ids.",
            resolved
                .as_deref()
                .map(|c| format!(" (alias of {c:?})"))
                .unwrap_or_default()
        )));
    };
    let row = TopicVersionRow {
        topic_id: canonical.to_owned(),
        version,
        document,
    };
    // The operator gate, read from the same table the challenge reads: `show`
    // must not report a topic as open for work when the submit path refuses
    // it. An unreadable gate is reported rather than assumed enabled.
    let gate = proof_topic_install::gate(&pool, canonical)
        .await
        .map_err(|e| Failure::Error(format!("{canonical} gate: {e}")))?;
    if let Some(canonical) = resolved.as_deref() {
        if !opts.json {
            println!("{topic_id} is an alias of {canonical}");
            println!();
        }
    }
    if opts.json {
        let mut body = topic_json(&row);
        if let Some(obj) = body.as_object_mut() {
            obj.insert(
                "disabled".to_owned(),
                serde_json::Value::Bool(
                    gate.as_ref()
                        .is_some_and(proof_topic_install::Gate::is_disabled),
                ),
            );
            if let Some(gate) = gate.as_ref().filter(|g| g.is_disabled()) {
                obj.insert(
                    "disabled_reason".to_owned(),
                    serde_json::Value::String(gate.reason.clone()),
                );
            }
        }
        print_json(&body)?;
        return Ok(());
    }
    print_row(&row);
    print_gate(gate.as_ref(), canonical);
    Ok(())
}

/// The operator gate line(s) for `topic show`.
fn print_gate(gate: Option<&proof_topic_install::Gate>, topic_id: &str) {
    println!();
    match gate {
        None => println!("Operator gate: enabled (no `proof_topic_gate` row)."),
        Some(gate) if !gate.is_disabled() => {
            println!(
                "Operator gate: enabled (gate row {}; the newest row is an enable).",
                gate.id
            );
        }
        Some(gate) => {
            println!("Operator gate: DISABLED (gate row {}).", gate.id);
            if gate.reason.is_empty() {
                println!("  reason            (none given)");
            } else {
                println!("  reason            {}", gate.reason);
            }
            if !gate.actor.is_empty() {
                println!("  actor             {}", gate.actor);
            }
            println!();
            println!("Submissions to this topic are refused. Re-enable with:");
            println!("  proof-admin topic enable {topic_id}");
        }
    }
}

/// The topic registry: the existing `proof_topic_version` rows.
///
/// A configured but unreachable database is fatal: falling back to an empty
/// in-memory view would report "nothing installed" for a host that has topics.
async fn open_store(opts: &Options) -> Result<Box<dyn RlmStore>, Failure> {
    let pool = open_pool(opts).await?;
    // `PgRlmStore` is the production registry; the memory store exists for
    // CI/local and is never selected here, so a real host never reads an
    // empty view by accident.
    let _ = MemoryRlmStore::new;
    Ok(Box::new(PgRlmStore::new(pool)))
}

/// A connection pool over the topic database.
///
/// The gate commands write (`proof_topic_gate`) as well as read, so they need
/// the pool itself and not only the registry trait object.
async fn open_pool(opts: &Options) -> Result<sqlx::PgPool, Failure> {
    let Some(url) = database_url(opts)? else {
        return Err(Failure::Usage(
            "this command reads the topic registry and needs a database: set \
             BASE_DATABASE_URL (or BASE_DATABASE_URL_FILE). `topic validate` and \
             `topic install --dry-run` need no database."
                .into(),
        ));
    };
    db::connect(&url)
        .await
        .map_err(|e| Failure::Error(format!("connect: {e}")))
}

/// `topic disable` / `topic enable`: throw the operator gate.
///
/// The topic must be published (or be an alias of one): a typo must not
/// silently disable nothing, because the operator would then believe a topic
/// is stopped while it is still taking submissions. The write is append-only
/// — the newest row is the state, the rows before it are the history — and it
/// is visible to the challenge on the next request, which is the point of the
/// switch.
async fn cmd_gate(
    opts: &Options,
    topic_id: &str,
    state: proof_topic_install::GateState,
    reason: Option<&str>,
    actor: Option<&str>,
) -> Result<(), Failure> {
    let pool = open_pool(opts).await?;
    let store = PgRlmStore::new(pool.clone());
    let resolved = store
        .resolve_alias(topic_id)
        .await
        .map_err(|e| Failure::Error(format!("resolve {topic_id}: {e}")))?;
    let canonical = resolved.as_deref().unwrap_or(topic_id);
    let row = store
        .latest_topic(canonical)
        .await
        .map_err(|e| Failure::Error(format!("{canonical}: {e}")))?;
    if row.is_none() {
        return Err(Failure::Error(format!(
            "no installed topic {topic_id:?}{}. Nothing was changed — check the id with \
             `proof-admin topic list`.",
            resolved
                .as_deref()
                .map(|c| format!(" (alias of {c:?})"))
                .unwrap_or_default()
        )));
    }
    let reason = reason.unwrap_or_default();
    let actor = actor.unwrap_or_default();
    let gate = proof_topic_install::set(&pool, canonical, state, reason, actor)
        .await
        .map_err(|e| Failure::Error(format!("{canonical}: {e}")))?;
    let disabled = gate.is_disabled();
    if opts.json {
        print_json(&serde_json::json!({
            "ok": true,
            "topic_id": canonical,
            "state": gate.state.as_str(),
            "disabled": disabled,
            "reason": gate.reason,
            "actor": gate.actor,
            "gate_row": gate.id,
        }))?;
        return Ok(());
    }
    if disabled {
        println!("topic {canonical} is disabled (gate row {}).", gate.id);
        if gate.reason.is_empty() {
            println!("  reason            (none given)");
        } else {
            println!("  reason            {}", gate.reason);
        }
        println!();
        println!(
            "Submissions are refused from the next request on, with this reason. The document \
             keeps its own status, in-flight evaluations finish, and rows already scored keep \
             their verdicts. Nothing was re-signed and nothing was restarted."
        );
        println!();
        println!("To let it take submissions again:");
        println!("  proof-admin topic enable {canonical}");
    } else {
        println!("topic {canonical} is enabled again (gate row {}).", gate.id);
        println!();
        println!(
            "Submissions are admitted from the next request on, under the topic's own document \
             (`status`), which was never changed. The disable rows stay in the history."
        );
    }
    Ok(())
}

/// `BASE_DATABASE_URL` value, or the contents of `BASE_DATABASE_URL_FILE`.
///
/// The two are mutually exclusive, matching `crates/config`: a value and a
/// file that disagree would be a silent choice between two databases.
pub(crate) fn database_url(opts: &Options) -> Result<Option<String>, Failure> {
    let value = opts
        .database_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let file = opts.database_url_file.as_deref();
    match (value, file) {
        (Some(_), Some(_)) => Err(Failure::Usage(
            "set BASE_DATABASE_URL or BASE_DATABASE_URL_FILE, not both".into(),
        )),
        (Some(url), None) => Ok(Some(url.to_owned())),
        (None, Some(path)) => {
            let raw = std::fs::read_to_string(path)
                .map_err(|e| Failure::Error(format!("read {}: {e}", path.display())))?;
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return Err(Failure::Usage(format!("{} is empty", path.display())));
            }
            Ok(Some(trimmed.to_owned()))
        }
        (None, None) => Ok(None),
    }
}

fn print_row(row: &TopicVersionRow) {
    let doc = &row.document;
    println!("topic {}", row.topic_id);
    println!("  version           {}", row.version);
    println!("  status            {}", status_word(doc.status));
    println!("  metric_family     {}", doc.metric.family.as_str());
    println!(
        "  custom_id         {}",
        dash_if_empty(&doc.metric.custom_id)
    );
    println!("  payout_mode       {}", doc.payout_mode.as_str());
    println!("  valid_from_epoch  {}", doc.valid_from_epoch);
    println!(
        "  valid_until_epoch {}",
        doc.valid_until_epoch
            .map_or_else(|| "-".to_owned(), |e| e.to_string())
    );
    println!("  baseline_sealed   {}", doc.baseline.is_sealed());
    println!(
        "  signature         {}…",
        doc.signature.get(..16).unwrap_or(doc.signature.as_str())
    );
    println!();
    println!("The signed document is the source of truth; this view reads it verbatim.");
}

/// One-line summary for `topic list`.
fn summarize(row: &TopicVersionRow) -> String {
    let doc = &row.document;
    format!(
        "{:<24} v{:<3} {:<10} {:<10} custom_id={}",
        row.topic_id,
        row.version,
        status_word(doc.status),
        doc.metric.family.as_str(),
        dash_if_empty(&doc.metric.custom_id)
    )
}

/// Lifecycle word, matching the wire spelling the document uses.
fn status_word(status: proof_task::TopicStatus) -> &'static str {
    match status {
        proof_task::TopicStatus::Draft => "draft",
        proof_task::TopicStatus::Open => "open",
        proof_task::TopicStatus::Closed => "closed",
    }
}

fn topic_json(row: &TopicVersionRow) -> serde_json::Value {
    serde_json::json!({
        "topic_id": row.topic_id,
        "version": row.version,
        "status": row.document.status,
        "metric_family": row.document.metric.family,
        "custom_id": row.document.metric.custom_id,
        "payout_mode": row.document.payout_mode.as_str(),
        "valid_from_epoch": row.document.valid_from_epoch,
        "valid_until_epoch": row.document.valid_until_epoch,
        "baseline_sealed": row.document.baseline.is_sealed(),
        "document": row.document,
    })
}

pub(crate) fn print_json<T: serde::Serialize>(value: &T) -> Result<(), Failure> {
    let body = serde_json::to_string_pretty(value).map_err(|e| Failure::Error(e.to_string()))?;
    println!("{body}");
    Ok(())
}

fn dash_if_empty(s: &str) -> String {
    if s.trim().is_empty() {
        "-".to_owned()
    } else {
        s.to_owned()
    }
}
