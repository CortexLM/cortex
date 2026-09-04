<div align="center">

# Cortex

**Bittensor subnet control plane (Rust).**

[![CI](https://github.com/CortexLM/cortex/actions/workflows/ci.yml/badge.svg)](https://github.com/CortexLM/cortex/actions/workflows/ci.yml)
[![License](https://img.shields.io/github/license/CortexLM/cortex)](https://github.com/CortexLM/cortex/blob/main/LICENSE)
[![Bittensor](https://img.shields.io/badge/Bittensor-subnet-black.svg)](https://bittensor.com/)

<a href="whitepaper.pdf">Whitepaper</a>

<img src="assets/banner.jpg" width="720" alt="Cortex Banner">

</div>

Cortex is the control plane for a Bittensor subnet with two live challenges.
The master host runs every live challenge service and the gateway. Miners
submit over HTTP (`ctx`). The gateway seals an epoch weight bundle.
Validators pull that bundle, verify it, and `set_weights` on-chain. They do
not run evals.

| Challenge | id | What miners improve | Default emission |
|-----------|-----|---------------------|------------------|
| **Bounty** | `bounty` | Bug hunters report defects across cortex.foundation and Cortex applications (product surfaces) so continuous production service stays low-defect for clients | 2000 bps |
| **Proof** | `proof` | Reproducible experiments (claim + code + FLOPs) against operator-published research topics; digest-pinned RLM judge | 8000 bps |

Live emission is **bounty 2000 / proof 8000** (20/80). The sum is 10000.
Proof's eval image is pinned at
`ghcr.io/cortexlm/proof-eval@sha256:78b614a1f51ce5dd80076c4e343a2b31b85d6c36025e02836cb83929867e7009`.
An empty digest would still **503**. `relearn`, `relearn-image`,
`relearn-agent`, `relearn-mm`, `design`, and `prism` are **off**: no
trust-root row, no emission.

Bounty pays precision times severity; an unpriced `valid` row is not
creditable, and the triage-noise ratio stays off the visible score. Proof
scores WTA or discovery over currently `open` operator-published
topics (sum of per-topic masses); empty `eval_image_digest` or an empty open set fails closed (`503`).
Missing evidence fails closed rather than passing.

Some env vars and host paths still spell `BASE_*`. That is leftover naming,
not a second product.

## Mine

Miners and validators talk to one public gateway:
**`https://network.cortex.foundation`**. Install the subnet CLI:

```bash
curl -fsSL https://raw.githubusercontent.com/CortexLM/cortex/main/scripts/install-ctx.sh | sh

ctx challenges   # the two live challenges and what they pay for
ctx status       # can each challenge score right now, and is the epoch sealed
```

`ctx` ([`bins/ctx`](bins/ctx)) submits to the two live challenges and handles
Bounty pairing. `ctx relearn|image|agent` still exist for a local stack;
those challenges are **off** and earn nothing. `curl` works against the same
routes.

| Challenge | Start with | Guide |
|-----------|-----------|-------|
| Bounty | `ctx bounty pair` | **[How to mine — Bounty](docs/external-miner/bounty.md)** |
| Proof | `ctx proof submit` | **[How to mine — Proof](docs/external-miner/proof.md)** |

Start at **[docs/external-miner/](docs/external-miner/README.md)** for the
A→Z, and **[How to validate](docs/external-miner/validators.md)** if you run a
validator.

Apache License 2.0 — see [LICENSE](./LICENSE).
