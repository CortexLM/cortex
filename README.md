# Cortex

[![CI](https://github.com/CortexLM/cortex/actions/workflows/ci.yml/badge.svg)](https://github.com/CortexLM/cortex/actions/workflows/ci.yml)
[![License](https://img.shields.io/github/license/CortexLM/cortex)](LICENSE)

Cortex is the Rust control plane for [Bittensor](https://bittensor.com/) subnet **100** (`CortexLM/cortex`). It is the software that accepts miner work over HTTP, scores the two live challenges on the master host, seals an epoch weight bundle, and lets validators verify that bundle before they `set_weights` on-chain.

If you want to **mine**, install `ctx` and start at [docs/external-miner/](docs/external-miner/README.md). If you want to **validate**, read [How to validate](docs/external-miner/validators.md). If you want to **change this repo**, see [Contributing](#contributing).

## What it is

This repo is the subnet control plane: the processes that take miner work, score it, seal weights, and submit them on-chain.

- **Gateway** (master only) — TLS, reverse proxy, registry, and the seal/serve path for epoch weights.
- **Challenge services** (master only) — `bounty-challenge` and `proof-challenge`. These score miner work. Validators never re-run evals.
- **Validator** — fetches the sealed bundle, checks it against owner-signed trust roots on disk, and submits weights on-chain.
- **`ctx`** — the miner CLI. Same HTTP routes as `curl`.

Live emission is **Bounty 2000 bps / Proof 8000 bps** (20/80). The two shares sum to 10000. Older products (`relearn`, `relearn-image`, `relearn-agent`, `relearn-mm`, `design`, `prism`) have no trust-root row and earn nothing.

Some environment variables and host paths still spell `BASE_*`. That is leftover naming from an earlier product identity, not a second stack. See [docs/NAMING.md](docs/NAMING.md).

## Why it is built this way

Subnet scoring is centralized on the owner host so miners have one public HTTP surface. Consensus is **not** “every validator re-runs every experiment.” Validators recompute the weight vector from a signed, merkle-rooted epoch bundle and from **local** owner-signed files (`config/challenges.toml`, `config/measurements.toml`). Challenge keys never come from gateway HTTP.

Missing evidence, an empty Proof eval digest, an empty open-topic set, or an unreadable Bounty score feed **fail closed** (`503` / `NoScore`) instead of inventing a verdict. `GET /v1/weights/latest` with no sealed bundle is a burn vector (`sealed: false`, uid 0 = 100%), not a stale last-known-good.

## Quickstart (miners)

Miners and validators talk to the public gateway at
**https://network.cortex.foundation**.

```bash
curl -fsSL https://raw.githubusercontent.com/CortexLM/cortex/main/scripts/install-ctx.sh | sh

ctx challenges   # the two live challenges and what they pay for
ctx status      # can each challenge score right now, and is the epoch sealed
```

`ctx` lives in [`bins/ctx`](bins/ctx). A local stack uses `--gateway http://127.0.0.1:8080` (or whatever tunnel URL you printed). Never put a mnemonic or a challenge signing key in a miner client.

| You want to | Command | Guide |
|-------------|----------|-------|
| File product/backend bugs | `ctx bounty pair` then `ctx bounty report` | [Bounty](docs/external-miner/bounty.md) |
| Reproduce a research topic | `ctx proof topics` then `ctx proof submit` | [Proof](docs/external-miner/proof.md) |
| Debug a 503 / install issue | `ctx status` | [Troubleshoot](docs/external-miner/troubleshoot.md) |

Check `can_score` before you spend GPU time or Lium rent. A host that cannot score stores nothing and rents nothing.

## Architecture

```text
 Miners (ctx / curl)
        │  HTTP submit
        ▼
 ┌─────────────────────────────────────┐
 │  Master host                         │
 │  postgres · gateway                   │
 │  bounty-challenge · proof-challenge   │
 └──────────────────┬────────────────────┘
                    │  GET /v1/weights/latest
                    │  (sealed epoch bundle)
                    ▼
          Validator hosts
          local trust roots on disk
                    │
                    ▼
              set_weights (CRV4)
```

One epoch, short form:

1. Challenge services sign leaves for the expected miner set.
2. The gateway seals `EpochBundleV1` (merkle root + signature).
3. Validators fetch latest, verify against the local trust root, cross-check peers, recompute, then submit.

The map, process list, and what this architecture does **not** claim: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md). Byte layout: [docs/BUNDLE_SPEC.md](docs/BUNDLE_SPEC.md).

## Live challenges

### Bounty (`bounty`, 2000 bps)

Pair a Bittensor hotkey to a **dedicated** Cortex Chat mining account, then file real bugs on Cortex product and backend surfaces. Operators adjudicate (`valid` / `already_fixed_not_prod` / `invalid_malicious` / `duplicate`). Pay is precision times severity; an unpriced `valid` row is not creditable, and triage noise stays off the visible score.

Scoring **reads** the CortexLM/backend public JSON feed. This repo does not serve a public leaderboard. If that feed is unreadable, reports answer **503** and the share pays nobody.

Guide: [docs/external-miner/bounty.md](docs/external-miner/bounty.md) · operator spec: [docs/BOUNTY.md](docs/BOUNTY.md)

### Proof (`proof`, 8000 bps)

Submit **claim + code + FLOPs + artifact** against an operator-published `topic_id`. Topics are signed documents, not a catalog in git. Each open topic pays `wta` (winner takes that topic's mass) or `discovery` (pass floor + novelty). Your paid score is the **sum** of per-topic masses, not a mean of binary lattices.

The judge is a digest-pinned eval image plus a live `InferenceOffer`. The pin lives in [`config/proof-pin.toml`](config/proof-pin.toml) — do not invent a digest. Empty digest, unwired harvest, unsealed baseline, or zero open topics → **503**. Proof miners pay Lium (`LIUM_API_KEY` / `X-Lium-Api-Key`); `ctx` forwards the key and never prints it.

`ctx proof topics` lists currently open topics and never leaks holdout records.

Guide: [docs/external-miner/proof.md](docs/external-miner/proof.md) · operator spec: [docs/PROOF.md](docs/PROOF.md)

## Validators

Validators do not run Bounty adjudication or Proof harvest. They:

1. Pull `GET /v1/weights/latest` from the master gateway.
2. Verify signatures, completeness, and the owner trust root on **local disk**.
3. `set_weights` on-chain (CRV4 timelock when enabled).

Do not submit an unsealed burn vector, and do not submit a persisted last-known-good seal while latest is unsealed. Runbook: [docs/external-miner/validators.md](docs/external-miner/validators.md). Compose role: [`deploy/compose/role-validator.yml`](deploy/compose/role-validator.yml).

## Repository layout

| Path | Role |
|------|--------|
| [`bins/`](bins/) | Runnable processes: `gateway`, `validator`, `ctx`, challenge services, `updater` |
| [`crates/`](crates/) | Shared libraries (bundle, aggregate, trustroot, chain, …) |
| [`xtask/`](xtask/) | Repo gates (`loc-cap`, `spec-check`, `external-docs-check`, …) |
| [`deploy/`](deploy/) | Compose matrix, Terraform, digest pins, remote deploy |
| [`docs/`](docs/) | Architecture, frozen specs, runbooks, miner guides |
| [`config/`](config/) | Non-secret configuration (trust-root TOML, Proof pin) |

Working branch is **`main`**. Production ships from annotated tags `v*.*.*` cut on `main`.

## Develop (this repo)

Rust **1.96.0** via [`rust-toolchain.toml`](rust-toolchain.toml). `unsafe_code` is forbidden; `unwrap` / `expect` stay in tests.

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check
cargo run -p xtask -- loc-cap
cargo run -p xtask -- consensus-lint
cargo run -p xtask -- spec-check
cargo run -p xtask -- design-check
cargo run -p xtask -- external-docs-check
```

Local full-subnet smoke (Docker Compose, testnet 541, optional tunnel):

```bash
./deploy/scripts/materialize-env.sh
./deploy/scripts/local-e2e.sh --smoke
```

Details: [docs/runbooks/local-testnet-e2e.md](docs/runbooks/local-testnet-e2e.md) and [deploy/README.md](deploy/README.md). Do not commit `deploy/env/*.env`, wallets, or age identities.

## Contributing

Read [CONTRIBUTING.md](CONTRIBUTING.md) and [AGENTS.md](AGENTS.md) before you open a PR.

- Target **`main`**. Subject: `type(scope): summary` (lowercase, ≤72 chars).
- Frozen specs (`docs/BUNDLE_SPEC.md`, `docs/DESIGN_CHALLENGE.md`) are pinned by xtask. Do not rewrite incentive or consensus semantics in a drive-by.
- Do not rename `BASE_*` env vars, `/opt/base` paths, or `base-*-v1` domain tags. Those strings are measured into live droplets and miner CVMs.
- PRs need a [Greptile](https://greptile.com) review (`.greptile/`). If the bot is silent, comment `@greptileai review`.
- Security reports go through [SECURITY.md](SECURITY.md), not a public issue.

## Documentation

| Doc | Audience |
|-----|----------|
| [docs/external-miner/README.md](docs/external-miner/README.md) | Miners (A→Z) |
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | Process topology |
| [docs/COMPLETENESS.md](docs/COMPLETENESS.md) | What is actually wired |
| [docs/THREAT_MODEL.md](docs/THREAT_MODEL.md) | What is claimed, and what is not |
| [docs/OPERATOR_SECURITY.md](docs/OPERATOR_SECURITY.md) | Operator checklist |
| [docs/runbooks/](docs/runbooks/) | Staging, local e2e, rotation, failover |
| [whitepaper.pdf](whitepaper.pdf) | Whitepaper |
| [SUPPORT.md](SUPPORT.md) | How to get help |

Apache License 2.0 — see [LICENSE](LICENSE).
