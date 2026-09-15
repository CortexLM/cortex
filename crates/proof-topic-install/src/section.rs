//! Reading the RLM section: the parts an install applies, and nothing else.
//!
//! The bundle carries its `rlm` section **verbatim** and opaque
//! ([`proof_topic_bundle::RlmSection`]): the bundle crate checks the shape
//! and hands the bytes over, so nothing about a topic is compiled in.
//!
//! The install is the consumer. To *apply* the section it has to read the
//! parts it knows how to apply, and that is what this module does — strictly,
//! and only for the parts named here:
//!
//! | Part | What the install does with it |
//! |------|-------------------------------|
//! | `migrations` | shape-checked, then executed under the SQL deny-list ([`proof_topic_sql_guard`]) |
//! | `apis` | recorded as topic-scoped routes ([`crate::install`]) |
//! | `rules` | installed as the topic's first rule version ([`crate::install`]) |
//! | `submission_format` | shape-checked and recorded; never interpreted |
//! | `scoring` | shape-checked and recorded; never interpreted |
//! | `handler` | allow-listed ([`crate::handler`]) |
//!
//! # Strictness, and why it is per-part
//!
//! An **unknown key inside a part this module reads** is refused. A part is
//! a step list: a `{"name": …, "sq": …}` migration whose `sql` this build
//! cannot see is a step nothing performs, and silently skipping it would
//! install a topic that is not the one the operator signed off. Refusing
//! costs an operator one edit.
//!
//! A **part this module has never heard of** is carried, not refused: that is
//! the bundle's own rule (`RlmSection`), and it is the whole point of the
//! boundary — a future part must not need a code change here to travel. Only
//! the parts listed above are read; the rest goes into the install record as
//! the RLM's business.
//!
//! Nothing here decides what a rule, a migration, an API, a submission
//! format, or a scoring function *means*. `submission_format` and `scoring`
//! are recorded as canonical JSON digests so an audit can prove which ones a
//! topic was installed with, and are otherwise untouched.

use proof_canon::is_custom_id;
use proof_task::ChecklistRule;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::handler::{check_handler, Handler};
use crate::InstallError;

/// Keys this module reads out of the RLM section.
pub const READ_KEYS: [&str; 6] = [
    "apis",
    "handler",
    "migrations",
    "rules",
    "scoring",
    "submission_format",
];

/// Longest one migration's SQL may be, in bytes.
pub const MAX_MIGRATION_SQL_BYTES: usize = 256 * 1024;

/// Most migrations one install may apply.
pub const MAX_MIGRATIONS: usize = 64;

/// Most routes one topic may register.
pub const MAX_APIS: usize = 64;

/// Longest route summary, in characters.
pub const MAX_API_SUMMARY_CHARS: usize = 256;

/// One SQL migration the topic's install applies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Migration {
    /// Operator-facing name, an id (`[a-z0-9][a-z0-9_-]{1,63}`).
    pub name: String,
    /// The SQL. Applied under the deny-list; never logged in full.
    pub sql: String,
}

/// One route the topic registers for itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiRoute {
    /// Path relative to the topic's own prefix: no leading `/`, no `..`.
    pub path: String,
    /// Upper-cased HTTP method, or `*`.
    pub method: String,
    /// What the route does, in the topic's words.
    pub summary: String,
}

/// The parts of an RLM section an install applies.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SectionPlan {
    /// Migrations, in bundle order.
    pub migrations: Vec<Migration>,
    /// Routes the topic claims.
    pub apis: Vec<ApiRoute>,
    /// Rule vector the install lands as version 1.
    pub rules: Vec<ChecklistRule>,
    /// Canonical-JSON digest of `submission_format`, when the bundle carries one.
    pub submission_format_digest: Option<String>,
    /// Canonical-JSON digest of `scoring`, when the bundle carries one.
    pub scoring_digest: Option<String>,
    /// Allow-listed handler the section named, when it named one.
    pub handler: Option<Handler>,
    /// Part names the section carried that this module does not read, sorted.
    /// They travel into the install record as the RLM's business.
    pub carried_unknown: Vec<String>,
}

impl SectionPlan {
    /// Whether the section asks for nothing this install applies.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.migrations.is_empty()
            && self.apis.is_empty()
            && self.rules.is_empty()
            && self.submission_format_digest.is_none()
            && self.scoring_digest.is_none()
            && self.handler.is_none()
    }
}

/// Refuse with the part and the reason.
fn bad(part: &str, why: impl Into<String>) -> InstallError {
    InstallError::Section {
        part: part.to_owned(),
        why: why.into(),
    }
}

/// A JSON value's kind, for an error that says what arrived.
fn kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

/// One field of an object, refusing an unknown key by name.
fn take<'a>(
    obj: &'a serde_json::Map<String, Value>,
    key: &str,
    part: &str,
    allowed: &[&str],
) -> Result<Option<&'a Value>, InstallError> {
    for k in obj.keys() {
        if !allowed.contains(&k.as_str()) {
            return Err(bad(
                part,
                format!(
                    "{k:?} is not a key this build reads here (it reads {}); a field the install \
                     cannot apply is a step nothing performs, so it is refused rather than \
                     skipped",
                    allowed.join(", ")
                ),
            ));
        }
    }
    Ok(obj.get(key))
}

/// A required string field.
fn string_field(
    obj: &serde_json::Map<String, Value>,
    key: &str,
    part: &str,
) -> Result<String, InstallError> {
    match obj.get(key) {
        Some(Value::String(s)) => Ok(s.clone()),
        Some(other) => Err(bad(
            part,
            format!("{key} must be a string, got {}", kind(other)),
        )),
        None => Err(bad(part, format!("{key} is required"))),
    }
}

/// Read a section's migrations.
///
/// # Errors
///
/// [`InstallError::Section`] naming the migration ordinal and the problem.
pub fn read_migrations(value: &Value) -> Result<Vec<Migration>, InstallError> {
    let Some(items) = value.as_array() else {
        return Err(bad(
            "migrations",
            format!("must be an array, got {}", kind(value)),
        ));
    };
    if items.len() > MAX_MIGRATIONS {
        return Err(bad(
            "migrations",
            format!(
                "carries {} migrations, at most {MAX_MIGRATIONS} are applied",
                items.len()
            ),
        ));
    }
    let mut out = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        let part = format!("migrations[{i}]");
        let Some(obj) = item.as_object() else {
            return Err(bad(&part, format!("must be an object, got {}", kind(item))));
        };
        take(obj, "", &part, &["name", "sql"])?;
        let name = string_field(obj, "name", &part)?;
        if !is_custom_id(&name) {
            return Err(bad(
                &part,
                format!("name {name:?} is not an id ([a-z0-9][a-z0-9_-]{{1,63}})"),
            ));
        }
        let sql = string_field(obj, "sql", &part)?;
        if sql.trim().is_empty() {
            return Err(bad(&part, "sql is empty; remove the migration instead"));
        }
        if sql.len() > MAX_MIGRATION_SQL_BYTES {
            return Err(bad(
                &part,
                format!(
                    "sql is {} bytes, at most {MAX_MIGRATION_SQL_BYTES} are applied",
                    sql.len()
                ),
            ));
        }
        out.push(Migration { name, sql });
    }
    Ok(out)
}

/// Read a section's routes.
///
/// # Errors
///
/// [`InstallError::Section`] naming the route ordinal and the problem.
pub fn read_apis(value: &Value) -> Result<Vec<ApiRoute>, InstallError> {
    let Some(items) = value.as_array() else {
        return Err(bad(
            "apis",
            format!("must be an array, got {}", kind(value)),
        ));
    };
    if items.len() > MAX_APIS {
        return Err(bad(
            "apis",
            format!(
                "carries {} routes, at most {MAX_APIS} may be registered",
                items.len()
            ),
        ));
    }
    let mut out: Vec<ApiRoute> = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        let part = format!("apis[{i}]");
        let Some(obj) = item.as_object() else {
            return Err(bad(&part, format!("must be an object, got {}", kind(item))));
        };
        take(obj, "", &part, &["path", "method", "summary"])?;
        let path = string_field(obj, "path", &part)?;
        if !is_relative_api_path(&path) {
            return Err(bad(
                &part,
                format!(
                    "path {path:?} must be a relative path of plain segments (no leading '/', \
                     no '..', no empty segment): a topic's routes live under its own prefix, and \
                     the prefix is the control plane's to set"
                ),
            ));
        }
        if is_reserved_api_path(&path) {
            return Err(bad(
                &part,
                format!(
                    "path {path:?} is inside the challenge's admin namespace ({}), which is not a \
                     topic's to claim: a topic route that reads like an operator route is a route \
                     a reader cannot tell apart from the real one. Register a different path.",
                    RESERVED_API_PREFIXES.join(", ")
                ),
            ));
        }
        let method = string_field(obj, "method", &part)?
            .trim()
            .to_ascii_uppercase();
        if !is_api_method(&method) {
            return Err(bad(
                &part,
                format!("method {method:?} must be one of GET, POST, PUT, PATCH, DELETE, *"),
            ));
        }
        let summary = match obj.get("summary") {
            None | Some(Value::Null) => String::new(),
            Some(Value::String(s)) => s.trim().to_owned(),
            Some(other) => {
                return Err(bad(
                    &part,
                    format!("summary must be a string, got {}", kind(other)),
                ))
            }
        };
        if summary.chars().count() > MAX_API_SUMMARY_CHARS {
            return Err(bad(
                &part,
                format!("summary is longer than {MAX_API_SUMMARY_CHARS} chars"),
            ));
        }
        out.push(ApiRoute {
            path,
            method,
            summary,
        });
    }
    Ok(out)
}

/// A relative path of plain segments: no leading `/`, no `.` / `..`, no empty
/// segment, no control characters, no backslash.
///
/// Mirrors `proof_experiment`'s pack-path rule, for the same reason: the
/// value becomes part of a route the control plane serves, so a `..` or a
/// leading slash would let a topic step outside the prefix it was given.
#[must_use]
pub fn is_relative_api_path(p: &str) -> bool {
    let p = p.trim();
    !p.is_empty()
        && p.len() <= 512
        && !p.starts_with('/')
        && !p.ends_with('/')
        && !p
            .chars()
            .any(|c| c.is_control() || c == '\\' || c == '?' || c == '#')
        && p.split('/')
            .all(|seg| !seg.is_empty() && seg != "." && seg != "..")
        && p.split('/').all(|seg| {
            seg.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '~' | '-'))
        })
}

/// A method a topic may claim.
#[must_use]
pub fn is_api_method(m: &str) -> bool {
    matches!(m, "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "*")
}

/// Path prefixes inside a topic's own namespace that are **not a topic's to
/// claim**: the challenge's operator surface.
///
/// A topic route is served under the topic's prefix
/// (`/challenge/{topic_id}/{path}`), so a stored `v1/admin/…` would answer at
/// `/challenge/{topic_id}/v1/admin/…` — a path a reader cannot tell apart
/// from the challenge's own admin surface, which is master-local. The install
/// refuses to record one, and the mux refuses to resolve one that is already
/// in the table (a row written before this rule existed).
pub const RESERVED_API_PREFIXES: [&str; 1] = ["v1/admin"];

/// Whether `p` is inside a [`RESERVED_API_PREFIXES`] namespace.
///
/// Segment-aware: `v1/admin` and `v1/admin/…` are reserved, `v1/administrator`
/// is not.
#[must_use]
pub fn is_reserved_api_path(p: &str) -> bool {
    let p = p.trim();
    RESERVED_API_PREFIXES.iter().any(|prefix| {
        p == *prefix
            || p.strip_prefix(prefix)
                .is_some_and(|rest| rest.starts_with('/'))
    })
}

/// Read a section's rule vector.
///
/// # Errors
///
/// [`InstallError::Section`] naming the rule ordinal and the problem. The
/// shared shape check ([`proof_canon::validate_rules`]) runs too, so a vector
/// the scoring path would refuse cannot be installed.
pub fn read_rules(value: &Value) -> Result<Vec<ChecklistRule>, InstallError> {
    let Some(items) = value.as_array() else {
        return Err(bad(
            "rules",
            format!("must be an array, got {}", kind(value)),
        ));
    };
    let mut out = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        let part = format!("rules[{i}]");
        let Some(obj) = item.as_object() else {
            return Err(bad(&part, format!("must be an object, got {}", kind(item))));
        };
        take(obj, "", &part, &["id", "text"])?;
        let id = string_field(obj, "id", &part)?;
        let text = string_field(obj, "text", &part)?;
        out.push(ChecklistRule { id, text });
    }
    // The same shape check the scoring path runs, so a vector it would refuse
    // cannot be installed. `ShapeError` carries its own fields rather than a
    // `Display`, so the message is built from them.
    proof_canon::validate_rules(&out)
        .map_err(|e| bad("rules", format!("{}: {}", e.field, e.why)))?;
    Ok(out)
}

/// Canonical-JSON digest of a part this module records but does not interpret.
///
/// Canonical, so the digest is stable across key order and formatting: an
/// audit can compare it to the bundle the operator signed off.
fn digest_of(value: &Value) -> String {
    let canonical = proof_canon::canonical_json(value);
    let mut hasher = Sha256::new();
    hasher.update(canonical.as_bytes());
    format!("sha256:{}", hex_encode(&hasher.finalize()))
}

/// Lower-case hex.
fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

/// Read an RLM section's raw text into the parts an install applies.
///
/// # Errors
///
/// [`InstallError::Section`] for a part that is malformed, carries a key this
/// build does not read, or names a handler outside the allow-list.
pub fn read_section(raw: &str) -> Result<SectionPlan, InstallError> {
    let parsed: serde_json::Map<String, Value> =
        serde_json::from_str(raw).map_err(|e| InstallError::Section {
            part: "rlm".to_owned(),
            why: format!("parse: {e}"),
        })?;
    let mut plan = SectionPlan::default();
    for (key, value) in &parsed {
        match key.as_str() {
            "migrations" => plan.migrations = read_migrations(value)?,
            "apis" => plan.apis = read_apis(value)?,
            "rules" => plan.rules = read_rules(value)?,
            "handler" => {
                let Some(name) = value.as_str() else {
                    return Err(bad(
                        "handler",
                        format!("must be a string, got {}", kind(value)),
                    ));
                };
                plan.handler = Some(check_handler(name)?);
            }
            "submission_format" => {
                if !value.is_object() {
                    return Err(bad(
                        "submission_format",
                        format!("must be an object, got {}", kind(value)),
                    ));
                }
                plan.submission_format_digest = Some(digest_of(value));
            }
            "scoring" => {
                if !value.is_object() {
                    return Err(bad(
                        "scoring",
                        format!("must be an object, got {}", kind(value)),
                    ));
                }
                plan.scoring_digest = Some(digest_of(value));
            }
            other => plan.carried_unknown.push(other.to_owned()),
        }
    }
    plan.carried_unknown.sort();
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_full_section_reads_every_part_this_install_applies() {
        let plan = read_section(
            r#"{"rules": [{"id": "no_short_circuit", "text": "run the task"}],
                "migrations": [{"name": "0001_scratch", "sql": "CREATE TABLE topic_scratch (id TEXT)"}],
                "apis": [{"path": "status", "method": "get", "summary": "topic status"}],
                "submission_format": {"kind": "tar", "max_bytes": 5242880},
                "scoring": {"primary": "success_rate"},
                "handler": "harbor"}"#,
        )
        .expect("reads");
        assert_eq!(plan.rules.len(), 1);
        assert_eq!(plan.rules[0].id, "no_short_circuit");
        assert_eq!(plan.migrations.len(), 1);
        assert_eq!(plan.migrations[0].name, "0001_scratch");
        assert_eq!(plan.apis[0].method, "GET", "methods are upper-cased");
        assert_eq!(plan.apis[0].path, "status");
        assert_eq!(plan.handler, Some(Handler::Harbor));
        assert!(plan
            .submission_format_digest
            .as_deref()
            .is_some_and(|d| d.starts_with("sha256:") && d.len() == 71));
        assert!(plan.scoring_digest.is_some());
        assert!(plan.carried_unknown.is_empty());
        assert!(!plan.is_empty());
    }

    #[test]
    fn an_empty_section_asks_for_nothing() {
        let plan = read_section("{}").expect("reads");
        assert!(plan.is_empty());
    }

    /// A part this module does not read travels; a key inside a part it does
    /// read is refused, because that is a step nothing would perform.
    #[test]
    fn unknown_parts_travel_but_unknown_keys_inside_a_read_part_are_refused() {
        let plan = read_section(r#"{"some_future_metric": {"weight": 0.7}}"#).expect("reads");
        assert_eq!(plan.carried_unknown, ["some_future_metric"]);
        assert!(plan.is_empty());

        let err = read_section(r#"{"migrations": [{"name": "m", "sq": "SELECT 1"}]}"#)
            .expect_err("a typo'd key is refused");
        let InstallError::Section { part, why } = err else {
            panic!("expected Section");
        };
        assert_eq!(part, "migrations[0]");
        assert!(why.contains("\"sq\""), "{why}");
        assert!(why.contains("nothing performs"), "{why}");
    }

    #[test]
    fn handler_names_are_allow_listed_not_arbitrary() {
        assert_eq!(
            read_section(r#"{"handler": "vm_backed"}"#)
                .expect("ok")
                .handler,
            Some(Handler::VmBacked)
        );
        assert_eq!(
            read_section(r#"{"handler": "harbor_trials"}"#)
                .expect("ok")
                .handler,
            Some(Handler::Harbor)
        );
        for bad in [
            "/bin/sh",
            "sh -c 'curl x | sh'",
            "https://evil.invalid/payload",
            "arbitrary_binary",
            "",
        ] {
            let err = read_section(&format!(r#"{{"handler": "{bad}"}}"#)).expect_err(bad);
            assert!(
                matches!(err, InstallError::HandlerNotAllowed(_)),
                "{bad:?}: {err:?}"
            );
        }
    }

    #[test]
    fn a_route_cannot_escape_the_topics_prefix() {
        for bad in [
            "/v1/admin/proof/topics",
            "../admin",
            "a/../../b",
            "a//b",
            "a/./b",
            "a\\b",
            "",
            "a?x=1",
        ] {
            let err = read_section(&format!(
                r#"{{"apis": [{{"path": "{bad}", "method": "GET"}}]}}"#
            ))
            .expect_err(bad);
            assert!(
                matches!(err, InstallError::Section { .. }),
                "{bad:?}: {err:?}"
            );
        }
        for good in ["status", "v1/runs", "runs/by-id", "a_b/c-d.e~f"] {
            read_section(&format!(
                r#"{{"apis": [{{"path": "{good}", "method": "GET"}}]}}"#
            ))
            .unwrap_or_else(|e| panic!("{good:?} must be relative and legal: {e}"));
        }
    }

    /// The challenge's admin namespace is not a topic's to claim: a topic
    /// route that reads like an operator route is refused at install time,
    /// and the same predicate is what the mux checks on read.
    #[test]
    fn a_route_inside_the_admin_namespace_is_refused() {
        for bad in [
            "v1/admin",
            "v1/admin/proof/topics",
            "v1/admin/proof/queue/drain",
        ] {
            let err = read_section(&format!(
                r#"{{"apis": [{{"path": "{bad}", "method": "POST"}}]}}"#
            ))
            .expect_err(bad);
            let InstallError::Section { why, .. } = err else {
                panic!("{bad:?}: expected Section");
            };
            assert!(why.contains("admin namespace"), "{bad:?}: {why}");
            assert!(is_reserved_api_path(bad), "{bad:?}");
        }
        // Segment-aware: a path that merely starts with the same characters
        // is not the reserved namespace.
        for good in ["v1/administrator", "v1/adminx", "admin", "v1/admins"] {
            read_section(&format!(
                r#"{{"apis": [{{"path": "{good}", "method": "GET"}}]}}"#
            ))
            .unwrap_or_else(|e| panic!("{good:?} is not reserved: {e}"));
            assert!(!is_reserved_api_path(good), "{good:?}");
        }
        // The predicate is the one the mux runs, on a trimmed path.
        assert!(is_reserved_api_path(" v1/admin/x "));
    }

    #[test]
    fn a_rule_vector_the_scoring_path_would_refuse_is_refused_here() {
        let err =
            read_section(r#"{"rules": [{"id": "Bad Id", "text": "x"}]}"#).expect_err("bad rule id");
        assert!(matches!(err, InstallError::Section { .. }), "{err:?}");
        let err =
            read_section(r#"{"rules": [{"id": "a_b", "text": ""}]}"#).expect_err("empty text");
        assert!(matches!(err, InstallError::Section { .. }), "{err:?}");
        read_section(r#"{"rules": [{"id": "a_b", "text": "ok"}]}"#).expect("legal vector");
    }

    #[test]
    fn bounds_are_enforced_on_every_read_part() {
        let many = json!({
            "migrations": (0..=MAX_MIGRATIONS)
                .map(|i| json!({"name": format!("m{i}"), "sql": "SELECT 1"}))
                .collect::<Vec<_>>()
        });
        assert!(read_section(&many.to_string()).is_err());
        let many = json!({
            "apis": (0..=MAX_APIS)
                .map(|i| json!({"path": format!("p{i}"), "method": "GET"}))
                .collect::<Vec<_>>()
        });
        assert!(read_section(&many.to_string()).is_err());
        let huge =
            json!({"migrations": [{"name": "m", "sql": "x".repeat(MAX_MIGRATION_SQL_BYTES + 1)}]});
        assert!(read_section(&huge.to_string()).is_err());
    }

    /// The digests are stable across key order and formatting, because they
    /// are over canonical JSON — an audit can compare them to the bundle.
    #[test]
    fn recorded_digests_are_canonical_and_stable() {
        let a =
            read_section(r#"{"submission_format": {"kind": "tar", "max_bytes": 5}}"#).expect("a");
        let b =
            read_section(r#"{"submission_format": {"max_bytes": 5, "kind": "tar"}}"#).expect("b");
        assert_eq!(a.submission_format_digest, b.submission_format_digest);
        let c =
            read_section(r#"{"submission_format": {"kind": "tar", "max_bytes": 6}}"#).expect("c");
        assert_ne!(a.submission_format_digest, c.submission_format_digest);
    }

    #[test]
    fn a_part_of_the_wrong_kind_is_refused_not_coerced() {
        for (raw, part) in [
            (r#"{"rules": "nope"}"#, "rules"),
            (r#"{"migrations": 3}"#, "migrations"),
            (r#"{"apis": {}}"#, "apis"),
            (r#"{"submission_format": []}"#, "submission_format"),
            (r#"{"scoring": "nope"}"#, "scoring"),
            (r#"{"handler": 7}"#, "handler"),
        ] {
            let err = read_section(raw).expect_err(part);
            let InstallError::Section { part: got, .. } = err else {
                panic!("{part}: expected Section, got {err:?}");
            };
            assert_eq!(got, part, "{raw}");
        }
    }
}
