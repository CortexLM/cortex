//! The operator gate: whether a topic is **disabled**, and the write the CLI
//! uses to change that.
//!
//! A topic's lifecycle lives in its signed document (`draft` / `open` /
//! `closed`), and moving it is a signing ceremony. This is the other switch:
//! an operator records "this topic is disabled" and the challenge refuses
//! every submission to it on the next request — no re-sign, no restart, no
//! redeploy. The table is `proof_topic_gate` (migration `0026`), a journal
//! whose newest row per topic is the current state.
//!
//! | Question | Function |
//! |----------|----------|
//! | Is the topic disabled right now? | [`disabled`] |
//! | Turn it off, with a reason | [`disable`] |
//! | Turn it back on | [`enable`] |
//! | What is the state, for an operator listing? | [`gate`] |
//!
//! # Fail-closed, and where
//!
//! The runtime read is [`disabled`], and the caller
//! (`proof_http::InstallJournal::disabled`) answers **503** on an `Err`: an
//! unreadable gate is not an enabled topic. That is the same direction the
//! install journal takes on the publish path, and it is the reason this
//! module returns `Result<Gate, _>` rather than a `bool` that a caller could
//! read as "not disabled" when the database is down.
//!
//! A topic with **no** row is enabled: the gate is a switch an operator
//! throws, not a registration every topic needs. Nothing here interprets a
//! document, a rule, or a route; the gate is one bit of operator state plus
//! its history.

use std::collections::BTreeMap;

use sqlx::PgPool;

use crate::InstallError;

/// The two states a gate row can hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateState {
    /// Submissions are refused.
    Disabled,
    /// Submissions are admitted (the state a topic starts in).
    Enabled,
}

impl GateState {
    /// The word stored in the table.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Enabled => "enabled",
        }
    }

    /// Parse a stored word.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "disabled" => Some(Self::Disabled),
            "enabled" => Some(Self::Enabled),
            _ => None,
        }
    }
}

/// The newest gate row for a topic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gate {
    /// Current state.
    pub state: GateState,
    /// Why the operator set it, in their words. Empty when they gave none.
    pub reason: String,
    /// Who set it (an operator label, never a token). Empty when unset.
    pub actor: String,
    /// Row id, for an audit trail.
    pub id: i64,
}

impl Gate {
    /// Whether submissions are refused right now.
    #[must_use]
    pub const fn is_disabled(&self) -> bool {
        matches!(self.state, GateState::Disabled)
    }
}

/// The gate for `topic_id`, or `None` when no operator ever set one.
///
/// `Ok(None)` is an **enabled** topic: no row means the switch was never
/// thrown. `Err` is the database refusing, which the caller must not read as
/// "enabled" — see the module docs.
///
/// # Errors
///
/// [`InstallError::Db`].
pub async fn gate(pool: &PgPool, topic_id: &str) -> Result<Option<Gate>, InstallError> {
    let row: Option<(i64, String, String, String)> = sqlx::query_as(
        "SELECT id, state, reason, actor FROM proof_topic_gate \
         WHERE topic_id = $1 ORDER BY id DESC LIMIT 1",
    )
    .bind(topic_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| InstallError::Db(e.to_string()))?;
    let Some((id, state, reason, actor)) = row else {
        return Ok(None);
    };
    // A stored word outside the CHECK cannot reach here; if one somehow did,
    // the honest answer is the database refusing, not a silent "enabled".
    let state = GateState::parse(&state).ok_or_else(|| {
        InstallError::Db(format!(
            "gate row {id} for topic {topic_id:?} holds an unknown state"
        ))
    })?;
    Ok(Some(Gate {
        state,
        reason,
        actor,
        id,
    }))
}

/// Whether `topic_id` is disabled right now.
///
/// The submit-path read: `Ok(false)` for a topic with no row, `Ok(true)` for
/// a disabled one, `Err` when the table cannot be read.
///
/// # Errors
///
/// [`InstallError::Db`].
pub async fn disabled(pool: &PgPool, topic_id: &str) -> Result<bool, InstallError> {
    Ok(gate(pool, topic_id).await?.is_some_and(|g| g.is_disabled()))
}

/// Every topic currently disabled, with the operator's reason, in one read.
///
/// For a listing that annotates a set of topics: one query rather than one
/// per topic. A topic whose newest row is `enabled` is absent, which is the
/// same answer [`disabled`] gives for it.
///
/// # Errors
///
/// [`InstallError::Db`], including a stored state the CHECK cannot produce.
pub async fn disabled_topics(pool: &PgPool) -> Result<BTreeMap<String, String>, InstallError> {
    let rows: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT DISTINCT ON (topic_id) topic_id, state, reason FROM proof_topic_gate \
         ORDER BY topic_id, id DESC",
    )
    .fetch_all(pool)
    .await
    .map_err(|e| InstallError::Db(e.to_string()))?;
    let mut out = BTreeMap::new();
    for (topic_id, state, reason) in rows {
        let state = GateState::parse(&state).ok_or_else(|| {
            InstallError::Db(format!(
                "the gate row for topic {topic_id:?} holds an unknown state"
            ))
        })?;
        if state == GateState::Disabled {
            out.insert(topic_id, reason);
        }
    }
    Ok(out)
}

/// Append a gate row and return it.
///
/// Append-only, like the install journal: `disable` after `enable` is a new
/// row, so "who turned it off, when, and why" survives the turn-on. The
/// `reason` is bounded (512 chars) and the `actor` (128); both are stored
/// verbatim, and neither is a secret — the reason is shown to a miner in the
/// 403.
///
/// # Errors
///
/// [`InstallError::Db`], including the CHECK refusing an over-long reason.
pub async fn set(
    pool: &PgPool,
    topic_id: &str,
    state: GateState,
    reason: &str,
    actor: &str,
) -> Result<Gate, InstallError> {
    let row: (i64,) = sqlx::query_as(
        "INSERT INTO proof_topic_gate (topic_id, state, reason, actor) VALUES ($1, $2, $3, $4) \
         RETURNING id",
    )
    .bind(topic_id)
    .bind(state.as_str())
    .bind(reason.trim())
    .bind(actor.trim())
    .fetch_one(pool)
    .await
    .map_err(|e| InstallError::Db(e.to_string()))?;
    Ok(Gate {
        state,
        reason: reason.trim().to_owned(),
        actor: actor.trim().to_owned(),
        id: row.0,
    })
}

/// [`set`] with [`GateState::Disabled`].
///
/// # Errors
///
/// [`InstallError::Db`].
pub async fn disable(
    pool: &PgPool,
    topic_id: &str,
    reason: &str,
    actor: &str,
) -> Result<Gate, InstallError> {
    set(pool, topic_id, GateState::Disabled, reason, actor).await
}

/// [`set`] with [`GateState::Enabled`]: the only way to clear a disable.
///
/// # Errors
///
/// [`InstallError::Db`].
pub async fn enable(
    pool: &PgPool,
    topic_id: &str,
    reason: &str,
    actor: &str,
) -> Result<Gate, InstallError> {
    set(pool, topic_id, GateState::Enabled, reason, actor).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stored_words_round_trip() {
        for state in [GateState::Disabled, GateState::Enabled] {
            assert_eq!(GateState::parse(state.as_str()), Some(state));
        }
        assert_eq!(GateState::parse(" disabled "), Some(GateState::Disabled));
        assert_eq!(GateState::parse("DISABLED"), None);
        assert_eq!(GateState::parse(""), None);
    }

    #[test]
    fn a_missing_row_is_not_a_disable() {
        let gate = Gate {
            state: GateState::Enabled,
            reason: String::new(),
            actor: String::new(),
            id: 1,
        };
        assert!(!gate.is_disabled());
        let gate = Gate {
            state: GateState::Disabled,
            reason: "incident 42".into(),
            actor: "ops".into(),
            id: 2,
        };
        assert!(gate.is_disabled());
    }
}
