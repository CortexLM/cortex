//! `proof-admin` — Proof operator CLI for dynamic topics.
//!
//! P0 skeleton of the dynamic-topics admin path. It validates and installs a
//! **topic install bundle** (the JSON record that carries a topic's runner,
//! image and pack pins, concurrency, and enable flag), lists and shows what is
//! installed, and refuses the operations that belong to later slices with a
//! clear "not implemented" rather than a half-working guess.
//!
//! What this binary does **not** do, deliberately:
//!
//! - It never enables a topic. `topic install` writes a row with
//!   `enabled = false`; `topic enable` / `topic disable` / `topic seal` are
//!   fail-closed stubs (exit code 3) for the later slices.
//! - It touches no route, no allocator, and no scoring path. Nothing in this
//!   repository reads `proof_topic` yet, so an install cannot move a score.
//! - It removes none of the compiled-in bindings the current live topic uses;
//!   that is the last slice.
//!
//! `topic install --dry-run` needs no database at all: it parses, validates,
//! and prints the resolved plan. A real install needs `BASE_DATABASE_URL`
//! (or `BASE_DATABASE_URL_FILE`) and writes one disabled row.
//!
//! Exit codes: `0` ok, `1` error, `2` usage or configuration, `3` not
//! implemented in this slice.

#![forbid(unsafe_code)]
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use db::{NewTopic, PgPool, TopicRow};
use proof_topic_bundle::{InstallEnvironment, TopicInstallBundle, TopicInstallPlan};

/// Successful run.
const EXIT_OK: u8 = 0;
/// A command failed (bad bundle, database error, ...).
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
    about = "Proof operator CLI: topic install bundles, topic list/show",
    long_about = "proof-admin manages Proof topic installs (dynamic-topics P0 skeleton).

Validate a bundle without touching anything:
  proof-admin topic validate --bundle tb4.json

Resolve an install without a database:
  proof-admin topic install --bundle tb4.json --env metal --dry-run

Install it (writes one DISABLED row; enabling is a later slice):
  BASE_DATABASE_URL=... proof-admin topic install --bundle tb4.json --env metal

Nothing here enables a topic, opens a route, or changes how a score is
computed. `topic enable`, `topic disable`, and `topic seal` exit 3 with a
'not implemented in this slice' message."
)]
struct Cli {
    /// Postgres URL. Falls back to `BASE_DATABASE_URL`.
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
    /// Check a topic install bundle. Reads the file, writes nothing.
    Validate {
        /// Bundle JSON.
        #[arg(long, value_name = "PATH")]
        bundle: PathBuf,
    },
    /// Install a topic bundle. `--dry-run` resolves and prints it; a real
    /// install writes one disabled row and needs a database.
    Install {
        /// Bundle JSON.
        #[arg(long, value_name = "PATH")]
        bundle: PathBuf,
        /// Install target. Must match the bundle's own `environment`.
        #[arg(long, value_name = "staging|metal")]
        env: String,
        /// Resolve and print the install plan without touching a database.
        #[arg(long)]
        dry_run: bool,
    },
    /// List installed topics. An empty table prints nothing and exits 0.
    List,
    /// Show one installed topic by its exact `topic_id`.
    Show {
        /// Topic slug. Aliases are not resolved in this slice.
        topic_id: String,
    },
    /// Not implemented in this slice.
    Enable {
        /// Topic slug.
        topic_id: String,
    },
    /// Not implemented in this slice.
    Disable {
        /// Topic slug.
        topic_id: String,
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
    /// Anything else (bad bundle, database error).
    Error(String),
}

async fn run(cli: Cli) -> Result<(), Failure> {
    // Split the global options from the subcommand so both can be borrowed
    // without a partial move of `Cli`.
    let opts = Options {
        database_url: cli.database_url,
        database_url_file: cli.database_url_file,
        json: cli.json,
    };
    match cli.cmd {
        Cmd::Topic { cmd } => run_topic(&opts, &cmd).await,
    }
}

/// Global options, split out of [`Cli`] so the subcommand can be borrowed.
#[derive(Debug)]
struct Options {
    database_url: Option<String>,
    database_url_file: Option<PathBuf>,
    json: bool,
}

async fn run_topic(opts: &Options, cmd: &TopicCmd) -> Result<(), Failure> {
    match cmd {
        TopicCmd::Validate { bundle } => cmd_validate(opts, bundle),
        TopicCmd::Install {
            bundle,
            env,
            dry_run,
        } => cmd_install(opts, bundle, env, *dry_run).await,
        TopicCmd::List => cmd_list(opts).await,
        TopicCmd::Show { topic_id } => cmd_show(opts, topic_id).await,
        TopicCmd::Enable { topic_id } => Err(not_implemented("topic enable", topic_id)),
        TopicCmd::Disable { topic_id } => Err(not_implemented("topic disable", topic_id)),
        TopicCmd::Seal { topic_id, value } => Err(not_implemented(
            &format!("topic seal (value {value})"),
            topic_id,
        )),
    }
}

/// A stub that names what is missing instead of guessing.
fn not_implemented(command: &str, topic_id: &str) -> Failure {
    Failure::NotImplemented(format!(
        "`{command}` for topic {topic_id:?} is not implemented in this slice (P0: topics table \
         + admin CLI skeleton). Nothing was changed. Enabling, disabling, and sealing a topic \
         are later slices; installing a topic today writes a disabled row that no scoring path \
         reads yet."
    ))
}

/// Read and validate a bundle file. Shared by `validate` and `install`.
fn load_bundle(path: &Path) -> Result<TopicInstallBundle, Failure> {
    let body = std::fs::read_to_string(path)
        .map_err(|e| Failure::Error(format!("read {}: {e}", path.display())))?;
    TopicInstallBundle::from_json(&body)
        .map_err(|e| Failure::Error(format!("{}: {e}", path.display())))
}

/// Parse `--env` into an install target.
fn parse_env(raw: &str) -> Result<InstallEnvironment, Failure> {
    raw.parse::<InstallEnvironment>().map_err(Failure::Usage)
}

fn cmd_validate(opts: &Options, path: &Path) -> Result<(), Failure> {
    let bundle = load_bundle(path)?;
    bundle
        .validate()
        .map_err(|e| Failure::Error(format!("{}: {e}", path.display())))?;
    let digest = bundle.digest().map_err(|e| Failure::Error(e.to_string()))?;
    if opts.json {
        let body = serde_json::json!({
            "ok": true,
            "bundle": path.display().to_string(),
            "topic_id": bundle.topic_id,
            "schema_version": bundle.schema_version,
            "version": bundle.version,
            "environment": bundle.environment.as_str(),
            "aliases": bundle.aliases,
            "bundle_digest": digest,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&body).unwrap_or_else(|_| "{}".into())
        );
        return Ok(());
    }
    println!("bundle {} is valid", path.display());
    println!("  topic_id       {}", bundle.topic_id);
    println!("  schema_version {}", bundle.schema_version);
    println!("  version        {}", bundle.version);
    println!("  environment    {}", bundle.environment);
    println!("  aliases        {}", join_or_dash(&bundle.aliases));
    println!("  bundle_digest  {digest}");
    println!();
    println!("Nothing was written. Install with `proof-admin topic install --bundle … --env …`.");
    Ok(())
}

async fn cmd_install(opts: &Options, path: &Path, env: &str, dry_run: bool) -> Result<(), Failure> {
    let bundle = load_bundle(path)?;
    let requested = parse_env(env)?;
    let plan = bundle
        .plan(requested)
        .map_err(|e| Failure::Error(format!("{}: {e}", path.display())))?;

    if dry_run {
        if opts.json {
            print_json(&plan)?;
            return Ok(());
        }
        print_plan(&plan);
        println!();
        println!("Dry run: nothing was written and no database was touched.");
        return Ok(());
    }

    let pool = connect(opts).await?;
    let bundle_value = serde_json::to_value(&bundle)
        .map_err(|e| Failure::Error(format!("serialize bundle: {e}")))?;
    let row = new_topic(&plan, &bundle_value);
    // The write returns the row it committed, so the reported state and the
    // write share one outcome: a failure here means the install did not land,
    // and a success means the fields below are the persisted ones. A separate
    // read afterwards could fail after the commit and tell a caller a
    // successful install failed — which is how an automation retries and
    // overwrites a newer concurrent install.
    //
    // The reported `enabled` is the **persisted** one, not the state an
    // install would have written: a re-install deliberately leaves the column
    // alone, so a topic that was already live stays live.
    let persisted = db::upsert_topic(&pool, &row)
        .await
        .map_err(|e| Failure::Error(format!("install {}: {e}", plan.topic_id)))?;

    if opts.json {
        let body = serde_json::json!({
            "ok": true,
            "installed": true,
            "topic_id": persisted.topic_id,
            "environment": persisted.environment,
            "bundle_digest": persisted.bundle_digest,
            "enabled": persisted.enabled,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&body).unwrap_or_else(|_| "{}".into())
        );
        return Ok(());
    }
    print_plan(&plan);
    println!();
    if persisted.enabled {
        println!(
            "Re-installed {} (still ENABLED, unchanged by this install).",
            persisted.topic_id
        );
    } else {
        println!(
            "Installed {} (DISABLED). Enabling is a later slice: nothing scores this topic yet.",
            persisted.topic_id
        );
    }
    Ok(())
}

/// Borrow the plan's fields as the row to write. `bundle` is the file
/// verbatim: what an operator reviews is what the row keeps.
///
/// Every numeric field is already range-checked by
/// [`TopicInstallPlan`]'s validation, so the casts cannot truncate: the bundle
/// refuses a value that would not fit the row rather than clamping it.
fn new_topic<'a>(plan: &'a TopicInstallPlan, bundle: &'a serde_json::Value) -> NewTopic<'a> {
    NewTopic {
        topic_id: &plan.topic_id,
        display_name: &plan.display_name,
        version: to_i32(plan.version),
        environment: plan.environment.as_str(),
        runner_id: &plan.runner_id,
        aliases: &plan.aliases,
        config: &plan.config,
        pin_rlm: &plan.pin_rlm,
        pin_experiment: &plan.pin_experiment,
        pack_digest: &plan.pack_digest,
        n_concurrent: to_i32(plan.n_concurrent),
        sealed_custom_value: plan.sealed_custom_value,
        schema_version: to_i32(plan.schema_version),
        bundle,
        bundle_digest: &plan.bundle_digest,
    }
}

/// A `u32` the bundle's own validation proved fits an `INTEGER` column.
fn to_i32(v: u32) -> i32 {
    i32::try_from(v).unwrap_or(i32::MAX)
}

async fn cmd_list(opts: &Options) -> Result<(), Failure> {
    let pool = connect(opts).await?;
    let rows = db::list_topics(&pool)
        .await
        .map_err(|e| Failure::Error(format!("list topics: {e}")))?;
    if opts.json {
        let body: Vec<serde_json::Value> = rows.iter().map(topic_json).collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&body).unwrap_or_else(|_| "[]".into())
        );
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
    println!("Nothing is enabled by this CLI in this slice; see `proof-admin topic --help`.");
    Ok(())
}

async fn cmd_show(opts: &Options, topic_id: &str) -> Result<(), Failure> {
    let pool = connect(opts).await?;
    let row = db::get_topic(&pool, topic_id)
        .await
        .map_err(|e| Failure::Error(format!("show {topic_id}: {e}")))?;
    let Some(row) = row else {
        return Err(Failure::Error(format!(
            "no installed topic {topic_id:?}. Aliases are not resolved in this slice; \
             use `proof-admin topic list` to see the exact ids."
        )));
    };
    if opts.json {
        print_json(&topic_json(&row))?;
        return Ok(());
    }
    print_row(&row);
    Ok(())
}

/// Open the database, or explain which variable to set.
async fn connect(opts: &Options) -> Result<PgPool, Failure> {
    let url = database_url(opts)?;
    db::connect(&url)
        .await
        .map_err(|e| Failure::Error(format!("connect: {e}")))
}

/// `BASE_DATABASE_URL` value, or the contents of `BASE_DATABASE_URL_FILE`.
///
/// The two are mutually exclusive, matching `crates/config`: a value and a
/// file that disagree would be a silent choice between two databases.
fn database_url(opts: &Options) -> Result<String, Failure> {
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
        (Some(url), None) => Ok(url.to_owned()),
        (None, Some(path)) => {
            let raw = std::fs::read_to_string(path)
                .map_err(|e| Failure::Error(format!("read {}: {e}", path.display())))?;
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return Err(Failure::Usage(format!("{} is empty", path.display())));
            }
            Ok(trimmed.to_owned())
        }
        (None, None) => Err(Failure::Usage(
            "this command needs a database: set BASE_DATABASE_URL (or BASE_DATABASE_URL_FILE). \
             `topic validate` and `topic install --dry-run` need no database."
                .into(),
        )),
    }
}

fn print_plan(plan: &TopicInstallPlan) {
    println!("topic install plan");
    println!("  topic_id          {}", plan.topic_id);
    println!("  display_name      {}", plan.display_name);
    println!("  version           {}", plan.version);
    println!("  environment       {}", plan.environment);
    println!("  aliases           {}", join_or_dash(&plan.aliases));
    println!("  runner_id         {}", dash_if_empty(&plan.runner_id));
    println!("  pin_rlm           {}", dash_if_empty(&plan.pin_rlm));
    println!(
        "  pin_experiment    {}",
        dash_if_empty(&plan.pin_experiment)
    );
    println!("  pack_digest       {}", dash_if_empty(&plan.pack_digest));
    println!("  n_concurrent      {}", plan.n_concurrent);
    println!(
        "  sealed_custom_value {}",
        plan.sealed_custom_value
            .map_or_else(|| "-".to_owned(), |v| v.to_string())
    );
    println!("  schema_version    {}", plan.schema_version);
    println!("  bundle_digest     {}", plan.bundle_digest);
    println!(
        "  enabled           {} (install never enables)",
        plan.enabled
    );
}

fn print_row(row: &TopicRow) {
    println!("topic {}", row.topic_id);
    println!("  display_name      {}", row.display_name);
    println!("  version           {}", row.version);
    println!("  environment       {}", row.environment);
    println!("  aliases           {}", join_or_dash(&row.aliases));
    println!("  enabled           {}", row.enabled);
    println!("  runner_id         {}", dash_if_empty(&row.runner_id));
    println!("  pin_rlm           {}", dash_if_empty(&row.pin_rlm));
    println!("  pin_experiment    {}", dash_if_empty(&row.pin_experiment));
    println!("  pack_digest       {}", dash_if_empty(&row.pack_digest));
    println!("  n_concurrent      {}", row.n_concurrent);
    println!(
        "  sealed_custom_value {}",
        row.sealed_custom_value
            .map_or_else(|| "-".to_owned(), |v| v.to_string())
    );
    println!("  schema_version    {}", row.schema_version);
    println!("  bundle_digest     {}", row.bundle_digest);
    println!("  config            {}", compact(&row.config));
    println!("  created_at        {}", row.created_at);
    println!("  updated_at        {}", row.updated_at);
}

/// One-line summary for `topic list`.
fn summarize(row: &TopicRow) -> String {
    let state = if row.enabled { "enabled" } else { "disabled" };
    let runner = dash_if_empty(&row.runner_id);
    let aliases = if row.aliases.is_empty() {
        String::new()
    } else {
        format!(" (aliases: {})", row.aliases.join(", "))
    };
    format!(
        "{:<24} v{:<3} {:<7} {:<8} runner={}{}",
        row.topic_id, row.version, row.environment, state, runner, aliases
    )
}

fn topic_json(row: &TopicRow) -> serde_json::Value {
    serde_json::json!({
        "topic_id": row.topic_id,
        "display_name": row.display_name,
        "version": row.version,
        "environment": row.environment,
        "aliases": row.aliases,
        "enabled": row.enabled,
        "runner_id": row.runner_id,
        "pin_rlm": row.pin_rlm,
        "pin_experiment": row.pin_experiment,
        "pack_digest": row.pack_digest,
        "n_concurrent": row.n_concurrent,
        "sealed_custom_value": row.sealed_custom_value,
        "schema_version": row.schema_version,
        "bundle_digest": row.bundle_digest,
        "config": row.config,
        "created_at": row.created_at,
        "updated_at": row.updated_at,
    })
}

fn print_json<T: serde::Serialize>(value: &T) -> Result<(), Failure> {
    let body = serde_json::to_string_pretty(value).map_err(|e| Failure::Error(e.to_string()))?;
    println!("{body}");
    Ok(())
}

fn join_or_dash(items: &[String]) -> String {
    if items.is_empty() {
        "-".to_owned()
    } else {
        items.join(", ")
    }
}

fn dash_if_empty(s: &str) -> String {
    if s.is_empty() {
        "-".to_owned()
    } else {
        s.to_owned()
    }
}

fn compact(value: &serde_json::Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "{}".into())
}
