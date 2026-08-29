# Frozen contracts

Normative specifications relocated here when the `docs/` site was deleted.
They are **pinned by `xtask` gates** — this is not prose you can tidy.

| Contract | Status | Gate |
|----------|--------|------|
| [`BUNDLE_SPEC.md`](BUNDLE_SPEC.md) + [`BUNDLE_SPEC_CHECKLIST.md`](BUNDLE_SPEC_CHECKLIST.md) | **FROZEN** — epoch bundle SCALE bytes, merkle, aggregation, on-chain payload | `cargo run -p xtask -- spec-check` |
| [`DESIGN_CHALLENGE.md`](DESIGN_CHALLENGE.md) + [`DESIGN_CHALLENGE_CHECKLIST.md`](DESIGN_CHALLENGE_CHECKLIST.md) | **FROZEN** — design harness sandbox, sanitize, rounds, admin winners, D24 leaves | `cargo run -p xtask -- design-check` |
| [`PRISM.md`](PRISM.md) | **off** — archived architecture competition, WTA, leaf emission | referenced as normative by `crates/prism-*` |
| [`PRISM_RECIPE.md`](PRISM_RECIPE.md) | **off** — archived recipe / battery / budget currency | referenced as normative by `crates/prism-recipe` |
| [`BOUNTY.md`](BOUNTY.md) | live — bounty pairing, backend-feed scoring, fail-closed 503 | referenced by `bins/bounty-*` / `deploy/AGENTS.md` |
| [`PROOF.md`](PROOF.md) | live — operator-published topics, RLM judge, empty-digest 503 | referenced by `bins/proof-*` |
| [`RELEARN.md`](RELEARN.md), [`RELEARN-IMAGE.md`](RELEARN-IMAGE.md), [`RELEARN-AGENT.md`](RELEARN-AGENT.md), [`RELEARN-MM.md`](RELEARN-MM.md) | **off** — archived operator specs; no trust-root row | historical; do not present as live |
| [`THREAT_MODEL.md`](THREAT_MODEL.md) | **pinned word-for-word** — D19 claim, D5, D11, R12 | `cargo run -p xtask -- external-docs-check` |
| [`external-miner/`](external-miner/README.md) | miner-facing HTTP submit docs + examples; `protocol_version` badge must equal `bundle::PROTOCOL_VERSION` | `cargo run -p xtask -- external-docs-check` |

## Rules

- Read the contract your change touches. Do not skim it, and do not implement
  against a stale memory of it.
- Do **not** weaken, delete, or relax a pin to make a diff pass. Change the
  product deliberately, then the contract, then the pin, in one PR that says so.
- Do **not** rewrite incentive, scoring, or consensus semantics as a side
  effect of a refactor, rename, or docs pass.
- Miner-facing changes must land in [`external-miner/`](external-miner/README.md)
  here and the matching operator spec (`BOUNTY.md` / `PROOF.md`). Public miner
  repos (when they exist) carry human docs and examples only — never
  control-plane, gateway, validator, or orchestrator source. Do not send
  miners to Design, Prism, or Relearn pages as live work.
- `external-miner/examples/` is live code: `crates/design-challenge` and
  `crates/design-sandbox` `include_str!` the design baseline as the agentic
  review corpus anchor, and `deploy/Dockerfile` copies it into the build. Moving
  those files breaks the build.

Repo-wide duties: [`../00-overview.md`](../00-overview.md).
