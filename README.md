<div align="center">

# Cortex

**Bittensor subnet control plane (Rust).**

[![CI](https://github.com/CortexLM/cortex/actions/workflows/ci.yml/badge.svg)](https://github.com/CortexLM/cortex/actions/workflows/ci.yml)
[![License](https://img.shields.io/github/license/CortexLM/cortex)](https://github.com/CortexLM/cortex/blob/main/LICENSE)
[![Bittensor](https://img.shields.io/badge/Bittensor-subnet-black.svg)](https://bittensor.com/)

![Cortex Banner](assets/banner.jpg)

</div>

Cortex is the control plane for a one-challenge Bittensor subnet: **Relearn**.
The master host runs the challenge service and the gateway. Miners submit an
artifact over HTTP and pay Lium. The gateway seals an epoch weight bundle.
Validators pull that bundle, verify it, and `set_weights` on-chain. They do
not run evals.

Some env vars and host paths still spell `BASE_*`. That is leftover naming,
not a second product.

- **[How to mine — Relearn](docs/external-miner/relearn.md)**
- **[How to validate](docs/external-miner/validators.md)**

Apache License 2.0 — see [LICENSE](./LICENSE).
