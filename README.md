<div align="center">

# Cortex

**Bittensor subnet control plane for decentralized collaborative AI research (Rust).**

[![CI](https://github.com/CortexLM/cortex/actions/workflows/ci.yml/badge.svg)](https://github.com/CortexLM/cortex/actions/workflows/ci.yml)
[![License](https://img.shields.io/github/license/CortexLM/cortex)](https://github.com/CortexLM/cortex/blob/main/LICENSE)
[![Bittensor](https://img.shields.io/badge/Bittensor-subnet-black.svg)](https://bittensor.com/)

![Cortex Banner](assets/banner.jpg)

</div>

## What it is

Cortex ([`CortexLM/cortex`](https://github.com/CortexLM/cortex)) is the Rust
control plane for a multi-challenge Bittensor subnet. Challenge services on
the **master** host accept miner work over HTTP, sign score leaves, and the
**gateway** (master-only) seals an epoch weight bundle. Validators **fetch**
`GET /v1/weights/latest` and submit on-chain weights. They do not execute
challenges.

Live challenges today:

| Challenge | How miners submit | Spec |
|-----------|-------------------|------|
| **Design** | ZIP harness (`agent.py` + `pyproject.toml`) → sandboxed pages + admin winners | [`docs/DESIGN_CHALLENGE.md`](docs/DESIGN_CHALLENGE.md) |
| **Prism** | AutoModel pin + patch → operator-owned GPU recipe eval | [`docs/PRISM.md`](docs/PRISM.md) |

There is **no miner Phala/CVM path** on this branch (agent-v1 / Harbor pack
executors were removed). Operator-facing map: [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md).

Some env vars, host paths, GHCR package names, and crypto domain tags still
spell `BASE_*` / `base`. That is intentional — see [`docs/NAMING.md`](docs/NAMING.md).

## Architecture (short)

```text
Miners --HTTP--> gateway (TLS) --proxy--> design-challenge / prism-challenge
                                          | signed leaves
                                          v
                              gateway seals EpochBundleV1
                                          |
Validators <--- GET /v1/weights/latest ---+
     |
     +--> on-chain set_weights / CRV4 timelock
```

- Gateway is the sole public edge and **only** runs on the subnet-owner host
  (`docker compose --profile master`).
- Trust roots (`config/challenges.toml`, `config/measurements.toml`) are
  owner-signed **local files**, never fetched from the gateway.
- Unsealed / decode-error latest weights are a **burn vector** (uid 0 = 100%,
  `sealed: false`), not a 404.

## Miners

HTTP submit only. Start at [docs/external-miner/](docs/external-miner/).

```text
https://<gateway>/challenge/design/...
https://<gateway>/challenge/prism/...
```

Public miner docs (examples only — no control-plane code):
[design-challenge](https://github.com/BaseIntelligence/design-challenge),
[prism](https://github.com/BaseIntelligence/prism).

Never put mnemonics or challenge signing keys in miner clients.

## Validators

Weight-only path after seal:

```bash
curl -fsS "$GATEWAY/v1/weights/latest"
```

Then `set_weights` / CRV4 with the validator wallet. Operator compose:

```bash
./deploy/scripts/materialize-env.sh
docker compose up -d                  # postgres, validator, updater, socket-proxy
docker compose --profile master up -d # + gateway (subnet owner host only)
```

Local full-stack smoke (testnet 541 + optional tunnel):

```bash
./deploy/scripts/local-e2e.sh --help
./deploy/scripts/local-e2e.sh --smoke
```

Details: [deploy/README.md](deploy/README.md), [docs/runbooks/local-testnet-e2e.md](docs/runbooks/local-testnet-e2e.md).

## Images (GHCR)

CI [`.github/workflows/images.yml`](.github/workflows/images.yml) builds
digest-pinned images. The registry path is still
`ghcr.io/baseintelligence/base/<suffix>` (historical package name; see
[docs/NAMING.md](docs/NAMING.md)). Never `:latest` in measured compose.

| Target | Image suffix |
|--------|----------------|
| validator | `validator` |
| gateway | `gateway` |
| updater | `updater` |
| prism-challenge | `prism-challenge` |
| design-challenge | `design-challenge` |
| design-egress-proxy | `design-egress-proxy` |

## Toolchain and gates

- Rust **1.96.0** (`rust-toolchain.toml`)
- Workspace: `crates/*`, `bins/*`, `xtask`
- Core gate: `cargo test --workspace`
- CI also runs `fmt`, `clippy -D warnings`, `cargo deny`, and

```bash
cargo run -p xtask -- loc-cap
cargo run -p xtask -- consensus-lint
cargo run -p xtask -- spec-check
cargo run -p xtask -- design-check
cargo run -p xtask -- external-docs-check
```

## Docs

| Doc | Content |
|-----|---------|
| [AGENTS.md](AGENTS.md) | Agent / operator contract |
| [docs/NAMING.md](docs/NAMING.md) | Cortex vs leftover `base` identifiers |
| [CONTRIBUTING.md](CONTRIBUTING.md) | How to change this repo |
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | System map |
| [docs/BUNDLE_SPEC.md](docs/BUNDLE_SPEC.md) | Sealed weight bundle (frozen) |
| [docs/DESIGN_CHALLENGE.md](docs/DESIGN_CHALLENGE.md) | Design challenge (frozen) |
| [docs/PRISM.md](docs/PRISM.md) | Prism challenge |
| [docs/THREAT_MODEL.md](docs/THREAT_MODEL.md) | Security claims |
| [docs/runbooks/](docs/runbooks/) | Ops procedures |

## License

Apache License 2.0 — see [LICENSE](./LICENSE).
