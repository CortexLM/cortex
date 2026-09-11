//! Canonical JSON, identifier shapes, and the generic document shapes shared
//! by the Proof crates: the `{id, text}` anti-cheat rule and the topic
//! `constraints` block.
//!
//! Nothing here names a challenge, a metric, a model, or a repository. These
//! are the byte-level rules every Proof document is checked against —
//! canonical signing form, hex digests, http origins, the slug shapes a
//! signed topic may mint for its own ids, and the shape (never the values)
//! of the bindings a topic carries — so the signer, the verifier, the store,
//! the harvest, and the RLM engine all agree on them.

#![forbid(unsafe_code)]
#![allow(clippy::doc_markdown, clippy::must_use_candidate)]

mod canonical;
mod miner_env;

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

pub use canonical::canonical_json;
pub use miner_env::{
    is_env_name, MinerEnv, MinerEnvError, MAX_ENV_NAME_LEN, MAX_MINER_ENV_VALUE_LEN,
    MAX_MINER_ENV_VARS, PARAM_INJECT_MINER_ENV_SISTER, PARAM_MINER_BYOK, PARAM_MINER_ENV_ALLOWLIST,
    RESERVED_ENV_NAMES, RESERVED_ENV_PREFIX,
};

/// Most anti-cheat rules one topic may carry.
pub const MAX_CHECKLIST_RULES: usize = 64;

/// Longest rule text.
pub const MAX_RULE_TEXT_LEN: usize = 2_048;

/// Most opaque constraint params one topic may carry.
pub const MAX_CONSTRAINT_PARAMS: usize = 32;

/// `constraints.params` key that **defers scoring** on an open topic.
///
/// `"true"` keeps the topic `open` — submissions are validated and persisted
/// as `queued` — but nothing is evaluated: no harvest rent, no topic VM, no
/// judge call, until the operator re-publishes the topic without the flag
/// and the queue is drained. This is how a topic accepts artefacts while its
/// baseline / harness is still being installed. It is not `draft` (a draft
/// answers 400) and it is not a scorer switch: `"false"` and an absent key
/// are the same thing. Any other spelling is a publish reject. A value, like
/// every param, that travels in the signed document — never host state.
pub const PARAM_DEFER_SCORING: &str = "defer_scoring";

/// `constraints.params` key that **requires a non-empty training manifest**.
///
/// Harvest families (`nll` / `throughput`) already require declared
/// `train_content_hashes` or `train_dataset_ids` — that is the contamination
/// evidence for a training recipe. Custom / agent topics have no training
/// step, so an empty manifest is accepted unless this flag is `"true"`.
/// `"false"` skips the empty-manifest gate on any family (a topic that
/// does not train). Holdout overlap in a *declared* manifest is always a
/// contamination reject, flag or not. `"false"` and an absent key are
/// **not** the same: absent follows the family default. Any other spelling
/// is a publish reject.
pub const PARAM_REQUIRE_TRAINING_EVIDENCE: &str = "require_training_evidence";

/// Why a shared shape is malformed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShapeError {
    /// Offending field (`constraints.model_pin`, `checklist[id]`, …).
    pub field: String,
    /// What is wrong.
    pub why: &'static str,
}

/// Machine-checkable constraints the eval image / topic runner enforces.
///
/// `deny_unknown_fields` is the point: a constraint this control plane does
/// not understand is a constraint nothing can be trusted to enforce, so an
/// unknown key rejects the topic at publish instead of being ignored. The
/// knobs are generic policy; their **values** come from the signed document,
/// never from a catalog. Knobs added after the first signed topics are
/// omitted from the signed payload when unset, so those documents keep
/// verifying (same rule as `eval_executor`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
#[allow(clippy::struct_excessive_bools)]
pub struct Constraints {
    /// No `InfiniBand` fabric.
    pub no_infiniband: bool,
    /// No NVLink between ranks.
    pub no_nvlink: bool,
    /// No NCCL all-reduce over a fast fabric.
    pub no_nccl_fast_fabric: bool,
    /// Inter-node (or emulated inter-rank) bandwidth cap in Gbit/s.
    pub max_inter_node_gbps: Option<f64>,
    /// Sandbox policy: miner code runs only inside a Firecracker guest under
    /// the topic's isolated VM, never on the control-plane host.
    #[serde(skip_serializing_if = "<&bool as std::ops::Not>::not")]
    pub firecracker_required: bool,
    /// Provider model id (`vendor/model[:tag]`, or `openrouter/vendor/model[:tag]`)
    /// every paid inference call made by the runner or the miner harness must name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_pin: Option<String>,
    /// Opaque task-slice label the runner interprets (the control plane does not).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_slice: Option<String>,
    /// Opaque runner params (bounded). Keys and values are topic data.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, String>,
}

impl Constraints {
    /// Shape check of the generic knobs (values are never interpreted).
    ///
    /// # Errors
    ///
    /// [`ShapeError`] naming the first malformed knob.
    pub fn validate_shape(&self) -> Result<(), ShapeError> {
        let bad = |field: &str, why| {
            Err(ShapeError {
                field: format!("constraints.{field}"),
                why,
            })
        };
        if !self.model_pin.as_deref().is_none_or(is_model_pin) {
            return bad(
                "model_pin",
                "provider/model[:tag] (OpenRouter: openrouter/vendor/model)",
            );
        }
        if !self.task_slice.as_deref().is_none_or(is_opaque_param) {
            return bad("task_slice", "single printable line, <=256 chars");
        }
        if self.params.len() > MAX_CONSTRAINT_PARAMS
            || self
                .params
                .iter()
                .any(|(k, v)| !is_custom_id(k) || !is_opaque_param(v))
        {
            return bad("params", "<=32 slug keys with printable values");
        }
        for key in [PARAM_DEFER_SCORING, PARAM_REQUIRE_TRAINING_EVIDENCE] {
            if self
                .params
                .get(key)
                .is_some_and(|v| parse_bool_param(v).is_none())
            {
                return bad(&format!("params.{key}"), "\"true\" or \"false\"");
            }
        }
        self.validate_miner_env()?;
        Ok(())
    }

    /// Whether the signed topic defers scoring
    /// (`params.defer_scoring = "true"`; see [`PARAM_DEFER_SCORING`]).
    /// Absent, `"false"`, or — for a document that skipped
    /// [`Self::validate_shape`] — malformed all read as **not** deferred, so
    /// the flag can only ever hold evaluation back, never unlock it.
    pub fn defer_scoring(&self) -> bool {
        self.params
            .get(PARAM_DEFER_SCORING)
            .and_then(|v| parse_bool_param(v))
            .unwrap_or(false)
    }

    /// Explicit `require_training_evidence` param (`Some(true/false)`), or
    /// `None` when the key is absent or — for a document that skipped
    /// [`Self::validate_shape`] — malformed. The family default lives on the
    /// topic document, not here: this crate does not name metric families.
    #[must_use]
    pub fn training_evidence_param(&self) -> Option<bool> {
        self.params
            .get(PARAM_REQUIRE_TRAINING_EVIDENCE)
            .and_then(|v| parse_bool_param(v))
    }
}

/// `"true"` / `"false"` (trimmed, ASCII case-insensitive); anything else `None`.
fn parse_bool_param(v: &str) -> Option<bool> {
    match v.trim().to_ascii_lowercase().as_str() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

/// One anti-cheat rule the RLM ticks before any paid inference spend.
///
/// Rules are a **vector carried by the signed topic** (and re-versioned in
/// the store when the topic's RLM rewrites them). This crate knows the shape
/// only; it never ships a rule list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChecklistRule {
    /// Rule id (`[a-z0-9][a-z0-9_-]{1,63}`), unique within the vector.
    pub id: String,
    /// What an inspector must show for the rule to pass (English).
    pub text: String,
}

/// Shape check for a rule vector: bounded, unique slug ids, bounded text.
///
/// # Errors
///
/// [`ShapeError`] naming the first offending rule (`checklist[id]`).
pub fn validate_rules(rules: &[ChecklistRule]) -> Result<(), ShapeError> {
    let bad = |id: &str, why| ShapeError {
        field: format!("checklist[{id}]"),
        why,
    };
    if rules.len() > MAX_CHECKLIST_RULES {
        return Err(bad("", "too many rules"));
    }
    for (i, r) in rules.iter().enumerate() {
        let text = r.text.trim();
        if !is_custom_id(&r.id) {
            return Err(bad(&r.id, "id must match [a-z0-9][a-z0-9_-]{1,63}"));
        }
        if rules[..i].iter().any(|prev| prev.id == r.id) {
            return Err(bad(&r.id, "duplicate id"));
        }
        if text.is_empty() || text.chars().count() > MAX_RULE_TEXT_LEN {
            return Err(bad(&r.id, "text must be 1..=2048 chars"));
        }
    }
    Ok(())
}

/// 64 hex chars (a sha256 digest or a 32-byte key), surrounding whitespace ignored.
pub fn is_hex64(s: &str) -> bool {
    let t = s.trim();
    t.len() == 64 && t.chars().all(|c| c.is_ascii_hexdigit())
}

/// `http://` / `https://` origin with no whitespace.
pub fn is_http_origin(url: &str) -> bool {
    let u = url.trim();
    (u.starts_with("http://") || u.starts_with("https://"))
        && u.len() >= 8
        && !u.contains(['\n', ' '])
}

fn ident(id: &str, max: usize, underscore: bool) -> bool {
    let b = id.as_bytes();
    (2..=max).contains(&b.len())
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter().all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-' || (underscore && *c == b'_')
        })
}

/// Topic / offer id: `[a-z0-9][a-z0-9-]{1,62}` — a **slug**, hyphens only.
///
/// Two identifier namespaces exist and nothing maps one onto the other: a
/// topic id (`staging-fc-colo-test`) is a slug because it becomes a URL
/// path segment, a VM / jail name, and a DB-checked key; a topic's
/// `metric.custom_id` / checklist ids (`staging_fc_colo_test`) are
/// [`is_custom_id`] identifiers that may also carry `_`. A runner is looked
/// up by `metric.custom_id` **byte-for-byte**, never by the topic id, and
/// `_` ≠ `-`. [`slug_hint`] / [`custom_id_hint`] spell that out to an
/// operator who mixed the two up.
pub fn is_slug(id: &str) -> bool {
    ident(id, 63, false)
}

/// Identifier a topic may mint for a custom metric or a checklist rule:
/// `[a-z0-9][a-z0-9_-]{1,63}` (underscores allowed — unlike a topic id, see
/// [`is_slug`]). Values come from the signed document, never from a list
/// compiled into any crate.
pub fn is_custom_id(id: &str) -> bool {
    ident(id, 64, true)
}

/// `id` with case and `_` / `-` folded away: two ids that fold to the same
/// string are *twins* — what an operator typing `staging_fc_colo_test` for
/// `staging-fc-colo-test` (or the reverse) produced.
fn fold_id(id: &str) -> String {
    id.trim().to_ascii_lowercase().replace('_', "-")
}

/// The entry of `known` that is a twin of `id` (same up to `_` ↔ `-` and
/// ASCII case) without being `id` itself.
pub fn id_twin<'a, I>(id: &str, known: I) -> Option<&'a str>
where
    I: IntoIterator<Item = &'a str>,
{
    let want = fold_id(id);
    known
        .into_iter()
        .find(|k| k.trim() != id.trim() && fold_id(k) == want)
}

/// Message tail naming a registered twin, or empty.
#[must_use]
pub fn twin_suffix(twin: Option<&str>) -> String {
    twin.map_or_else(String::new, |t| {
        format!(
            " (ids match byte-for-byte: {t:?} is registered, and '_' is not '-'; use exactly that id in metric.custom_id / PROOF_VM_RUNNER_CUSTOM_IDS)"
        )
    })
}

/// The nearest well-formed id: lower-cased, every run of characters the
/// namespace does not allow (whitespace, dots, `_` for a slug, …) turned
/// into one separator (`-` for a slug, `_` for a custom id), ends trimmed.
fn corrected(id: &str, underscore: bool) -> String {
    let sep = if underscore { '_' } else { '-' };
    let mut out = String::with_capacity(id.len());
    for c in id.trim().to_ascii_lowercase().chars() {
        let keep =
            c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || (underscore && c == '_');
        if keep {
            out.push(c);
        } else if !out.ends_with(sep) && !out.is_empty() {
            out.push(sep);
        }
    }
    out.trim_matches(['-', '_']).to_owned()
}

/// Why `id` is not a topic / offer slug, for an operator — names the
/// namespace mix-up (underscores belong to custom ids) and a corrected form
/// when one exists. `None` when `id` is a slug.
#[must_use]
pub fn slug_hint(id: &str) -> Option<String> {
    if is_slug(id) {
        return None;
    }
    let mut why: Vec<&str> = Vec::new();
    if id.contains('_') {
        why.push("topic ids are slugs with hyphens only — underscores belong to metric.custom_id / checklist ids, which are a separate namespace");
    }
    if id.chars().any(|c| c.is_ascii_uppercase()) {
        why.push("lower-case only");
    }
    if id.chars().any(char::is_whitespace) {
        why.push("no whitespace");
    }
    if why.is_empty() {
        why.push("2..=63 chars of [a-z0-9-], starting with a letter or digit");
    }
    let fixed = corrected(id, false);
    let mut hint = why.join("; ");
    if is_slug(&fixed) && fixed != id.trim() {
        let _ = write!(hint, "; did you mean {fixed:?}?");
    }
    Some(hint)
}

/// [`slug_hint`] as a message tail (`; …`), or empty for a slug.
#[must_use]
pub fn slug_suffix(id: &str) -> String {
    slug_hint(id).map_or_else(String::new, |h| format!("; {h}"))
}

/// Why `id` is not a custom / checklist identifier, for an operator, with a
/// corrected form when one exists. `None` when `id` is well-formed.
#[must_use]
pub fn custom_id_hint(id: &str) -> Option<String> {
    if is_custom_id(id) {
        return None;
    }
    let mut why: Vec<&str> = Vec::new();
    if id.chars().any(|c| c.is_ascii_uppercase()) {
        why.push("lower-case only");
    }
    if id.chars().any(char::is_whitespace) {
        why.push("no whitespace");
    }
    if id
        .chars()
        .any(|c| !c.is_ascii_alphanumeric() && c != '_' && c != '-' && !c.is_whitespace())
    {
        why.push("only [a-z0-9_-] (underscores and hyphens are both fine here, unlike a topic id)");
    }
    if why.is_empty() {
        why.push("2..=64 chars of [a-z0-9_-], starting with a letter or digit");
    }
    let fixed = corrected(id, true);
    let mut hint = why.join("; ");
    if is_custom_id(&fixed) && fixed != id.trim() {
        let _ = write!(hint, "; did you mean {fixed:?}?");
    }
    Some(hint)
}

/// [`custom_id_hint`] as a message tail (`; …`), or empty when well-formed.
#[must_use]
pub fn custom_id_suffix(id: &str) -> String {
    custom_id_hint(id).map_or_else(String::new, |h| format!("; {h}"))
}

fn is_segment(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'.'))
}

/// Slash-separated provider model id, optional `:tag`. Shape only — the
/// model itself is topic data, never a default anywhere in this repository.
///
/// Two or more segments: `vendor/model`, LiteLLM/OpenRouter
/// `openrouter/vendor/model`, optional `:tag` on the last segment. A single
/// slash-less name is not a pin (that would drop the provider).
pub fn is_model_pin(s: &str) -> bool {
    let (name, tag) = s.split_once(':').unwrap_or((s, "x"));
    let name = name.trim();
    if name.is_empty() || name.starts_with('/') || name.ends_with('/') || name.contains("//") {
        return false;
    }
    let n = name.split('/').count();
    (2..=8).contains(&n) && name.split('/').all(is_segment) && is_segment(tag)
}

/// Opaque runner-facing text (task slice label, constraint param): printable,
/// single line, bounded. The control plane never interprets it.
pub fn is_opaque_param(s: &str) -> bool {
    !s.trim().is_empty() && s.len() <= 256 && !s.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifier_shapes_are_data_not_catalog() {
        for good in ["placeholder_metric", "bench-x_primary-value", "a1"] {
            assert!(is_custom_id(good), "{good}");
        }
        for bad in [
            "",
            "a",
            "_lead",
            "Upper",
            "has space",
            "dot.id",
            &"x".repeat(65),
        ] {
            assert!(!is_custom_id(bad), "{bad:?}");
        }
        assert!(is_slug("dt-no-ib-v0") && !is_slug("has_underscore"));
        assert!(is_custom_id("staging_fc_colo_test") && !is_slug("staging_fc_colo_test"));
        assert!(is_slug("staging-fc-colo-test") && is_custom_id("staging-fc-colo-test"));
        for good in [
            "vendor/model",
            "vendor/model-2.5:thinking",
            "a/b",
            "a/b/c",
            "openrouter/moonshotai/kimi-k3",
            "openrouter/moonshotai/kimi-k3:thinking",
        ] {
            assert!(is_model_pin(good), "{good}");
        }
        for bad in [
            "",
            "model",
            "/model",
            "vendor/",
            "a b/c",
            "a/b:",
            "openrouter/",
        ] {
            assert!(!is_model_pin(bad), "{bad:?}");
        }
        assert!(is_opaque_param("0..20"));
        assert!(is_opaque_param("split:public"));
        assert!(!is_opaque_param("   "));
        assert!(!is_opaque_param("two\nlines"));
        assert!(!is_opaque_param(&"x".repeat(257)));
        assert!(is_hex64(&"ab".repeat(32)) && !is_hex64("abc"));
        assert!(is_http_origin("https://example.invalid/v1") && !is_http_origin("ftp://x"));
    }

    /// The staging mix-up, spelled out: a topic id typed with underscores
    /// gets the namespace explanation and the hyphenated form; a custom id
    /// looked up under its hyphenated twin is told which id is registered.
    #[test]
    fn hyphen_underscore_mixups_get_named_and_corrected() {
        assert_eq!(slug_hint("staging-fc-colo-test"), None);
        assert_eq!(slug_suffix("staging-fc-colo-test"), "");
        let hint = slug_hint("staging_fc_colo_test").expect("not a slug");
        assert!(
            hint.contains("underscores belong to metric.custom_id"),
            "{hint}"
        );
        assert!(
            hint.contains("did you mean \"staging-fc-colo-test\"?"),
            "{hint}"
        );
        assert!(slug_suffix("staging_fc_colo_test").starts_with("; "));
        let upper = slug_hint("Staging-FC").expect("upper");
        assert!(upper.contains("lower-case only"), "{upper}");
        assert!(upper.contains("did you mean \"staging-fc\"?"), "{upper}");
        let short = slug_hint("a").expect("too short");
        assert!(short.contains("2..=63"), "{short}");
        assert!(!short.contains("did you mean"), "{short}");
        assert_eq!(custom_id_hint("staging_fc_colo_test"), None);
        let spaced = custom_id_hint("Staging FC.colo").expect("bad");
        assert!(spaced.contains("lower-case only"), "{spaced}");
        assert!(spaced.contains("no whitespace"), "{spaced}");
        assert!(spaced.contains("only [a-z0-9_-]"), "{spaced}");
        assert!(
            spaced.contains("did you mean \"staging_fc_colo\"?"),
            "{spaced}"
        );
        let dotted = slug_hint("Topic.Name  v0").expect("bad slug");
        assert!(
            dotted.contains("did you mean \"topic-name-v0\"?"),
            "{dotted}"
        );
        assert_eq!(custom_id_suffix("ok_id"), "");

        let registered = ["other_metric", "staging_fc_colo_test"];
        assert_eq!(
            id_twin("staging-fc-colo-test", registered),
            Some("staging_fc_colo_test")
        );
        assert_eq!(
            id_twin("STAGING_FC_COLO_TEST", registered),
            Some("staging_fc_colo_test")
        );
        assert_eq!(
            id_twin("staging_fc_colo_test", registered),
            None,
            "itself is no twin"
        );
        assert_eq!(id_twin("something-else", registered), None);
        assert_eq!(twin_suffix(None), "");
        let tail = twin_suffix(Some("staging_fc_colo_test"));
        assert!(
            tail.contains("byte-for-byte") && tail.contains("PROOF_VM_RUNNER_CUSTOM_IDS"),
            "{tail}"
        );
    }

    /// `defer_scoring` is a plain boolean word carried by the signed
    /// document: a typo is a shape reject (never silently "not deferred"
    /// at publish), and only the exact word `true` holds scoring back.
    #[test]
    fn defer_scoring_is_a_boolean_param_and_a_typo_is_a_shape_reject() {
        let with = |v: &str| Constraints {
            params: BTreeMap::from([(PARAM_DEFER_SCORING.to_owned(), v.to_owned())]),
            ..Constraints::default()
        };
        assert!(!Constraints::default().defer_scoring());
        for (value, want) in [("true", true), (" TRUE ", true), ("false", false)] {
            let c = with(value);
            c.validate_shape().expect(value);
            assert_eq!(c.defer_scoring(), want, "{value:?}");
        }
        for bad in ["yes", "1", "maybe", "ture"] {
            let c = with(bad);
            let err = c.validate_shape().expect_err(bad);
            assert_eq!(err.field, "constraints.params.defer_scoring", "{bad:?}");
            assert!(
                err.why.contains("\"true\" or \"false\""),
                "{bad:?}: {}",
                err.why
            );
            assert!(!c.defer_scoring(), "a malformed flag never defers: {bad:?}");
        }
        let other = Constraints {
            params: BTreeMap::from([("unrelated_knob".to_owned(), "true".to_owned())]),
            ..Constraints::default()
        };
        other.validate_shape().expect("other params are opaque");
        assert!(!other.defer_scoring());
        assert!(other.training_evidence_param().is_none());
    }

    /// `require_training_evidence` is the same boolean-word shape as
    /// `defer_scoring`. Absent is not `"false"`: the topic document applies
    /// a family default. A typo is a publish reject.
    #[test]
    fn require_training_evidence_is_a_boolean_param() {
        let with = |v: &str| Constraints {
            params: BTreeMap::from([(PARAM_REQUIRE_TRAINING_EVIDENCE.to_owned(), v.to_owned())]),
            ..Constraints::default()
        };
        assert!(Constraints::default().training_evidence_param().is_none());
        for (value, want) in [("true", true), (" TRUE ", true), ("false", false)] {
            let c = with(value);
            c.validate_shape().expect(value);
            assert_eq!(c.training_evidence_param(), Some(want), "{value:?}");
        }
        for bad in ["yes", "1", "maybe"] {
            let c = with(bad);
            let err = c.validate_shape().expect_err(bad);
            assert_eq!(
                err.field, "constraints.params.require_training_evidence",
                "{bad:?}"
            );
            assert!(c.training_evidence_param().is_none(), "{bad:?}");
        }
    }

    #[test]
    fn rule_vectors_are_bounded_unique_and_non_empty_text() {
        let rule = |id: &str, text: &str| ChecklistRule {
            id: id.into(),
            text: text.into(),
        };
        validate_rules(&[]).expect("no rules is a legal (empty) vector");
        validate_rules(&[rule("a_1", "x"), rule("b-2", "y")]).expect("two rules");
        assert_eq!(
            validate_rules(&[rule("a_1", "x"), rule("a_1", "y")]).map_err(|e| e.why),
            Err("duplicate id")
        );
        assert!(validate_rules(&[rule("Bad Id", "x")]).is_err());
        assert!(validate_rules(&[rule("ok", "   ")]).is_err());
        assert!(validate_rules(&[rule("ok", &"x".repeat(MAX_RULE_TEXT_LEN + 1))]).is_err());
        let many: Vec<ChecklistRule> = (0..=MAX_CHECKLIST_RULES)
            .map(|i| rule(&format!("r_{i}"), "x"))
            .collect();
        assert_eq!(
            validate_rules(&many).map_err(|e| e.why),
            Err("too many rules")
        );
    }
}
