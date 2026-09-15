//! The **registry** half of the CLI: reading what is installed, and the
//! operator gate.
//!
//! `topic list` / `show` / `install-log` and `topic disable` / `enable` are
//! all one shape — open the topic database, read or append one row, print it —
//! so they live together here rather than in `main.rs`, which is at the
//! repository's per-crate LOC cap. The *decision* logic they depend on (what
//! an install journal means, what a gate row is) stays in
//! `proof_topic_install`; this module is the CLI's reading and writing of it.
//!
//! Everything here is **fail-closed on the database**: a configured but
//! unreachable database is fatal, because falling back to an empty in-memory
//! view would report "nothing installed" for a host that has topics.

use proof_rlm_store::{MemoryRlmStore, PgRlmStore, RlmStore, TopicVersionRow};

use crate::{Failure, Options};

pub(crate) async fn cmd_list(opts: &Options) -> Result<(), Failure> {
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

pub(crate) async fn cmd_show(opts: &Options, topic_id: &str) -> Result<(), Failure> {
    let pool = open_pool(opts).await?;
    let store = PgRlmStore::new(pool.clone());
    // An alias resolves to its canonical slug first, so `show <alias>` finds
    // the topic it points at. Resolution is fail-closed in the store: an alias
    // whose topic has
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
pub(crate) fn print_gate(gate: Option<&proof_topic_install::Gate>, topic_id: &str) {
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
pub(crate) async fn open_store(opts: &Options) -> Result<Box<dyn RlmStore>, Failure> {
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
pub(crate) async fn open_pool(opts: &Options) -> Result<sqlx::PgPool, Failure> {
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
pub(crate) async fn cmd_gate(
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

pub(crate) fn print_row(row: &TopicVersionRow) {
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
pub(crate) fn summarize(row: &TopicVersionRow) -> String {
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
pub(crate) fn status_word(status: proof_task::TopicStatus) -> &'static str {
    match status {
        proof_task::TopicStatus::Draft => "draft",
        proof_task::TopicStatus::Open => "open",
        proof_task::TopicStatus::Closed => "closed",
    }
}

pub(crate) fn topic_json(row: &TopicVersionRow) -> serde_json::Value {
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

pub(crate) fn dash_if_empty(s: &str) -> String {
    if s.trim().is_empty() {
        "-".to_owned()
    } else {
        s.to_owned()
    }
}
