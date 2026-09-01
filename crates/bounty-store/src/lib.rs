//! In-memory Bounty store: pairings, reports, champion.

#![forbid(unsafe_code)]
#![allow(
    clippy::missing_errors_doc,
    clippy::doc_markdown,
    clippy::must_use_candidate
)]

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use bounty_challenge_task::{
    hotkey_hex, MAX_PENDING_REPORTS_PER_HOTKEY, MIN_REPORT_INTERVAL_SECS, SESSION_DOMAIN,
};
use bounty_score::{judge_challenger, Adjudication, ChampionVerdict, MinerHoldout, Severity};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Report lifecycle after submit / adjudicate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportState {
    /// Waiting operator adjudication.
    Pending,
    /// Unique reproducing bug.
    Valid,
    /// Already fixed, not in prod. Ack only.
    AlreadyFixedNotProd,
    /// Malicious / fabricated.
    InvalidMalicious,
    /// Duplicate of an open report.
    Duplicate,
}

impl From<Adjudication> for ReportState {
    fn from(v: Adjudication) -> Self {
        match v {
            Adjudication::Valid => Self::Valid,
            Adjudication::AlreadyFixedNotProd => Self::AlreadyFixedNotProd,
            Adjudication::InvalidMalicious => Self::InvalidMalicious,
            Adjudication::Duplicate => Self::Duplicate,
        }
    }
}

/// Bound Cortex account ↔ miner hotkey.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pairing {
    /// Cortex Chat account id.
    pub account_id: String,
    /// 64-hex miner hotkey.
    pub miner_hotkey: String,
    /// Session claim id (hex).
    pub session_id: String,
    /// Unix-seconds bind time.
    pub bound_at: u64,
}

/// One miner bug report. Always tagged with the bound hotkey.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    /// Stable id (`by_` + 16 hex).
    pub id: String,
    /// 64-hex miner hotkey (attribution).
    pub miner_hotkey: String,
    /// Cortex Chat account id.
    pub account_id: String,
    /// Short title.
    pub title: String,
    /// Report body.
    pub body: String,
    /// Reproduction steps.
    pub repro_steps: String,
    /// Dedup fingerprint (hex).
    pub fingerprint: String,
    /// Lifecycle.
    pub state: ReportState,
    /// Operator verdict (if any).
    pub adjudication: Option<Adjudication>,
    /// Operator severity, set with a `valid` verdict. A valid report without
    /// one is not creditable.
    #[serde(default)]
    pub severity: Option<Severity>,
    /// If duplicate, the original report id.
    pub duplicate_of: Option<String>,
    /// Displacement verdict after this adjudication (if any).
    pub champion_verdict: Option<ChampionVerdict>,
    /// Unix-seconds submit time. Drives the per-hotkey rate window.
    #[serde(default)]
    pub created_at: u64,
}

/// Session claim returned by `POST /v1/pair`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionClaim {
    /// Opaque hex token.
    pub token: String,
    /// Bound account.
    pub account_id: String,
    /// Bound hotkey (64-hex).
    pub miner_hotkey: String,
    /// Session id.
    pub session_id: String,
}

/// Store errors.
#[derive(Debug, Error)]
pub enum StoreError {
    /// Lock poisoned.
    #[error("store lock poisoned")]
    Poison,
    /// Unknown row.
    #[error("unknown {0}")]
    NotFound(String),
    /// Illegal transition or reuse.
    #[error("{0}")]
    Illegal(String),
    /// The hotkey is over an ingest quota.
    ///
    /// Separate from [`Self::Illegal`] because the caller answers `429`, not
    /// `409`: the report is fine, the queue is not.
    #[error("{0}")]
    Quota(String),
}

/// In-memory store (v0).
#[derive(Clone, Default)]
pub struct MemoryStore {
    inner: Arc<Mutex<Inner>>,
}

#[derive(Default)]
struct Inner {
    next: u64,
    pairings: BTreeMap<String, Pairing>,
    sessions: BTreeMap<String, Pairing>,
    used_nonces: BTreeSet<String>,
    reports: BTreeMap<String, Report>,
    fingerprints: BTreeMap<String, String>,
    champion_hotkey: Option<String>,
}

impl MemoryStore {
    /// Empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Inner>, StoreError> {
        self.inner.lock().map_err(|_| StoreError::Poison)
    }

    /// Bind account ↔ hotkey after a verified pairing signature.
    pub fn bind_pair(
        &self,
        account_id: &str,
        miner_hotkey: &str,
        nonce: &str,
        now_unix: u64,
        session_secret: &[u8],
    ) -> Result<SessionClaim, StoreError> {
        let mut g = self.lock()?;
        if !g.used_nonces.insert(nonce.to_owned()) {
            return Err(StoreError::Illegal("nonce reused".into()));
        }
        let session_id = next_id(&mut g.next, "bs");
        let token = mint_session_token(session_secret, &session_id, account_id, miner_hotkey);
        let row = Pairing {
            account_id: account_id.to_owned(),
            miner_hotkey: miner_hotkey.to_owned(),
            session_id: session_id.clone(),
            bound_at: now_unix,
        };
        g.pairings.insert(account_id.to_owned(), row.clone());
        g.sessions.insert(session_id.clone(), row);
        Ok(SessionClaim {
            token,
            account_id: account_id.to_owned(),
            miner_hotkey: miner_hotkey.to_owned(),
            session_id,
        })
    }

    /// Resolve a session token to the bound pairing.
    pub fn lookup_session(
        &self,
        token: &str,
        session_secret: &[u8],
    ) -> Result<Pairing, StoreError> {
        let g = self.lock()?;
        for row in g.sessions.values() {
            let expect = mint_session_token(
                session_secret,
                &row.session_id,
                &row.account_id,
                &row.miner_hotkey,
            );
            if crypto_eq(&expect, token) {
                return Ok(row.clone());
            }
        }
        Err(StoreError::NotFound("session".into()))
    }

    /// Insert a pending report, subject to the per-hotkey ingest quotas.
    ///
    /// Same fingerprint as an open/valid report → duplicate.
    ///
    /// The quotas exist because adjudication, not storage, is what this
    /// challenge is short of: one miner filling the queue starves every other
    /// miner's reports of the triage pass that turns them into weight.
    pub fn insert_report(&self, mut row: Report, now_unix: u64) -> Result<Report, StoreError> {
        let mut g = self.lock()?;
        let pending = g
            .reports
            .values()
            .filter(|r| r.miner_hotkey == row.miner_hotkey && r.state == ReportState::Pending)
            .count();
        if pending >= MAX_PENDING_REPORTS_PER_HOTKEY {
            return Err(StoreError::Quota(format!(
                "{pending} reports already awaiting adjudication for this hotkey \
                 (max {MAX_PENDING_REPORTS_PER_HOTKEY})"
            )));
        }
        let last = g
            .reports
            .values()
            .filter(|r| r.miner_hotkey == row.miner_hotkey)
            .map(|r| r.created_at)
            .max()
            .unwrap_or(0);
        if last > 0 && now_unix.saturating_sub(last) < MIN_REPORT_INTERVAL_SECS {
            return Err(StoreError::Quota(format!(
                "one report per {MIN_REPORT_INTERVAL_SECS}s per hotkey"
            )));
        }
        row.created_at = now_unix;
        if row.id.is_empty() {
            row.id = next_id(&mut g.next, "by");
        }
        if let Some(orig) = g.fingerprints.get(&row.fingerprint) {
            if let Some(existing) = g.reports.get(orig) {
                if matches!(existing.state, ReportState::Pending | ReportState::Valid) {
                    row.state = ReportState::Duplicate;
                    row.adjudication = Some(Adjudication::Duplicate);
                    row.duplicate_of = Some(orig.clone());
                }
            }
        } else {
            g.fingerprints
                .insert(row.fingerprint.clone(), row.id.clone());
        }
        g.reports.insert(row.id.clone(), row.clone());
        Ok(row)
    }

    /// Fetch one report.
    pub fn get_report(&self, id: &str) -> Result<Report, StoreError> {
        let g = self.lock()?;
        g.reports
            .get(id)
            .cloned()
            .ok_or_else(|| StoreError::NotFound(id.to_owned()))
    }

    /// List reports newest-first.
    pub fn list_reports(&self) -> Result<Vec<Report>, StoreError> {
        let g = self.lock()?;
        let mut rows: Vec<_> = g.reports.values().cloned().collect();
        rows.sort_by(|a, b| b.id.cmp(&a.id));
        Ok(rows)
    }

    /// Operator adjudicate. Updates champion when the reporter displaces.
    ///
    /// `severity` prices a `valid` verdict. Omitting it does not silently
    /// drop the report: it lands as an unpriced valid, which the severity
    /// evidence gate refuses to crown.
    pub fn adjudicate(
        &self,
        id: &str,
        verdict: Adjudication,
        severity: Option<Severity>,
        duplicate_of: Option<String>,
    ) -> Result<Report, StoreError> {
        let mut g = self.lock()?;
        {
            let row = g
                .reports
                .get(id)
                .ok_or_else(|| StoreError::NotFound(id.to_owned()))?;
            if row.state != ReportState::Pending
                && row.adjudication != Some(Adjudication::Duplicate)
            {
                return Err(StoreError::Illegal("already adjudicated".into()));
            }
        }
        if verdict == Adjudication::Duplicate {
            let orig = duplicate_of
                .as_ref()
                .ok_or_else(|| StoreError::Illegal("duplicate_of required".into()))?;
            if !g.reports.contains_key(orig) {
                return Err(StoreError::NotFound(orig.clone()));
            }
        }
        let hotkey = {
            let row = g
                .reports
                .get_mut(id)
                .ok_or_else(|| StoreError::NotFound(id.to_owned()))?;
            row.state = ReportState::from(verdict);
            row.adjudication = Some(verdict);
            row.severity = if verdict == Adjudication::Valid {
                severity
            } else {
                None
            };
            row.duplicate_of = duplicate_of;
            row.miner_hotkey.clone()
        };

        let chall = holdout_for(&g.reports, &hotkey);
        let champ = g
            .champion_hotkey
            .as_ref()
            .map(|h| holdout_for(&g.reports, h))
            .unwrap_or_default();
        let cv = judge_challenger(&champ, &chall);
        if cv.eligible {
            g.champion_hotkey = Some(hotkey);
        }
        let row = g
            .reports
            .get_mut(id)
            .ok_or_else(|| StoreError::NotFound(id.to_owned()))?;
        row.champion_verdict = Some(cv);
        Ok(row.clone())
    }

    /// Live champion hotkey (64-hex), if any.
    pub fn champion_hotkey(&self) -> Result<Option<String>, StoreError> {
        Ok(self.lock()?.champion_hotkey.clone())
    }

    /// Holdout tallies for a hotkey.
    pub fn holdout(&self, hotkey: &str) -> Result<MinerHoldout, StoreError> {
        let g = self.lock()?;
        Ok(holdout_for(&g.reports, hotkey))
    }

    /// Pairing for an account, if bound.
    pub fn pairing_for_account(&self, account_id: &str) -> Result<Option<Pairing>, StoreError> {
        Ok(self.lock()?.pairings.get(account_id).cloned())
    }
}

fn holdout_for(reports: &BTreeMap<String, Report>, hotkey: &str) -> MinerHoldout {
    let mut h = MinerHoldout::default();
    for r in reports.values() {
        if r.miner_hotkey != hotkey {
            continue;
        }
        if let Some(v) = r.adjudication {
            h.record(v, r.severity);
        }
    }
    h
}

fn next_id(next: &mut u64, prefix: &str) -> String {
    let n = *next;
    *next = next.saturating_add(1);
    format!("{prefix}_{n:016x}")
}

fn mint_session_token(secret: &[u8], session_id: &str, account_id: &str, hotkey: &str) -> String {
    let mut h = Sha256::new();
    h.update(SESSION_DOMAIN);
    h.update(secret);
    h.update(session_id.as_bytes());
    h.update(account_id.as_bytes());
    h.update(hotkey.as_bytes());
    hex::encode(h.finalize())
}

fn crypto_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.as_bytes()
        .iter()
        .zip(b.as_bytes())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

/// SHA-256 fingerprint of normalized title+body.
#[must_use]
pub fn report_fingerprint(title: &str, body: &str) -> String {
    let mut h = Sha256::new();
    h.update(REPORT_TAG);
    h.update(title.trim().to_ascii_lowercase().as_bytes());
    h.update([0xff]);
    h.update(body.trim().to_ascii_lowercase().as_bytes());
    hex::encode(h.finalize())
}

const REPORT_TAG: &[u8] = bounty_challenge_task::REPORT_DOMAIN;

/// Helper: 64-hex from a raw hotkey.
#[must_use]
pub fn hex_hotkey(bytes: &[u8; 32]) -> String {
    hotkey_hex(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secret() -> [u8; 16] {
        [7u8; 16]
    }

    #[test]
    fn pair_and_session_round_trip() {
        let s = MemoryStore::new();
        let claim = s
            .bind_pair("acct", &"aa".repeat(32), "0123456789abcdef", 10, &secret())
            .expect("bind");
        let row = s.lookup_session(&claim.token, &secret()).expect("lookup");
        assert_eq!(row.account_id, "acct");
        assert!(s
            .bind_pair("acct", &"aa".repeat(32), "0123456789abcdef", 11, &secret())
            .is_err());
    }

    fn report(hotkey: &str, title: &str, body: &str) -> Report {
        Report {
            id: String::new(),
            miner_hotkey: hotkey.to_owned(),
            account_id: "acct".into(),
            title: title.to_owned(),
            body: body.to_owned(),
            repro_steps: "curl the route and watch it 500".into(),
            fingerprint: report_fingerprint(title, body),
            state: ReportState::Pending,
            adjudication: None,
            severity: None,
            duplicate_of: None,
            champion_verdict: None,
            created_at: 0,
        }
    }

    #[test]
    fn duplicate_fingerprint_auto_marks() {
        let s = MemoryStore::new();
        let a = s
            .insert_report(report(&"aa".repeat(32), "same bug", "steps"), 1_000)
            .expect("a");
        assert_eq!(a.state, ReportState::Pending);
        let b = s
            .insert_report(report(&"bb".repeat(32), "same bug", "steps"), 1_000)
            .expect("b");
        assert_eq!(b.state, ReportState::Duplicate);
        assert_eq!(b.duplicate_of.as_deref(), Some(a.id.as_str()));
    }

    #[test]
    fn adjudicate_valid_can_crown() {
        let s = MemoryStore::new();
        let hk = "cc".repeat(32);
        let mut last = String::new();
        for i in 0..3 {
            let row = s
                .insert_report(
                    report(&hk, &format!("bug {i}"), &format!("body {i}")),
                    1_000 + i * MIN_REPORT_INTERVAL_SECS,
                )
                .expect("ins");
            last = row.id;
            s.adjudicate(&last, Adjudication::Valid, Some(Severity::Major), None)
                .expect("adj");
        }
        let row = s.get_report(&last).expect("get");
        assert_eq!(row.adjudication, Some(Adjudication::Valid));
        assert_eq!(row.severity, Some(Severity::Major));
        assert_eq!(
            s.champion_hotkey().expect("champ").as_deref(),
            Some(hk.as_str())
        );
        assert!(row.champion_verdict.expect("verdict").eligible);
    }

    /// Adjudication is the scarce resource: one hotkey must not be able to
    /// fill the queue and starve everyone else's reports of a triage pass.
    #[test]
    fn a_hotkey_cannot_flood_the_adjudication_queue() {
        let s = MemoryStore::new();
        let hk = "dd".repeat(32);
        let mut at = 1_000;
        for i in 0..MAX_PENDING_REPORTS_PER_HOTKEY {
            s.insert_report(report(&hk, &format!("bug {i}"), &format!("body {i}")), at)
                .expect("within quota");
            at += MIN_REPORT_INTERVAL_SECS;
        }
        let err = s
            .insert_report(report(&hk, "one too many", "body"), at)
            .expect_err("over quota");
        assert!(matches!(err, StoreError::Quota(_)), "{err}");

        // Another hotkey is unaffected: the quota is per miner, not global.
        s.insert_report(report(&"ee".repeat(32), "someone else", "body"), at)
            .expect("other hotkey");

        // Clearing the queue frees the slot again.
        let pending = s
            .list_reports()
            .expect("list")
            .into_iter()
            .find(|r| r.miner_hotkey == hk)
            .expect("pending");
        s.adjudicate(&pending.id, Adjudication::InvalidMalicious, None, None)
            .expect("adjudicate");
        s.insert_report(report(&hk, "after triage", "body"), at + 10_000)
            .expect("slot freed");
    }

    #[test]
    fn reports_from_one_hotkey_are_rate_limited() {
        let s = MemoryStore::new();
        let hk = "ff".repeat(32);
        s.insert_report(report(&hk, "first", "body"), 1_000)
            .expect("first");
        let err = s
            .insert_report(report(&hk, "second", "body"), 1_001)
            .expect_err("too fast");
        assert!(matches!(err, StoreError::Quota(_)), "{err}");
        s.insert_report(report(&hk, "second", "body"), 1_000 + MIN_REPORT_INTERVAL_SECS)
            .expect("after the window");
    }

    /// A valid verdict with no severity is recorded as unpriced rather than
    /// dropped, so the scoring gate sees it.
    #[test]
    fn a_valid_verdict_without_severity_is_unpriced_not_silent() {
        let s = MemoryStore::new();
        let hk = "ab".repeat(32);
        let row = s
            .insert_report(report(&hk, "unpriced", "body"), 1_000)
            .expect("ins");
        s.adjudicate(&row.id, Adjudication::Valid, None, None)
            .expect("adj");
        let tally = s.holdout(&hk).expect("tally");
        assert_eq!(tally.valid_unpriced, 1);
        assert_eq!(tally.valid(), 0);
    }
}
