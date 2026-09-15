//! The install engine: apply a bundle's RLM section, fail-closed, resumable,
//! and journaled.
//!
//! One install does four things, in this order, each of which either
//! completes or stops the install with a named reason:
//!
//! 1. **Migrations** — the topic's SQL, applied through the deny-list
//!    ([`proof_topic_sql_guard`]). A statement touching a `proof_*` object, a
//!    role, or another topic's namespace is refused *before* the first one
//!    runs, so a bundle cannot leave half its migrations applied. Resumable
//!    by journal: a migration whose name already appears in this topic's
//!    install rows is skipped, so a re-run continues rather than repeating.
//! 2. **APIs** — the routes the topic claims, recorded in `proof_topic_api`
//!    under the topic's own prefix. The control plane has no compile-time
//!    route table a topic could extend, so this table *is* the dynamic route
//!    registry; `ON CONFLICT DO NOTHING` makes a re-run idempotent.
//! 3. **Rules** — the topic's rule vector, installed as rule version 1
//!    through the store's own [`RlmStore::put_rules`], so the gate the
//!    scoring path reads is the one the install landed. A topic already past
//!    version 1 keeps what its RLM wrote.
//! 4. **Executor binding** — the allow-listed handler, the runner the signed
//!    document selects, the custom id, the pack pin, the submission-format
//!    and scoring digests, and the **VMs-per-submission pin**. Recorded in
//!    the journal; never a second registry the scoring path reads *instead of*
//!    the signed document.
//!
//! The RLM's own lifecycle (`TopicSetup`: provision → propose_rules →
//! baseline) is driven by the **caller** over the topic-VM orchestrator,
//! because that step talks to a KVM host and holds a lifecycle that outlives
//! one install call. [`InstallReport::setup`] says what the caller did with
//! it, so the journal and the operator output agree.
//!
//! # What an install is not
//!
//! Not a scoring path, and it cannot change a score. It writes the rules the
//! gate reads (through the same store the scoring path uses), records routes,
//! and appends a journal row. It cannot publish a document (the operator's
//! bearer does that), cannot seal a baseline (the operator does that with the
//! RLM's measurement), and cannot move a topic's status.
//!
//! # Fail-closed, and where the topic is left
//!
//! A failure leaves the topic **draft or disabled**: an install never
//! publishes an `open` document, so a failed setup cannot produce a topic
//! miners can submit to.
//!
//! Refusals come in two kinds, and they differ in what they leave behind:
//!
//! - **Pre-flight refusals** — the deny-list, the handler allow-list, the
//!   section shape, and the open-custom-id gate — run before the journal
//!   opens, so a bundle they refuse **writes nothing at all**. Not a row, not
//!   a rule, not a table. The operator sees the refusal on stderr and fixes
//!   the bundle; there is nothing to roll back.
//! - **Step failures** — a migration the database rejected, a store error —
//!   happen after the journal opens, so they append a `failed` row naming the
//!   step and the reason. The migrations already applied stay applied and are
//!   recorded, so the next attempt resumes from them rather than restarting.

use std::collections::BTreeSet;

use proof_rlm::{RuleSet, RuleSource};
use proof_rlm_store::{RlmStore, StoreError};
use proof_task::{MetricFamily, TopicDocument, TopicStatus};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use crate::handler::{bound_runner, Handler};
use crate::section::{ApiRoute, SectionPlan, MAX_MIGRATIONS};
use crate::InstallError;
use proof_topic_sql_guard::{check_migration, Statement};

/// VMs one submission may use. **1**, always.
///
/// A submission is one artefact evaluated once: the topic's RLM inspects it,
/// and either rejects it without spend or runs it in exactly one guest. This
/// constant is the pin the install records; a future slice cannot quietly
/// allow a second concurrent VM per submission without changing this value
/// and the journal rows that carry it.
pub const VMS_PER_SUBMISSION: u32 = 1;

/// Install states the journal records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallState {
    /// Appended before the first applied step: a crash leaves evidence.
    Pending,
    /// Every step succeeded.
    Applied,
    /// A step refused; `detail` says which and why.
    Failed,
}

impl InstallState {
    /// Wire word, matching the migration's CHECK.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Applied => "applied",
            Self::Failed => "failed",
        }
    }
}

/// The executor binding an install resolved and recorded.
///
/// Every field comes from the **signed document** or from the operator's
/// bundle. `vms_per_submission` is the pin ([`VMS_PER_SUBMISSION`]), carried
/// so the journal says what the topic was installed with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutorBinding {
    /// Handler family the install bound ([`Handler::as_str`]).
    pub handler: String,
    /// In-guest runner the signed document selects, when it selects one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runner_id: Option<String>,
    /// The topic's `metric.custom_id` (empty on non-custom families).
    pub custom_id: String,
    /// Pack digest the signed document pins, when it pins one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pack_digest: Option<String>,
    /// VMs one submission may use. Always [`VMS_PER_SUBMISSION`].
    pub vms_per_submission: u32,
    /// Digest of the bundle's `submission_format` part, when it carries one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub submission_format_digest: Option<String>,
    /// Digest of the bundle's `scoring` part, when it carries one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scoring_digest: Option<String>,
}

/// One row of the install journal, as the operator reads it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InstallRow {
    /// Journal row id.
    pub id: i64,
    /// Topic the install ran against.
    pub topic_id: String,
    /// `sha256:` digest of the canonical bundle.
    pub bundle_digest: String,
    /// Install target.
    pub environment: String,
    /// Where the install got to.
    pub state: String,
    /// Rule version landed, once one was.
    pub rules_version: Option<u32>,
    /// Rule ids installed.
    pub rule_ids: Vec<String>,
    /// Migration names applied.
    pub migrations: Vec<String>,
    /// The executor binding, verbatim.
    pub binding: serde_json::Value,
    /// Why the install stopped, when it did.
    pub detail: String,
}

/// What the RLM setup step did, as the caller reported it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SetupSummary {
    /// The setup ran: the RLM proposed rules and measured a baseline.
    Baselined {
        /// Rule version the RLM wrote.
        rules_version: u32,
        /// The baseline primary the operator seals next.
        baseline_primary: String,
    },
    /// The setup ran with `--skip-baseline`: rules were installed, no
    /// baseline was measured.
    Skipped {
        /// Why (the flag).
        reason: String,
    },
    /// The install did not drive the RLM for this topic.
    NotDriven {
        /// Why.
        reason: String,
    },
}

/// What one install did, for the CLI to print.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InstallReport {
    /// Topic installed.
    pub topic_id: String,
    /// Bundle digest applied.
    pub bundle_digest: String,
    /// Install target.
    pub environment: String,
    /// Migrations applied by **this** run (already-applied ones are skipped).
    pub migrations_applied: Vec<String>,
    /// Migrations skipped because the journal shows they were applied.
    pub migrations_skipped: Vec<String>,
    /// Routes registered (or already present), as `METHOD /path`.
    pub apis: Vec<String>,
    /// Rule version the topic is on after this run.
    pub rules_version: u32,
    /// Rule ids in force.
    pub rule_ids: Vec<String>,
    /// Executor binding recorded.
    pub binding: ExecutorBinding,
    /// What the RLM setup step did.
    pub setup: SetupSummary,
    /// The journal row this run appended.
    pub journal_id: i64,
    /// The document's own status, as the bundle carries it.
    ///
    /// Reported so an operator (and `topic install --json`) can see whether
    /// the install left a **scorable** topic: a `draft` is not one, whatever
    /// the setup step measured, and the remaining step is the operator's seal
    /// (`proof-admin topic seal --publish`).
    pub document_status: TopicStatus,
}

/// Everything one install needs, resolved by the caller.
pub struct InstallRequest<'a> {
    /// The signed document, verbatim.
    pub topic: &'a TopicDocument,
    /// The canonical bundle's `sha256:` digest.
    pub bundle_digest: String,
    /// Install target (`staging` / `metal`).
    pub environment: String,
    /// The bundle's RLM section, verbatim.
    pub rlm_raw: &'a str,
    /// Registered custom ids on this host (`PROOF_VM_RUNNER_CUSTOM_IDS`), for
    /// the open-custom-topic check.
    pub registered_custom: Vec<String>,
    /// Stop before the RLM's baseline job.
    pub skip_baseline: bool,
}

/// The install engine: a database to write through, plus the store the
/// scoring path reads.
pub struct Installer<'a> {
    /// Pool for the topic's own SQL and the journal.
    pub pool: &'a PgPool,
    /// The store the scoring path reads (rules land here).
    pub store: &'a dyn RlmStore,
}

impl Installer<'_> {
    /// Apply `request` and return what was done.
    ///
    /// # Errors
    ///
    /// [`InstallError`]. Every refusal happens **before** the step it
    /// refuses: the deny-list runs over every statement of every migration
    /// before the first statement executes, the handler allow-list and the
    /// custom-id gate run before the binding is recorded, and the route
    /// shapes are checked before a row is written.
    pub async fn install(
        &self,
        request: &InstallRequest<'_>,
        setup: SetupSummary,
    ) -> Result<InstallReport, InstallError> {
        let plan = crate::section::read_section(request.rlm_raw)?;
        // Check every migration up front: a bundle that would fail on its
        // third statement must not leave its first two applied.
        let checked = check_all_migrations(&plan, &request.topic.id)?;
        let handler = plan.handler.unwrap_or(Handler::VmBacked);
        let binding = resolve_binding(request, &plan, handler)?;

        // The `pending` row lands first, so a crash mid-install is visible
        // rather than silent. A failure appends its own `failed` row naming
        // the step, so the journal says how far the run got.
        let pending_id = self
            .journal(request, InstallState::Pending, None, &[], &[], &binding, "")
            .await?;
        match self
            .apply_all(request, &plan, &checked, &binding, setup)
            .await
        {
            Ok(report) => Ok(report),
            Err(e) => {
                let _ = self
                    .journal(
                        request,
                        InstallState::Failed,
                        None,
                        &[],
                        &[],
                        &binding,
                        &e.to_string(),
                    )
                    .await;
                let _ = pending_id;
                Err(e)
            }
        }
    }

    /// Apply migrations, routes, and rules, then journal the result.
    async fn apply_all(
        &self,
        request: &InstallRequest<'_>,
        plan: &SectionPlan,
        checked: &[(String, Vec<Statement>)],
        binding: &ExecutorBinding,
        setup: SetupSummary,
    ) -> Result<InstallReport, InstallError> {
        let already = self.applied_migrations(&request.topic.id).await?;
        let mut applied: Vec<String> = already.iter().cloned().collect();
        applied.sort();
        let mut applied_now = Vec::new();
        let mut skipped = Vec::new();
        for (name, statements) in checked {
            if already.contains(name) {
                skipped.push(name.clone());
                continue;
            }
            // The migration and the journal row that records it commit in
            // **one** transaction, so a crash cannot leave a migration applied
            // with no durable record of it. See `run_migration`.
            self.run_migration(request, name, statements, &applied)
                .await?;
            applied.push(name.clone());
            applied_now.push(name.clone());
        }

        let apis = self.register_apis(&request.topic.id, &plan.apis).await?;
        let rules = self.install_rules(request, plan).await?;
        let rule_ids: Vec<String> = rules.rules.iter().map(|r| r.id.clone()).collect();

        let journal_id = self
            .journal(
                request,
                InstallState::Applied,
                Some(rules.version),
                &rule_ids,
                &applied,
                binding,
                "",
            )
            .await?;
        Ok(InstallReport {
            topic_id: request.topic.id.clone(),
            bundle_digest: request.bundle_digest.clone(),
            environment: request.environment.clone(),
            migrations_applied: applied_now,
            migrations_skipped: skipped,
            apis,
            rules_version: rules.version,
            rule_ids,
            binding: binding.clone(),
            setup,
            journal_id,
            document_status: request.topic.status,
        })
    }

    /// Migration names this topic has already applied, from the journal.
    ///
    /// Every row this reads was written **in the same transaction as the
    /// migration it names**, so the set is exactly the migrations whose
    /// effects are durably in the database. A run that crashed mid-way leaves
    /// a `pending` row naming the migrations that committed before the crash,
    /// and a resume skips them.
    async fn applied_migrations(&self, topic_id: &str) -> Result<BTreeSet<String>, InstallError> {
        let rows: Vec<(serde_json::Value,)> = sqlx::query_as(
            "SELECT migrations FROM proof_topic_install \
             WHERE topic_id = $1 AND state IN ('pending', 'applied')",
        )
        .bind(topic_id)
        .fetch_all(self.pool)
        .await
        .map_err(|e| InstallError::Db(e.to_string()))?;
        let mut out = BTreeSet::new();
        for (value,) in rows {
            if let Some(names) = value.as_array() {
                for name in names {
                    if let Some(s) = name.as_str() {
                        out.insert(s.to_owned());
                    }
                }
            }
        }
        Ok(out)
    }

    /// Run one migration's statements **and record it, in one transaction**.
    ///
    /// All-or-nothing per migration, and — the part that makes resume correct
    /// — the journal row that names the migration commits with it. A crash
    /// can therefore leave two states and no third:
    ///
    /// - the migration's effects are in the database *and* the journal names
    ///   it, so a resume skips it; or
    /// - neither is, so a resume applies it.
    ///
    /// The alternative (apply, commit, then journal separately) has a window
    /// where a migration has run and nothing records it: a resume would
    /// re-apply it, and ordinary non-idempotent DDL such as `CREATE TABLE`
    /// would fail on a duplicate relation. Writing the row inside the same
    /// transaction closes that window rather than narrowing it.
    ///
    /// The row is `pending` with the migrations applied **so far** (including
    /// this one). A run that finishes writes its `applied` row afterwards;
    /// the `pending` rows are what a resume reads, so an interrupted install
    /// resumes from exactly what landed.
    async fn run_migration(
        &self,
        request: &InstallRequest<'_>,
        name: &str,
        statements: &[Statement],
        applied_before: &[String],
    ) -> Result<(), InstallError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| InstallError::Db(e.to_string()))?;
        for statement in statements {
            sqlx::query(&statement.text)
                .execute(&mut *tx)
                .await
                .map_err(|e| InstallError::MigrationFailed {
                    name: name.to_owned(),
                    ordinal: statement.ordinal,
                    detail: e.to_string(),
                })?;
        }
        let mut progress: Vec<String> = applied_before.to_vec();
        if !progress.iter().any(|n| n == name) {
            progress.push(name.to_owned());
        }
        sqlx::query(
            "INSERT INTO proof_topic_install \
             (topic_id, bundle_digest, environment, state, migrations, detail) \
             VALUES ($1, $2, $3, 'pending', $4, $5)",
        )
        .bind(&request.topic.id)
        .bind(&request.bundle_digest)
        .bind(&request.environment)
        .bind(serde_json::Value::Array(
            progress
                .iter()
                .cloned()
                .map(serde_json::Value::String)
                .collect(),
        ))
        .bind(format!("migration {name} applied"))
        .execute(&mut *tx)
        .await
        .map_err(|e| InstallError::Db(e.to_string()))?;
        tx.commit()
            .await
            .map_err(|e| InstallError::MigrationFailed {
                name: name.to_owned(),
                ordinal: 0,
                detail: e.to_string(),
            })?;
        Ok(())
    }

    /// Record the routes a topic claims.
    async fn register_apis(
        &self,
        topic_id: &str,
        apis: &[ApiRoute],
    ) -> Result<Vec<String>, InstallError> {
        let mut out = Vec::with_capacity(apis.len());
        for route in apis {
            sqlx::query(
                "INSERT INTO proof_topic_api (topic_id, path, method, summary) \
                 VALUES ($1, $2, $3, $4) ON CONFLICT (topic_id, method, path) DO NOTHING",
            )
            .bind(topic_id)
            .bind(&route.path)
            .bind(&route.method)
            .bind(&route.summary)
            .execute(self.pool)
            .await
            .map_err(|e| InstallError::Db(e.to_string()))?;
            out.push(format!("{} /{}", route.method, route.path));
        }
        Ok(out)
    }

    /// Install the section's rule vector as the topic's rule version 1.
    ///
    /// The **store** decides the version, so a re-run does not double-install:
    /// an existing current rule set is returned untouched, which is what keeps
    /// a topic whose RLM has already written version 2 from being reset to
    /// the bundle's vector.
    ///
    /// Provenance is the point here. What this seeds is `topic_document`: the
    /// vector the **operator** signed. That is honest provenance, not a
    /// substitute for RLM authorship — the topic's RLM advances the store to
    /// `rlm` by running its own `propose_rules` job in its VM
    /// ([`proof_topic_setup::TopicSetup`]). An install therefore never makes a
    /// topic's behavior RLM-authored, and the gates that admit an `open` topic
    /// read the provenance rather than this row's presence.
    async fn install_rules(
        &self,
        request: &InstallRequest<'_>,
        plan: &SectionPlan,
    ) -> Result<RuleSet, InstallError> {
        if let Some(current) = self
            .store
            .current_rules(&request.topic.id)
            .await
            .map_err(|e| map_store(&e))?
        {
            return Ok(current);
        }
        let rules = if plan.rules.is_empty() {
            RuleSet::from_topic(request.topic).map_err(|e| InstallError::Rules(e.to_string()))?
        } else {
            let set = RuleSet {
                topic_id: request.topic.id.clone(),
                version: 1,
                source: RuleSource::TopicDocument,
                rules: plan.rules.clone(),
            };
            set.validate()
                .map_err(|e| InstallError::Rules(e.to_string()))?;
            set
        };
        self.store
            .put_rules(&rules)
            .await
            .map_err(|e| map_store(&e))?;
        Ok(rules)
    }

    /// Append a journal row.
    #[allow(clippy::too_many_arguments)]
    async fn journal(
        &self,
        request: &InstallRequest<'_>,
        state: InstallState,
        rules_version: Option<u32>,
        rule_ids: &[String],
        migrations: &[String],
        binding: &ExecutorBinding,
        detail: &str,
    ) -> Result<i64, InstallError> {
        let version = rules_version
            .map(i32::try_from)
            .transpose()
            .map_err(|e| InstallError::Db(format!("rules_version out of range: {e}")))?;
        let json = |items: &[String]| {
            serde_json::Value::Array(
                items
                    .iter()
                    .cloned()
                    .map(serde_json::Value::String)
                    .collect(),
            )
        };
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO proof_topic_install \
             (topic_id, bundle_digest, environment, state, rules_version, rule_ids, migrations, \
              binding, detail) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) RETURNING id",
        )
        .bind(&request.topic.id)
        .bind(&request.bundle_digest)
        .bind(&request.environment)
        .bind(state.as_str())
        .bind(version)
        .bind(json(rule_ids))
        .bind(json(migrations))
        .bind(serde_json::to_value(binding).map_err(|e| InstallError::Db(e.to_string()))?)
        .bind(detail)
        .fetch_one(self.pool)
        .await
        .map_err(|e| InstallError::Db(e.to_string()))?;
        Ok(id)
    }
}

/// Resolve the executor binding from the signed document and the section.
///
/// The **document** is authoritative for the runner and the pack; the section
/// supplies only the handler family, which is allow-listed. The one gate here
/// that is not a shape check is the open-custom-id check: an open custom topic
/// whose id this host does not register answers 503 when a miner submits, so
/// the install refuses rather than recording a binding that cannot score.
///
/// # Errors
///
/// [`InstallError::Binding`] for a malformed document binding,
/// [`InstallError::CustomIdNotRegistered`] for an unregistered open custom id.
fn resolve_binding(
    request: &InstallRequest<'_>,
    plan: &SectionPlan,
    handler: Handler,
) -> Result<ExecutorBinding, InstallError> {
    let doc_binding =
        proof_experiment::ExperimentBinding::from_params(&request.topic.constraints.params)
            .map_err(|e| InstallError::Binding(e.to_string()))?;
    let doc_runner = doc_binding.as_ref().map(|b| b.runner.clone());
    let (runner_id, handler) = bound_runner(doc_runner.as_deref(), Some(handler));
    let custom_id = request.topic.metric.custom_id.trim().to_owned();
    if request.topic.status == TopicStatus::Open
        && request.topic.metric.family == MetricFamily::Custom
        && !request.registered_custom.iter().any(|c| c == &custom_id)
    {
        return Err(InstallError::CustomIdNotRegistered {
            custom_id,
            registered: request.registered_custom.clone(),
        });
    }
    Ok(ExecutorBinding {
        handler: handler.as_str().to_owned(),
        runner_id,
        custom_id,
        pack_digest: doc_binding.map(|b| b.pack.digest),
        vms_per_submission: VMS_PER_SUBMISSION,
        submission_format_digest: plan.submission_format_digest.clone(),
        scoring_digest: plan.scoring_digest.clone(),
    })
}

/// Check every migration of a section against the deny-list.
fn check_all_migrations(
    plan: &SectionPlan,
    topic_id: &str,
) -> Result<Vec<(String, Vec<Statement>)>, InstallError> {
    if plan.migrations.len() > MAX_MIGRATIONS {
        return Err(InstallError::TooManyMigrations {
            count: plan.migrations.len(),
        });
    }
    let mut out = Vec::with_capacity(plan.migrations.len());
    for m in &plan.migrations {
        out.push((m.name.clone(), check_migration(&m.sql, topic_id)?));
    }
    Ok(out)
}

/// Map a store failure.
fn map_store(e: &StoreError) -> InstallError {
    InstallError::Store(e.to_string())
}

/// Read a topic's newest install row.
///
/// # Errors
///
/// [`InstallError::Db`].
pub async fn latest_install(
    pool: &PgPool,
    topic_id: &str,
) -> Result<Option<InstallRow>, InstallError> {
    #[allow(clippy::type_complexity)]
    let row: Option<(
        i64,
        String,
        String,
        String,
        String,
        Option<i32>,
        serde_json::Value,
        serde_json::Value,
        serde_json::Value,
        String,
    )> = sqlx::query_as(
        "SELECT id, topic_id, bundle_digest, environment, state, rules_version, rule_ids, \
                migrations, binding, detail \
         FROM proof_topic_install WHERE topic_id = $1 ORDER BY id DESC LIMIT 1",
    )
    .bind(topic_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| InstallError::Db(e.to_string()))?;
    let Some((
        id,
        topic_id,
        bundle_digest,
        environment,
        state,
        rules_version,
        rule_ids,
        migrations,
        binding,
        detail,
    )) = row
    else {
        return Ok(None);
    };
    Ok(Some(InstallRow {
        id,
        topic_id,
        bundle_digest,
        environment,
        state,
        rules_version: rules_version.and_then(|v| u32::try_from(v).ok()),
        rule_ids: strings(&rule_ids),
        migrations: strings(&migrations),
        binding,
        detail,
    }))
}

/// A JSON array of strings, as the journal stores it.
fn strings(value: &serde_json::Value) -> Vec<String> {
    value
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// What the newest install row says about the rules **it** landed.
///
/// The publish gate's answer, as a value rather than a boolean, so a caller
/// that has to explain *why* a topic is not ready reads the same fact the
/// boolean was derived from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstalledRules {
    /// The newest install is `applied` and the rule version **that install
    /// recorded** is `rlm`-sourced.
    RlmAuthored {
        /// The rule version the install landed.
        version: u32,
    },
    /// The newest install is `applied`, but the rule version it recorded is
    /// not RLM-authored (or its row is gone).
    NotRlmAuthored {
        /// The rule version the install recorded, when it recorded one.
        version: Option<u32>,
        /// `topic_document` / `operator` / `rlm`, or `None` when no row.
        provenance: Option<String>,
    },
    /// The install recorded an RLM-authored version, but a **different**
    /// version is in force and it is not RLM-authored.
    ///
    /// The other half of the same defect, from the opposite direction: the
    /// install landed the RLM's vector and a later `operator` edit superseded
    /// it. The topic would open with rules no RLM wrote — refused exactly as a
    /// document-sourced vector is.
    SupersededByOperator {
        /// The RLM-authored version the install recorded.
        installed: u32,
        /// The version now in force.
        in_force: u32,
        /// Its provenance (`operator`, or `topic_document`).
        provenance: String,
    },
    /// The newest install is not `applied`, or the topic has no install row.
    NotApplied {
        /// The state the journal holds (`pending` / `failed`), or `None` when
        /// no row exists at all.
        state: Option<String>,
    },
}

/// Whether `topic_id` has an install in the **`applied`** state.
///
/// This is the durable fact a publish of an `open` document is gated on. The
/// journal is append-only, so the newest row for a topic is its current
/// install state: `applied` means every migration, route, and rule the
/// operator's bundle carries is in place, and `pending` / `failed` mean it is
/// not.
///
/// The read is **fail-closed at the call site**: a database error is an
/// `Err`, never a `false` that a caller could mistake for "not installed" or
/// — worse, if inverted — for "installed". [`is_installed`] is the boolean
/// form, for callers that want it.
///
/// # Errors
///
/// [`InstallError::Db`].
pub async fn applied_install(pool: &PgPool, topic_id: &str) -> Result<bool, InstallError> {
    let state: Option<String> = sqlx::query_scalar(
        "SELECT state FROM proof_topic_install WHERE topic_id = $1 ORDER BY id DESC LIMIT 1",
    )
    .bind(topic_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| InstallError::Db(e.to_string()))?;
    Ok(state.as_deref() == Some(InstallState::Applied.as_str()))
}

/// The admission query's row: the newest install's state and recorded rule
/// version with that version's provenance, plus the version in force and its
/// provenance. A named alias rather than an inline tuple, because five
/// positional `Option`s read as noise at the call site.
type AdmissionRow = (
    String,
    Option<i32>,
    Option<String>,
    Option<i32>,
    Option<String>,
);

/// The provenance of the rule version the newest install **recorded**, and of
/// the version actually in force.
///
/// Two facts, read as **one statement** so they come from one snapshot:
///
/// 1. The newest install row is `applied`, and the rule version it recorded is
///    `rlm`-sourced (joined on
///    `proof_rule_version.version = proof_topic_install.rules_version`).
/// 2. The version **in force** is also `rlm`-sourced.
///
/// That pair is the gate, and each half closes a different way for an
/// operator-authored vector to reach a live topic:
///
/// - **Only (1)** would admit a topic whose install landed the RLM's version
///   while a later `operator` edit superseded it: the vector in force would be
///   one no RLM wrote.
/// - **Only (2)** — the original defect — would admit a topic whose install
///   landed the signed document's version 1 (`topic_document`) while an
///   unrelated later version 2 happened to be RLM-authored: the topic would
///   open with the operator's vector in force, which is the operator-cloned
///   document the gate exists to refuse.
///
/// Requiring both is not "the install's version must equal the one in force":
/// an RLM that rewrites its own rules after the install (version N → N+1, both
/// `rlm`) is exactly the autonomy this track wants, and it stays admitted.
///
/// Fail-closed: an `applied` row that recorded **no** rule version, a missing
/// rule row, and any non-`rlm` provenance are refusals, never an admission,
/// and a database error is an `Err`.
///
/// # Errors
///
/// [`InstallError::Db`].
pub async fn installed_rules(
    pool: &PgPool,
    topic_id: &str,
) -> Result<InstalledRules, InstallError> {
    // `LEFT JOIN` so an `applied` row whose rule version has no row is
    // visible as "no provenance" rather than as "no install". The two scalar
    // subqueries read the version in force in the same snapshot.
    let row: Option<AdmissionRow> = sqlx::query_as(
        "SELECT i.state, i.rules_version, r.source, \
                    f.version, f.source \
             FROM proof_topic_install i \
             LEFT JOIN proof_rule_version r \
               ON r.topic_id = i.topic_id AND r.version = i.rules_version \
             LEFT JOIN LATERAL ( \
                 SELECT version, source FROM proof_rule_version \
                 WHERE topic_id = i.topic_id ORDER BY version DESC LIMIT 1 \
             ) f ON true \
             WHERE i.topic_id = $1 \
             ORDER BY i.id DESC \
             LIMIT 1",
    )
    .bind(topic_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| InstallError::Db(e.to_string()))?;
    let Some((state, version, source, in_force_raw, in_force_source)) = row else {
        return Ok(InstalledRules::NotApplied { state: None });
    };
    if state != InstallState::Applied.as_str() {
        return Ok(InstalledRules::NotApplied { state: Some(state) });
    }
    let version = version.and_then(|v| u32::try_from(v).ok());
    let in_force = in_force_raw.and_then(|v| u32::try_from(v).ok());
    let Some(installed_version) = version else {
        // An `applied` row with no recorded rule version cannot be admitted:
        // there is no vector it can be shown to have landed.
        return Ok(InstalledRules::NotRlmAuthored {
            version: None,
            provenance: source,
        });
    };
    if source.as_deref() != Some(RULES_SOURCE_RLM) {
        return Ok(InstalledRules::NotRlmAuthored {
            version: Some(installed_version),
            provenance: source,
        });
    }
    if in_force_source.as_deref() == Some(RULES_SOURCE_RLM) {
        return Ok(InstalledRules::RlmAuthored {
            version: installed_version,
        });
    }
    // The install landed an RLM vector and something else is in force. A rule
    // row for the install's version exists (the join matched), so the version
    // in force exists too; the fallback is unreachable and only avoids a
    // panic in a read that must stay total.
    Ok(InstalledRules::SupersededByOperator {
        installed: installed_version,
        in_force: in_force.unwrap_or(installed_version),
        provenance: in_force_source.unwrap_or_else(|| "absent".to_owned()),
    })
}

/// The `proof_rule_version.source` value meaning the topic's own RLM wrote
/// the vector (the store's `RuleSource::Rlm` wire word).
pub const RULES_SOURCE_RLM: &str = "rlm";

/// [`applied_install`], as a plain boolean.
///
/// # Errors
///
/// [`InstallError::Db`].
pub async fn is_installed(pool: &PgPool, topic_id: &str) -> Result<bool, InstallError> {
    applied_install(pool, topic_id).await
}

/// Whether the topic's **newest** rule version was authored by its RLM.
///
/// The provenance half of the publish gate. `proof_rule_version.source` is
/// written by the store, not by a caller: `topic_document` means the vector is
/// still the operator's signed checklist (the install seeds it that way) and
/// `rlm` means the topic's own RLM authored it inside the topic VM. An
/// `operator` edit is an operator's vector too, so it does not count.
///
/// Read straight from the column so the answer does not depend on the rule
/// bodies still deserializing. A topic with **no** rule row is `false`: an
/// install always leaves one, so its absence means nothing was installed.
///
/// # Errors
///
/// [`InstallError::Db`].
pub async fn rlm_authored_rules(pool: &PgPool, topic_id: &str) -> Result<bool, InstallError> {
    let source: Option<String> = sqlx::query_scalar(
        "SELECT source FROM proof_rule_version WHERE topic_id = $1 ORDER BY version DESC LIMIT 1",
    )
    .bind(topic_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| InstallError::Db(e.to_string()))?;
    Ok(source.as_deref() == Some("rlm"))
}

/// [`rlm_authored_rules`] as the operator-facing provenance word.
///
/// `None` when the topic has no rule row at all. Used by the gate refusals so
/// an operator reads *which* provenance blocked the publish rather than only
/// that one did.
///
/// # Errors
///
/// [`InstallError::Db`].
pub async fn rules_source(pool: &PgPool, topic_id: &str) -> Result<Option<String>, InstallError> {
    sqlx::query_scalar(
        "SELECT source FROM proof_rule_version WHERE topic_id = $1 ORDER BY version DESC LIMIT 1",
    )
    .bind(topic_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| InstallError::Db(e.to_string()))
}

/// Every route a topic registered, for the dynamic mux.
///
/// A stored path is **relative**, so the caller owns the prefix and a topic
/// can never claim a route outside it.
///
/// # Errors
///
/// [`InstallError::Db`].
pub async fn topic_routes(pool: &PgPool, topic_id: &str) -> Result<Vec<ApiRoute>, InstallError> {
    let rows: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT path, method, summary FROM proof_topic_api \
         WHERE topic_id = $1 ORDER BY path, method",
    )
    .bind(topic_id)
    .fetch_all(pool)
    .await
    .map_err(|e| InstallError::Db(e.to_string()))?;
    Ok(rows
        .into_iter()
        .map(|(path, method, summary)| ApiRoute {
            path,
            method,
            summary,
        })
        .collect())
}

/// The install journal, newest first: `(topic, environment, state, digest)`.
///
/// # Errors
///
/// [`InstallError::Db`].
pub async fn install_history(
    pool: &PgPool,
    limit: i64,
) -> Result<Vec<(String, String, String, String)>, InstallError> {
    let rows: Vec<(String, String, String, String)> = sqlx::query_as(
        "SELECT topic_id, environment, state, bundle_digest FROM proof_topic_install \
         ORDER BY id DESC LIMIT $1",
    )
    .bind(limit.clamp(1, 500))
    .fetch_all(pool)
    .await
    .map_err(|e| InstallError::Db(e.to_string()))?;
    Ok(rows)
}
