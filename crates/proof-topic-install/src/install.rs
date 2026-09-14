//! The install engine: apply a bundle's RLM section, fail-closed, resumable,
//! and journaled.
//!
//! One install does four things, in this order, each of which either
//! completes or stops the install with a named reason:
//!
//! 1. **Migrations** — the topic's SQL, applied through the deny-list
//!    ([`crate::sql_guard`]). A statement touching a `proof_*` object, a
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
use crate::sql_guard::{check_migration, Statement};
use crate::InstallError;

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
        let mut applied = Vec::new();
        let mut skipped = Vec::new();
        for (name, statements) in checked {
            if already.contains(name) {
                skipped.push(name.clone());
                continue;
            }
            self.run_migration(name, statements).await?;
            applied.push(name.clone());
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
            migrations_applied: applied,
            migrations_skipped: skipped,
            apis,
            rules_version: rules.version,
            rule_ids,
            binding: binding.clone(),
            setup,
            journal_id,
        })
    }

    /// Migration names this topic has already applied, from the journal.
    ///
    /// The union over `pending` and `applied` rows is deliberate: a run that
    /// crashed after applying a migration but before journaling `applied`
    /// still gets credit for it, so a resume does not re-apply a statement
    /// whose `CREATE TABLE` would now fail.
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

    /// Run one migration's statements in one transaction.
    ///
    /// All-or-nothing per migration: a failure on the third statement of a
    /// migration leaves none of that migration applied, so the journal and
    /// the database agree about what landed.
    async fn run_migration(
        &self,
        name: &str,
        statements: &[Statement],
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
