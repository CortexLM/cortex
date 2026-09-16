//! The committed trust root must keep bounty live and payable.
//!
//! Bounty's reward linkage has a precondition that lives outside its own code:
//! the challenge has to be in the owner-signed trust root with a nonzero
//! emission share, and the keys the emitter signs with have to be the keys that
//! root names. Nothing in the emitter can check either one — a host whose
//! `bounty_sk` does not match the root's `bounty` row signs leaves that every
//! validator rejects, and a host whose root has no bounty row emits into a
//! challenge that does not exist.
//!
//! These tests read the files an operator actually deploys
//! (`config/challenges.toml` and the staging override) and verify them the way
//! a validator does: owner signature, then the row the reward path depends on.
//! The point is not to restate the numbers — it is that changing them is a
//! decision, not a drift.

#![forbid(unsafe_code)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};

use bounty_challenge_task::{CHALLENGE_ID, SCORING_VERSION};
use trustroot::{load_challenges_file, ChallengesBody, ChallengesToml, BPS_DENOM};

/// Bounty's committed share. Proof holds the remainder; the pair must sum to
/// `BPS_DENOM` or `ChallengesBody::validate` refuses the document outright.
const BOUNTY_BPS: u16 = 2000;

/// Proof's committed share (20/80 with bounty).
const PROOF_BPS: u16 = 8000;

fn repo_config_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../config")
        .canonicalize()
        .expect("config dir")
}

fn owner_public() -> [u8; 32] {
    trustroot::load_owner_public_key(&repo_config_dir().join("owner.pubkey")).expect("owner pubkey")
}

/// Load a root the way a validator does: verify the owner signature first, so
/// a hand-edited row without a re-sign fails here rather than in production.
fn verified(body_path: &Path) -> ChallengesBody {
    load_challenges_file(body_path, &owner_public())
        .unwrap_or_else(|e| {
            panic!(
                "{} must verify under the committed owner: {e}",
                body_path.display()
            )
        })
        .body
}

fn row<'a>(body: &'a ChallengesBody, id: &str) -> &'a trustroot::ChallengeEntry {
    body.get(id.as_bytes())
        .unwrap_or_else(|| panic!("{id} row missing from the trust root"))
}

fn files() -> Vec<(&'static str, PathBuf)> {
    let dir = repo_config_dir();
    vec![
        ("prod", dir.join("challenges.toml")),
        ("staging", dir.join("challenges.staging.toml")),
    ]
}

/// Bounty is a live, payable challenge in every committed root.
///
/// An emission share of 0 is not a degraded bounty — it is a challenge that
/// cannot pay anyone while still being asked for leaves, and `assert_
/// participant_completeness` skips 0-bps rows entirely, so the emitter would be
/// posting into a set nothing seals.
#[test]
fn bounty_is_live_and_payable_in_every_committed_root() {
    for (label, path) in files() {
        let body = verified(&path);
        body.validate()
            .unwrap_or_else(|e| panic!("{label}: committed root must validate: {e}"));

        let bounty = row(&body, CHALLENGE_ID);
        assert_eq!(
            bounty.emission_share_bps, BOUNTY_BPS,
            "{label}: bounty share drifted from the committed {BOUNTY_BPS} bps"
        );
        assert!(
            bounty.emission_share_bps > 0,
            "{label}: a 0-bps bounty is not payable"
        );
        assert_eq!(
            bounty.policy,
            trustroot::ParticipantPolicy::AllMetagraphHotkeys,
            "{label}: bounty derives E from the whole metagraph"
        );

        // Proof is the other live row; the two shares are the whole budget.
        let proof = row(&body, "proof");
        assert_eq!(proof.emission_share_bps, PROOF_BPS, "{label}: proof share");

        let total: u32 = body
            .challenges
            .iter()
            .map(|c| u32::from(c.emission_share_bps))
            .sum();
        assert_eq!(
            total,
            u32::from(BPS_DENOM),
            "{label}: shares must sum to {BPS_DENOM}"
        );
        assert_eq!(body.challenges.len(), 2, "{label}: two live challenges");
    }
}

/// The challenge id the emitter signs under is the id the root publishes.
///
/// `CHALLENGE_ID_BYTES` is what `emit_signed_leaf_set` puts in every leaf, and
/// `ChallengesBody::get` is what the gateway and validators look it up by. A
/// mismatch would make every leaf unattributable — and it would look like a
/// signature failure, which is the wrong place to go looking.
#[test]
fn the_emitters_challenge_id_is_the_trust_roots_id() {
    assert_eq!(CHALLENGE_ID, "bounty");
    for (label, path) in files() {
        let body = verified(&path);
        let ids: Vec<String> = body
            .challenges
            .iter()
            .map(|c| String::from_utf8_lossy(&c.id).into_owned())
            .collect();
        assert!(
            ids.iter().any(|id| id == CHALLENGE_ID),
            "{label}: no row for the id the emitter signs under: {ids:?}"
        );
    }
}

/// The retired products stay absent. A leftover row would restore an emission
/// the owner removed, and `relearn` / `design` / `prism` have no code left to
/// serve it.
#[test]
fn retired_products_have_no_row_in_any_committed_root() {
    for (label, path) in files() {
        let body = verified(&path);
        for off in [
            "relearn",
            "relearn-image",
            "relearn-agent",
            "relearn-mm",
            "design",
            "prism",
        ] {
            assert!(
                body.get(off.as_bytes()).is_none(),
                "{label}: {off} must not have a trust-root row"
            );
        }
    }
}

/// Staging mirrors production's split.
///
/// Staging exists to prove the production path, so a split that diverges there
/// would let the staging soak pass while prod seals a different vector. The
/// validator compares emission shares against its local root (D23), so a
/// divergence also shows up as a staging-only failure.
#[test]
fn staging_mirrors_prod_shares_and_keys() {
    let dir = repo_config_dir();
    let prod = verified(&dir.join("challenges.toml"));
    let staging = verified(&dir.join("challenges.staging.toml"));

    for id in [CHALLENGE_ID, "proof"] {
        let p = row(&prod, id);
        let s = row(&staging, id);
        assert_eq!(
            p.emission_share_bps, s.emission_share_bps,
            "{id}: staging share must mirror prod"
        );
        assert_eq!(
            p.public_key, s.public_key,
            "{id}: staging key must mirror prod, or a staging leaf cannot be \
             verified against the same trust root shape"
        );
    }
}

/// The committed roots parse as the deployed TOML shape, and the parsed body
/// re-derives the same rows. This catches a file that verifies but carries a
/// field the loader drops.
#[test]
fn committed_roots_round_trip_through_the_toml_shape() {
    for (label, path) in files() {
        let text = std::fs::read_to_string(&path).expect("read");
        let doc: ChallengesToml = toml::from_str(&text).expect("toml shape");
        let body = doc.to_body().expect("to_body");
        assert_eq!(
            body,
            verified(&path),
            "{label}: the parsed document must equal the verified body"
        );
        assert_eq!(doc.version, 1, "{label}: document version");
        assert_eq!(doc.introduced_epoch, 0, "{label}: introduced_epoch");
    }
}

/// The scoring version is part of the published identity a miner checks.
#[test]
fn the_scoring_version_is_pinned() {
    assert_eq!(SCORING_VERSION, 1);
}
