# Cortex

[![CI](https://github.com/CortexLM/cortex/actions/workflows/ci.yml/badge.svg)](https://github.com/CortexLM/cortex/actions/workflows/ci.yml)
[![License](https://img.shields.io/github/license/CortexLM/cortex)](LICENSE)

Rust control plane for [Bittensor](https://bittensor.com/) subnet **100**: miners submit over HTTP, the master scores two live challenges, validators verify a sealed weight bundle and `set_weights`.

## Features

| Capability | Detail |
|---|---|
| **Two live challenges** | **Bounty** (`bounty`, 2000 bps) and **Proof** (`proof`, 8000 bps). Sum is 10000. `relearn`, `relearn-image`, `relearn-agent`, `relearn-mm`, `design`, and `prism` are off. |
| **One public gateway** | [https://network.cortex.foundation](https://network.cortex.foundation) — `ctx` or `curl`. |
| **Master-only scoring** | Gateway + `bounty-challenge` + `proof-challenge` run on the owner host. Validators do not re-run evals. |
| **Fail-closed scoring** | Empty Proof eval digest, no open topic, or an unreadable Bounty feed answers **503** instead of inventing a verdict. |
| **Sealed weights** | Gateway seals an epoch bundle. Validators check it against owner-signed files on disk, then submit (CRV4 when enabled). |

Some env vars and host paths still spell `BASE_*`. That is leftover naming, not a second product. See [docs/NAMING.md](docs/NAMING.md).

## Quickstart

```bash
curl -fsSL https://raw.githubusercontent.com/CortexLM/cortex/main/scripts/install-ctx.sh | sh

ctx challenges   # the two live challenges and what they pay for
ctx status       # can each challenge score right now, and is the epoch sealed
```

`ctx` lives in [`bins/ctx`](bins/ctx). A local stack uses `--gateway http://127.0.0.1:8080`. Never put a mnemonic or a challenge signing key in a miner client. Check `can_score` before you spend GPU time or Lium rent.

## Mine

| Challenge | id | Emission | Start with | Guide |
|-----------|-----|----------|------------|-------|
| **Bounty** | `bounty` | 2000 bps | `ctx bounty pair` then `ctx bounty report` | [How to mine — Bounty](docs/external-miner/bounty.md) |
| **Proof** | `proof` | 8000 bps | `ctx proof topics` then `ctx proof submit` | [How to mine — Proof](docs/external-miner/proof.md) |

A→Z index: [docs/external-miner/](docs/external-miner/README.md). Install or 503 issues: [troubleshoot](docs/external-miner/troubleshoot.md).

### Bounty

Pair a Bittensor hotkey to a **dedicated** Cortex Chat mining account, then file real bugs on Cortex product and backend surfaces. Operators adjudicate (`valid` / `already_fixed_not_prod` / `invalid_malicious` / `duplicate`). Pay is precision times severity; an unpriced `valid` row is not creditable.

Scoring **reads** the CortexLM/backend public JSON feed. This repo does not serve a public leaderboard. If that feed is unreadable, reports answer **503** and the share pays nobody.

### Proof

Submit **claim + code + FLOPs + artifact** against an operator-published `topic_id`. Topics are signed documents, not a catalog in git. Each open topic pays `wta` or `discovery`. Your paid score is the **sum** of per-topic masses.

The judge is a digest-pinned eval image plus a live `InferenceOffer`. The pin is in [`config/proof-pin.toml`](config/proof-pin.toml) (`ghcr.io/cortexlm/proof-eval`, digest `sha256:78b614a1…`). Do not invent a digest. Empty digest, unwired harvest, unsealed baseline, or zero open topics → **503**. Proof miners pay Lium (`LIUM_API_KEY` / `X-Lium-Api-Key`); `ctx` forwards the key and never prints it.

`ctx proof topics` never leaks holdout records.

## Validate

Validators pull the sealed bundle, verify it, and submit weights on-chain. They do not run Bounty adjudication or Proof harvest.

1. Pull `GET /v1/weights/latest` from the master gateway.
2. Verify signatures, completeness, and the owner trust root on **local disk** (`config/challenges.toml`, `config/measurements.toml`).
3. `set_weights` on-chain (CRV4 timelock when enabled).

Do not submit an unsealed burn vector (`sealed: false`, uid 0 = 100%), and do not submit a persisted last-known-good seal while latest is unsealed.

Guide: [How to validate](docs/external-miner/validators.md) · compose role: [`deploy/compose/role-validator.yml`](deploy/compose/role-validator.yml).

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

One epoch: challenge services sign leaves → the gateway seals `EpochBundleV1` → validators fetch, verify, recompute, and submit.

| Path | Role |
|------|------|
| [`bins/`](bins/) | `gateway`, `validator`, `ctx`, challenge services, `updater` |
| [`crates/`](crates/) | Shared libraries (bundle, aggregate, trustroot, chain, …) |
| [`deploy/`](deploy/) | Compose matrix, Terraform, digest pins |
| [`config/`](config/) | Trust-root TOML, Proof pin |
| [`docs/`](docs/) | Specs, runbooks, miner guides |

Full map: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md). Byte contract: [docs/BUNDLE_SPEC.md](docs/BUNDLE_SPEC.md).

## Docs

| Doc | Audience |
|-----|----------|
| [docs/external-miner/README.md](docs/external-miner/README.md) | Miners |
| [docs/external-miner/bounty.md](docs/external-miner/bounty.md) | Bounty miners |
| [docs/external-miner/proof.md](docs/external-miner/proof.md) | Proof miners |
| [docs/external-miner/validators.md](docs/external-miner/validators.md) | Validators |
| [docs/BOUNTY.md](docs/BOUNTY.md) / [docs/PROOF.md](docs/PROOF.md) | Operator challenge specs |
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | Process topology |
| [docs/COMPLETENESS.md](docs/COMPLETENESS.md) | What is actually wired |
| [docs/THREAT_MODEL.md](docs/THREAT_MODEL.md) | What is claimed, and what is not |
| [docs/runbooks/](docs/runbooks/) | Local e2e, staging, rotation, failover |
| [deploy/README.md](deploy/README.md) | Compose / droplets |
| [whitepaper.pdf](whitepaper.pdf) | Whitepaper |
| [SUPPORT.md](SUPPORT.md) | How to get help |

## Contributing

Read [CONTRIBUTING.md](CONTRIBUTING.md) and [AGENTS.md](AGENTS.md) before you open a PR.

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

Local smoke (Docker Compose, testnet 541):

```bash
./deploy/scripts/materialize-env.sh
./deploy/scripts/local-e2e.sh --smoke
```

- Target **`main`**. Subject: `type(scope): summary` (lowercase, ≤72 chars).
- Frozen specs are pinned by xtask. Do not rewrite incentive or consensus semantics in a drive-by.
- Do not rename `BASE_*` env vars, `/opt/base` paths, or `base-*-v1` domain tags.
- PRs need a [Greptile](https://greptile.com) review (`.greptile/`). If the bot is silent, comment `@greptileai review`.
- Security: [SECURITY.md](SECURITY.md).

## License

Apache License 2.0 — see [LICENSE](LICENSE).
