//! Honest mappers from live challenge JSON → site contract (no invented scores).

use std::collections::HashMap;

use serde_json::Value;

use keystore::{ss58_encode, BITTENSOR_SS58_PREFIX};
use site_types::{ActivityEvent, Agent, Arena, LeaderboardRow, Submission};

pub use crate::timefmt::{ms_to_clock, ms_to_iso};

/// SS58-encode a 32-byte hex hotkey; `None` for anything else.
#[must_use]
fn ss58_from_hex(hk: &str) -> Option<String> {
    let bytes: [u8; 32] = hex::decode(hk).ok()?.try_into().ok()?;
    Some(ss58_encode(&bytes, BITTENSOR_SS58_PREFIX))
}

/// `first…last` middle truncation for display; short values pass through.
#[must_use]
fn truncate_middle(v: &str) -> String {
    if v.chars().count() > 16 {
        let head: String = v.chars().take(8).collect();
        let tail: String = v
            .chars()
            .rev()
            .take(4)
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        format!("{head}…{tail}")
    } else {
        v.to_owned()
    }
}

/// Hotkey → agent shell (no invented miner numbers / models).
#[must_use]
pub fn agent_from_hotkey(hotkey: &str, joined_epoch: u64) -> Agent {
    let hk = hotkey.trim();
    let display = ss58_from_hex(hk).unwrap_or_else(|| hk.to_owned());
    let known = !display.is_empty() && display != "—";
    let slug = if known {
        display
            .chars()
            .take(8)
            .collect::<String>()
            .to_ascii_lowercase()
    } else {
        "unknown".to_owned()
    };
    Agent {
        slug: slug.clone(),
        handle: format!("@{slug}"),
        miner_number: "—".into(),
        model: "—".into(),
        operator: truncate_middle(&display),
        hotkey: if known { display } else { "—".into() },
        uid: None,
        joined_epoch,
    }
}

/// Attach an on-chain UID (and zero-padded `miner_number`) when known.
pub fn apply_agent_uid(agent: &mut Agent, uid: Option<u16>) {
    let Some(uid) = uid else {
        return;
    };
    agent.uid = Some(uid);
    agent.miner_number = format!("{uid:03}");
}

/// Resolve a hotkey (hex or SS58) against a map keyed by lowercase hex + SS58.
#[must_use]
pub fn lookup_uid<S: ::std::hash::BuildHasher>(
    uid_by_key: &HashMap<String, u16, S>,
    hotkey: &str,
) -> Option<u16> {
    let hk = hotkey.trim();
    if hk.is_empty() || hk == "—" {
        return None;
    }
    if let Some(uid) = uid_by_key.get(hk) {
        return Some(*uid);
    }
    let lower = hk.to_ascii_lowercase();
    if let Some(uid) = uid_by_key.get(&lower) {
        return Some(*uid);
    }
    if let Some(ss58) = ss58_from_hex(hk) {
        return uid_by_key.get(&ss58).copied();
    }
    None
}

/// Build a hotkey → UID index from a metagraph's UID-ordered hotkey list.
#[must_use]
pub fn uid_index_from_hotkeys(hotkeys: &[Vec<u8>]) -> HashMap<String, u16> {
    let mut map = HashMap::with_capacity(hotkeys.len().saturating_mul(2));
    for (i, hk) in hotkeys.iter().enumerate() {
        let Ok(uid) = u16::try_from(i) else {
            continue;
        };
        let Ok(arr) = <[u8; 32]>::try_from(hk.as_slice()) else {
            continue;
        };
        let hex = hex::encode(arr);
        map.insert(hex, uid);
        map.insert(ss58_encode(&arr, BITTENSOR_SS58_PREFIX), uid);
    }
    map
}

/// Attach UID / miner number onto every leaderboard agent from a metagraph index.
pub fn enrich_leaderboard_uids<S: ::std::hash::BuildHasher>(
    rows: &mut [LeaderboardRow],
    uid_by_key: &HashMap<String, u16, S>,
) {
    for row in rows {
        let uid = lookup_uid(uid_by_key, &row.agent.hotkey);
        apply_agent_uid(&mut row.agent, uid);
    }
}

/// Attach UID / miner number onto every submission agent from a metagraph index.
pub fn enrich_submission_uids<S: ::std::hash::BuildHasher>(
    rows: &mut [Submission],
    uid_by_key: &HashMap<String, u16, S>,
) {
    for row in rows {
        let uid = lookup_uid(uid_by_key, &row.agent.hotkey);
        apply_agent_uid(&mut row.agent, uid);
    }
}

/// Attach sealed weight share + estimated TAO/day onto leaderboard rows.
pub fn enrich_leaderboard_weights<S: ::std::hash::BuildHasher>(
    rows: &mut [LeaderboardRow],
    weight_by_hotkey: &HashMap<String, f64, S>,
    emission_per_day: f64,
) {
    for row in rows {
        let w = lookup_weight(weight_by_hotkey, &row.agent.hotkey);
        let Some(weight) = w else {
            continue;
        };
        row.weight = Some(weight);
        if emission_per_day > 0.0 {
            row.tao_per_day = Some(weight * emission_per_day);
        }
    }
}

fn lookup_weight<S: ::std::hash::BuildHasher>(
    weight_by_hotkey: &HashMap<String, f64, S>,
    hotkey: &str,
) -> Option<f64> {
    let hk = hotkey.trim();
    if hk.is_empty() || hk == "—" {
        return None;
    }
    if let Some(w) = weight_by_hotkey.get(hk) {
        return Some(*w);
    }
    let lower = hk.to_ascii_lowercase();
    if let Some(w) = weight_by_hotkey.get(&lower) {
        return Some(*w);
    }
    if let Some(ss58) = ss58_from_hex(hk) {
        return weight_by_hotkey.get(&ss58).copied();
    }
    None
}

/// Fill an arena card from that challenge's `/v1/status`.
///
/// A down backend leaves the static frame in place rather than dropping the
/// arena, so the site's emission column still accounts for every live challenge.
#[must_use]
pub fn hydrate_arena(mut arena: Arena, status: Option<&Value>) -> Arena {
    if let Some(s) = status {
        if let Some(id) = s.get("champion_id").and_then(|v| v.as_str()) {
            if !id.is_empty() {
                id.clone_into(&mut arena.best_score);
            }
        }
        arena.status = "live".into();
    }
    arena
}

/// Activity feed for live challenges. Empty until bounty/proof publish an
/// ops-style event stream (never invent design/prism copy).
#[must_use]
pub fn activity_from_lives(limit: usize) -> Vec<ActivityEvent> {
    let _ = limit;
    Vec::new()
}

fn hex_from_ss58_hotkey(ss58: &str) -> Option<String> {
    let (bytes, prefix) = keystore::ss58_decode(ss58).ok()?;
    if prefix != BITTENSOR_SS58_PREFIX {
        return None;
    }
    Some(hex::encode(bytes))
}

fn agent_matches_query(agent: &Agent, needle: &str) -> bool {
    let hay: [&str; 4] = [
        agent.hotkey.as_str(),
        agent.operator.as_str(),
        agent.slug.as_str(),
        agent.handle.as_str(),
    ];
    if hay.iter().any(|h| h.to_ascii_lowercase().contains(needle)) {
        return true;
    }
    if let Some(ss58) = ss58_from_hex(needle) {
        if agent.hotkey.eq_ignore_ascii_case(&ss58) {
            return true;
        }
    }
    if let Some(hex_hk) = hex_from_ss58_hotkey(&agent.hotkey) {
        if hex_hk.contains(needle) {
            return true;
        }
    }
    false
}

/// Case-insensitive substring match against hotkey / handle / prompt / id.
#[must_use]
pub fn submission_matches_query(sub: &Submission, q: &str) -> bool {
    let needle = q.trim().to_ascii_lowercase();
    if needle.is_empty() {
        return true;
    }
    if agent_matches_query(&sub.agent, &needle) {
        return true;
    }
    let hay: [&str; 4] = [
        sub.prompt_id.as_str(),
        sub.prompt_title.as_deref().unwrap_or(""),
        sub.title.as_str(),
        sub.id.as_str(),
    ];
    hay.iter().any(|h| h.to_ascii_lowercase().contains(&needle))
}

/// Leaderboard row match for `?q=` (hotkey / handle / slug / operator).
#[must_use]
pub fn leaderboard_matches_query(row: &LeaderboardRow, q: &str) -> bool {
    let needle = q.trim().to_ascii_lowercase();
    if needle.is_empty() {
        return true;
    }
    agent_matches_query(&row.agent, &needle)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use site_types::{bounty_frame, proof_frame};

    #[test]
    fn hydrate_keeps_static_frame_when_backend_is_down() {
        let a = hydrate_arena(bounty_frame(), None);
        assert_eq!(a.status, "live");
        let live = serde_json::json!({"champion_id": "abc"});
        let p = hydrate_arena(proof_frame(), Some(&live));
        assert_eq!(p.best_score, "abc");
        assert_eq!(p.status, "live");
    }

    #[test]
    fn activity_is_empty_without_invented_copy() {
        assert!(activity_from_lives(10).is_empty());
    }
}
