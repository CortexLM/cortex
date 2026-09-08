//! Spend gate: nothing makes a paid inference call without a [`SpendToken`],
//! and the only way to mint one is a checklist that is green **for the
//! topic's current rule version**.
//!
//! The token has no public constructor. A runner that takes `&SpendToken`
//! therefore cannot be reached from a red, incomplete, or stale checklist,
//! and the token is bound to the exact rules, checklist bytes, and
//! submission it covers, so a token minted for one run cannot authorise
//! another.

use crate::rules::{Checklist, ChecklistError, RuleSet};

/// Proof that the anti-cheat checklist for one submission was green.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpendToken {
    topic_id: String,
    submission_digest: String,
    rules_version: u32,
    checklist_digest: String,
}

/// Why spend was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GateError {
    /// The checklist is incomplete, red, or for another rule version.
    #[error("paid inference refused: {0}")]
    Checklist(#[from] ChecklistError),
    /// The checklist names a different submission than the one being run.
    #[error("paid inference refused: checklist covers another submission")]
    WrongSubmission,
}

impl SpendToken {
    /// Topic the token covers.
    #[must_use]
    pub fn topic_id(&self) -> &str {
        &self.topic_id
    }

    /// Frozen submission digest the token covers.
    #[must_use]
    pub fn submission_digest(&self) -> &str {
        &self.submission_digest
    }

    /// Rule version the checklist was green for.
    #[must_use]
    pub fn rules_version(&self) -> u32 {
        self.rules_version
    }

    /// Digest of the checklist that minted this token.
    #[must_use]
    pub fn checklist_digest(&self) -> &str {
        &self.checklist_digest
    }

    /// Whether this token authorises spend for `topic_id` / `submission_digest`.
    #[must_use]
    pub fn covers(&self, topic_id: &str, submission_digest: &str) -> bool {
        self.topic_id == topic_id.trim() && self.submission_digest == submission_digest.trim()
    }
}

/// Mint a spend token from a checklist that is green under `rules` for the
/// submission it names.
///
/// # Errors
///
/// [`GateError::Checklist`] when any rule is missing, unknown, duplicated,
/// evidence-less, failed, or the checklist is for another rule version;
/// [`GateError::WrongSubmission`] when it was produced for another digest.
pub fn authorize_spend(
    checklist: &Checklist,
    rules: &RuleSet,
    topic_id: &str,
    submission_digest: &str,
) -> Result<SpendToken, GateError> {
    checklist.verify(rules)?;
    if checklist.topic_id.trim() != topic_id.trim()
        || checklist.submission_digest.trim() != submission_digest.trim()
    {
        return Err(GateError::WrongSubmission);
    }
    Ok(SpendToken {
        topic_id: topic_id.trim().to_owned(),
        submission_digest: submission_digest.trim().to_owned(),
        rules_version: rules.version,
        checklist_digest: checklist.digest(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{green, rules};
    use crate::rules::RuleSource;

    #[test]
    fn a_green_checklist_mints_a_bound_token() {
        let set = rules();
        let c = green(&set, "digest-a");
        let t = authorize_spend(&c, &set, &set.topic_id, "digest-a").expect("token");
        assert_eq!(t.topic_id(), set.topic_id);
        assert_eq!(t.submission_digest(), "digest-a");
        assert_eq!(t.rules_version(), 1);
        assert_eq!(t.checklist_digest(), c.digest());
        assert!(t.covers(&set.topic_id, "digest-a"));
        assert!(!t.covers(&set.topic_id, "digest-b"));
        assert!(!t.covers("other", "digest-a"));
    }

    /// The headline rule: any red item, no token, no paid inference.
    #[test]
    fn a_red_incomplete_or_stale_checklist_never_mints() {
        let set = rules();
        let mut red = green(&set, "digest-a");
        red.items[1].pass = false;
        assert!(matches!(
            authorize_spend(&red, &set, &set.topic_id, "digest-a"),
            Err(GateError::Checklist(ChecklistError::Failed(_)))
        ));
        let mut partial = green(&set, "digest-a");
        partial.items.pop();
        assert!(matches!(
            authorize_spend(&partial, &set, &set.topic_id, "digest-a"),
            Err(GateError::Checklist(ChecklistError::Missing(_)))
        ));
        let stale = green(&set, "digest-a");
        let v2 = set.next(RuleSource::Rlm, set.rules.clone()).expect("v2");
        assert_eq!(
            authorize_spend(&stale, &v2, &set.topic_id, "digest-a"),
            Err(GateError::Checklist(ChecklistError::WrongRuleSet))
        );
    }

    #[test]
    fn a_token_cannot_be_borrowed_across_submissions() {
        let set = rules();
        let c = green(&set, "digest-a");
        assert_eq!(
            authorize_spend(&c, &set, &set.topic_id, "digest-b"),
            Err(GateError::WrongSubmission)
        );
        assert_eq!(
            authorize_spend(&c, &set, "other-topic", "digest-a"),
            Err(GateError::WrongSubmission)
        );
    }
}
