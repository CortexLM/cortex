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
        round_id: None,
        round_ends_at: None,
        seconds_remaining: None,
    }
}

/// Design arena frame; counters filled by caller from live dashboard.
#[must_use]
pub fn design_frame() -> Arena {
    Arena {
        slug: ArenaSlug::Design,
        name: "Design Arena".into(),
        tagline: "Agents turn a product brief into a working landing page; operators award 1–2 winners per round.".into(),
        description: "Miners submit an agent harness that produces sanitised HTML pages for a prompt set. Clean runs reach admin review; winners receive lattice scores and Elo-style ratings for the round.".into(),
        status: "retired".into(),
        scoring: ScoringMethod::Elo,
        mechanism: vec![
            "Harness → sandboxed pages".into(),
            "Sanitize + agentic anti-cheat".into(),
            "Admin winners → rating / leaf".into(),
        ],
        agents: 0,
        best_score: "—".into(),
        best_score_label: "ELO".into(),
        emission_share: 0.0,
        weight: 0.0,
        rewards_per_day: 0.0,
        references: vec![ProjectReference {
            name: "Design challenge".into(),
            repo: "CortexLM/cortex".into(),
            repo_url: "https://github.com/CortexLM/cortex".into(),
        }],
        source_url: "https://github.com/CortexLM/cortex".into(),
        plate: "/plates/design.svg".into(),
        round_id: None,
        round_ends_at: None,
        seconds_remaining: None,
    }
}

/// Prism arena frame; counters filled by caller from live status/list.
#[must_use]
pub fn prism_frame() -> Arena {
    Arena {
        slug: ArenaSlug::Prism,
        name: "Prism".into(),
        tagline: "Prism v2.1 — AutoModel pin+patch, 4h train on 1× B200, dense 1B reference (850M–1B). Public board is G2 lattice; 2.0 harvests cannot win.".into(),
        description: "New competition (prism-v2.1, scoring generation 21, recipe 2.1.0). Every miner trains inside the operator-owned recipe on the same pinned shard, seed, and caps. The public board lists only v2.1 harvests. Until the first eligible 2.1 run finishes, subnet weights stay burn (uid 0). Rankings use measured G2 / G1–G8 fields — never invented curves.".into(),
        status: "retired".into(),
        scoring: ScoringMethod::SpectralFusion,
        mechanism: vec![
            "Recipe 2.1.0 · prism-v2.1 · 4h / 1× B200".into(),
            "G2 lattice (0–100) + transparent G1–G8 stats".into(),
            "Agentic / similarity gate → leaf (WTA)".into(),
        ],
        agents: 0,
        best_score: "—".into(),
        best_score_label: "BEST G2".into(),
        emission_share: 0.0,
        weight: 0.0,
        rewards_per_day: 0.0,
        references: vec![ProjectReference {
            name: "PRISM recipe".into(),
            repo: "CortexLM/cortex".into(),
            repo_url: "https://github.com/CortexLM/cortex".into(),
        }],
        source_url: "https://github.com/CortexLM/cortex".into(),
        plate: "/plates/prism.svg".into(),
        round_id: None,
        round_ends_at: None,
        seconds_remaining: None,
    }
}

/// Relearn Image arena frame; counters filled by caller from live status.
#[must_use]
pub fn relearn_image_frame() -> Arena {
    Arena {
        slug: ArenaSlug::RelearnImage,
        name: "Relearn Image".into(),
        tagline: "Image generation: miners fine-tune Cosmos3-Super-Text2Image; Q-Judger scores frozen prompts at shared seeds.".into(),
        description: "Fine-tune the pinned NVIDIA Cosmos3 generator (OpenMDW 1.1). Every miner renders the same Qwen-Image-Bench prompt ids at the same derived seeds, so images are directly comparable. Q-Judger is the only judge. Promotion needs a paired win on a private prompt holdout with no L1 pillar regression. Flux-family bases are rejected.".into(),
        status: "live".into(),
        scoring: ScoringMethod::Displacement,
        mechanism: vec![
            "Miners pay Lium (BYOK)".into(),
            "Q-Judger on frozen prompts at shared seeds".into(),
            "No pillar may regress; seed replay + contamination gates".into(),
            "Capability canary off the paid score; regression is a hard zero".into(),
        ],
        agents: 0,
        best_score: "—".into(),
        best_score_label: "DISPLACE".into(),
        emission_share: 1.0,
        weight: 1.0,
        rewards_per_day: 0.0,
        references: vec![ProjectReference {
            name: "Relearn".into(),
            repo: "CortexLM/relearn".into(),
            repo_url: "https://github.com/CortexLM/relearn".into(),
        }],
        source_url: "https://github.com/CortexLM/relearn".into(),
        plate: "/plates/relearn.svg".into(),
        round_id: None,
        round_ends_at: None,
        seconds_remaining: None,
    }
}

/// Relearn Agent arena frame; counters filled by caller from live status.
#[must_use]
pub fn relearn_agent_frame() -> Arena {
    Arena {
        slug: ArenaSlug::RelearnAgent,
        name: "Relearn Agent".into(),
        tagline: "Tool use, not answers: miners post-train Qwen3.8-27B into an agent, and every run is replayed to prove it used the environment.".into(),
        description: "Solve episodes — a goal plus a tool environment — on a private set you never see. Task success alone is not enough: the emitted tool calls are re-executed for grounding, and the same episodes are re-run with the tools stubbed and with the observation swapped. A model that answers just as well without them never used them, and scores zero however high its success rate.".into(),
        status: "live".into(),
        scoring: ScoringMethod::Displacement,
        mechanism: vec![
            "Miners pay Lium (BYOK)".into(),
            "Emitted tool traces replayed for grounding".into(),
            "Tool-ablation and observation-shuffle arms must both drop".into(),
        ],
        agents: 0,
        best_score: "—".into(),
        best_score_label: "DISPLACE".into(),
        emission_share: 1.0,
        weight: 1.0,
        rewards_per_day: 0.0,
        references: vec![ProjectReference {
            name: "Relearn".into(),
            repo: "CortexLM/relearn".into(),
            repo_url: "https://github.com/CortexLM/relearn".into(),
        }],
        source_url: "https://github.com/CortexLM/relearn".into(),
        plate: "/plates/relearn.svg".into(),
        round_id: None,
        round_ends_at: None,
        seconds_remaining: None,
    }
}

/// Relearn arena frame; counters filled by caller from live status.
#[must_use]
pub fn relearn_frame() -> Arena {
    Arena {
        slug: ArenaSlug::Relearn,
        name: "Relearn".into(),
        tagline: "Post-training factory: miners improve Qwen3.8-27B; score is displacement vs the previous champion.".into(),
        description: "Submit an improved artifact of the pinned base model. Miners pay Lium. Promotion requires a significant paired win, retention/overfit gates, and an operator audit. Regressions are never crowned.".into(),
        status: "live".into(),
        scoring: ScoringMethod::Displacement,
        mechanism: vec![
            "Miners pay Lium (BYOK)".into(),
            "Paired displacement vs champion".into(),
            "Operator-audited promote (never a regression)".into(),
        ],
        agents: 0,
        best_score: "—".into(),
        best_score_label: "DISPLACE".into(),
        emission_share: 1.0,
        weight: 1.0,
        rewards_per_day: 0.0,
        references: vec![ProjectReference {
            name: "Relearn".into(),
            repo: "CortexLM/relearn".into(),
            repo_url: "https://github.com/CortexLM/relearn".into(),
        }],
        source_url: "https://github.com/CortexLM/relearn".into(),
        plate: "/plates/relearn.svg".into(),
        round_id: None,
        round_ends_at: None,
        seconds_remaining: None,
    }
}
