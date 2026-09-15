//! `topic install` — the real install, and the dry run.
//!
//! The procedure is the same in both modes; only the writes differ. A dry run
//! stops after the plan is printed. A real install:
//!
//! 1. **Drives** the topic's RLM setup (`TopicSetup`: provision →
//!    `propose_rules` → baseline) when `--drive-rlm` is given. This is the
//!    step that provisions a VM and runs a paid baseline, so it needs
//!    `--owner-approved` as well.
//! 2. **Applies** the bundle's RLM section through
//!    [`proof_topic_install::Installer`]: migrations under the deny-list,
//!    routes, the rule vector, and the executor binding.
//! 3. **Publishes** the signed document through the existing admin route
//!    (`POST /v1/admin/proof/topics`), with the operator bearer read from a
//!    file — never printed, never logged.
//! 4. **Points** the bundle's declared aliases at the topic.
//!
//! # Why the publish is last
//!
//! Publishing is what makes a topic **reachable**: a miner can submit to a
//! document whose `status` is `open`, and the topic's routes answer as soon as
//! their rows are in `proof_topic_api`. The install before it is the fallible
//! half — a deny-listed migration, a refused handler, an unregistered custom
//! id, a store error — and every one of those failures leaves the topic
//! **unpublished**: there is nothing for a miner to reach, so a failed install
//! cannot produce a topic that is live but not installed.
//!
//! The other order (publish, then install) makes an `open` document
//! submitable for as long as the install takes, and leaves it submitable
//! forever if the install fails. Its old justification was that the topic had
//! to exist before the rest could key on it; it does not — the rule store,
//! the route table, and the journal all key on `topic_id` with no dependency
//! on the published row, and the RLM setup writes the document itself when it
//! is not there yet. Aliases are the one step that does need a published
//! topic, which is why they stay last.
//!
//! # Fail-closed, and what the operator does next
//!
//! Any refusal stops the install and prints rollback notes naming the step
//! that failed and what remains to undo. Nothing is published unless the
//! install reached green, so the topic is not reachable at all; the install
//! journal (`proof_topic_install`) records the attempt so the next run
//! resumes from the migrations that already applied.

use std::path::{Path, PathBuf};

use proof_rlm::{RLM_VM_IMAGE_DIGEST_ENV, VM_ORCHESTRATOR_TOKEN_FILE_ENV, VM_ORCHESTRATOR_URL_ENV};
use proof_rlm_store::{PgRlmStore, RlmStore};
use proof_task::{InferenceOffer, ProofPin, TopicDocument};
use proof_topic_bundle::{InstallEnvironment, TopicInstallBundle, TopicInstallPlan};
use proof_topic_install::install::{InstallRequest, Installer, SetupSummary};
use proof_topic_install::InstallError;

use crate::{Failure, Options};
use proof_topic_ops::PublishTarget;

/// What the operator asserted, and what the install is therefore allowed to do.
///
/// Grouped rather than scattered so every authorization is visible in one
/// place. A `bool` that authorizes spend is exactly the kind of flag that
/// should be read together with the others.
#[derive(Debug, Clone, Copy)]
pub struct Gates {
    /// Resolve and print only.
    pub dry_run: bool,
    /// Owner assertion for a metal target.
    pub owner_metal_ack: bool,
    /// Owner assertion for provisioning and spend.
    pub owner_approved: bool,
}

/// Everything `topic install` was asked to do.
pub struct InstallArgs<'a> {
    /// Bundle JSON.
    pub bundle: &'a Path,
    /// Install target (`staging` / `metal`).
    pub env: &'a str,
    /// Pin the document is checked against.
    pub pin: &'a Path,
    /// What the operator authorized.
    pub gates: Gates,
    /// Stop before the RLM's baseline job.
    pub skip_baseline: bool,
    /// Master base URL for the admin publish call.
    pub admin_url: Option<&'a str>,
    /// File holding the operator bearer.
    pub admin_token_file: Option<&'a Path>,
    /// Drive the RLM setup over the topic-VM orchestrator.
    pub drive_rlm: bool,
    /// Live judge offer the baseline's paid run needs.
    pub inference_offer_file: Option<&'a Path>,
    /// Owner inference key file the lifecycle's key probe checks.
    pub owner_key_file: Option<&'a Path>,
}

/// Run `topic install`.
///
/// # Errors
///
/// [`Failure::Usage`] for a bad flag combination, [`Failure::Error`] for a
/// refused bundle, a refused document, a refused section, or a failed step.
pub async fn run(opts: &Options, args: &InstallArgs<'_>) -> Result<(), Failure> {
    let bundle = crate::load_bundle(args.bundle)?;
    let requested = crate::parse_env(args.env)?;
    // Owner default: metal is Owner-only, and staging goes first. A metal
    // install is refused unless the operator asserts both, so a live target
    // can never be reached by a default or a copy-pasted staging command.
    if requested == InstallEnvironment::Metal && !args.gates.owner_metal_ack {
        return Err(Failure::Usage(format!(
            "`--env metal` is Owner-only and requires --owner-metal-ack, which asserts that \
             (a) an Owner authorized this install and (b) staging has passed for this bundle \
             ({}). Install to staging first: `proof-admin topic install --bundle {} --env \
             staging --dry-run`.",
            args.bundle.display(),
            args.bundle.display()
        )));
    }
    // Driving the RLM provisions a VM and runs a paid baseline, so it needs
    // the same explicit Owner assertion the metal gate does. This is checked
    // before anything is written.
    if args.drive_rlm && !args.gates.owner_approved {
        return Err(Failure::Usage(
            "`--drive-rlm` provisions a topic VM and runs a paid baseline, so it requires \
             --owner-approved, which asserts that an Owner authorized the provisioning and the \
             spend. Without --drive-rlm the install applies the bundle's migrations, routes, \
             rules, and executor binding, and stops before any VM."
                .to_owned(),
        ));
    }
    if args.skip_baseline && !args.drive_rlm {
        return Err(Failure::Usage(
            "`--skip-baseline` only means something with --drive-rlm: without it the install \
             never reaches the baseline job. Drop one of the two flags."
                .to_owned(),
        ));
    }
    let pin = crate::load_pin(args.pin)?;
    let plan = bundle
        .plan(requested)
        .map_err(|e| Failure::Error(format!("{}: {e}", args.bundle.display())))?;
    // The same acceptance the publish route runs, so a dry run cannot print a
    // call the route would refuse and a real install cannot publish a document
    // the route would reject.
    crate::accept_document(&bundle, &pin)?;

    if args.gates.dry_run {
        if opts.json {
            crate::print_json(&plan)?;
            return Ok(());
        }
        crate::print_plan(&plan, args.bundle, args.pin);
        println!();
        println!("Dry run: nothing was written and no host was touched.");
        return Ok(());
    }

    run_real(opts, args, &bundle, &plan, &pin).await
}

/// The real install.
///
/// One function rather than a chain of helpers because the **order** is the
/// contract here: drive → apply → publish → aliases, each step's output
/// feeding the next, and a reader has to be able to see that no step runs
/// before the one it depends on.
#[allow(clippy::too_many_lines)]
async fn run_real(
    opts: &Options,
    args: &InstallArgs<'_>,
    bundle: &TopicInstallBundle,
    plan: &TopicInstallPlan,
    pin: &ProofPin,
) -> Result<(), Failure> {
    // The bearer and the URL are resolved before anything is written, so a
    // misconfiguration cannot leave a half-installed topic.
    let admin = PublishTarget::resolve(args.admin_url, args.admin_token_file)
        .map_err(crate::ops_to_failure)?;
    let database_url = crate::database_url(opts)?.ok_or_else(|| {
        Failure::Usage(
            "a real install writes to the topic registry, so it needs a database: set \
             BASE_DATABASE_URL (or BASE_DATABASE_URL_FILE). `--dry-run` needs none."
                .to_owned(),
        )
    })?;
    let pool = db::connect(&database_url)
        .await
        .map_err(|e| Failure::Error(format!("connect: {e}")))?;
    let store = PgRlmStore::new(pool.clone());
    let bundle_digest = bundle.digest().map_err(|e| Failure::Error(e.to_string()))?;
    let registered = crate::registered_custom_from_env();

    if !opts.json {
        println!("topic install (real)");
        println!("  topic_id          {}", plan.topic_id);
        println!("  environment       {}", plan.environment);
        println!("  bundle_digest     {bundle_digest}");
        println!("  admin             {}", admin.redacted());
        println!();
        println!(
            "1) Apply the RLM install section (the topic is not published until this is green)…"
        );
    }

    let installer = Installer {
        pool: &pool,
        store: &store,
    };
    // The RLM setup is driven **before** the static half lands, so the rules
    // the driver proposes are the topic's current version and the install
    // keeps them rather than overwriting them with the bundle's vector.
    let driven = if args.drive_rlm {
        if !opts.json {
            println!("   (driving the RLM: provision → rules → baseline)");
        }
        Some(drive_rlm(args, &bundle.topic, pin, &pool, &store).await?)
    } else {
        None
    };
    let setup = match &driven {
        Some(outcome) => match outcome.baseline_primary {
            Some(v) => SetupSummary::Baselined {
                rules_version: outcome.rules_version,
                baseline_primary: format!("{v}"),
            },
            None => SetupSummary::Skipped {
                reason: "--skip-baseline: the RLM's rules were installed, no baseline was \
                         measured"
                    .to_owned(),
            },
        },
        None => SetupSummary::NotDriven {
            reason: "not driven: --drive-rlm was not given, so no VM was provisioned and no \
                     baseline was run"
                .to_owned(),
        },
    };
    let report = installer
        .install(
            &InstallRequest {
                topic: &bundle.topic,
                bundle_digest: bundle_digest.clone(),
                environment: plan.environment.to_string(),
                rlm_raw: bundle.rlm.raw(),
                registered_custom: registered,
                skip_baseline: args.skip_baseline,
            },
            setup,
        )
        .await
        .map_err(|e| Failure::Error(install_failure(&e, &plan.topic_id)))?;

    if !opts.json {
        print_install_report(&report);
        if let Some(outcome) = &driven {
            println!("   rlm_drive         {}", outcome.summary());
            println!("   lifecycle         {}", outcome.state);
        }
        println!();
        println!("2) Publish the signed document through the existing admin route…");
    }
    // Publish **last**, once the install is green: a document reaches the
    // registry only when the rules, routes, and migrations it depends on are
    // already in place, so an `open` topic is never submitable before its
    // install landed.
    admin
        .publish(&bundle.topic)
        .await
        .map_err(|e| Failure::Error(publish_failure(&e, &report)))?;
    if !opts.json {
        println!("   published (the topic's status is the document's own).");
        println!();
        println!("3) Point the bundle's aliases at the topic…");
    }

    // Aliases last: they are a lookup convenience, so a failure here leaves
    // the topic installed and says exactly which alias to add by hand.
    let mut alias_notes = Vec::new();
    for alias in bundle.aliases() {
        match store.put_alias(&alias, &plan.topic_id).await {
            Ok(()) => alias_notes.push(format!("{alias} -> {}", plan.topic_id)),
            Err(e) => {
                return Err(Failure::Error(format!(
                    "the topic is installed, but the alias {alias:?} could not be pointed at it: \
                     {e}\n  Add it by hand once the cause is fixed:\n    \
                     proof-admin topic alias set {alias} --topic {}",
                    plan.topic_id
                )))
            }
        }
    }

    if opts.json {
        crate::print_json(&serde_json::json!({
            "ok": true,
            "topic_id": report.topic_id,
            "environment": report.environment,
            "bundle_digest": report.bundle_digest,
            "journal_id": report.journal_id,
            "migrations_applied": report.migrations_applied,
            "migrations_skipped": report.migrations_skipped,
            "apis": report.apis,
            "rules_version": report.rules_version,
            "rule_ids": report.rule_ids,
            "binding": report.binding,
            "setup": report.setup,
            "aliases": alias_notes,
            // Whether the topic can be scored right now, and what is left.
            // A machine caller needs this to decide whether to go on to the
            // seal; the install never makes a topic scorable on its own.
            "scorable": scorable(&report, args),
            "remaining": remaining_steps(&report, args, &plan.topic_id),
        }))?;
        return Ok(());
    }

    println!();
    println!("Install complete. Journal row {}.", report.journal_id);
    if !alias_notes.is_empty() {
        println!();
        println!("Aliases now point at the topic:");
        for note in &alias_notes {
            println!("  {note}");
        }
    }
    println!();
    println!("{}", scorable_line(&report, args));
    println!();
    println!("{}", next_steps(plan, args));
    Ok(())
}

/// Whether this install left the topic **scorable**, and why not when it did
/// not.
///
/// The install never makes a topic scorable by itself: scoring needs an
/// `open` document whose baseline is sealed, and the seal is the operator's
/// (the CLI holds no `proof` key). What this reports is which of the two
/// halves is still missing, so an operator — or a script — can tell "run the
/// next command" from "something is wrong".
fn scorable(report: &proof_topic_install::InstallReport, args: &InstallArgs<'_>) -> bool {
    matches!(report.setup, SetupSummary::Baselined { .. })
        && report.document_status == proof_task::TopicStatus::Open
        && !args.skip_baseline
}

/// The one-line answer to "can it score now?".
fn scorable_line(report: &proof_topic_install::InstallReport, args: &InstallArgs<'_>) -> String {
    if scorable(report, args) {
        return "Scorable: the baseline is measured and the document is open. Confirm with \
                GET /v1/status (`scorable_topics`)."
            .to_owned();
    }
    let missing = if args.skip_baseline {
        "--skip-baseline: no baseline was measured, and a topic cannot open without one"
    } else if !matches!(report.setup, SetupSummary::Baselined { .. }) {
        "the RLM setup was not driven: no baseline was measured"
    } else {
        "the document is not `open`: the signed open document has not been sealed and published"
    };
    format!("NOT scorable yet — {missing}.")
}

/// The steps still standing between this install and a scorable topic, for a
/// machine caller. Empty when the topic is already scorable.
fn remaining_steps(
    report: &proof_topic_install::InstallReport,
    args: &InstallArgs<'_>,
    topic_id: &str,
) -> Vec<String> {
    if scorable(report, args) {
        return Vec::new();
    }
    let mut steps = Vec::new();
    if args.skip_baseline || !matches!(report.setup, SetupSummary::Baselined { .. }) {
        steps.push(format!(
            "proof-admin topic install --bundle {} --env {} --drive-rlm --owner-approved",
            args.bundle.display(),
            args.env
        ));
    }
    steps.push(format!("proof-admin topic baseline {topic_id}"));
    steps.push(format!(
        "sign the open document sealing that commitment, then: proof-admin topic seal \
         {topic_id} --document <open.json> --publish --admin-url <master-or-gateway> \
         --admin-token-file <file>"
    ));
    steps
}

/// Read the live judge offer the baseline's paid run needs.
///
/// Read and validated here rather than inside the driver so a misconfigured
/// offer is a usage error **before** the static half writes anything: an
/// install that applies migrations and then discovers it cannot measure a
/// baseline would leave a topic that is half-installed for no reason.
fn load_offer(path: Option<&Path>, pin: &ProofPin) -> Result<InferenceOffer, Failure> {
    let Some(path) = path else {
        return Err(Failure::Usage(
            "driving the RLM measures a baseline, which is a paid run that needs a live judge \
             offer: set PROOF_INFERENCE_OFFER_FILE (or pass --inference-offer-file). To install \
             without measuring one, add --skip-baseline — but note the topic cannot open until a \
             baseline is sealed."
                .to_owned(),
        ));
    };
    let body = std::fs::read_to_string(path)
        .map_err(|e| Failure::Error(format!("read {}: {e}", path.display())))?;
    let offer = InferenceOffer::from_json(&body)
        .map_err(|e| Failure::Error(format!("{}: {e}", path.display())))?;
    offer
        .validate(pin)
        .map_err(|e| Failure::Error(format!("{}: {e}", path.display())))?;
    Ok(offer)
}

/// Drive the topic's RLM setup over the topic-VM orchestrator.
///
/// With `--skip-baseline` the offer is not needed: no paid run happens, so a
/// missing offer is not an error on that path.
async fn drive_rlm(
    args: &InstallArgs<'_>,
    topic: &TopicDocument,
    pin: &ProofPin,
    pool: &sqlx::PgPool,
    store: &PgRlmStore,
) -> Result<proof_topic_ops::DriveOutcome, Failure> {
    let _ = store;
    let offer = if args.skip_baseline {
        None
    } else {
        Some(load_offer(args.inference_offer_file, pin)?)
    };
    let url = std::env::var(VM_ORCHESTRATOR_URL_ENV).ok();
    let token = std::env::var(VM_ORCHESTRATOR_TOKEN_FILE_ENV)
        .ok()
        .map(|p| PathBuf::from(p.trim().to_owned()));
    let digest = std::env::var(RLM_VM_IMAGE_DIGEST_ENV).ok();
    proof_topic_ops::drive(
        topic,
        pin,
        PgRlmStore::new(pool.clone()),
        args.skip_baseline,
        args.gates.owner_approved,
        url.as_deref(),
        token.as_deref(),
        digest.as_deref(),
        offer,
        args.owner_key_file,
    )
    .await
    .map_err(crate::ops_to_failure)
}

/// Print the install report.
fn print_install_report(report: &proof_topic_install::InstallReport) {
    println!("   topic_id          {}", report.topic_id);
    println!("   journal_row       {}", report.journal_id);
    if report.migrations_applied.is_empty() && report.migrations_skipped.is_empty() {
        println!("   migrations        none in this bundle");
    } else {
        println!(
            "   migrations        {} applied, {} already applied",
            report.migrations_applied.len(),
            report.migrations_skipped.len()
        );
        for name in &report.migrations_applied {
            println!("     + {name}");
        }
        for name in &report.migrations_skipped {
            println!("     = {name} (already applied)");
        }
    }
    if report.apis.is_empty() {
        println!("   apis              none in this bundle");
    } else {
        println!("   apis              {} registered", report.apis.len());
        for route in &report.apis {
            println!("     {route}");
        }
    }
    println!(
        "   rules             version {} ({} rules)",
        report.rules_version,
        report.rule_ids.len()
    );
    println!("   handler           {}", report.binding.handler);
    println!(
        "   runner_id         {}",
        report.binding.runner_id.as_deref().unwrap_or("-")
    );
    println!(
        "   vms_per_submission {}",
        report.binding.vms_per_submission
    );
    match &report.setup {
        SetupSummary::Baselined {
            rules_version,
            baseline_primary,
        } => {
            println!("   rlm_setup         baselined (rules v{rules_version}, primary {baseline_primary})");
        }
        SetupSummary::Skipped { reason } | SetupSummary::NotDriven { reason } => {
            println!("   rlm_setup         {reason}");
        }
    }
}

/// What the operator does next, which depends on where the install stopped.
///
/// The last two steps of the ceremony are the **same** for a draft and for an
/// installed-and-measured topic, so they are named once: `topic baseline`
/// reads the measurement and prints the commitment an `open` document must
/// seal, and `topic seal --publish` records the seal and publishes the open
/// document. Between them the operator signs the open document (the CLI never
/// holds the `proof` key). That is the whole remaining path to a **scorable**
/// topic — nothing else is required, and nothing here spends.
fn next_steps(plan: &TopicInstallPlan, args: &InstallArgs<'_>) -> String {
    let seal = seal_steps(&plan.topic_id);
    if plan.document_status == proof_task::TopicStatus::Draft {
        return format!(
            "The document is a draft, so miners cannot submit to it yet. To go live:\n  \
             1. Drive the RLM setup (provision, rules, baseline):\n       \
             proof-admin topic install --bundle {} --env {} --drive-rlm --owner-approved\n  \
             {}",
            args.bundle.display(),
            args.env,
            seal
        );
    }
    if args.skip_baseline {
        return format!(
            "The document is {}, but --skip-baseline was given, so no baseline was measured.\n\
             Re-run without it — that is the only way to a scorable topic:\n    \
             proof-admin topic install --bundle {} --env {} --drive-rlm --owner-approved\n  \
             {}",
            crate::status_word(plan.document_status),
            args.bundle.display(),
            args.env,
            seal
        );
    }
    format!(
        "The document is {}. If the RLM setup was not driven, do that first:\n    \
         proof-admin topic install --bundle {} --env {} --drive-rlm --owner-approved\n  \
         {}",
        crate::status_word(plan.document_status),
        args.bundle.display(),
        args.env,
        seal
    )
}

/// The last two steps: read the measurement, sign the open document, seal it.
///
/// Shared by every branch above because they all end here, and because these
/// are the commands that make the topic **scorable** — the install alone never
/// does, whichever way it was run.
fn seal_steps(topic_id: &str) -> String {
    format!(
        "2. Read the measured baseline and the commitment the open document must seal:\n       \
         proof-admin topic baseline {topic_id}\n  \
         3. Put that `metrics_commitment` into the document, set `status: open`, sign it\n     \
         (the `proof` key stays with you: `xtask proof-topic` signs a draft), then:\n       \
         proof-admin topic seal {topic_id} --document <open.json> --publish \\\n         \
         --admin-url <master-or-gateway> --admin-token-file <file>\n  \
         4. Confirm the host scores it: GET /v1/status reports `can_score` and lists the topic\n     \
         in `scorable_topics` (`ctx proof status` from a miner host)."
    )
}

/// Turn a publish refusal into an operator instruction.
///
/// The install has already run by the time this can happen, so the message
/// says what is and is not in place rather than claiming nothing changed.
fn publish_failure(why: &str, report: &proof_topic_install::InstallReport) -> String {
    format!(
        "the install is applied, but the publish step failed: {why}\n  The topic is NOT \
         published, so miners cannot reach it and nothing is live. What is already in place:\n  \
         - the RLM install section applied (journal row {}, {} migration(s) applied, {} already \
         applied, rules v{})\n  - the routes it registered are in `proof_topic_api`\n  Nothing \
         needs to be undone. Fix the admin URL or bearer and re-run the same command: the \
         install resumes (its migrations are skipped) and publishes.",
        report.journal_id,
        report.migrations_applied.len(),
        report.migrations_skipped.len(),
        report.rules_version
    )
}

/// Turn an install refusal into an operator instruction, with rollback notes.
fn install_failure(err: &InstallError, topic_id: &str) -> String {
    let step = match err {
        InstallError::MigrationDenied { .. } => "the migration deny-list refused a statement",
        InstallError::MigrationFailed { .. } => "a migration failed in the database",
        InstallError::TooManyMigrations { .. } => "the bundle declares too many migrations",
        InstallError::HandlerNotAllowed(_) => "the handler allow-list refused the run backend",
        InstallError::CustomIdNotRegistered { .. } => {
            "the topic's custom id is not registered on this host"
        }
        InstallError::Section { .. } => "the RLM section is malformed",
        InstallError::Rules(_) => "the rule vector was refused",
        InstallError::Store(_) => "the rule store refused",
        InstallError::Db(_) => "the database refused",
        InstallError::Binding(_) => "the signed document's runner binding is malformed",
    };
    format!(
        "the install stopped: {step}\n  {err}\n\n  Rollback notes — what is and is not changed:\n  \
         - The document was **not** published: publishing is the last step, so the topic is not \
         in the\n    registry at all and miners cannot reach it (no status, no route, no \
         submission).\n  - Migrations already applied are recorded in `proof_topic_install`; a \
         re-run skips\n    them\n    (they are not rolled back automatically — drop them by hand \
         if the bundle is being\n    replaced rather than fixed).\n  - Rules already installed \
         stay installed; a re-run keeps the version it finds.\n  - Inspect the journal: \
         `proof-admin topic install-log --topic {topic_id}`\n  - Fix the bundle and re-run the \
         same command; the install resumes rather than restarts."
    )
}

/// The install journal for one topic, newest first.
///
/// # Errors
///
/// [`Failure::Usage`] without a database, [`Failure::Error`] on a query error.
pub async fn install_log(opts: &Options, topic_id: &str) -> Result<(), Failure> {
    let Some(url) = crate::database_url(opts)? else {
        return Err(Failure::Usage(
            "this command reads the install journal and needs a database: set \
             BASE_DATABASE_URL (or BASE_DATABASE_URL_FILE)."
                .to_owned(),
        ));
    };
    let pool = db::connect(&url)
        .await
        .map_err(|e| Failure::Error(format!("connect: {e}")))?;
    let row = proof_topic_install::latest_install(&pool, topic_id)
        .await
        .map_err(|e| Failure::Error(e.to_string()))?;
    let Some(row) = row else {
        return Err(Failure::Error(format!(
            "no install recorded for topic {topic_id:?}. The install journal is written by \
             `proof-admin topic install` (without --dry-run)."
        )));
    };
    if opts.json {
        crate::print_json(&row)?;
        return Ok(());
    }
    println!("topic {topic_id} — newest install (row {})", row.id);
    println!("  state             {}", row.state);
    println!("  environment       {}", row.environment);
    println!("  bundle_digest     {}", row.bundle_digest);
    println!(
        "  rules_version     {}",
        row.rules_version
            .map_or_else(|| "-".to_owned(), |v| v.to_string())
    );
    if !row.rule_ids.is_empty() {
        println!("  rule_ids          {}", row.rule_ids.join(", "));
    }
    if !row.migrations.is_empty() {
        println!("  migrations        {}", row.migrations.join(", "));
    }
    println!("  binding           {}", row.binding);
    if !row.detail.is_empty() {
        println!("  detail            {}", row.detail);
    }
    println!();
    println!("The journal is append-only; the newest row is the current install state.");
    Ok(())
}
