//! Rule sets and checklists: the anti-cheat gate as **data**.
//!
//! A [`RuleSet`] is a versioned vector of `{id, text}` rules for one topic.
//! Version 1 is the vector the signed topic document carries; later versions
//! are whatever the topic's RLM writes (persisted by the store, never
//! compiled in). A [`Checklist`] is one inspection: `{id, pass, evidence}`
//! per rule of a named version. It is green only when every rule of that
//! version is present exactly once with evidence and `pass: true`. A missing
//! rule, an unknown id, a duplicate, an evidence-less pass, or a red item is
//! not green, and nothing spends behind a checklist that is not green.

use proof_canon::{canonical_json, is_custom_id};
use proof_task::{ChecklistRule, TopicDocument, MAX_CHECKLIST_RULES, MAX_RULE_TEXT_LEN};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Only accepted `schema_version` of `checklist.json`.
pub const CHECKLIST_SCHEMA: u32 = 1;

/// Longest evidence string kept per item (ingest truncates, verify refuses longer).
pub const MAX_EVIDENCE_LEN: usize = 2_048;

/// Who wrote a rule version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleSource {
    /// The vector carried by the signed topic document (version 1).
    TopicDocument,
    /// Rewritten by the topic's RLM inside its VM, persisted by the store.
    Rlm,
    /// Operator edit.
    Operator,
}

/// One versioned rule vector for one topic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleSet {
    /// Topic id.
    pub topic_id: String,
    /// Monotonic version (1 = the signed document's vector).
    pub version: u32,
    /// Who wrote it.
    pub source: RuleSource,
    /// The rules, in evaluation order.
    pub rules: Vec<ChecklistRule>,
}

/// Why a rule set or checklist is not usable.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ChecklistError {
    /// JSON did not parse.
    #[error("parse: {0}")]
    Parse(String),
    /// Schema drift.
    #[error("checklist schema_version {got}, this build reads {CHECKLIST_SCHEMA}")]
    WrongSchema {
        /// What the document said.
        got: u32,
    },
    /// A rule vector is malformed (shape mirrors the topic validator).
    #[error("rule set: {0}")]
    BadRules(String),
    /// A rule set with no rules cannot authorise anything.
    #[error("rule set is empty; nothing to verify, nothing to spend behind")]
    NoRules,
    /// The checklist names another topic / version than the rules.
    #[error("checklist is for another topic or rule version")]
    WrongRuleSet,
    /// A rule was never ticked. Absence of evidence is a failed gate.
    #[error("checklist item {0:?} missing")]
    Missing(String),
    /// An item names a rule the version does not have.
    #[error("checklist item {0:?} is not a rule of this version")]
    Unknown(String),
    /// A rule was ticked twice.
    #[error("checklist item {0:?} duplicated")]
    Duplicate(String),
    /// A pass with nothing to show for it, or an oversized blob.
    #[error("checklist item {0:?} has empty or oversized evidence")]
    BadEvidence(String),
    /// At least one item failed. No paid inference.
    #[error("checklist red: {}", .0.join(", "))]
    Failed(Vec<String>),
}

fn validate_rules(rules: &[ChecklistRule]) -> Result<(), ChecklistError> {
    if rules.is_empty() {
        return Err(ChecklistError::NoRules);
    }
    if rules.len() > MAX_CHECKLIST_RULES {
        return Err(ChecklistError::BadRules("too many rules".into()));
    }
    for (i, r) in rules.iter().enumerate() {
        if !is_custom_id(&r.id) {
            return Err(ChecklistError::BadRules(format!("bad id {:?}", r.id)));
        }
        if rules[..i].iter().any(|p| p.id == r.id) {
            return Err(ChecklistError::BadRules(format!("duplicate id {:?}", r.id)));
        }
        let t = r.text.trim();
        if t.is_empty() || t.chars().count() > MAX_RULE_TEXT_LEN {
            return Err(ChecklistError::BadRules(format!("bad text for {:?}", r.id)));
        }
    }
    Ok(())
}

impl RuleSet {
    /// Version 1: the vector the signed topic carries.
    ///
    /// # Errors
    ///
    /// [`ChecklistError::NoRules`] / [`ChecklistError::BadRules`].
    pub fn from_topic(doc: &TopicDocument) -> Result<Self, ChecklistError> {
        let set = Self {
            topic_id: doc.id.clone(),
            version: 1,
            source: RuleSource::TopicDocument,
            rules: doc.checklist.clone(),
        };
        set.validate()?;
        Ok(set)
    }

    /// A later version written by the RLM or the operator.
    ///
    /// # Errors
    ///
    /// [`ChecklistError::BadRules`] when `version` does not advance or the
    /// rules are malformed.
    pub fn next(
        &self,
        source: RuleSource,
        rules: Vec<ChecklistRule>,
    ) -> Result<Self, ChecklistError> {
        let set = Self {
            topic_id: self.topic_id.clone(),
            version: self.version.saturating_add(1),
            source,
            rules,
        };
        set.validate()?;
        Ok(set)
    }

    /// Shape check (same rules as the topic validator, plus non-empty).
    ///
    /// # Errors
    ///
    /// [`ChecklistError::NoRules`] / [`ChecklistError::BadRules`].
    pub fn validate(&self) -> Result<(), ChecklistError> {
        if self.version == 0 {
            return Err(ChecklistError::BadRules("version must be >= 1".into()));
        }
        validate_rules(&self.rules)
    }

    /// Rule ids in evaluation order.
    #[must_use]
    pub fn ids(&self) -> Vec<&str> {
        self.rules.iter().map(|r| r.id.as_str()).collect()
    }

    /// SHA-256 hex of the canonical JSON (what a checklist binds to).
    #[must_use]
    pub fn digest(&self) -> String {
        domain_digest(b"proof-rlm-rules-v1", self)
    }
}

fn domain_digest<T: Serialize>(domain: &[u8], value: &T) -> String {
    let value = serde_json::to_value(value).unwrap_or(serde_json::Value::Null);
    let mut h = Sha256::new();
    h.update(domain);
    h.update([0xff]);
    h.update(canonical_json(&value).as_bytes());
    hex::encode(h.finalize())
}

/// One ticked rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckItem {
    /// Rule id from the rule set.
    pub id: String,
    /// Whether it passed.
    pub pass: bool,
    /// What the inspector saw (file, line, command output). Never a secret.
    pub evidence: String,
}

/// `checklist.json`: one inspection of one submission against one rule version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checklist {
    /// Must equal [`CHECKLIST_SCHEMA`].
    pub schema_version: u32,
    /// Topic the artefact was inspected for.
    pub topic_id: String,
    /// Rule version the items tick.
    pub rules_version: u32,
    /// Digest of that rule set, so a checklist cannot be replayed against edited rules.
    pub rules_digest: String,
    /// Frozen submission digest.
    pub submission_digest: String,
    /// Artefact digest that was inspected.
    pub artifact_digest: String,
    /// Items, ideally in rule order.
    pub items: Vec<CheckItem>,
}

impl Checklist {
    /// Empty checklist for one submission against `rules`.
    #[must_use]
    pub fn new(rules: &RuleSet, submission_digest: &str, artifact_digest: &str) -> Self {
        Self {
            schema_version: CHECKLIST_SCHEMA,
            topic_id: rules.topic_id.clone(),
            rules_version: rules.version,
            rules_digest: rules.digest(),
            submission_digest: submission_digest.to_owned(),
            artifact_digest: artifact_digest.to_owned(),
            items: Vec::new(),
        }
    }

    /// Record one item (evidence truncated to [`MAX_EVIDENCE_LEN`]).
    pub fn record(&mut self, id: &str, pass: bool, evidence: &str) -> &mut Self {
        let mut evidence = evidence.trim().to_owned();
        if evidence.len() > MAX_EVIDENCE_LEN {
            let mut cut = MAX_EVIDENCE_LEN;
            while !evidence.is_char_boundary(cut) {
                cut = cut.saturating_sub(1);
            }
            evidence.truncate(cut);
        }
        self.items.push(CheckItem {
            id: id.to_owned(),
            pass,
            evidence,
        });
        self
    }

    /// Parse `checklist.json`.
    ///
    /// # Errors
    ///
    /// [`ChecklistError::Parse`].
    pub fn from_json(body: &str) -> Result<Self, ChecklistError> {
        serde_json::from_str(body).map_err(|e| ChecklistError::Parse(e.to_string()))
    }

    /// Pretty JSON for the artefact bundle.
    #[must_use]
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".into())
    }

    /// Structural check against `rules`: schema, same topic + version +
    /// digest, every rule exactly once, no unknown ids, evidence present.
    ///
    /// # Errors
    ///
    /// See [`ChecklistError`].
    pub fn verify_complete(&self, rules: &RuleSet) -> Result<(), ChecklistError> {
        if self.schema_version != CHECKLIST_SCHEMA {
            return Err(ChecklistError::WrongSchema {
                got: self.schema_version,
            });
        }
        rules.validate()?;
        if self.topic_id != rules.topic_id
            || self.rules_version != rules.version
            || !self.rules_digest.eq_ignore_ascii_case(&rules.digest())
        {
            return Err(ChecklistError::WrongRuleSet);
        }
        let ids = rules.ids();
        for item in &self.items {
            if !ids.contains(&item.id.as_str()) {
                return Err(ChecklistError::Unknown(item.id.clone()));
            }
        }
        for id in ids {
            let mut seen = 0usize;
            for item in self.items.iter().filter(|i| i.id == id) {
                seen = seen.saturating_add(1);
                let e = item.evidence.trim();
                if e.is_empty() || e.len() > MAX_EVIDENCE_LEN {
                    return Err(ChecklistError::BadEvidence(id.to_owned()));
                }
            }
            match seen {
                0 => return Err(ChecklistError::Missing(id.to_owned())),
                1 => {}
                _ => return Err(ChecklistError::Duplicate(id.to_owned())),
            }
        }
        Ok(())
    }

    /// Complete **and** every item passed.
    ///
    /// # Errors
    ///
    /// A structural error, or [`ChecklistError::Failed`] naming every red id.
    pub fn verify(&self, rules: &RuleSet) -> Result<(), ChecklistError> {
        self.verify_complete(rules)?;
        let failed = self.failed_ids();
        if failed.is_empty() {
            Ok(())
        } else {
            Err(ChecklistError::Failed(failed))
        }
    }

    /// Ids recorded as `pass: false`, sorted, deduplicated.
    #[must_use]
    pub fn failed_ids(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .items
            .iter()
            .filter(|i| !i.pass)
            .map(|i| i.id.clone())
            .collect();
        out.sort();
        out.dedup();
        out
    }

    /// Whether this checklist authorises spend under `rules`.
    #[must_use]
    pub fn is_green(&self, rules: &RuleSet) -> bool {
        self.verify(rules).is_ok()
    }

    /// SHA-256 hex of the canonical JSON; binds a spend token to these exact items.
    #[must_use]
    pub fn digest(&self) -> String {
        domain_digest(b"proof-rlm-checklist-v1", self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{green, rules, topic};

    #[test]
    fn version_one_is_the_signed_documents_vector() {
        let t = topic();
        let set = RuleSet::from_topic(&t).expect("rules");
        assert_eq!(set.version, 1);
        assert_eq!(set.source, RuleSource::TopicDocument);
        assert_eq!(
            set.ids(),
            t.checklist
                .iter()
                .map(|r| r.id.as_str())
                .collect::<Vec<_>>()
        );
        assert_eq!(set.digest().len(), 64);
        let mut none = t.clone();
        none.checklist.clear();
        assert_eq!(RuleSet::from_topic(&none), Err(ChecklistError::NoRules));
    }

    #[test]
    fn later_versions_advance_and_are_shape_checked() {
        let v1 = rules();
        let v2 = v1
            .next(
                RuleSource::Rlm,
                vec![ChecklistRule {
                    id: "rewritten".into(),
                    text: "the rlm rewrote the rule".into(),
                }],
            )
            .expect("v2");
        assert_eq!(v2.version, 2);
        assert_eq!(v2.source, RuleSource::Rlm);
        assert_ne!(v2.digest(), v1.digest());
        assert!(matches!(
            v1.next(RuleSource::Operator, Vec::new()),
            Err(ChecklistError::NoRules)
        ));
        assert!(matches!(
            v1.next(
                RuleSource::Operator,
                vec![ChecklistRule {
                    id: "Bad Id".into(),
                    text: "x".into()
                }]
            ),
            Err(ChecklistError::BadRules(_))
        ));
        let mut zero = v1;
        zero.version = 0;
        assert!(matches!(zero.validate(), Err(ChecklistError::BadRules(_))));
    }

    #[test]
    fn a_complete_all_pass_checklist_is_green_for_its_rule_version() {
        let set = rules();
        let c = green(&set, "d");
        c.verify(&set).expect("green");
        assert!(c.is_green(&set));
        assert!(c.failed_ids().is_empty());
        let round = Checklist::from_json(&c.to_json()).expect("round trip");
        assert_eq!(round, c);
        assert_eq!(round.digest(), c.digest());
        // The same items are not green for an edited rule version.
        let v2 = set.next(RuleSource::Rlm, set.rules.clone()).expect("v2");
        assert_eq!(c.verify(&v2), Err(ChecklistError::WrongRuleSet));
    }

    /// Any red item is red — one is enough, and the error names every red id.
    #[test]
    fn any_false_item_is_red() {
        let set = rules();
        for id in set.ids() {
            let mut c = green(&set, "d");
            for item in &mut c.items {
                if item.id == id {
                    item.pass = false;
                }
            }
            assert!(!c.is_green(&set));
            assert_eq!(
                c.verify(&set),
                Err(ChecklistError::Failed(vec![id.to_owned()]))
            );
        }
        let mut two = green(&set, "d");
        two.items[0].pass = false;
        two.items[1].pass = false;
        let mut want = vec![two.items[0].id.clone(), two.items[1].id.clone()];
        want.sort();
        assert_eq!(two.verify(&set), Err(ChecklistError::Failed(want)));
    }

    /// Absence of evidence is a failed gate: a missing rule, an unknown id, a
    /// duplicate vote, or an evidence-less pass all refuse.
    #[test]
    fn missing_unknown_duplicate_or_evidence_less_items_refuse() {
        let set = rules();
        let first = set.ids()[0].to_owned();

        let mut missing = green(&set, "d");
        missing.items.retain(|i| i.id != first);
        assert_eq!(
            missing.verify(&set),
            Err(ChecklistError::Missing(first.clone()))
        );

        let mut unknown = green(&set, "d");
        unknown.record("not_a_rule", true, "x");
        assert_eq!(
            unknown.verify(&set),
            Err(ChecklistError::Unknown("not_a_rule".into()))
        );

        let mut dup = green(&set, "d");
        dup.record(&first, true, "again");
        assert_eq!(
            dup.verify(&set),
            Err(ChecklistError::Duplicate(first.clone()))
        );

        let mut blank = green(&set, "d");
        blank.items[0].evidence = "   ".into();
        assert_eq!(blank.verify(&set), Err(ChecklistError::BadEvidence(first)));

        let mut schema = green(&set, "d");
        schema.schema_version = 2;
        assert_eq!(
            schema.verify(&set),
            Err(ChecklistError::WrongSchema { got: 2 })
        );

        let empty = Checklist::new(&set, "d", "a");
        assert!(matches!(
            empty.verify(&set),
            Err(ChecklistError::Missing(_))
        ));
        assert!(Checklist::from_json("nope").is_err());
    }

    #[test]
    fn evidence_is_truncated_on_record_and_digest_tracks_content() {
        let set = rules();
        let first = set.ids()[0].to_owned();
        let mut c = green(&set, "d");
        c.items.retain(|i| i.id != first);
        c.record(&first, true, &"é".repeat(MAX_EVIDENCE_LEN));
        assert!(c.items.last().expect("item").evidence.len() <= MAX_EVIDENCE_LEN);
        c.verify(&set).expect("truncated evidence still verifies");
        let a = c.digest();
        c.items[0].evidence.push('!');
        assert_ne!(a, c.digest(), "digest must move with the evidence");
    }
}
