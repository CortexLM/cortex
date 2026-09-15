//! In-memory [`RlmStore`] for CI / local hosts. Same contract as Postgres,
//! no durability: a host that boots without `BASE_DATABASE_URL` logs that
//! rules, checklists, transitions, and promotions will not survive a restart.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use proof_rlm::{Lifecycle, RuleSet};
use proof_task::TopicDocument;

use crate::{
    check_artefact, check_promotion, check_rules, parse_row_id, replay, ArtefactRow, BaselineRow,
    ChecklistRow, PromotionRow, RlmStore, StoreError, TopicVersionRow, TransitionRow,
};

#[derive(Default)]
struct Inner {
    topics: BTreeMap<String, Vec<TopicDocument>>,
    rules: BTreeMap<String, Vec<RuleSet>>,
    checklists: BTreeMap<String, ChecklistRow>,
    transitions: BTreeMap<String, Vec<TransitionRow>>,
    baselines: BTreeMap<String, Vec<BaselineRow>>,
    artefacts: BTreeMap<String, Vec<ArtefactRow>>,
    promotions: BTreeMap<String, Vec<PromotionRow>>,
    aliases: BTreeMap<String, String>,
}

/// In-memory store.
#[derive(Clone, Default)]
pub struct MemoryRlmStore {
    inner: Arc<Mutex<Inner>>,
}

impl MemoryRlmStore {
    /// Empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Inner>, StoreError> {
        self.inner.lock().map_err(|_| StoreError::Poison)
    }
}

#[async_trait]
impl RlmStore for MemoryRlmStore {
    async fn put_topic_version(&self, doc: &TopicDocument) -> Result<u32, StoreError> {
        let mut g = self.lock()?;
        let versions = g.topics.entry(doc.id.clone()).or_default();
        versions.push(doc.clone());
        u32::try_from(versions.len()).map_err(|_| StoreError::VersionGap("topic"))
    }

    async fn latest_topic(
        &self,
        topic_id: &str,
    ) -> Result<Option<(u32, TopicDocument)>, StoreError> {
        let g = self.lock()?;
        Ok(g.topics.get(topic_id).and_then(|v| {
            v.last()
                .cloned()
                .map(|d| (u32::try_from(v.len()).unwrap_or(u32::MAX), d))
        }))
    }

    async fn latest_topics(&self) -> Result<Vec<TopicVersionRow>, StoreError> {
        let g = self.lock()?;
        // `BTreeMap` iteration is already ordered by topic id, which is the
        // order the Postgres view returns.
        let mut out = Vec::with_capacity(g.topics.len());
        for (topic_id, versions) in &g.topics {
            let Some(doc) = versions.last() else {
                continue;
            };
            out.push(TopicVersionRow {
                topic_id: topic_id.clone(),
                version: u32::try_from(versions.len()).unwrap_or(u32::MAX),
                document: doc.clone(),
            });
        }
        Ok(out)
    }

    async fn put_alias(&self, alias: &str, topic_id: &str) -> Result<(), StoreError> {
        let mut g = self.lock()?;
        if !g.topics.contains_key(topic_id) {
            return Err(StoreError::Malformed(format!(
                "alias {alias:?} names topic {topic_id:?}, which has no published version"
            )));
        }
        // A canonical slug is never shadowed (see the Postgres store).
        if g.topics.contains_key(alias) {
            return Err(StoreError::Malformed(format!(
                "alias {alias:?} is already a published topic id; a canonical slug is never \
                 shadowed by an alias"
            )));
        }
        g.aliases.insert(alias.to_owned(), topic_id.to_owned());
        Ok(())
    }

    async fn resolve_alias(&self, alias: &str) -> Result<Option<String>, StoreError> {
        let g = self.lock()?;
        // Fail closed like Postgres: the target must still have a version.
        // A canonical slug wins: never resolve an alias whose own name is a
        // published topic, or `show` would return a different topic.
        if g.topics.contains_key(alias) {
            return Ok(None);
        }
        Ok(g.aliases
            .get(alias)
            .filter(|t| g.topics.contains_key(*t))
            .cloned())
    }

    async fn aliases_for(&self, topic_id: &str) -> Result<Vec<String>, StoreError> {
        let g = self.lock()?;
        Ok(g.aliases
            .iter()
            .filter(|(_, t)| t.as_str() == topic_id)
            .map(|(a, _)| a.clone())
            .collect())
    }

    async fn delete_alias(&self, alias: &str) -> Result<bool, StoreError> {
        Ok(self.lock()?.aliases.remove(alias).is_some())
    }

    async fn put_rules(&self, rules: &RuleSet) -> Result<(), StoreError> {
        let mut g = self.lock()?;
        let versions = g.rules.entry(rules.topic_id.clone()).or_default();
        check_rules(rules, versions.last().map(|r| r.version))?;
        versions.push(rules.clone());
        Ok(())
    }

    async fn current_rules(&self, topic_id: &str) -> Result<Option<RuleSet>, StoreError> {
        Ok(self
            .lock()?
            .rules
            .get(topic_id)
            .and_then(|v| v.last().cloned()))
    }

    async fn rules_at(&self, topic_id: &str, version: u32) -> Result<Option<RuleSet>, StoreError> {
        Ok(self
            .lock()?
            .rules
            .get(topic_id)
            .and_then(|v| v.iter().find(|r| r.version == version).cloned()))
    }

    async fn current_rules_source(
        &self,
        topic_id: &str,
    ) -> Result<Option<proof_rlm::RuleSource>, StoreError> {
        Ok(self
            .lock()?
            .rules
            .get(topic_id)
            .and_then(|v| v.last().map(|r| r.source)))
    }

    async fn rlm_authored_rules(&self, topic_id: &str) -> Result<bool, StoreError> {
        Ok(self.current_rules_source(topic_id).await? == Some(proof_rlm::RuleSource::Rlm))
    }

    async fn put_checklist(&self, row: &ChecklistRow) -> Result<(), StoreError> {
        // Same digest may be re-inspected (miner resubmit). Overwrite.
        self.lock()?
            .checklists
            .insert(row.submission_digest.clone(), row.clone());
        Ok(())
    }

    async fn checklist(&self, submission_digest: &str) -> Result<Option<ChecklistRow>, StoreError> {
        Ok(self.lock()?.checklists.get(submission_digest).cloned())
    }

    async fn record_transition(&self, row: &TransitionRow) -> Result<(), StoreError> {
        self.lock()?
            .transitions
            .entry(row.topic_id.clone())
            .or_default()
            .push(row.clone());
        Ok(())
    }

    async fn lifecycle(&self, topic_id: &str) -> Result<Option<Lifecycle>, StoreError> {
        let g = self.lock()?;
        Ok(g.transitions
            .get(topic_id)
            .and_then(|rows| replay(topic_id, rows)))
    }

    async fn put_baseline(&self, row: &BaselineRow) -> Result<(), StoreError> {
        if !row.primary_value.is_finite() {
            return Err(StoreError::Malformed("baseline primary".into()));
        }
        let mut g = self.lock()?;
        let rows = g.baselines.entry(row.topic_id.clone()).or_default();
        rows.retain(|b| b.rules_version != row.rules_version);
        rows.push(row.clone());
        rows.sort_by_key(|b| b.rules_version);
        Ok(())
    }

    async fn baseline(&self, topic_id: &str) -> Result<Option<BaselineRow>, StoreError> {
        Ok(self
            .lock()?
            .baselines
            .get(topic_id)
            .and_then(|v| v.last().cloned()))
    }

    async fn put_artefact(&self, row: &ArtefactRow) -> Result<(), StoreError> {
        check_artefact(row)?;
        let mut g = self.lock()?;
        let rows = g.artefacts.entry(row.topic_id.clone()).or_default();
        if rows.iter().any(|a| a.submission_id == row.submission_id) {
            tracing::warn!(
                topic_id = %row.topic_id,
                submission_id = %row.submission_id,
                sha256 = %row.sha256,
                "proof_artefact collision: replaced metadata for existing (topic_id, submission_id)"
            );
        }
        rows.retain(|a| a.submission_id != row.submission_id);
        rows.push(row.clone());
        Ok(())
    }

    async fn artefacts(&self, topic_id: &str) -> Result<Vec<ArtefactRow>, StoreError> {
        Ok(self
            .lock()?
            .artefacts
            .get(topic_id)
            .cloned()
            .unwrap_or_default())
    }

    async fn max_artefact_numeric_id(&self) -> Result<Option<u64>, StoreError> {
        Ok(self
            .lock()?
            .artefacts
            .values()
            .flatten()
            .filter_map(|a| parse_row_id(&a.submission_id))
            .max())
    }

    async fn record_promotion(&self, row: &PromotionRow) -> Result<(), StoreError> {
        check_promotion(row)?;
        self.lock()?
            .promotions
            .entry(row.topic_id.clone())
            .or_default()
            .push(row.clone());
        Ok(())
    }

    async fn best(&self, topic_id: &str) -> Result<Option<PromotionRow>, StoreError> {
        Ok(self
            .lock()?
            .promotions
            .get(topic_id)
            .and_then(|v| v.last().cloned()))
    }

    async fn promotions(&self, topic_id: &str) -> Result<Vec<PromotionRow>, StoreError> {
        Ok(self
            .lock()?
            .promotions
            .get(topic_id)
            .cloned()
            .unwrap_or_default())
    }
}
