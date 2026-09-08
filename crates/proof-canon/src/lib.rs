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

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub use canonical::canonical_json;

/// Most anti-cheat rules one topic may carry.
pub const MAX_CHECKLIST_RULES: usize = 64;

/// Longest rule text.
pub const MAX_RULE_TEXT_LEN: usize = 2_048;

/// Most opaque constraint params one topic may carry.
pub const MAX_CONSTRAINT_PARAMS: usize = 32;

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
    /// Provider model id (`vendor/model[:tag]`) every paid inference call
    /// made by the runner or the miner harness must name.
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
            return bad("model_pin", "vendor/model[:tag]");
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
        Ok(())
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

/// Topic / offer id: `[a-z0-9][a-z0-9-]{1,62}`.
pub fn is_slug(id: &str) -> bool {
    ident(id, 63, false)
}

/// Identifier a topic may mint for a custom metric or a checklist rule:
/// `[a-z0-9][a-z0-9_-]{1,63}`. Values come from the signed document, never
/// from a list compiled into any crate.
pub fn is_custom_id(id: &str) -> bool {
    ident(id, 64, true)
}

fn is_segment(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'.'))
}

/// `vendor/model` provider model id, optional `:tag`. Shape only — the model
/// itself is topic data, never a default anywhere in this repository.
pub fn is_model_pin(s: &str) -> bool {
    let (name, tag) = s.split_once(':').unwrap_or((s, "x"));
    matches!(name.trim().split_once('/'), Some((v, m)) if is_segment(v) && is_segment(m))
        && is_segment(tag)
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
        for good in ["vendor/model", "vendor/model-2.5:thinking", "a/b"] {
            assert!(is_model_pin(good), "{good}");
        }
        for bad in ["", "model", "/model", "vendor/", "a/b/c", "a b/c", "a/b:"] {
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
