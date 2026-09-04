//! Canonical JSON used for topic-document signatures.
//!
//! This is the repo's existing canonical-JSON rule (the one the Relearn Agent
//! trace commitment already uses), not a second one: object keys sorted, no
//! insignificant whitespace, and numbers in serde_json's shortest
//! round-tripping form. For the value shapes a [`crate::TopicDocument`] may
//! contain — strings, booleans, integers, finite decimals, arrays, objects —
//! that agrees with RFC 8785 JCS.
//!
//! Determinism is the whole point: the signer (`xtask proof-topic`) and every
//! verifier go through this function, so a document that verifies once
//! verifies everywhere. Non-finite numbers cannot appear (serde_json refuses
//! them), and `null` is preserved so an absent-vs-null distinction cannot be
//! signed away.

use serde_json::Value;

/// Canonical serialization of a JSON value.
///
/// Sorted keys, no whitespace. `Value`'s map is already ordered, but the sort
/// is explicit so the rule does not depend on a serde_json feature flag.
#[must_use]
pub fn canonical_json(value: &Value) -> String {
    match value {
        Value::Null => "null".into(),
        Value::Bool(true) => "true".into(),
        Value::Bool(false) => "false".into(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into()),
        Value::Array(items) => {
            let parts: Vec<String> = items.iter().map(canonical_json).collect();
            format!("[{}]", parts.join(","))
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_unstable();
            let parts: Vec<String> = keys
                .into_iter()
                .map(|k| {
                    format!(
                        "{}:{}",
                        serde_json::to_string(k).unwrap_or_else(|_| "\"\"".into()),
                        map.get(k).map_or_else(|| "null".into(), canonical_json)
                    )
                })
                .collect();
            format!("{{{}}}", parts.join(","))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_sort_and_whitespace_is_dropped() {
        let v: Value = serde_json::json!({ "b": 1, "a": { "d": [1, 2], "c": "x" } });
        assert_eq!(
            canonical_json(&v),
            r#"{"a":{"c":"x","d":[1,2]},"b":1}"#.to_owned()
        );
    }

    /// Re-parsing the canonical form must reproduce it byte for byte, or a
    /// signature made over one spelling would not verify over the other.
    #[test]
    fn canonical_form_is_a_fixed_point() {
        let body = r#"{
            "lr": 3e-4, "eps": 1e-8, "betas": [0.9, 0.95],
            "flops_budget": 2000000000000000000,
            "max_inter_node_gbps": 12.5,
            "proxy_model": null, "no_nvlink": true
        }"#;
        let once = canonical_json(&serde_json::from_str::<Value>(body).expect("parse"));
        let twice = canonical_json(&serde_json::from_str::<Value>(&once).expect("reparse"));
        assert_eq!(once, twice, "canonical form must round-trip");
        assert!(once.contains("\"proxy_model\":null"), "{once}");
        assert!(
            once.contains("\"flops_budget\":2000000000000000000"),
            "a 2e18 budget must not become a float: {once}"
        );
    }

    #[test]
    fn strings_are_json_escaped() {
        let v = serde_json::json!({ "s": "a\"b\n" });
        assert_eq!(canonical_json(&v), r#"{"s":"a\"b\n"}"#.to_owned());
    }
}
