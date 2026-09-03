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
| **Relearn** | `relearn` | Post-train `Qwen/Qwen3.8-27B` (native VLM) | 3000 bps |
| **Relearn Image** | `relearn-image` | Fine-tune `nvidia/Cosmos3-Super-Text2Image`, judged by Q-Judger | 1000 bps |
| **Relearn Agent** | `relearn-agent` | Post-train the same checkpoint into a tool-using agent, scored on replayed tool traces | 1000 bps |
| **Bounty** | `bounty` | File real bug reports against the subnet | 3000 bps |
| **Proof** | `proof` | Reproducible experiments against operator-published research topics; digest-pinned RLM judge | 2000 bps |

Encoder-attach Multimodal (`relearn-mm`) is **off**: no trust-root row, no
emission.

Every Relearn challenge promotes champion-versus-challenger on a private
holdout, so winning the published split is not enough. Relearn Image is judged
only by **Q-Judger** (`Qwen/Qwen-Image-Bench`) on Qwen-Image-Bench prompts; its
generator seed is Cosmos3 under OpenMDW 1.1 and **Flux-family checkpoints are
rejected**. Relearn Agent scores episodes rather than prompts: the emitted tool
calls are replayed for grounding, and the same episodes are re-run with the
tools stubbed and the observation swapped, so a model that answers without
using them scores zero however high its success rate.

Every challenge also runs a measurement that is deliberately absent from the
number miners are paid on — a capability canary for the Relearn challenges, a
triage-noise ratio for Bounty — and missing evidence fails closed rather than
passing.

Some env vars and host paths still spell `BASE_*`. That is leftover naming,
not a second product.

- **[How to mine — Relearn](docs/external-miner/relearn.md)**
- **[How to mine — Relearn Image](docs/external-miner/relearn-image.md)**
- **[How to mine — Relearn Agent](docs/external-miner/relearn-agent.md)**
- **[How to mine — Bounty](docs/external-miner/bounty.md)**
- **[How to mine — Proof](docs/external-miner/proof.md)**
- **[How to validate](docs/external-miner/validators.md)**

Apache License 2.0 — see [LICENSE](./LICENSE).
