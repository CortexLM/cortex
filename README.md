<div align="center">

# Cortex

**Bittensor subnet control plane (Rust).**

[![CI](https://github.com/CortexLM/cortex/actions/workflows/ci.yml/badge.svg)](https://github.com/CortexLM/cortex/actions/workflows/ci.yml)
[![License](https://img.shields.io/github/license/CortexLM/cortex)](https://github.com/CortexLM/cortex/blob/main/LICENSE)
[![Bittensor](https://img.shields.io/badge/Bittensor-subnet-black.svg)](https://bittensor.com/)

![Cortex Banner](assets/banner.jpg)

</div>

Cortex is the control plane for a Bittensor subnet with two live challenges.
The master host runs every live challenge service and the gateway. Miners
submit over HTTP (`ctx`). The gateway seals an epoch weight bundle.
Validators pull that bundle, verify it, and `set_weights` on-chain. They do
not run evals.

| Challenge | id | What miners improve | Default emission |
|-----------|-----|---------------------|------------------|
| **Bounty** | `bounty` | File real bug reports against the subnet | 7000 bps |
| **Proof** | `proof` | Reproducible experiments against operator-published research topics; digest-pinned RLM judge | 3000 bps |

Proof's `eval_image_digest` is empty (submits **503**), so 7000/3000 keeps
most emission payable. Retune to 5000/5000 in the same ceremony that pins a
non-empty proof-eval digest. The sum is 10000. Neither share is a leftover
Relearn / Prism / Design inheritance. `relearn`,
`relearn-image`, `relearn-agent`, `relearn-mm`, `design`, and `prism` are
**off**: no trust-root row, no emission.

Bounty pays precision times severity; an unpriced `valid` row is not
creditable, and the triage-noise ratio stays off the visible score. Proof
scores the mean of per-topic lattices over currently `open` operator-published
topics; empty `eval_image_digest` or an empty open set fails closed (`503`).
Missing evidence fails closed rather than passing.

Some env vars and host paths still spell `BASE_*`. That is leftover naming,
not a second product.

- **[How to mine — Bounty](docs/external-miner/bounty.md)**
- **[How to mine — Proof](docs/external-miner/proof.md)**
- **[How to validate](docs/external-miner/validators.md)**

Apache License 2.0 — see [LICENSE](./LICENSE).
