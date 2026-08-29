# 10 — Maintenance: keep the repo true A→Z

## Code hygiene

- `unsafe_code` is **forbidden**. No `unwrap` / `expect` outside tests
  (`clippy::unwrap_used` / `expect_used` are `deny` in `Cargo.toml`).
- Clippy `pedantic` is on for the whole workspace and CI runs
  `-D warnings`. Do not `#[allow]` your way past a lint without a comment
  explaining the constraint.
- Per-crate cap: **1500 non-test LOC** (`cargo run -p xtask -- loc-cap`).
  Split a crate rather than raising the cap.
- Consensus crates (see `xtask/consensus-crates.txt`) must not use `HashMap`,
  `f32`/`f64`, `wrapping_*`, or bare `u128` arithmetic
  (`cargo run -p xtask -- consensus-lint`).

## No dead code, no stale text

- Replace, do not accumulate. When you supersede a function, module, script,
  compose overlay, or workflow, delete the old one in the same PR.
- Comments state constraints the code cannot. Do not narrate the diff and do
  not leave "TODO(agent)" breadcrumbs in shipped code.
- A command written in `README.md`, `AGENTS.md`, `.rules/`, or `deploy/` must
  be a command that works today. If you rename a flag or a script, grep for it
  and fix every mention.
- Markdown links must resolve. `cargo run -p xtask -- rules-check` checks the
  links in `README.md`, `AGENTS.md`, `SECURITY.md`, `deploy/*.md`, and all of
  `.rules/`.

## Documentation surfaces (there are only three)

| Change you make | What you must also update |
|-----------------|---------------------------|
| New / removed crate, bin, or top-level directory | `AGENTS.md` monorepo map, `README.md` if a human would look for it |
| New / changed HTTP route, quota, round, or scoring rule | the relevant contract in [`contracts/`](contracts/README.md) **and** [`contracts/external-miner/`](contracts/external-miner/README.md) **and** the public miner repo |
| New / changed local or CI command | [`20-pre-prod-local.md`](20-pre-prod-local.md), `README.md`, and `.github/workflows/ci.yml` together |
| New / changed deploy topology, compose overlay, secret path | `deploy/README.md` + `deploy/AGENTS.md` |
| New rule for agents | a numbered file in `.rules/` (and `rules_check.rs` if it is machine-checkable) |

Historical `docs/evidence/` and `docs/spikes/` were removed with the rest of
the doc site. Code comments that still cite those paths are provenance notes
pointing at git history before that commit; treat them as history, never as
spec. Do not re-create the tree, and do not edit consensus-adjacent hashed
artifacts (`crates/db/migrations/*.sql`, `crates/prism-recipe/anchors/*.json`,
`crates/design-prompts/prompts/*.json`) just to tidy a comment.

## Frozen contracts

`contracts/BUNDLE_SPEC.md` and `contracts/DESIGN_CHALLENGE.md` are frozen and
pinned by `spec-check` / `design-check`. `contracts/THREAT_MODEL.md` D19 is
pinned word-for-word by `external-docs-check`.

- Never weaken a gate to make a diff pass. Change the product, then the spec,
  then the pin — deliberately, as the point of the PR.
- Never rewrite incentive, scoring, or consensus semantics as a side effect of
  a refactor, rename, or docs pass.
- Never rename `BASE_*` env vars, deployed paths, GHCR package paths, or
  `base-*-v1` crypto domain tags. See [`60-naming.md`](60-naming.md).

## Before you call anything done

Run [`20-pre-prod-local.md`](20-pre-prod-local.md) in full, bump the version
per [`50-versioning.md`](50-versioning.md), and fill the attestation in
[`30-pr.md`](30-pr.md).
