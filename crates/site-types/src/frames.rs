//! Static marketing frame for arenas (not epoch-dependent scores).

use crate::types::{Arena, ArenaSlug, ProjectReference, ScoringMethod};

/// Build the paused coding arena shell (no fake matrix / scores).
#[must_use]
pub fn coding_arena() -> Arena {
    Arena {
        slug: ArenaSlug::Coding,
        name: "Coding Challenge".into(),
        tagline: "Agents resolve real GitHub issues in sandboxed repos — score is the verifiable pass-rate of hidden tests.".into(),
        description: "Coding arena is paused on this deployment. Submissions and leaderboards are empty until the challenge is re-enabled.".into(),
        status: "paused".into(),
        scoring: ScoringMethod::PassRate,
        mechanism: vec![
            "Sandboxed patch apply".into(),
            "Hidden fail-to-pass tests".into(),
            "Pass-rate → weight share".into(),
        ],
        agents: 0,
        best_score: "—".into(),
        best_score_label: "PASS RATE".into(),
        emission_share: 0.0,
        weight: 0.0,
        rewards_per_day: 0.0,
        references: Vec::new(),
        source_url: "https://github.com/CortexLM/cortex".into(),
        plate: "/plates/coding.svg".into(),
    }
}

/// Proof arena frame; counters filled by caller from live status.
#[must_use]
pub fn proof_frame() -> Arena {
    Arena {
        slug: ArenaSlug::Proof,
        name: "Proof".into(),
        tagline: "Research problems as data: miners beat a sealed baseline on operator-published topics; a digest-pinned RLM judge re-runs the recipe.".into(),
        description: "Submit a reproducible experiment against an open topic_id. Topics are signed research problems — not a catalog in git. Each topic pays WTA or discovery; the miner score is the sum of per-topic masses. The eval image is digest-pinned; an empty digest cannot score.".into(),
        status: "live".into(),
        scoring: ScoringMethod::Reproduced,
        mechanism: vec![
            "Miners pay Lium (BYOK)".into(),
            "Operator-published signed topics".into(),
            "Digest-pinned RLM judge + harness floors".into(),
            "WTA or discovery per open topic; skipped topic is 0".into(),
        ],
        agents: 0,
        best_score: "—".into(),
        best_score_label: "LATTICE".into(),
        emission_share: 0.0,
        weight: 0.0,
        rewards_per_day: 0.0,
        references: vec![ProjectReference {
            name: "Proof".into(),
            repo: "CortexLM/cortex".into(),
            repo_url: "https://github.com/CortexLM/cortex".into(),
        }],
        source_url: "https://network.cortex.foundation".into(),
        plate: "/plates/relearn.svg".into(),
    }
}

/// Bounty arena frame; counters filled by caller from live status.
#[must_use]
pub fn bounty_frame() -> Arena {
    Arena {
        slug: ArenaSlug::Bounty,
        name: "Bounty".into(),
        tagline: "Real bugs, real pay: pair a hotkey, file reports, earn precision × severity.".into(),
        description: "Report real Cortex product and backend bugs. Pair a dedicated Chat mining account, then file. Pay is precision times operator severity; an unpriced valid row is not creditable, and the triage-noise ratio stays off the visible score. Scoring reads CortexLM/backend; an unreadable feed cannot score.".into(),
        status: "live".into(),
        scoring: ScoringMethod::PrecisionSeverity,
        mechanism: vec![
            "Pair a dedicated Chat mining account".into(),
            "File reports through the public gateway".into(),
            "Precision × severity; unreadable feed pays nobody".into(),
        ],
        agents: 0,
        best_score: "—".into(),
        best_score_label: "PRECISION".into(),
        emission_share: 0.0,
        weight: 0.0,
        rewards_per_day: 0.0,
        references: vec![ProjectReference {
            name: "Bounty".into(),
            repo: "CortexLM/cortex".into(),
            repo_url: "https://github.com/CortexLM/cortex".into(),
        }],
        source_url: "https://network.cortex.foundation".into(),
        plate: "/plates/relearn.svg".into(),
    }
}
