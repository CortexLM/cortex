//! `topic install` — the real install, and the dry run.
//!
//! The procedure is the same in both modes; only the writes differ. A dry run
//! stops after the plan is printed. A real install:
//!
//! 1. **Publishes** the signed document through the existing admin route
//!    (`POST /v1/admin/proof/topics`), with the operator bearer read from a
//!    file — never printed, never logged.
//! 2. **Applies** the bundle's RLM section through
//!    [`proof_topic_install::Installer`]: migrations under the deny-list,
//!    routes, the rule vector, and the executor binding.
//! 3. **Points** the bundle's declared aliases at the topic.
//! 4. **Drives** the topic's RLM setup (`TopicSetup`: provision →
//!    `propose_rules` → baseline) when `--drive-rlm` is given. This is the
//!    step that provisions a VM and runs a paid baseline, so it needs
//!    `--owner-approved` as well.
//!
//! # Why the order is publish-last for a *new* topic, and why it is not here
//!
//! The document is published first because everything after it needs the
//! topic to exist in the registry: the rule store keys on `topic_id`, the
//! route table keys on it, and the RLM setup reads the published document.
//! What makes that safe is the **status**: an install publishes the document
//! exactly as the operator signed it, and a topic that is not `open` cannot
//! be submitted to. A failed install therefore leaves a `draft` topic that
//! miners cannot reach — never a half-live one.
//!
//! # Fail-closed, and what the operator does next
//!
//! Any refusal stops the install and prints rollback notes naming the step
//! that failed and what remains to undo. The topic stays draft/disabled; the
//! install journal (`proof_topic_install`) records the attempt so the next
//! run resumes from the migrations that already applied.

use std::path::{Path, PathBuf};

use proof_rlm::{RLM_VM_IMAGE_DIGEST_ENV, VM_ORCHESTRATOR_TOKEN_FILE_ENV, VM_ORCHESTRATOR_URL_ENV};
use proof_rlm_store::{PgRlmStore, RlmStore};
use proof_task::{InferenceOffer, ProofPin, TopicDocument};
use proof_topic_bundle::{InstallEnvironment, TopicInstallBundle, TopicInstallPlan};
use proof_topic_install::install::{InstallRequest, Installer, SetupSummary};
use proof_topic_install::InstallError;

use crate::{Failure, Options};

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
async fn run_real(
    opts: &Options,
    args: &InstallArgs<'_>,
    bundle: &TopicInstallBundle,
    plan: &TopicInstallPlan,
    pin: &ProofPin,
) -> Result<(), Failure> {
    // The bearer and the URL are resolved before anything is written, so a
    // misconfiguration cannot leave a half-installed topic.
    let admin = AdminTarget::resolve(args)?;
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
        println!("1) Publish the signed document through the existing admin route…");
    }
    admin
        .publish(&bundle.topic)
        .await
        .map_err(|e| Failure::Error(publish_failure(&e)))?;
    if !opts.json {
        println!("   published (the topic's status is the document's own).");
        println!();
        println!("2) Apply the RLM install section…");
    }

    let installer = Installer {
        pool: &pool,
        store: &store,
    };
    // The RLM setup is driven **after** the static half lands, so the rules
    // the driver proposes supersede a version the topic already has rather
    // than being overwritten by the bundle's vector.
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
    println!("{}", next_steps(plan, args));
    Ok(())
}

/// Read the live judge offer the baseline's paid run needs.
///
/// Read and validated here rather than inside the driver so a misconfigured
/// offer is a usage error **before** the static half writes anything: an
/// install that applies migrations and then discovers it cannot measure a
/// baseline would leave a topic that is half-installed for no reason.
fn load_offer(path: Option<&Path>, pin: &ProofPin) -> Result<InferenceOffer, Failure> {
    let Some(path) = path else {
        return Err(Failure::Usage(format!(
            "driving the RLM measures a baseline, which is a paid run that needs a live judge \
             offer: set PROOF_INFERENCE_OFFER_FILE (or pass --inference-offer-file). To install \
             without measuring one, add --skip-baseline — but note the topic cannot open until a \
             baseline is sealed."
        )));
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
) -> Result<crate::drive::DriveOutcome, Failure> {
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
    crate::drive::drive(
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
fn next_steps(plan: &TopicInstallPlan, args: &InstallArgs<'_>) -> String {
    if plan.document_status == proof_task::TopicStatus::Draft {
        return format!(
            "The document is a draft, so miners cannot submit to it yet. To go live:\n  \
             1. Drive the RLM setup (provision, rules, baseline):\n       \
             proof-admin topic install --bundle {} --env {} --drive-rlm --owner-approved\n  \
             2. Seal the baseline the RLM measured, re-sign the document as `open`, and\n     \
             publish it through POST /v1/admin/proof/topics.\n  \
             3. Confirm it is live: proof-admin topic show {}",
            args.bundle.display(),
            args.env,
            plan.topic_id
        );
    }
    if args.skip_baseline {
        return format!(
            "The document is {}, but --skip-baseline was given, so no baseline was measured.\n\
             Re-run without it before the topic can score:\n    \
             proof-admin topic install --bundle {} --env {} --drive-rlm --owner-approved",
            crate::status_word(plan.document_status),
            args.bundle.display(),
            args.env
        );
    }
    format!(
        "The document is {}. If the RLM setup was not driven, do that before miners submit:\n    \
         proof-admin topic install --bundle {} --env {} --drive-rlm --owner-approved",
        crate::status_word(plan.document_status),
        args.bundle.display(),
        args.env
    )
}

/// Where the admin publish call goes, and the bearer it uses.
struct AdminTarget {
    base_url: String,
    token: String,
}

impl AdminTarget {
    /// Resolve the URL and bearer, refusing a half-configured pair.
    fn resolve(args: &InstallArgs<'_>) -> Result<Self, Failure> {
        let Some(base_url) = args.admin_url.map(str::trim).filter(|u| !u.is_empty()) else {
            return Err(Failure::Usage(
                "a real install publishes through the admin route, so it needs the master's \
                 base URL: pass --admin-url (or set PROOF_ADMIN_URL), e.g. \
                 --admin-url http://127.0.0.1:8100 for the challenge service directly, or the \
                 gateway's address. `--dry-run` needs none."
                    .to_owned(),
            ));
        };
        let Some(path) = args.admin_token_file else {
            return Err(Failure::Usage(
                "a real install needs the operator bearer for /v1/admin/*: pass \
                 --admin-token-file (or set PROOF_ADMIN_TOKEN_FILE). The file is read and never \
                 logged or printed. `--dry-run` needs none."
                    .to_owned(),
            ));
        };
        let token = std::fs::read_to_string(path)
            .map_err(|e| Failure::Error(format!("read {}: {e}", path.display())))?;
        // A tokens file holds one bearer per line; the first non-comment line
        // is the one this call uses.
        let token = token
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty() && !l.starts_with('#'))
            .map(str::to_owned);
        let Some(token) = token else {
            return Err(Failure::Error(format!(
                "{} holds no bearer (every line is blank or a comment)",
                path.display()
            )));
        };
        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_owned(),
            token,
        })
    }

    /// How this target is printed: the URL, never the bearer.
    fn redacted(&self) -> String {
        format!("{} (bearer read, never printed)", self.base_url)
    }

    /// Publish the document through the existing admin route.
    async fn publish(&self, doc: &proof_task::TopicDocument) -> Result<(), String> {
        let url = format!("{}{}", self.base_url, proof_topic_bundle::PUBLISH_PATH);
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_mins(1))
            .build()
            .map_err(|e| format!("http client: {e}"))?;
        let response = client
            .post(&url)
            .header("authorization", format!("Bearer {}", self.token))
            .header("content-type", "application/json")
            .body(
                serde_json::to_string(doc)
                    .map_err(|e| format!("serialize the signed document: {e}"))?,
            )
            .send()
            .await
            .map_err(|e| format!("POST {url}: {e}"))?;
        let status = response.status();
        if status.is_success() {
            return Ok(());
        }
        let body = response.text().await.unwrap_or_default();
        Err(format!(
            "POST {url} answered {status}: {}",
            body.trim().chars().take(400).collect::<String>()
        ))
    }
}

/// Turn a publish refusal into an operator instruction.
fn publish_failure(why: &str) -> String {
    format!(
        "the publish step failed, so nothing was applied: {why}\n  Nothing was changed on the \
         host. Check the admin URL and bearer, then re-run."
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
         - The topic's status is the published document's own, and an install never publishes an \
         `open`\n    document, so the topic is draft/disabled and miners cannot submit to it.\n  \
         - Migrations already applied are recorded in `proof_topic_install`; a re-run skips \
         them\n    (they are not rolled back automatically — drop them by hand if the bundle is \
         being\n    replaced rather than fixed).\n  - Rules already installed stay installed; a \
         re-run keeps the version it finds.\n  - Inspect the journal: \
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
