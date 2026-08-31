<div align="center">

# Cortex

**Bittensor subnet control plane (Rust).**

[![CI](https://github.com/CortexLM/cortex/actions/workflows/ci.yml/badge.svg)](https://github.com/CortexLM/cortex/actions/workflows/ci.yml)
[![License](https://img.shields.io/github/license/CortexLM/cortex)](https://github.com/CortexLM/cortex/blob/main/LICENSE)
[![Bittensor](https://img.shields.io/badge/Bittensor-subnet-black.svg)](https://bittensor.com/)

![Cortex Banner](assets/banner.jpg)

</div>

Cortex is the control plane for a Bittensor subnet with several challenges.
The master host runs every challenge service and the gateway. Miners submit an
artifact over HTTP and pay Lium. The gateway seals an epoch weight bundle.
Validators pull that bundle, verify it, and `set_weights` on-chain. They do
not run evals.

| Challenge | id | What miners improve | Default emission |
|-----------|-----|---------------------|------------------|
| **Relearn LLM** | `relearn` | Post-train `Qwen/Qwen3.8-27B` (native VLM) | 4000 bps |
| **Relearn T2I** | `relearn-t2i` | Fine-tune `nvidia/Cosmos3-Super-Text2Image`, judged by Q-Judger | 1500 bps |
| **Relearn Multimodal** | `relearn-mm` | Attach a permissive vision encoder to the champion LLM without regressing it | 1500 bps |
| **Bounty** | `bounty` | File real bug reports against the subnet | 3000 bps |

Every Relearn challenge promotes champion-versus-challenger on a private
holdout, so winning the published split is not enough. Relearn T2I is judged
only by **Q-Judger** (`Qwen/Qwen-Image-Bench`) on Qwen-Image-Bench prompts; its
generator seed is Cosmos3 under OpenMDW 1.1 and **Flux-family checkpoints are
rejected**. Relearn Multimodal accepts **Apache-2.0 / MIT / BSD / ISC** vision
encoders only.

Some env vars and host paths still spell `BASE_*`. That is leftover naming,
not a second product.

- **[How to mine — Relearn LLM](docs/external-miner/relearn.md)**
- **[How to mine — Relearn T2I](docs/external-miner/relearn-t2i.md)**
- **[How to mine — Relearn Multimodal](docs/external-miner/relearn-mm.md)**
- **[How to mine — Bounty](docs/external-miner/bounty.md)**
- **[How to validate](docs/external-miner/validators.md)**

Apache License 2.0 — see [LICENSE](./LICENSE).
